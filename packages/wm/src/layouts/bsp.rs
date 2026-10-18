//! The binary space partitioning (BSP) layout.
//!
//! Tiling children form a binary tree of split containers: every split
//! divides its space between exactly two children, and each window is a
//! leaf of the tree (like the bspwm tiling model). Inserting a window
//! wraps the focused window in a new split container and puts the new
//! window next to it, halving the focused window's space. The split
//! direction of a new split is chosen per the workspace's
//! `bsp.automatic_scheme` config:
//!
//! - `longest_side` (default): splits the longer side of the focused
//!   window's rect. A wide rect is cut into side-by-side children, a tall
//!   rect into stacked children.
//! - `alternate`: alternates the split direction with the depth of the
//!   tree, starting from the workspace's tiling direction.
//!
//! The binary tree is the structure of the layout itself: it is rebuilt
//! when a workspace switches to the layout (see [`rebuild_tree`]), and
//! `flatten_child_split_containers` only collapses splits that have a
//! single child (see `super::split_child_flattening`).

use anyhow::Context;
use wm_common::{
  BspAutomaticScheme, GapsConfig, TilingDirection, VecDequeExt,
  WindowState,
};
use wm_platform::Rect;

use crate::{
  commands::container::{
    attach_container, flatten_split_container, move_container_within_tree,
    wrap_in_split_container,
  },
  models::{
    Container, SplitContainer, TilingContainer, WindowContainer, Workspace,
  },
  traits::{
    CommonGetters, PositionGetters, TilingDirectionGetters,
    TilingSizeGetters,
  },
  wm_state::WmState,
};

/// Gets where to insert a new window in the container tree.
///
/// Rules:
/// - For tiling windows:
///   1. Wrap the focused tiling window in a new split container, and
///      insert the new window next to it (halving the focused window's
///      space).
///   2. If a non-tiling window is focused, wrap the most recently focused
///      tiling window instead.
///   3. If no tiling windows exist, append to the workspace.
/// - For non-tiling windows: Always append to the workspace.
pub(super) fn insertion_target(
  window_state: &WindowState,
  focused_container: &Container,
  focused_workspace: &Workspace,
) -> anyhow::Result<(Container, usize)> {
  if *window_state == WindowState::Tiling {
    let sibling = match focused_container {
      Container::TilingWindow(_) => Some(focused_container.clone()),
      _ => focused_workspace
        .descendant_focus_order()
        .find(Container::is_tiling_window),
    };

    if let Some(sibling) = sibling {
      let split = split_for_insertion(&sibling, focused_workspace)?;
      let target_index = split.child_count();

      return Ok((split.into(), target_index));
    }
  }

  Ok((
    focused_workspace.clone().into(),
    focused_workspace.child_count(),
  ))
}

/// Inserts a window into the target workspace as part of moving it
/// there.
///
/// The window is inserted like a newly managed tiling window: the
/// insertion sibling is wrapped in a new split container, and the window
/// takes the other half of its space. Windows with no insertion sibling
/// are appended to the workspace.
pub(super) fn insert_window_into_workspace(
  window: &WindowContainer,
  target_workspace: &Workspace,
  insertion_sibling: Option<WindowContainer>,
  state: &mut WmState,
) -> anyhow::Result<()> {
  if window.is_tiling_window() {
    if let Some(insertion_sibling) = insertion_sibling {
      let split =
        split_for_insertion(&insertion_sibling.into(), target_workspace)?;

      let target_index = split.child_count();

      return move_container_within_tree(
        &window.clone().into(),
        &split.into(),
        target_index,
        state,
      );
    }
  }

  move_container_within_tree(
    &window.clone().into(),
    &target_workspace.clone().into(),
    target_workspace.child_count(),
    state,
  )
}

/// Wraps `sibling` in a new split container, inserted at the sibling's
/// position, and returns the new split.
///
/// The split's tiling direction is chosen per the workspace's configured
/// automatic scheme.
fn split_for_insertion(
  sibling: &Container,
  workspace: &Workspace,
) -> anyhow::Result<SplitContainer> {
  let parent = sibling.parent().context("No parent.")?;
  let tiling_sibling = sibling.as_tiling_container()?;
  let tiling_direction = insertion_direction(sibling, workspace)?;

  let split =
    SplitContainer::new(tiling_direction, workspace.gaps_config());

  wrap_in_split_container(
    &split,
    &parent,
    std::slice::from_ref(&tiling_sibling),
  )?;

  Ok(split)
}

/// Tiling direction of the split that wraps `sibling` on insertion.
///
/// - `longest_side`: splits the longer side of the sibling's rect (see
///   [`longest_side_direction`]).
/// - `alternate`: inverts the sibling's parent split direction, falling
///   back to the workspace's tiling direction at the root level.
fn insertion_direction(
  sibling: &Container,
  workspace: &Workspace,
) -> anyhow::Result<TilingDirection> {
  match workspace.config().bsp.automatic_scheme {
    BspAutomaticScheme::LongestSide => {
      // Fall back to the workspace rect for containers that have no
      // rect yet.
      let rect = sibling.to_rect().or_else(|_| workspace.to_rect())?;

      Ok(longest_side_direction(&rect))
    }
    BspAutomaticScheme::Alternate => {
      let parent_direction = sibling.parent().and_then(|parent| {
        parent
          .as_split()
          .map(TilingDirectionGetters::tiling_direction)
      });

      Ok(match parent_direction {
        Some(direction) => direction.inverse(),
        None => workspace.tiling_direction(),
      })
    }
  }
}

/// Direction to halve `rect` along: a wide rect is cut by a vertical
/// split line into side-by-side children (a horizontal tiling
/// direction), a tall rect into stacked children.
fn longest_side_direction(rect: &Rect) -> TilingDirection {
  if rect.width() >= rect.height() {
    TilingDirection::Horizontal
  } else {
    TilingDirection::Vertical
  }
}

/// Rebuilds the workspace's tiling children as a balanced binary split
/// tree.
///
/// The binary tree is what defines the BSP layout, so switching to it
/// arranges the workspace's children as a mosaic instead of applying
/// another layout's tree as-is. Each tiling child at the time of the
/// rebuild becomes a leaf of the new tree: windows stay windows, and
/// nested subtrees keep their internal structure. Sizes are reset so
/// that every split divides its space evenly between its two children.
pub(super) fn rebuild_tree(workspace: &Workspace) -> anyhow::Result<()> {
  let children = workspace.tiling_children().collect::<Vec<_>>();

  // A single tiling child fills the workspace as-is.
  if children.len() == 1 {
    children[0].set_tiling_size(1.0);
    return Ok(());
  }

  if children.is_empty() {
    return Ok(());
  }

  // The leaf that is first in focus order (where focus falls back to in
  // this workspace). Captured before detaching: the rebuilt tree's focus
  // order would otherwise start out in attach order.
  let focus_target = workspace.descendant_focus_order().next();

  // Detach the children from the workspace without redistributing the
  // freed up space; the tree is built from scratch below.
  let child_ids =
    children.iter().map(CommonGetters::id).collect::<Vec<_>>();

  workspace
    .borrow_children_mut()
    .retain(|child| !child_ids.contains(&child.id()));

  workspace
    .borrow_child_focus_order_mut()
    .retain(|id| !child_ids.contains(id));

  for child in &children {
    *child.borrow_parent_mut() = None;
  }

  let tree = build_balanced_tree(
    children,
    &workspace.to_rect()?,
    &workspace.tiling_direction(),
    0,
    &workspace.config().bsp.automatic_scheme,
    &workspace.gaps_config(),
  )?;

  attach_container(&tree.clone().into(), &workspace.clone().into(), None)?;

  // Detaching and re-attaching the children put the new tree at the back
  // of the workspace's focus order, and its splits in attach order. The
  // focused container is resolved from focus order (see
  // `WmState::focused_container`), so restore the captured leaf as the
  // first in focus order — without this, the first window of the new
  // tree would look like the focused one (e.g. a window opened right
  // after switching layouts would be inserted next to the wrong window).
  if let Some(focus_target) =
    focus_target.filter(Container::is_tiling_window)
  {
    promote_focus_order(&focus_target, workspace);
  }

  Ok(())
}

/// Shifts `focus_target` and its ancestors to the front of their
/// parents' focus order, up to and including the workspace's child.
///
/// The workspace itself is left as-is in its parent's focus order, so
/// rebuilding a workspace that is not the focused one doesn't change
/// which workspace is treated as focused.
fn promote_focus_order(focus_target: &Container, workspace: &Workspace) {
  let mut target = focus_target.clone();

  while let Some(parent) = target.parent() {
    parent
      .borrow_child_focus_order_mut()
      .shift_to_index(0, target.id());

    if parent.id() == workspace.id() {
      break;
    }

    target = parent;
  }
}

/// Recursively builds a balanced binary split tree from `items`, in
/// order, for the given `rect` (the space that `items` fill).
///
/// The items are split at the middle: the first half becomes the left
/// (or top) child and the second half the right (or bottom) child, and
/// each half is built recursively. The direction of each split follows
/// the workspace's automatic scheme, with the same rules as
/// [`insertion_direction`] applied to the space at that depth.
fn build_balanced_tree(
  items: Vec<TilingContainer>,
  rect: &Rect,
  base_direction: &TilingDirection,
  depth: usize,
  automatic_scheme: &BspAutomaticScheme,
  gaps_config: &GapsConfig,
) -> anyhow::Result<TilingContainer> {
  if items.len() == 1 {
    let leaf = items.into_iter().next().context("No items.")?;
    leaf.set_tiling_size(1.0);
    return Ok(leaf);
  }

  // The left (or top) half gets the extra item when the count is odd.
  let mut items = items;
  let right_items = items.split_off(items.len().div_ceil(2));

  let tiling_direction = match automatic_scheme {
    BspAutomaticScheme::LongestSide => longest_side_direction(rect),
    BspAutomaticScheme::Alternate => {
      if depth % 2 == 0 {
        base_direction.clone()
      } else {
        base_direction.inverse()
      }
    }
  };

  let (left_rect, right_rect) = split_rect(rect, &tiling_direction);

  let left_child = build_balanced_tree(
    items,
    &left_rect,
    base_direction,
    depth + 1,
    automatic_scheme,
    gaps_config,
  )?;

  let right_child = build_balanced_tree(
    right_items,
    &right_rect,
    base_direction,
    depth + 1,
    automatic_scheme,
    gaps_config,
  )?;

  let split = SplitContainer::new(tiling_direction, gaps_config.clone());

  attach_container(
    &left_child.clone().into(),
    &split.clone().into(),
    None,
  )?;

  attach_container(
    &right_child.clone().into(),
    &split.clone().into(),
    None,
  )?;

  Ok(split.into())
}

/// Halves `rect` along `tiling_direction` for the two children of a
/// split.
///
/// Gap sizes are ignored: the rect is only used to pick split
/// directions.
fn split_rect(
  rect: &Rect,
  tiling_direction: &TilingDirection,
) -> (Rect, Rect) {
  match tiling_direction {
    TilingDirection::Horizontal => {
      let left_width = rect.width() / 2;

      (
        Rect::from_xy(rect.x(), rect.y(), left_width, rect.height()),
        Rect::from_xy(
          rect.x() + left_width,
          rect.y(),
          rect.width() - left_width,
          rect.height(),
        ),
      )
    }
    TilingDirection::Vertical => {
      let top_height = rect.height() / 2;

      (
        Rect::from_xy(rect.x(), rect.y(), rect.width(), top_height),
        Rect::from_xy(
          rect.x(),
          rect.y() + top_height,
          rect.width(),
          rect.height() - top_height,
        ),
      )
    }
  }
}

/// Dissolves a redundant root split container into the workspace.
///
/// A BSP workspace with two or more tiling windows has a single tiling
/// child: the root split of its binary tree. The scrolling layout lays
/// its direct children out individually, so the root split is
/// flattened first and its children become separate strip items.
pub(super) fn collapse_root_split(
  workspace: &Workspace,
) -> anyhow::Result<()> {
  let tiling_children = workspace.tiling_children().collect::<Vec<_>>();

  if let [TilingContainer::Split(split)] = tiling_children.as_slice() {
    flatten_split_container(split.clone())?;
  }

  Ok(())
}

#[cfg(test)]
mod tests {
  use wm_common::{BspConfig, WorkspaceLayout};

  use super::*;
  use crate::{
    commands::container::{attach_container, set_focused_descendant},
    models::{Monitor, SplitContainer, TilingWindow},
  };

  /// Creates a `WmState` with a displayed BSP workspace containing
  /// `window_count` tiling windows. The window at `focused_index` is
  /// focused (when it exists).
  fn mock_bsp_state(
    window_count: usize,
    focused_index: usize,
    automatic_scheme: BspAutomaticScheme,
  ) -> (WmState, Workspace, Vec<TilingWindow>) {
    let workspace = Workspace::mock()
      .layout(WorkspaceLayout::Bsp)
      .bsp(BspConfig { automatic_scheme })
      .call();

    let windows = (0..window_count)
      .map(|_| TilingWindow::mock().call())
      .collect::<Vec<_>>();

    for window in &windows {
      attach_container(
        &window.clone().into(),
        &workspace.clone().into(),
        None,
      )
      .unwrap();
    }

    let monitor =
      Monitor::mock().workspaces(vec![workspace.clone()]).call();

    let state = WmState::mock();
    attach_container(
      &monitor.clone().into(),
      &state.root_container.clone().into(),
      None,
    )
    .unwrap();

    if let Some(window) = windows.get(focused_index) {
      set_focused_descendant(&window.clone().into(), None);
    }

    (state, workspace, windows)
  }

  #[test]
  fn appends_when_no_tiling_window() {
    let (state, workspace, _windows) =
      mock_bsp_state(0, 0, BspAutomaticScheme::default());

    let (parent, index) =
      crate::layouts::insertion_target(&WindowState::Tiling, &state)
        .unwrap();

    assert_eq!(parent.id(), workspace.id());
    assert_eq!(index, 0);
  }

  #[test]
  fn wraps_focused_window_in_split_container() {
    let (state, workspace, windows) =
      mock_bsp_state(1, 0, BspAutomaticScheme::LongestSide);

    let (parent, index) =
      crate::layouts::insertion_target(&WindowState::Tiling, &state)
        .unwrap();

    // The focused window was wrapped in a new split container at its
    // position, and the new window is inserted after it.
    assert_eq!(index, 1);
    assert_eq!(workspace.child_count(), 1);

    let split = parent.as_split().expect("Expected a split container.");

    assert_eq!(split.child_count(), 1);
    assert_eq!(split.children()[0].id(), windows[0].id());
    assert!((split.tiling_size() - 1.0).abs() < f32::EPSILON);

    // The mock viewport (1680x1000) is wider than tall: the focused
    // window is split into side-by-side halves.
    assert_eq!(split.tiling_direction(), TilingDirection::Horizontal);

    // Simulate the attach that `manage_window` performs: the two split
    // children each take half of its space.
    let new_window = TilingWindow::mock().call();
    attach_container(&new_window.clone().into(), &parent, Some(index))
      .unwrap();

    assert_eq!(split.child_count(), 2);
    assert!((windows[0].tiling_size() - 0.5).abs() < f32::EPSILON);
    assert!((new_window.tiling_size() - 0.5).abs() < f32::EPSILON);
  }

  #[test]
  fn splits_longer_side_of_focused_window() {
    let workspace = Workspace::mock().layout(WorkspaceLayout::Bsp).call();

    let windows = (0..2)
      .map(|_| TilingWindow::mock().call())
      .collect::<Vec<_>>();

    let root = SplitContainer::mock()
      .tiling_direction(TilingDirection::Horizontal)
      .tiling_containers(vec![
        windows[0].clone().into(),
        windows[1].clone().into(),
      ])
      .call();

    attach_container(
      &root.clone().into(),
      &workspace.clone().into(),
      None,
    )
    .unwrap();

    windows[0].set_tiling_size(0.2);
    windows[1].set_tiling_size(0.8);

    let monitor =
      Monitor::mock().workspaces(vec![workspace.clone()]).call();

    let state = WmState::mock();
    attach_container(
      &monitor.clone().into(),
      &state.root_container.clone().into(),
      None,
    )
    .unwrap();
    set_focused_descendant(&windows[1].clone().into(), None);

    let (parent, _index) =
      crate::layouts::insertion_target(&WindowState::Tiling, &state)
        .unwrap();

    // The focused window (the right 80% of the viewport) is wider than
    // tall: it is split into side-by-side halves.
    let split = parent.as_split().expect("Expected a split container.");

    assert_eq!(split.tiling_direction(), TilingDirection::Horizontal);
    assert_eq!(split.children()[0].id(), windows[1].id());
    assert_eq!(root.children()[1].id(), split.id());
  }

  #[test]
  fn alternate_scheme_alternates_split_directions() {
    // At the root level, the workspace's tiling direction is used.
    let (state, workspace, _windows) =
      mock_bsp_state(1, 0, BspAutomaticScheme::Alternate);

    let (parent, _index) =
      crate::layouts::insertion_target(&WindowState::Tiling, &state)
        .unwrap();

    assert_eq!(
      parent.as_split().unwrap().tiling_direction(),
      workspace.tiling_direction()
    );

    // Deeper splits invert their parent's direction.
    let workspace = Workspace::mock()
      .tiling_direction(TilingDirection::Horizontal)
      .layout(WorkspaceLayout::Bsp)
      .bsp(BspConfig {
        automatic_scheme: BspAutomaticScheme::Alternate,
      })
      .call();

    let windows = (0..2)
      .map(|_| TilingWindow::mock().call())
      .collect::<Vec<_>>();

    let root = SplitContainer::mock()
      .tiling_direction(TilingDirection::Horizontal)
      .tiling_containers(vec![
        windows[0].clone().into(),
        windows[1].clone().into(),
      ])
      .call();

    attach_container(
      &root.clone().into(),
      &workspace.clone().into(),
      None,
    )
    .unwrap();

    windows[0].set_tiling_size(0.2);
    windows[1].set_tiling_size(0.8);

    let monitor =
      Monitor::mock().workspaces(vec![workspace.clone()]).call();

    let state = WmState::mock();
    attach_container(
      &monitor.clone().into(),
      &state.root_container.clone().into(),
      None,
    )
    .unwrap();
    set_focused_descendant(&windows[1].clone().into(), None);

    let (parent, _index) =
      crate::layouts::insertion_target(&WindowState::Tiling, &state)
        .unwrap();

    // The focused window's parent is split horizontally, so the new
    // split stacks its children vertically.
    let split = parent.as_split().expect("Expected a split container.");

    assert_eq!(split.tiling_direction(), TilingDirection::Vertical);
  }

  #[test]
  fn rebuilds_balanced_tree_on_layout_change() {
    let workspace =
      Workspace::mock().layout(WorkspaceLayout::Scrolling).call();

    let windows = (0..3)
      .map(|_| TilingWindow::mock().call())
      .collect::<Vec<_>>();

    for window in &windows {
      attach_container(
        &window.clone().into(),
        &workspace.clone().into(),
        None,
      )
      .unwrap();
      window.set_tiling_size(0.7);
    }

    let monitor =
      Monitor::mock().workspaces(vec![workspace.clone()]).call();

    let state = WmState::mock();
    attach_container(
      &monitor.clone().into(),
      &state.root_container.clone().into(),
      None,
    )
    .unwrap();

    crate::layouts::on_layout_changed(
      &workspace,
      &WorkspaceLayout::Scrolling,
      &WorkspaceLayout::Bsp,
      wm_common::LayoutSwitchBehavior::Reflow,
      &state,
    )
    .unwrap();

    // The three windows form a balanced tree within the (wider than
    // tall) mock viewport: a horizontal split of a vertical split with
    // the first two windows, and the third window.
    assert_eq!(workspace.child_count(), 1);

    let root = workspace.children()[0]
      .as_split()
      .cloned()
      .expect("Expected a root split container.");

    assert!((root.tiling_size() - 1.0).abs() < f32::EPSILON);
    assert_eq!(root.tiling_direction(), TilingDirection::Horizontal);
    assert_eq!(root.child_count(), 2);
    assert!(
      (root.children()[0]
        .as_tiling_container()
        .unwrap()
        .tiling_size()
        - 0.5)
        .abs()
        < f32::EPSILON
    );
    assert!(
      (root.children()[1]
        .as_tiling_container()
        .unwrap()
        .tiling_size()
        - 0.5)
        .abs()
        < f32::EPSILON
    );

    let left = root.children()[0]
      .as_split()
      .cloned()
      .expect("Expected a nested split container.");

    assert_eq!(left.tiling_direction(), TilingDirection::Vertical);
    assert_eq!(left.children()[0].id(), windows[0].id());
    assert_eq!(left.children()[1].id(), windows[1].id());
    assert_eq!(root.children()[1].id(), windows[2].id());

    // Every split divides its space evenly.
    assert!((windows[0].tiling_size() - 0.5).abs() < f32::EPSILON);
    assert!((windows[1].tiling_size() - 0.5).abs() < f32::EPSILON);
    assert!((windows[2].tiling_size() - 0.5).abs() < f32::EPSILON);
  }

  #[test]
  fn rebuild_preserves_focus_order() {
    let (state, workspace, windows) =
      mock_bsp_state(2, 1, BspAutomaticScheme::LongestSide);

    // Sanity check: the second window is the focused one.
    assert_eq!(state.focused_container().unwrap().id(), windows[1].id());

    rebuild_tree(&workspace).unwrap();

    // The rebuilt tree's focus order still puts the focused window
    // first, so `WmState::focused_container` keeps resolving to it (a
    // window opened after a layout switch is inserted next to it).
    assert_eq!(state.focused_container().unwrap().id(), windows[1].id());
  }

  #[test]
  fn collapses_root_split_when_switching_to_scrolling() {
    let workspace =
      Workspace::mock().layout(WorkspaceLayout::Scrolling).call();

    let windows = (0..3)
      .map(|_| TilingWindow::mock().call())
      .collect::<Vec<_>>();

    for window in &windows {
      attach_container(
        &window.clone().into(),
        &workspace.clone().into(),
        None,
      )
      .unwrap();
    }

    let monitor =
      Monitor::mock().workspaces(vec![workspace.clone()]).call();

    let state = WmState::mock();
    attach_container(
      &monitor.clone().into(),
      &state.root_container.clone().into(),
      None,
    )
    .unwrap();

    crate::layouts::on_layout_changed(
      &workspace,
      &WorkspaceLayout::Scrolling,
      &WorkspaceLayout::Bsp,
      wm_common::LayoutSwitchBehavior::Reflow,
      &state,
    )
    .unwrap();

    crate::layouts::on_layout_changed(
      &workspace,
      &WorkspaceLayout::Bsp,
      &WorkspaceLayout::Scrolling,
      wm_common::LayoutSwitchBehavior::Reflow,
      &state,
    )
    .unwrap();

    // The root split was dissolved: the strip is the nested split of the
    // first two windows (half the viewport) followed by the third.
    assert_eq!(workspace.child_count(), 2);
    assert!(workspace.children()[0].as_split().is_some());
    assert_eq!(workspace.children()[1].id(), windows[2].id());

    assert!(
      (workspace.children()[0]
        .as_tiling_container()
        .unwrap()
        .tiling_size()
        - 0.5)
        .abs()
        < f32::EPSILON
    );
    assert!((windows[2].tiling_size() - 0.5).abs() < f32::EPSILON);
  }

  #[test]
  fn wraps_insertion_sibling_when_moving_window_into_workspace() {
    let (mut state, workspace, windows) =
      mock_bsp_state(1, 0, BspAutomaticScheme::LongestSide);

    // A window in a second workspace on the same monitor.
    let source_workspace = Workspace::mock().name("2".to_string()).call();

    let monitor = workspace.monitor().unwrap();
    attach_container(
      &source_workspace.clone().into(),
      &monitor.clone().into(),
      None,
    )
    .unwrap();

    let moving_window = TilingWindow::mock().call();
    attach_container(
      &moving_window.clone().into(),
      &source_workspace.clone().into(),
      None,
    )
    .unwrap();

    insert_window_into_workspace(
      &moving_window.clone().into(),
      &workspace,
      Some(windows[0].clone().into()),
      &mut state,
    )
    .unwrap();

    // The insertion sibling was wrapped in a split container, with the
    // moved window in the other half of its space.
    assert_eq!(source_workspace.child_count(), 0);
    assert_eq!(workspace.child_count(), 1);

    let split = workspace.children()[0]
      .as_split()
      .cloned()
      .expect("Expected a split container.");

    assert_eq!(split.child_count(), 2);
    assert_eq!(split.children()[0].id(), windows[0].id());
    assert_eq!(split.children()[1].id(), moving_window.id());
    assert!((windows[0].tiling_size() - 0.5).abs() < f32::EPSILON);
    assert!((moving_window.tiling_size() - 0.5).abs() < f32::EPSILON);
  }

  /// A displayed scrolling workspace with three strip items, each 70% of
  /// the viewport.
  fn mock_overflowing_strip() -> (Workspace, Vec<TilingWindow>, WmState) {
    let workspace =
      Workspace::mock().layout(WorkspaceLayout::Scrolling).call();

    let windows = (0..3)
      .map(|_| TilingWindow::mock().call())
      .collect::<Vec<_>>();

    for window in &windows {
      attach_container(
        &window.clone().into(),
        &workspace.clone().into(),
        None,
      )
      .unwrap();
      window.set_tiling_size(0.7);
    }

    let monitor =
      Monitor::mock().workspaces(vec![workspace.clone()]).call();

    let state = WmState::mock();
    attach_container(
      &monitor.clone().into(),
      &state.root_container.clone().into(),
      None,
    )
    .unwrap();

    (workspace, windows, state)
  }

  #[test]
  fn keeps_arrangement_when_switching_to_bsp() {
    let (workspace, windows, state) = mock_overflowing_strip();

    crate::layouts::on_layout_changed(
      &workspace,
      &WorkspaceLayout::Scrolling,
      &WorkspaceLayout::Bsp,
      wm_common::LayoutSwitchBehavior::Keep,
      &state,
    )
    .unwrap();

    // The windows stay direct children of the workspace, and are scaled
    // down to fill the viewport together.
    assert_eq!(workspace.child_count(), 3);

    for window in &windows {
      assert_eq!(window.parent().unwrap().id(), workspace.id());
      assert!((window.tiling_size() - 1. / 3.).abs() < 0.001);
    }
  }

  #[test]
  fn keeps_bsp_tree_when_switching_to_scrolling() {
    let (workspace, _windows, state) = mock_overflowing_strip();

    crate::layouts::on_layout_changed(
      &workspace,
      &WorkspaceLayout::Scrolling,
      &WorkspaceLayout::Bsp,
      wm_common::LayoutSwitchBehavior::Reflow,
      &state,
    )
    .unwrap();

    crate::layouts::on_layout_changed(
      &workspace,
      &WorkspaceLayout::Bsp,
      &WorkspaceLayout::Scrolling,
      wm_common::LayoutSwitchBehavior::Keep,
      &state,
    )
    .unwrap();

    // The tree stays a single strip item that fills the viewport.
    assert_eq!(workspace.child_count(), 1);
    assert!(workspace.children()[0].as_split().is_some());
  }
}
