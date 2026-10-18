//! Layout system for workspaces.
//!
//! Each workspace arranges its top-level tiling children according to its
//! layout ([`WorkspaceLayout`]). The layout is set per workspace in the
//! config and can be switched at runtime with the `set-layout` and
//! `cycle-layout` commands.
//!
//! This module is the single place that knows how each layout behaves.
//! The rest of the codebase asks layout-agnostic questions, such as
//! "what rect does this child get?" or "what happens to sizes when a
//! child is attached?". Call sites outside of this module never match on
//! a workspace's layout.
//!
//! # Layouts
//!
//! - [`WorkspaceLayout::Tiling`]: children share the viewport; each
//!   child's tiling size is its share of the space alongside its siblings.
//!   Implemented in [`tiling`].
//! - [`WorkspaceLayout::Scrolling`]: children form a strip along the
//!   workspace's scroll axis; each child's tiling size is a fraction of
//!   the viewport, and strip items can be scrolled out of view.
//!   Implemented in [`scrolling`].
//! - [`WorkspaceLayout::Bsp`]: children form a binary tree of split
//!   containers; inserting a window splits the focused window's space in
//!   half. Implemented in [`bsp`].
//!
//! # Adding a layout
//!
//! 1. Add a variant to [`WorkspaceLayout`] (in the `wm-common` package).
//! 2. Implement the layout's behavior in a new submodule of this module,
//!    next to [`tiling`] and [`scrolling`].
//! 3. Dispatch to it from the behavior functions in this module. The
//!    matches on [`WorkspaceLayout`] are exhaustive, so the compiler
//!    points out every function that needs a new arm.

pub(crate) mod bsp;

pub mod scrolling;

pub(crate) mod tiling;

use anyhow::Context;
use wm_common::{
  LayoutSwitchBehavior, TilingDirection, WindowState, WorkspaceConfig,
  WorkspaceLayout,
};
use wm_platform::{Direction, Rect};

use crate::{
  commands::window::move_to_workspace_in_direction,
  models::{
    Container, DirectionContainer, TilingContainer, TilingWindow,
    WindowContainer, Workspace,
  },
  traits::{
    CommonGetters, PositionGetters, TilingDirectionGetters, WindowGetters,
  },
  user_config::UserConfig,
  wm_state::WmState,
};

/// Rect to position a window at, paired with whether the layout has
/// scrolled the window out of its workspace's viewport.
pub struct WindowPlacement {
  /// Rect (including border delta) where the window appears. The rect of
  /// windows that are scrolled out of view is clamped just beyond the
  /// edge of the viewport.
  pub rect: Rect,

  /// Whether the window is a tiling window that the layout has scrolled
  /// out of its workspace's viewport.
  pub is_scrolled_out: bool,

  /// Rect beyond the edges of all monitors that a scrolled-out window is
  /// parked at instead of being hidden (see `OffscreenWindows::Park`).
  pub parked_rect: Option<Rect>,
}

impl WindowPlacement {
  /// Rect to position the window itself at: the parked rect for parked
  /// windows, and `rect` otherwise.
  #[must_use]
  pub fn window_rect(&self) -> &Rect {
    self.parked_rect.as_ref().unwrap_or(&self.rect)
  }

  /// Whether the layout hides the window (i.e. it's scrolled out of view
  /// and not parked).
  #[must_use]
  pub fn is_hidden_by_layout(&self) -> bool {
    self.is_scrolled_out && self.parked_rect.is_none()
  }
}

/// The layout of `container` when it is a direction container (i.e. a
/// container that arranges tiling children).
///
/// Only workspaces have a configurable layout; anything else has tiling
/// semantics.
fn layout_of_direction_container(
  container: &DirectionContainer,
) -> WorkspaceLayout {
  match container {
    DirectionContainer::Workspace(workspace) => workspace.layout(),
    DirectionContainer::Split(_) => WorkspaceLayout::Tiling,
  }
}

/// The layout of the workspace that `container` belongs to, or the
/// tiling layout for containers that arrange children with tiling
/// semantics (e.g. split containers).
fn layout_of_container(container: &Container) -> WorkspaceLayout {
  match container {
    Container::Workspace(workspace) => workspace.layout(),
    _ => WorkspaceLayout::Tiling,
  }
}

/// Rect of a tiling child of the given workspace within the workspace's
/// viewport.
///
/// `child` is expected to be a direct child of `workspace`, and
/// `parent_rect` to be the workspace's rect.
pub fn child_rect(
  workspace: &Workspace,
  child: &TilingContainer,
  parent_rect: &Rect,
) -> anyhow::Result<Rect> {
  match workspace.layout() {
    WorkspaceLayout::Tiling | WorkspaceLayout::Bsp => {
      tiling::child_rect(child, workspace, parent_rect)
    }
    WorkspaceLayout::Scrolling => {
      scrolling::child_rect(workspace, child, parent_rect)
    }
  }
}

/// Default tiling size for a new direct child of the given workspace,
/// when the layout overrides the size that attaching gives it.
///
/// Layouts that size their children as shares of the space alongside
/// their siblings return `None`; the child is then sized by
/// [`on_child_attached`].
pub fn default_child_size(workspace: &Workspace) -> Option<f32> {
  match workspace.layout() {
    WorkspaceLayout::Tiling | WorkspaceLayout::Bsp => None,
    WorkspaceLayout::Scrolling => {
      Some(scrolling::default_item_size(workspace))
    }
  }
}

/// Extent (in pixels) that the tiling sizes of the direct children of
/// the given workspace are a fraction of, when the layout sizes them
/// relative to the workspace viewport.
///
/// Returns `None` for layouts that size direct children as shares of
/// the space alongside their siblings.
pub fn child_length_reference(
  workspace: &Workspace,
) -> anyhow::Result<Option<i32>> {
  match workspace.layout() {
    WorkspaceLayout::Tiling | WorkspaceLayout::Bsp => Ok(None),
    WorkspaceLayout::Scrolling => {
      Ok(Some(scrolling::viewport_extent(workspace)?))
    }
  }
}

/// Sizes `child` after it has been attached to `parent`.
///
/// The default tiling behavior sizes the child to an even share of the
/// space alongside its siblings. Layouts that size their children
/// independently (e.g. as fractions of the viewport) leave the sizes
/// untouched.
pub fn on_child_attached(child: &TilingContainer, parent: &Container) {
  match layout_of_container(parent) {
    WorkspaceLayout::Tiling | WorkspaceLayout::Bsp => {
      tiling::size_attached_child(child);
    }
    WorkspaceLayout::Scrolling => {}
  }
}

/// Redistributes the space freed up by detaching `child` from `parent`.
///
/// The default tiling behavior resizes the remaining siblings to fill
/// the freed up space. Layouts whose children are sized independently
/// leave the sizes untouched.
pub fn on_child_detached(child: &TilingContainer, parent: &Container) {
  match layout_of_container(parent) {
    WorkspaceLayout::Tiling | WorkspaceLayout::Bsp => {
      tiling::redistribute_freed_space(child, parent);
    }
    WorkspaceLayout::Scrolling => {}
  }
}

/// Resizes a tiling container so that its tiling size becomes the given
/// target size.
///
/// The sizes that are freed up by the resize are distributed amongst the
/// container's tiling siblings. Layouts whose direct children are sized
/// independently (e.g. strip items) resize the child directly instead.
pub fn resize_tiling_container(
  container: &TilingContainer,
  target_size: f32,
) {
  let layout = match container.parent() {
    Some(parent) => layout_of_container(&parent),
    None => WorkspaceLayout::Tiling,
  };

  match layout {
    WorkspaceLayout::Tiling | WorkspaceLayout::Bsp => {
      tiling::resize_container(container, target_size);
    }
    WorkspaceLayout::Scrolling => {
      scrolling::resize_item(container, target_size);
    }
  }
}

/// Rule for resizing a container that is a direct child of a direction
/// container.
///
/// Some layouts resize their direct children directly (e.g. strip items
/// are resized along the scroll axis), instead of going through the
/// generic sibling-based resize logic.
pub enum DirectChildResize {
  /// Resize the container directly.
  Resize,

  /// Skip the resize entirely.
  Skip,

  /// No layout-specific rule; use the generic sibling-based logic.
  FallThrough,
}

/// Gets the resize rule for a direct child of `parent`.
pub fn direct_child_resize(
  parent: &DirectionContainer,
  is_width_resize: bool,
) -> DirectChildResize {
  match layout_of_direction_container(parent) {
    WorkspaceLayout::Tiling | WorkspaceLayout::Bsp => {
      DirectChildResize::FallThrough
    }
    WorkspaceLayout::Scrolling => {
      // A strip item is resized directly along the scroll axis,
      // regardless of whether it has siblings. Cross-axis resizes are
      // ignored, since strip items fill the viewport cross-axis.
      let is_inverse_resize = match parent.tiling_direction() {
        TilingDirection::Horizontal => !is_width_resize,
        TilingDirection::Vertical => is_width_resize,
      };

      if is_inverse_resize {
        DirectChildResize::Skip
      } else {
        DirectChildResize::Resize
      }
    }
  }
}

/// How `flatten_child_split_containers` treats redundant split children
/// of a direction container.
pub enum SplitChildFlattening {
  /// Fold redundant split children into the parent (the default tiling
  /// behavior).
  Fold,

  /// Keep split children as first-class "columns": a column is only
  /// redundant when it contains a single window, and the parent's
  /// tiling direction is never inverted (which would flip the strip).
  PreserveColumns,

  /// Keep multi-child splits as they are (the binary tree is the
  /// structure of the BSP layout); only splits that have a single child
  /// are collapsed.
  PreserveBinarySplits,
}

/// Gets how redundant split children of `parent` are cleaned up.
///
/// For the BSP layout, splits nested inside a BSP workspace are kept as
/// binary splits too, since they are part of the workspace's tree.
/// Splits nested in other layouts have the default tiling behavior
/// (e.g. the interiors of a scrolling workspace's columns).
pub fn split_child_flattening(
  parent: &DirectionContainer,
) -> SplitChildFlattening {
  match parent {
    DirectionContainer::Workspace(workspace) => match workspace.layout() {
      WorkspaceLayout::Tiling => SplitChildFlattening::Fold,
      WorkspaceLayout::Scrolling => SplitChildFlattening::PreserveColumns,
      WorkspaceLayout::Bsp => SplitChildFlattening::PreserveBinarySplits,
    },
    DirectionContainer::Split(split) => {
      let is_in_bsp_workspace =
        split.workspace().is_some_and(|workspace| {
          workspace.layout() == WorkspaceLayout::Bsp
        });

      if is_in_bsp_workspace {
        SplitChildFlattening::PreserveBinarySplits
      } else {
        SplitChildFlattening::Fold
      }
    }
  }
}

/// Gets where to insert a new window in the container tree.
///
/// Returns tuple of (parent container, insertion index).
pub fn insertion_target(
  window_state: &WindowState,
  state: &WmState,
) -> anyhow::Result<(Container, usize)> {
  let focused_container =
    state.focused_container().context("No focused container.")?;

  let focused_workspace =
    focused_container.workspace().context("No workspace.")?;

  match focused_workspace.layout() {
    WorkspaceLayout::Tiling => tiling::insertion_target(
      window_state,
      &focused_container,
      &focused_workspace,
    ),
    WorkspaceLayout::Scrolling => Ok(scrolling::insertion_target(
      window_state,
      &focused_container,
      &focused_workspace,
    )),
    WorkspaceLayout::Bsp => bsp::insertion_target(
      window_state,
      &focused_container,
      &focused_workspace,
    ),
  }
}

/// Inserts a window into the target workspace as part of moving it there.
///
/// Layouts with a strip (or similar) insert tiling windows at the level
/// of their direct children, directly after the item that contains
/// `insertion_sibling`. The default tiling behavior inserts the window
/// into the sibling's parent, at the sibling's index + 1.
pub fn insert_window_into_workspace(
  window: &WindowContainer,
  target_workspace: &Workspace,
  insertion_sibling: Option<WindowContainer>,
  state: &mut WmState,
) -> anyhow::Result<()> {
  match target_workspace.layout() {
    WorkspaceLayout::Tiling => tiling::insert_window_into_workspace(
      window,
      target_workspace,
      insertion_sibling,
      state,
    ),
    WorkspaceLayout::Scrolling => scrolling::insert_window_into_workspace(
      window,
      target_workspace,
      insertion_sibling,
      state,
    ),
    WorkspaceLayout::Bsp => bsp::insert_window_into_workspace(
      window,
      target_workspace,
      insertion_sibling,
      state,
    ),
  }
}

/// Handles moving a window in the given direction when the window is a
/// direct child of a workspace.
///
/// Returns `None` when the workspace's layout uses the generic move
/// behavior, or `Some` with the result of the layout-specific move.
pub fn move_child_in_direction(
  window: &TilingWindow,
  direction: &Direction,
  state: &mut WmState,
) -> Option<anyhow::Result<()>> {
  let parent = window.direction_container()?;

  let DirectionContainer::Workspace(workspace) = &parent else {
    return None;
  };

  match workspace.layout() {
    WorkspaceLayout::Tiling | WorkspaceLayout::Bsp => None,
    WorkspaceLayout::Scrolling => {
      Some(scrolling::move_child_in_direction(
        window, workspace, direction, state,
      ))
    }
  }
}

/// Handles moving a window in the given direction when the window has no
/// ancestor that can contain the move.
///
/// The default tiling behavior inverts the workspace's tiling direction
/// to make room for the window. Strip-style layouts never flip their
/// axis and move the window to the workspace in the given direction
/// instead.
pub fn move_without_target_ancestor(
  window: &TilingWindow,
  direction: &Direction,
  state: &mut WmState,
  config: &UserConfig,
) -> anyhow::Result<()> {
  let workspace = window.workspace().context("No workspace.")?;

  match workspace.layout() {
    WorkspaceLayout::Tiling | WorkspaceLayout::Bsp => {
      tiling::invert_workspace_direction(window, direction, state, config)
    }
    WorkspaceLayout::Scrolling => move_to_workspace_in_direction(
      &window.clone().into(),
      direction,
      state,
    ),
  }
}

/// Handles toggling the tiling direction of a direct child of `parent`.
///
/// Returns the direction container whose direction changed, or `None`
/// when the layout uses the generic direction toggle behavior.
pub fn toggle_child_direction(
  parent: &DirectionContainer,
) -> Option<DirectionContainer> {
  match layout_of_direction_container(parent) {
    WorkspaceLayout::Tiling | WorkspaceLayout::Bsp => None,
    WorkspaceLayout::Scrolling => {
      let DirectionContainer::Workspace(workspace) = parent else {
        return None;
      };

      // Toggling the direction of a strip item flips the workspace's
      // scroll axis. Wrapping the item in a split container would
      // create an unwanted column.
      workspace
        .set_tiling_direction(workspace.tiling_direction().inverse());

      Some(workspace.clone().into())
    }
  }
}

/// Factor by which a container's tiling size is rescaled when it is
/// moved into the given target parent.
///
/// The default behavior is `1.0` (the size is kept). Layouts whose
/// direct children are sized relative to the workspace viewport rescale
/// containers that leave a nested parent, so that the fraction of the
/// viewport that the container fills is preserved.
pub fn rescale_factor_for_move(
  container_to_move: &Container,
  target_parent: &Container,
) -> f32 {
  let Container::Workspace(target_workspace) = target_parent else {
    return 1.;
  };

  match target_workspace.layout() {
    WorkspaceLayout::Tiling | WorkspaceLayout::Bsp => 1.,
    WorkspaceLayout::Scrolling => {
      scrolling::rescale_factor_for_move(container_to_move)
    }
  }
}

/// Tiling direction to initialize a newly activated workspace with.
pub fn initial_tiling_direction(
  config: &WorkspaceConfig,
  monitor_rect: &Rect,
) -> TilingDirection {
  match config.layout {
    // Scrolling workspaces initialize their scroll axis from config.
    WorkspaceLayout::Scrolling => config.scroll.axis.clone(),
    // Default the tiling direction based on the monitor's orientation.
    WorkspaceLayout::Tiling | WorkspaceLayout::Bsp => {
      if monitor_rect.height() > monitor_rect.width() {
        TilingDirection::Vertical
      } else {
        TilingDirection::Horizontal
      }
    }
  }
}

/// Whether the workspace's layout supports scrolling its children into
/// and out of the viewport (e.g. via the `scroll` command).
pub fn supports_scrolling(workspace: &Workspace) -> bool {
  match workspace.layout() {
    WorkspaceLayout::Tiling | WorkspaceLayout::Bsp => false,
    WorkspaceLayout::Scrolling => true,
  }
}

/// Whether hit tests must be limited to the workspace viewport, because
/// the layout can place tiling windows outside of it.
pub fn clips_children_to_viewport(workspace: &Workspace) -> bool {
  match workspace.layout() {
    WorkspaceLayout::Tiling | WorkspaceLayout::Bsp => false,
    WorkspaceLayout::Scrolling => true,
  }
}

/// Computes the rect to position the given window at, applying the
/// layout's placement rules.
///
/// Tiling windows that are fully outside the viewport of a displayed
/// workspace are clamped so that their edge sits flush just beyond the
/// viewport's edge. This pairs with hiding the window in
/// `redraw_containers`: cloaking guards against peek-through while the
/// cloak is applied (see `docs/scrolling-layout.md` §5).
pub fn window_placement(
  window: &WindowContainer,
) -> anyhow::Result<WindowPlacement> {
  let rect = window
    .to_rect()?
    .apply_delta(&window.total_border_delta()?, None);

  let is_positioned = || WindowPlacement {
    rect: rect.clone(),
    is_scrolled_out: false,
    parked_rect: None,
  };

  let WindowContainer::TilingWindow(window) = window else {
    return Ok(is_positioned());
  };

  let Some(workspace) = window.workspace() else {
    return Ok(is_positioned());
  };

  match workspace.layout() {
    WorkspaceLayout::Tiling | WorkspaceLayout::Bsp => Ok(is_positioned()),
    WorkspaceLayout::Scrolling => scrolling::placement(window, rect),
  }
}

/// How the existing windows of a workspace are treated when its layout
/// is switched: the workspace's own `layout_switch` config, falling back
/// to `general.layout_switch`.
pub fn layout_switch_behavior(
  workspace_config: &WorkspaceConfig,
  config: &UserConfig,
) -> LayoutSwitchBehavior {
  workspace_config
    .layout_switch
    .unwrap_or(config.value.general.layout_switch)
}

/// Adjusts a workspace's state after its layout was switched.
///
/// The scrolling layout normalizes sizes and scroll position so that the
/// focused container stays visible. The tiling layout rescales the sizes
/// of the workspace's children so that they fill the viewport; sizes set
/// while the scrolling layout was active are viewport fractions that can
/// sum to more than the full viewport (see the size semantics in
/// `docs/scrolling-layout.md`).
///
/// With [`LayoutSwitchBehavior::Reflow`], the existing children are also
/// rearranged into the new layout: the BSP layout rebuilds them as a
/// balanced binary tree (the tree is the structure of the layout
/// itself), and switching from BSP to scrolling turns the windows of the
/// tree into separate strip items. With [`LayoutSwitchBehavior::Keep`],
/// the arrangement is left as-is and only sizes are adjusted where the
/// new layout requires it.
pub fn on_layout_changed(
  workspace: &Workspace,
  prev_layout: &WorkspaceLayout,
  new_layout: &WorkspaceLayout,
  behavior: LayoutSwitchBehavior,
  state: &WmState,
) -> anyhow::Result<()> {
  if prev_layout == new_layout {
    return Ok(());
  }

  let is_reflow = behavior == LayoutSwitchBehavior::Reflow;

  match new_layout {
    WorkspaceLayout::Tiling => {
      // A BSP tree is made of even splits, so its sizes already fill
      // the viewport.
      tiling::normalize_sizes(workspace);
      Ok(())
    }
    WorkspaceLayout::Scrolling => {
      // The BSP tree's root split holds the entire mosaic; dissolve it
      // so that its children become individual strip items.
      if is_reflow && prev_layout == &WorkspaceLayout::Bsp {
        bsp::collapse_root_split(workspace)?;
      }

      scrolling::normalize(workspace, state)
    }
    WorkspaceLayout::Bsp => {
      if is_reflow {
        bsp::rebuild_tree(workspace)
      } else {
        // The BSP layout positions its tree like the tiling layout, so
        // the existing children must fill the viewport.
        tiling::normalize_sizes(workspace);
        Ok(())
      }
    }
  }
}

/// Adjusts a workspace's state after its config was replaced on a config
/// reload.
///
/// A layout switch behaves like the `set-layout` command
/// ([`on_layout_changed`]). The scroll axis from the config is applied
/// as the workspace's tiling direction while the scrolling layout is
/// active.
pub fn on_workspace_config_reloaded(
  workspace: &Workspace,
  prev_config: &WorkspaceConfig,
  new_config: &WorkspaceConfig,
  behavior: LayoutSwitchBehavior,
  state: &WmState,
) -> anyhow::Result<()> {
  if prev_config.layout != new_config.layout {
    if new_config.layout == WorkspaceLayout::Scrolling {
      workspace.set_tiling_direction(new_config.scroll.axis.clone());
    }

    return on_layout_changed(
      workspace,
      &prev_config.layout,
      &new_config.layout,
      behavior,
      state,
    );
  }

  if new_config.layout == WorkspaceLayout::Scrolling
    && prev_config.scroll.axis != new_config.scroll.axis
  {
    workspace.set_tiling_direction(new_config.scroll.axis.clone());
  }

  Ok(())
}

#[cfg(test)]
mod tests {
  use wm_common::{GapsConfig, ScrollConfig, TilingDirection};

  use super::*;
  use crate::{
    commands::container::attach_container,
    models::{Monitor, SplitContainer, TilingWindow, Workspace},
    test_utils::mock_scrolling_workspace,
    traits::TilingSizeGetters,
  };

  #[test]
  fn resizes_strip_item_without_affecting_siblings() {
    let (_workspace, windows) = mock_scrolling_workspace(
      TilingDirection::Horizontal,
      2,
      0.5,
      &GapsConfig::default(),
      ScrollConfig::default(),
    );

    resize_tiling_container(&windows[0].clone().into(), 0.25);

    assert!((windows[0].tiling_size() - 0.25).abs() < f32::EPSILON);
    assert!((windows[1].tiling_size() - 0.5).abs() < f32::EPSILON);
  }

  #[test]
  fn clamps_strip_item_size() {
    let (_workspace, windows) = mock_scrolling_workspace(
      TilingDirection::Horizontal,
      1,
      0.5,
      &GapsConfig::default(),
      ScrollConfig::default(),
    );

    let strip_item = windows[0].clone();

    // A strip item can be at most the full viewport.
    resize_tiling_container(&strip_item.clone().into(), 2.0);
    assert!((strip_item.tiling_size() - 1.0).abs() < f32::EPSILON);

    // A strip item cannot be resized below the minimum strip item size.
    resize_tiling_container(&strip_item.clone().into(), 0.0);
    assert!(
      (strip_item.tiling_size() - ScrollConfig::MIN_ITEM_SIZE).abs()
        < f32::EPSILON
    );
  }

  #[test]
  fn redistributes_among_siblings_in_tiling_split() {
    let first = TilingWindow::mock().call();
    let second = TilingWindow::mock().call();

    let _split = SplitContainer::mock()
      .tiling_direction(TilingDirection::Horizontal)
      .tiling_containers(vec![first.clone().into(), second.clone().into()])
      .call();

    resize_tiling_container(&first.clone().into(), 0.25);

    assert!((first.tiling_size() - 0.25).abs() < f32::EPSILON);
    assert!((second.tiling_size() - 0.75).abs() < f32::EPSILON);
  }

  #[test]
  fn resolves_layout_switch_behavior() {
    let mut config = UserConfig::mock();
    let mut workspace_config = Workspace::mock().call().config();

    assert_eq!(
      layout_switch_behavior(&workspace_config, &config),
      LayoutSwitchBehavior::Reflow
    );

    config.value.general.layout_switch = LayoutSwitchBehavior::Keep;
    assert_eq!(
      layout_switch_behavior(&workspace_config, &config),
      LayoutSwitchBehavior::Keep
    );

    // The workspace's own config takes precedence.
    workspace_config.layout_switch = Some(LayoutSwitchBehavior::Reflow);
    assert_eq!(
      layout_switch_behavior(&workspace_config, &config),
      LayoutSwitchBehavior::Reflow
    );
  }

  #[test]
  fn keeps_rect_of_windows_of_other_layouts() {
    // A tiling window of a workspace that doesn't use the scrolling
    // layout is positioned at its natural rect and is never reported as
    // scrolled out, even when it lies outside of the workspace.
    let workspace = Workspace::mock().call();
    let window = TilingWindow::mock().call();

    attach_container(
      &window.clone().into(),
      &workspace.clone().into(),
      None,
    )
    .unwrap();

    let _monitor =
      Monitor::mock().workspaces(vec![workspace.clone()]).call();

    let expected_rect = window
      .to_rect()
      .unwrap()
      .apply_delta(&window.total_border_delta().unwrap(), None);

    let placement = window_placement(&window.clone().into()).unwrap();

    assert!(!placement.is_scrolled_out);
    assert_eq!(placement.rect, expected_rect);
  }
}
