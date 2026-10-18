//! The scrolling layout (Niri-style).
//!
//! Tiling children of a scrolling workspace form a strip along the
//! workspace's scroll axis. Each strip item is sized as a fraction of
//! the viewport along that axis (its `tiling_size`), independent of its
//! siblings, and the viewport scrolls to keep the focused item in view.
//!
//! See `docs/scrolling-layout.md` for the design and semantics of the
//! layout, and `docs/layouts.md` for how layouts fit into the codebase.

use wm_common::{
  OffscreenWindows, ScrollAlign, ScrollConfig, ScrollInsertPosition,
  TilingDirection, WindowState,
};
use wm_platform::{Direction, Rect};

use super::WindowPlacement;
use crate::{
  commands::{
    container::move_container_within_tree,
    window::{
      move_to_workspace_in_direction, tiling_sibling_in_direction,
    },
  },
  models::{Container, TilingContainer, TilingWindow, Workspace},
  traits::{
    CommonGetters, PositionGetters, TilingDirectionGetters,
    TilingSizeGetters,
  },
  wm_state::WmState,
};

/// Extent (width or height) of the workspace viewport along the scroll
/// axis.
///
/// The viewport is the workspace rect, i.e. the monitor's working area
/// with outer gaps applied.
pub fn viewport_extent(workspace: &Workspace) -> anyhow::Result<i32> {
  let rect = workspace.to_rect()?;

  Ok(match workspace.tiling_direction() {
    TilingDirection::Horizontal => rect.width(),
    TilingDirection::Vertical => rect.height(),
  })
}

/// Extent of a single strip item along the scroll axis in pixels, based
/// on its stored `tiling_size`.
fn item_extent(item: &TilingContainer, viewport_extent: i32) -> i32 {
  #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
  let extent =
    (item.tiling_size() * viewport_extent as f32).round() as i32;

  extent.max(1)
}

/// Extent of a single strip item along the scroll axis in pixels.
///
/// A lone strip item always fills the entire viewport (like a single
/// column in Niri). Its stored tiling size is kept as-is and applies
/// again as soon as the workspace has more than one tiling child.
fn strip_item_extent(
  workspace: &Workspace,
  item: &TilingContainer,
  viewport_extent: i32,
) -> i32 {
  if workspace.tiling_children().nth(1).is_none() {
    return viewport_extent;
  }

  item_extent(item, viewport_extent)
}

/// Total extent of the strip of tiling children in pixels along the
/// scroll axis.
///
/// This includes the inner gap between each pair of adjacent strip
/// items. Returns 0 if the workspace has no tiling children.
pub fn strip_extent(workspace: &Workspace) -> anyhow::Result<i32> {
  let viewport = viewport_extent(workspace)?;
  let children = workspace.tiling_children().collect::<Vec<_>>();

  let Some(first_child) = children.first() else {
    return Ok(0);
  };

  let (horizontal_gap, vertical_gap) = first_child.inner_gaps()?;
  let inner_gap = match workspace.tiling_direction() {
    TilingDirection::Vertical => vertical_gap,
    TilingDirection::Horizontal => horizontal_gap,
  };

  let mut extent = 0;

  for (index, child) in children.iter().enumerate() {
    if index > 0 {
      extent += inner_gap;
    }

    extent += strip_item_extent(workspace, child, viewport);
  }

  Ok(extent)
}

/// Maximum scroll offset for the current strip contents.
///
/// Returns 0 when the strip is narrower than the viewport.
pub fn max_scroll_offset(workspace: &Workspace) -> anyhow::Result<i32> {
  Ok((strip_extent(workspace)? - viewport_extent(workspace)?).max(0))
}

/// Clamps the scroll offset into bounds that are valid for the current
/// strip contents. Returns whether the offset changed.
pub fn clamp_scroll_offset(workspace: &Workspace) -> anyhow::Result<bool> {
  let clamped = workspace
    .scroll_offset()
    .clamp(0, max_scroll_offset(workspace)?);

  Ok(workspace.set_scroll_offset(clamped))
}

/// Start and extent of a range along the scroll axis, in pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct AxisSpan {
  start: i32,
  extent: i32,
}

impl AxisSpan {
  /// Span of `rect` along the given scroll axis.
  fn of_rect(rect: &Rect, axis: &TilingDirection) -> Self {
    match axis {
      TilingDirection::Horizontal => Self {
        start: rect.x(),
        extent: rect.width(),
      },
      TilingDirection::Vertical => Self {
        start: rect.y(),
        extent: rect.height(),
      },
    }
  }

  fn end(self) -> i32 {
    self.start + self.extent
  }

  fn center(self) -> i32 {
    self.start + self.extent / 2
  }
}

/// Distance that the scroll offset must change by to bring `item` into
/// the viewport per the given alignment.
///
/// - `Minimal`: the least distance that makes the item fully visible (`0`
///   if it already is).
/// - `Center`: the distance that centers the item in the viewport.
fn scroll_delta(
  align: &ScrollAlign,
  viewport: AxisSpan,
  item: AxisSpan,
) -> i32 {
  match align {
    ScrollAlign::Minimal => {
      if item.start < viewport.start {
        item.start - viewport.start
      } else if item.end() > viewport.end() {
        item.end() - viewport.end()
      } else {
        0
      }
    }
    ScrollAlign::Center => item.center() - viewport.center(),
  }
}

/// Gets the top-level strip item of the workspace that contains the
/// given descendant. This can be the descendant itself.
///
/// Returns `None` if the container is not a tiling descendant of the
/// workspace.
pub fn strip_item_of(
  workspace: &Workspace,
  descendant: &Container,
) -> Option<TilingContainer> {
  descendant
    .self_and_ancestors()
    .find(|ancestor| {
      ancestor
        .parent()
        .is_some_and(|parent| parent.id() == workspace.id())
    })
    .and_then(|strip_item| strip_item.as_tiling_container().ok())
}

/// Scrolls the viewport so that the given descendant is in view, per the
/// workspace's `scroll.align` config.
///
/// The descendant is resolved to the top-level strip item that contains
/// it. Returns whether the offset changed. Has no effect for windows
/// that are not tiling children of this workspace.
pub fn ensure_descendant_visible(
  workspace: &Workspace,
  container: &Container,
) -> anyhow::Result<bool> {
  let align = workspace.config().scroll.align;
  scroll_descendant_into_view(workspace, container, &align)
}

/// Scrolls the viewport so that the given descendant is in view, per the
/// given alignment.
///
/// The descendant is resolved to the top-level strip item that contains
/// it. The resulting offset is clamped to the valid range, so items at
/// either end of the strip can't be centered past the strip's edge.
/// Returns whether the offset changed. Has no effect for workspaces that
/// don't use the scrolling layout, or for containers that aren't tiling
/// descendants of the workspace.
pub fn scroll_descendant_into_view(
  workspace: &Workspace,
  container: &Container,
  align: &ScrollAlign,
) -> anyhow::Result<bool> {
  if !super::supports_scrolling(workspace) {
    return Ok(false);
  }

  let Some(strip_item) = strip_item_of(workspace, container) else {
    return Ok(false);
  };

  let axis = workspace.tiling_direction();
  let viewport = AxisSpan::of_rect(&workspace.to_rect()?, &axis);
  let item = AxisSpan::of_rect(&strip_item.to_rect()?, &axis);

  let new_offset = (workspace.scroll_offset()
    + scroll_delta(align, viewport, item))
  .clamp(0, max_scroll_offset(workspace)?);

  Ok(workspace.set_scroll_offset(new_offset))
}

/// Next preset size to cycle a strip item of size `current` to.
///
/// Cycling forward picks the smallest preset that is larger than
/// `current`, and wraps around to the smallest preset. Cycling in reverse
/// picks the largest preset that is smaller than `current`, and wraps
/// around to the largest preset. Sizes within a small tolerance of a
/// preset count as that preset, so that rounding doesn't get a strip
/// item stuck.
///
/// Returns `None` if `presets` is empty. `presets` is expected to be
/// sorted in ascending order.
pub fn next_preset(
  current: f32,
  presets: &[f32],
  reverse: bool,
) -> Option<f32> {
  /// Tolerance for treating a size as equal to a preset.
  const TOLERANCE: f32 = 0.001;

  if reverse {
    presets
      .iter()
      .rev()
      .find(|preset| **preset < current - TOLERANCE)
      .or_else(|| presets.last())
      .copied()
  } else {
    presets
      .iter()
      .find(|preset| **preset > current + TOLERANCE)
      .or_else(|| presets.first())
      .copied()
  }
}

/// Gets the index of the top-level strip item that contains the given
/// descendant.
///
/// Returns `None` if the container is not a descendant of this
/// workspace.
fn strip_item_index(
  workspace: &Workspace,
  descendant: &Container,
) -> Option<usize> {
  strip_item_of(workspace, descendant).map(|strip_item| strip_item.index())
}

/// Default tiling size for a new strip item, from the workspace's
/// `scroll.window_width` (horizontal scroll axis) or
/// `scroll.window_height` (vertical scroll axis) config.
///
/// The current scroll axis is used rather than the configured
/// `scroll.axis`, since the axis can be flipped at runtime (see
/// `super::toggle_child_direction`).
pub(super) fn default_item_size(workspace: &Workspace) -> f32 {
  workspace
    .config()
    .scroll
    .default_item_size(&workspace.tiling_direction())
}

/// Rect of a top-level child of a scrolling workspace.
///
/// The child's extent along the workspace's scroll axis is its
/// `tiling_size` interpreted as a fraction of the viewport extent (see
/// [`item_extent`]). Strip items are laid out in order with an inner gap
/// between each pair of adjacent items, and the strip is shifted along
/// the scroll axis by the workspace's current scroll offset.
pub(super) fn child_rect(
  workspace: &Workspace,
  child: &TilingContainer,
  parent_rect: &Rect,
) -> anyhow::Result<Rect> {
  let scroll_axis = workspace.tiling_direction();

  let viewport = match scroll_axis {
    TilingDirection::Horizontal => parent_rect.width(),
    TilingDirection::Vertical => parent_rect.height(),
  };

  let (horizontal_gap, vertical_gap) = child.inner_gaps()?;
  let inner_gap = match scroll_axis {
    TilingDirection::Vertical => vertical_gap,
    TilingDirection::Horizontal => horizontal_gap,
  };

  // Offset of this child from the start of the strip (i.e. the sum of
  // the extents and gaps of all preceding strip items).
  let preceding_extent = child
    .prev_siblings()
    .filter_map(|sibling| sibling.as_tiling_container().ok())
    .map(|sibling| item_extent(&sibling, viewport) + inner_gap)
    .sum::<i32>();

  let along_extent = strip_item_extent(workspace, child, viewport);
  let along_start = match scroll_axis {
    TilingDirection::Horizontal => parent_rect.x(),
    TilingDirection::Vertical => parent_rect.y(),
  } - workspace.scroll_offset()
    + preceding_extent;

  Ok(match scroll_axis {
    TilingDirection::Horizontal => Rect::from_xy(
      along_start,
      parent_rect.y(),
      along_extent,
      parent_rect.height(),
    ),
    TilingDirection::Vertical => Rect::from_xy(
      parent_rect.x(),
      along_start,
      parent_rect.width(),
      along_extent,
    ),
  })
}

/// Distance beyond the edge of the virtual screen that parked windows
/// are moved to, so that their shadows stay off-screen too.
const PARK_MARGIN_PX: i32 = 64;

/// Applies positioning rules for a tiling window of a displayed
/// scrolling workspace.
///
/// Tiling windows that are scrolled fully outside the viewport are
/// clamped so that their edge sits flush just beyond the viewport's
/// edge, and are reported as scrolled out. Clamping pairs with hiding
/// the window in `redraw_containers`: cloaking guards against
/// peek-through while the cloak is applied (see
/// `docs/scrolling-layout.md` §5). With `OffscreenWindows::Park`, the
/// window is parked beyond the edges of all monitors instead of being
/// hidden.
pub(super) fn placement(
  window: &TilingWindow,
  rect: Rect,
) -> anyhow::Result<WindowPlacement> {
  let is_positioned = || WindowPlacement {
    rect: rect.clone(),
    is_scrolled_out: false,
    parked_rect: None,
  };

  let Some(workspace) = window.workspace() else {
    return Ok(is_positioned());
  };

  if !workspace.is_displayed() {
    return Ok(is_positioned());
  }

  let viewport_rect = workspace.to_rect()?;

  let (rect, is_scrolled_out) = match workspace.tiling_direction() {
    TilingDirection::Horizontal => {
      if rect.right <= viewport_rect.left {
        // Scrolled out of view to the left: clamp so that the right
        // edge of the window is flush with the viewport's left edge.
        (
          rect.translate_to_coordinates(
            viewport_rect.left - rect.width(),
            rect.top,
          ),
          true,
        )
      } else if rect.left >= viewport_rect.right {
        (
          rect.translate_to_coordinates(viewport_rect.right, rect.top),
          true,
        )
      } else {
        (rect, false)
      }
    }
    TilingDirection::Vertical => {
      if rect.bottom <= viewport_rect.top {
        (
          rect.translate_to_coordinates(
            rect.left,
            viewport_rect.top - rect.height(),
          ),
          true,
        )
      } else if rect.top >= viewport_rect.bottom {
        (
          rect.translate_to_coordinates(rect.left, viewport_rect.bottom),
          true,
        )
      } else {
        (rect, false)
      }
    }
  };

  let is_parked = is_scrolled_out
    && workspace.config().scroll.offscreen_windows
      == OffscreenWindows::Park;

  let parked_rect = is_parked
    .then(|| parked_rect(&workspace, &rect, &viewport_rect))
    .flatten();

  Ok(WindowPlacement {
    rect,
    is_scrolled_out,
    parked_rect,
  })
}

/// Rect beyond the edges of all monitors to park a scrolled-out window
/// at, on the side of the viewport that it's scrolled out of.
///
/// `clamped_rect` is the window's rect clamped just beyond the viewport's
/// edge. Returns `None` if the workspace isn't attached to a monitor.
fn parked_rect(
  workspace: &Workspace,
  clamped_rect: &Rect,
  viewport_rect: &Rect,
) -> Option<Rect> {
  let Container::Root(root) = workspace.monitor()?.parent()? else {
    return None;
  };

  let screen = root
    .monitors()
    .iter()
    .map(|monitor| monitor.native_properties().bounds)
    .reduce(|bounds, monitor_bounds| bounds.union(&monitor_bounds))?;

  Some(match workspace.tiling_direction() {
    TilingDirection::Horizontal => {
      let x = if clamped_rect.left >= viewport_rect.right {
        screen.right + PARK_MARGIN_PX
      } else {
        screen.left - clamped_rect.width() - PARK_MARGIN_PX
      };

      clamped_rect.translate_to_coordinates(x, clamped_rect.top)
    }
    TilingDirection::Vertical => {
      let y = if clamped_rect.top >= viewport_rect.bottom {
        screen.bottom + PARK_MARGIN_PX
      } else {
        screen.top - clamped_rect.height() - PARK_MARGIN_PX
      };

      clamped_rect.translate_to_coordinates(clamped_rect.left, y)
    }
  })
}

/// Normalizes a workspace's state for the scrolling layout.
///
/// Clamps the sizes of the workspace's top-level tiling children into
/// the range of valid viewport fractions, resets the scroll offset, and
/// scrolls the focused descendant into view (a no-op if the focused
/// container is not in the workspace).
pub(super) fn normalize(
  workspace: &Workspace,
  state: &WmState,
) -> anyhow::Result<()> {
  // Sizes of strip items are fractions of the viewport. Clamp sizes
  // that were set as shares of the workspace (e.g. `1/3` with three
  // windows) into the valid range.
  for child in workspace.tiling_children() {
    child
      .set_tiling_size(ScrollConfig::clamp_item_size(child.tiling_size()));
  }

  workspace.set_scroll_offset(0);

  if let Some(focused_container) = state.focused_container() {
    ensure_descendant_visible(workspace, &focused_container)?;
  }

  Ok(())
}

/// Where to insert a new window in a scrolling workspace.
///
/// Tiling windows always become top-level strip items (columns are only
/// created explicitly, e.g. by dropping a window onto another). The
/// position within the strip is given by the workspace's
/// `insert_position` scroll config.
///
/// Returns tuple of (parent container, insertion index).
pub(super) fn insertion_target(
  window_state: &WindowState,
  focused_container: &Container,
  focused_workspace: &Workspace,
) -> (Container, usize) {
  if *window_state != WindowState::Tiling {
    return (
      focused_workspace.clone().into(),
      focused_workspace.child_count(),
    );
  }

  let insert_index =
    match focused_workspace.config().scroll.insert_position {
      ScrollInsertPosition::AfterFocused => {
        focused_strip_item_index(focused_container, focused_workspace)
      }
      ScrollInsertPosition::End => focused_workspace.child_count(),
    };

  (focused_workspace.clone().into(), insert_index)
}

/// Gets the index directly after the strip item that contains the
/// focused container.
///
/// Falls back to the end of the workspace if it has no tiling windows.
fn focused_strip_item_index(
  focused_container: &Container,
  workspace: &Workspace,
) -> usize {
  let tiling_container = match focused_container {
    // Insert after the focused tiling window.
    Container::TilingWindow(_) => Some(focused_container.clone()),
    // If a non-tiling window is focused, insert after the first tiling
    // window found.
    _ => workspace
      .descendant_focus_order()
      .find(Container::is_tiling_window),
  };

  tiling_container
    .and_then(|container| strip_item_index(workspace, &container))
    .map_or(workspace.child_count(), |index| index + 1)
}

/// Inserts a window into the target workspace as part of moving it
/// there.
///
/// Tiling windows are inserted at the strip level directly after the
/// strip item that contains `insertion_sibling` (or appended to the end
/// of the strip if the target workspace has no tiling windows). Columns
/// are never entered, so the window always becomes a strip item of its
/// own.
pub(super) fn insert_window_into_workspace(
  window: &crate::models::WindowContainer,
  target_workspace: &Workspace,
  insertion_sibling: Option<crate::models::WindowContainer>,
  state: &mut WmState,
) -> anyhow::Result<()> {
  if window.is_tiling_window() {
    let insert_index = insertion_sibling
      .and_then(|sibling| {
        strip_item_index(target_workspace, &sibling.into())
      })
      .map_or(target_workspace.child_count(), |index| index + 1);

    return move_container_within_tree(
      &window.clone().into(),
      &target_workspace.clone().into(),
      insert_index,
      state,
    );
  }

  move_container_within_tree(
    &window.clone().into(),
    &target_workspace.clone().into(),
    target_workspace.child_count(),
    state,
  )
}

/// Moves a strip item in the given direction.
///
/// Moving along the scroll axis reorders the item within the strip;
/// moving cross-axis moves it to the workspace (or monitor) in that
/// direction. The workspace's tiling direction (i.e. its scroll axis)
/// is never inverted.
pub(super) fn move_child_in_direction(
  window: &TilingWindow,
  workspace: &Workspace,
  direction: &Direction,
  state: &mut WmState,
) -> anyhow::Result<()> {
  let parent: Container = workspace.clone().into();

  let has_matching_tiling_direction = workspace.tiling_direction()
    == TilingDirection::from_direction(direction);

  if has_matching_tiling_direction {
    if let Some(sibling) = tiling_sibling_in_direction(window, direction) {
      // Swap positions with the sibling strip item, which can be a
      // window or a column (split container). Sizes are preserved.
      move_container_within_tree(
        &window.clone().into(),
        &parent,
        sibling.index(),
        state,
      )?;

      state
        .pending_sync
        .queue_container_to_redraw(sibling)
        .queue_container_to_redraw(window.clone());

      return Ok(());
    }
  }

  move_to_workspace_in_direction(&window.clone().into(), direction, state)
}

/// Resizes a strip item directly along the scroll axis.
///
/// Strip items are sized as a fraction of the viewport, independent of
/// their siblings.
pub(super) fn resize_item(
  container_to_resize: &TilingContainer,
  target_size: f32,
) {
  container_to_resize
    .set_tiling_size(ScrollConfig::clamp_item_size(target_size));
}

/// Gets the factor by which a container's tiling size should be rescaled
/// when it moves out of a column (or other nested split) to become a
/// strip item of a scrolling workspace.
///
/// Strip items are sized as fractions of the viewport (along the scroll
/// axis), whereas nested containers are sized as fractions of their
/// parent's extent. For example, a window that fills half of a column
/// that is in turn half of the viewport ends up as a quarter of the
/// viewport.
///
/// Returns `1.0` when no rescale should be applied: when the container
/// already is a strip item, or when it is not a descendant of a
/// scrolling workspace.
pub(super) fn rescale_factor_for_move(
  container_to_move: &Container,
) -> f32 {
  // Get the top-level strip item containing the container (i.e. its
  // ancestor that is a direct child of a scrolling workspace). This can
  // be the container itself.
  let strip_item =
    container_to_move.self_and_ancestors().find(|ancestor| {
      ancestor.parent().is_some_and(|parent| {
        matches!(
          &parent,
          Container::Workspace(workspace)
            if super::supports_scrolling(workspace)
        )
      })
    });

  let Some(strip_item) = strip_item else {
    return 1.;
  };

  // The container is already a strip item, so its size already is a
  // fraction of the viewport.
  if strip_item.id() == container_to_move.id() {
    return 1.;
  }

  // Multiply the sizes of the ancestors from the container's parent up to
  // and including the strip item. All of these are tiling containers.
  let mut rescale_factor = 1.;

  for ancestor in container_to_move.self_and_ancestors().skip(1) {
    if let Ok(tiling_ancestor) = ancestor.as_tiling_container() {
      rescale_factor *= tiling_ancestor.tiling_size();
    }

    if ancestor.id() == strip_item.id() {
      break;
    }
  }

  rescale_factor
}

#[cfg(test)]
mod tests {
  use wm_common::{GapsConfig, ScrollConfig, TilingDirection};

  use super::*;
  use crate::test_utils::mock_scrolling_workspace;

  #[test]
  fn clamps_horizontally_scrolled_out_windows() {
    let (_workspace, windows) = mock_scrolling_workspace(
      TilingDirection::Horizontal,
      1,
      0.5,
      &GapsConfig::default(),
      ScrollConfig::default(),
    );

    // Viewport is the monitor's working area: (0, 0, 1680, 1000).
    let window = &windows[0];

    // Fully out of view to the right: clamp so that the left edge of
    // the window is flush with the viewport's right edge.
    let rect = Rect::from_xy(3000, 0, 840, 1000);
    let result = placement(window, rect).unwrap();
    assert!(result.is_scrolled_out);
    assert_eq!(result.rect.x(), 1680);
    assert_eq!(result.rect.width(), 840);

    // Fully out of view to the left: clamp so that the right edge of
    // the window is flush with the viewport's left edge.
    let rect = Rect::from_xy(-2000, 0, 840, 1000);
    let result = placement(window, rect).unwrap();
    assert!(result.is_scrolled_out);
    assert_eq!(result.rect.right, 0);

    // Partially visible windows keep their natural rect.
    let rect = Rect::from_xy(1500, 0, 840, 1000);
    let result = placement(window, rect).unwrap();
    assert!(!result.is_scrolled_out);
    assert_eq!(result.rect.x(), 1500);
  }

  #[test]
  fn parks_scrolled_out_windows_beyond_all_monitors() {
    let (workspace, windows) = mock_scrolling_workspace(
      TilingDirection::Horizontal,
      1,
      0.5,
      &GapsConfig::default(),
      ScrollConfig {
        offscreen_windows: OffscreenWindows::Park,
        ..ScrollConfig::default()
      },
    );

    // Parking needs the monitor setup, so the workspace's monitor is
    // attached to the container tree.
    let state = crate::wm_state::WmState::mock();
    crate::commands::container::attach_container(
      &workspace.parent().unwrap(),
      &state.root_container.clone().into(),
      None,
    )
    .unwrap();

    let screen = workspace.monitor().unwrap().native_properties().bounds;
    let window = &windows[0];

    let result =
      placement(window, Rect::from_xy(3000, 0, 840, 1000)).unwrap();
    let parked = result.parked_rect.clone().unwrap();

    // The window appears just beyond the viewport, but is parked beyond
    // the edge of all monitors.
    assert!(result.is_scrolled_out);
    assert!(!result.is_hidden_by_layout());
    assert_eq!(result.rect.x(), 1680);
    assert!(parked.left > screen.right);
    assert_eq!(result.window_rect(), &parked);

    let result =
      placement(window, Rect::from_xy(-2000, 0, 840, 1000)).unwrap();
    assert!(result.parked_rect.unwrap().right < screen.left);

    // Windows in view aren't parked.
    let result =
      placement(window, Rect::from_xy(0, 0, 840, 1000)).unwrap();
    assert!(result.parked_rect.is_none());
  }

  #[test]
  fn hides_scrolled_out_windows_by_default() {
    let (_workspace, windows) = mock_scrolling_workspace(
      TilingDirection::Horizontal,
      1,
      0.5,
      &GapsConfig::default(),
      ScrollConfig::default(),
    );

    let result =
      placement(&windows[0], Rect::from_xy(3000, 0, 840, 1000)).unwrap();

    assert!(result.parked_rect.is_none());
    assert!(result.is_hidden_by_layout());
  }

  #[test]
  fn clamps_vertically_scrolled_out_windows() {
    let (_workspace, windows) = mock_scrolling_workspace(
      TilingDirection::Vertical,
      1,
      0.5,
      &GapsConfig::default(),
      ScrollConfig::default(),
    );

    let window = &windows[0];

    // Fully out of view at the bottom: clamp so that the top edge of
    // the window is flush with the viewport's bottom edge.
    let rect = Rect::from_xy(0, 3000, 1680, 500);
    let result = placement(window, rect).unwrap();
    assert!(result.is_scrolled_out);
    assert_eq!(result.rect.y(), 1000);
    assert_eq!(result.rect.height(), 500);

    // Fully out of view at the top: clamp so that the bottom edge of
    // the window is flush with the viewport's top edge.
    let rect = Rect::from_xy(0, -2000, 1680, 500);
    let result = placement(window, rect).unwrap();
    assert!(result.is_scrolled_out);
    assert_eq!(result.rect.bottom, 0);
  }

  #[test]
  fn default_item_size_follows_scroll_axis() {
    let scroll_config = ScrollConfig {
      window_width: 0.4,
      window_height: 0.8,
      ..ScrollConfig::default()
    };

    let (workspace, _windows) = mock_scrolling_workspace(
      TilingDirection::Horizontal,
      1,
      0.5,
      &GapsConfig::default(),
      scroll_config,
    );

    assert!((default_item_size(&workspace) - 0.4).abs() < f32::EPSILON);

    // Flipping the scroll axis at runtime (e.g. `toggle-tiling-direction`)
    // switches to the size configured for the new axis.
    workspace.set_tiling_direction(TilingDirection::Vertical);
    assert!((default_item_size(&workspace) - 0.8).abs() < f32::EPSILON);
  }

  #[test]
  fn cycles_to_next_preset() {
    let presets = [0.25, 0.5, 1.0];

    assert_eq!(next_preset(0.25, &presets, false), Some(0.5));
    assert_eq!(next_preset(1.0, &presets, false), Some(0.25));
    assert_eq!(next_preset(0.5, &presets, true), Some(0.25));
    assert_eq!(next_preset(0.25, &presets, true), Some(1.0));

    // Sizes between presets go to the adjacent preset in the cycle
    // direction.
    assert_eq!(next_preset(0.3, &presets, false), Some(0.5));
    assert_eq!(next_preset(0.3, &presets, true), Some(0.25));

    // Sizes within the tolerance of a preset count as that preset.
    assert_eq!(next_preset(0.5004, &presets, false), Some(1.0));
    assert_eq!(next_preset(0.4996, &presets, true), Some(0.25));

    assert_eq!(next_preset(0.5, &[], false), None);
  }

  #[test]
  fn computes_minimal_scroll_delta() {
    let viewport = AxisSpan {
      start: 0,
      extent: 1000,
    };

    let delta = |start, extent| {
      scroll_delta(
        &ScrollAlign::Minimal,
        viewport,
        AxisSpan { start, extent },
      )
    };

    // Fully visible items don't scroll.
    assert_eq!(delta(100, 500), 0);
    // Items cut off at the start scroll back by the cut-off distance.
    assert_eq!(delta(-200, 500), -200);
    // Items cut off at the end scroll forward by the cut-off distance.
    assert_eq!(delta(800, 500), 300);
  }

  #[test]
  fn computes_center_scroll_delta() {
    let viewport = AxisSpan {
      start: 100,
      extent: 1000,
    };

    let delta = |start, extent| {
      scroll_delta(
        &ScrollAlign::Center,
        viewport,
        AxisSpan { start, extent },
      )
    };

    assert_eq!(delta(400, 400), 0);
    assert_eq!(delta(900, 400), 500);
    assert_eq!(delta(-300, 400), -700);
  }
}
