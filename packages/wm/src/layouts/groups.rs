//! Layout groups: how `layout_switch: keep` arranges windows opened after
//! a layout switch.
//!
//! A workspace has a single container tree, and its layout decides the
//! rect of every direct child. Switching away from the scrolling layout
//! therefore can't leave the existing windows alone and also arrange new
//! windows by the new layout, unless the two live in different places of
//! the tree. With `keep`, the workspace instead retains its strip (see
//! [`Workspace::has_retained_strip`]): the existing strip items keep
//! their sizes and positions, and windows opened afterwards are put into
//! a *layout group*, a split container that is a direct child of the
//! workspace, takes a full viewport (strip item size `1.0`), and arranges
//! its windows by the layout that it is tagged with
//! ([`SplitContainer::layout_group`]). Users scroll between the old
//! strip items and the group like between any other strip items.
//!
//! The workspace's [`Workspace::layout`] is the layout that new windows
//! are arranged by; the strip itself is always a scrolling strip.

use anyhow::Context;
use wm_common::{BspAutomaticScheme, WindowState, WorkspaceLayout};

use super::bsp;
use crate::{
  commands::container::{
    attach_container, flatten_split_container, move_container_within_tree,
  },
  models::{Container, SplitContainer, WindowContainer, Workspace},
  traits::{
    CommonGetters, PositionGetters, TilingDirectionGetters,
    TilingSizeGetters,
  },
  wm_state::WmState,
};

/// The layout that arranges the contents of `split`.
///
/// Splits inside a layout group follow the group's layout. Splits in the
/// retained strip of a workspace are columns of the strip, which have
/// tiling semantics. Everything else follows its workspace's layout (and
/// anything but BSP is arranged with tiling semantics).
pub(super) fn layout_of_split(split: &SplitContainer) -> WorkspaceLayout {
  let group_layout = split.self_and_ancestors().find_map(|ancestor| {
    ancestor.as_split().and_then(SplitContainer::layout_group)
  });

  if let Some(layout) = group_layout {
    return layout;
  }

  match split.workspace() {
    Some(workspace)
      if !workspace.has_retained_strip()
        && workspace.layout() == WorkspaceLayout::Bsp =>
    {
      WorkspaceLayout::Bsp
    }
    _ => WorkspaceLayout::Tiling,
  }
}

/// Dissolves all layout groups of the workspace, and ends the retained
/// strip.
///
/// The windows of each group become direct children of the workspace.
pub(super) fn dissolve_all(workspace: &Workspace) -> anyhow::Result<()> {
  let groups = workspace
    .children()
    .into_iter()
    .filter_map(|child| child.as_split().cloned())
    .filter(SplitContainer::is_layout_group)
    .collect::<Vec<_>>();

  for group in groups {
    group.set_layout_group(None);
    flatten_split_container(group)?;
  }

  workspace.set_retained_strip(false);

  Ok(())
}

/// Gets where to insert a new window in a workspace with a retained
/// strip, when the workspace's layout is not the scrolling layout.
///
/// Tiling windows are inserted into a layout group that is arranged by
/// the workspace's layout, and non-tiling windows are appended to the
/// workspace.
///
/// Returns tuple of (parent container, insertion index).
pub(super) fn insertion_target(
  window_state: &WindowState,
  focused_container: &Container,
  workspace: &Workspace,
) -> anyhow::Result<(Container, usize)> {
  if *window_state != WindowState::Tiling {
    return Ok((workspace.clone().into(), workspace.child_count()));
  }

  // Insert next to the focused window, or the most recently focused
  // tiling window when a non-tiling window is focused.
  let sibling = match focused_container {
    Container::TilingWindow(_) => Some(focused_container.clone()),
    _ => workspace
      .descendant_focus_order()
      .find(Container::is_tiling_window),
  };

  target_in_group(sibling, workspace)
}

/// Inserts a window into a workspace with a retained strip as part of
/// moving it there.
///
/// Tiling windows go into a layout group that is arranged by the
/// workspace's layout, next to `insertion_sibling` when it is in that
/// group.
pub(super) fn insert_window_into_workspace(
  window: &WindowContainer,
  target_workspace: &Workspace,
  insertion_sibling: Option<WindowContainer>,
  state: &mut WmState,
) -> anyhow::Result<()> {
  let (parent, index) = if window.is_tiling_window() {
    target_in_group(
      insertion_sibling.map(Container::from),
      target_workspace,
    )?
  } else {
    (
      target_workspace.clone().into(),
      target_workspace.child_count(),
    )
  };

  move_container_within_tree(&window.clone().into(), &parent, index, state)
}

/// Finds (or creates) the layout group that a new window belongs in, and
/// returns where to insert the window in it.
fn target_in_group(
  sibling: Option<Container>,
  workspace: &Workspace,
) -> anyhow::Result<(Container, usize)> {
  let layout = workspace.layout();
  let group = group_for_insertion(sibling.as_ref(), workspace, &layout)?;

  // Only a window inside the group can be inserted next to.
  let sibling = sibling
    .filter(|sibling| {
      sibling.is_tiling_window()
        && sibling
          .ancestors()
          .any(|ancestor| ancestor.id() == group.id())
    })
    .or_else(|| {
      group
        .descendant_focus_order()
        .find(Container::is_tiling_window)
    });

  let Some(sibling) = sibling else {
    // A new (empty) group.
    return Ok((group.clone().into(), group.child_count()));
  };

  match layout {
    WorkspaceLayout::Bsp => {
      let is_lone_window = group.child_count() == 1
        && sibling
          .parent()
          .is_some_and(|parent| parent.id() == group.id());

      if is_lone_window {
        // The group is the root of the binary tree: set its direction,
        // and add the window next to the lone one.
        let direction = match workspace.config().bsp.automatic_scheme {
          BspAutomaticScheme::LongestSide => {
            let rect =
              sibling.to_rect().or_else(|_| workspace.to_rect())?;
            bsp::longest_side_direction(&rect)
          }
          BspAutomaticScheme::Alternate => group.tiling_direction(),
        };

        group.set_tiling_direction(direction);

        return Ok((group.into(), 1));
      }

      let split = bsp::split_for_insertion(&sibling, workspace)?;
      let target_index = split.child_count();

      Ok((split.into(), target_index))
    }
    _ => {
      Ok((sibling.parent().context("No parent.")?, sibling.index() + 1))
    }
  }
}

/// Gets the layout group (arranged by `layout`) to insert a window into:
/// the group of `sibling`, otherwise the last strip item when it is such
/// a group, otherwise a new group appended to the end of the strip.
fn group_for_insertion(
  sibling: Option<&Container>,
  workspace: &Workspace,
  layout: &WorkspaceLayout,
) -> anyhow::Result<SplitContainer> {
  let is_matching_group =
    |split: &SplitContainer| split.layout_group().as_ref() == Some(layout);

  let sibling_group = sibling.and_then(|sibling| {
    sibling
      .self_and_ancestors()
      .filter_map(|ancestor| ancestor.as_split().cloned())
      .find(is_matching_group)
  });

  if let Some(group) = sibling_group {
    return Ok(group);
  }

  let tail_group = workspace
    .tiling_children()
    .last()
    .and_then(|child| child.as_split().cloned())
    .filter(is_matching_group);

  if let Some(group) = tail_group {
    return Ok(group);
  }

  // A group fills a whole viewport, and splits it along its longest side
  // like a freshly activated workspace.
  let direction = bsp::longest_side_direction(&workspace.to_rect()?);
  let group = SplitContainer::new(direction, workspace.gaps_config());
  group.set_layout_group(Some(layout.clone()));

  attach_container(
    &group.clone().into(),
    &workspace.clone().into(),
    None,
  )?;
  group.set_tiling_size(1.0);
  workspace.set_retained_strip(true);

  Ok(group)
}

#[cfg(test)]
mod tests {
  use wm_common::{
    GapsConfig, LayoutSwitchBehavior, ScrollConfig, TilingDirection,
  };

  use super::*;
  use crate::{
    commands::{
      container::{
        attach_container, detach_container, flatten_child_split_containers,
      },
      workspace::set_workspace_layout,
    },
    models::TilingWindow,
    test_utils::mock_scrolling_workspace,
  };

  /// A displayed vertical scrolling workspace with three strip items,
  /// each 90% of the viewport and scrolled a bit.
  fn mock_strip() -> (Workspace, Vec<TilingWindow>, WmState) {
    let (workspace, windows) = mock_scrolling_workspace(
      TilingDirection::Vertical,
      3,
      0.9,
      &GapsConfig::default(),
      ScrollConfig::default(),
    );

    workspace.set_scroll_offset(120);

    (workspace, windows, WmState::mock())
  }

  fn rect_of(window: &TilingWindow) -> (i32, i32, i32, i32) {
    let rect = window.to_rect().unwrap();

    (rect.x(), rect.y(), rect.width(), rect.height())
  }

  fn switch(
    workspace: &Workspace,
    layout: WorkspaceLayout,
    behavior: LayoutSwitchBehavior,
    state: &mut WmState,
  ) {
    set_workspace_layout(workspace, &layout, behavior, state).unwrap();
  }

  /// Inserts a new tiling window the way `manage_window` does.
  fn open_window(
    workspace: &Workspace,
    focused: &TilingWindow,
  ) -> TilingWindow {
    let (parent, index) = crate::layouts::insertion_target_for(
      &WindowState::Tiling,
      &focused.clone().into(),
      workspace,
    )
    .unwrap();

    let window = TilingWindow::mock().call();
    attach_container(&window.clone().into(), &parent, Some(index))
      .unwrap();

    window
  }

  #[test]
  fn keeping_leaves_existing_windows_where_they_are() {
    for layout in [WorkspaceLayout::Bsp, WorkspaceLayout::Tiling] {
      let (workspace, windows, mut state) = mock_strip();
      let rects = windows.iter().map(rect_of).collect::<Vec<_>>();

      switch(&workspace, layout, LayoutSwitchBehavior::Keep, &mut state);

      assert!(workspace.has_retained_strip());
      assert_eq!(workspace.child_count(), 3);
      assert_eq!(workspace.scroll_offset(), 120);
      assert_eq!(workspace.tiling_direction(), TilingDirection::Vertical);

      for (window, rect) in windows.iter().zip(&rects) {
        assert!((window.tiling_size() - 0.9).abs() < f32::EPSILON);
        assert_eq!(&rect_of(window), rect);
      }
    }
  }

  #[test]
  fn new_windows_fill_a_new_viewport_arranged_by_the_new_layout() {
    let (workspace, windows, mut state) = mock_strip();

    switch(
      &workspace,
      WorkspaceLayout::Bsp,
      LayoutSwitchBehavior::Keep,
      &mut state,
    );

    let first = open_window(&workspace, &windows[2]);

    // The window is the only one of a new group, which is a strip item.
    let group = first.parent().unwrap().as_split().cloned().unwrap();
    assert_eq!(group.layout_group(), Some(WorkspaceLayout::Bsp));
    assert_eq!(group.parent().unwrap().id(), workspace.id());
    assert_eq!(group.index(), 3);
    assert!((group.tiling_size() - 1.0).abs() < f32::EPSILON);
    assert_eq!(workspace.child_count(), 4);

    let viewport = workspace.to_rect().unwrap();
    assert_eq!(rect_of(&first).2, viewport.width());
    assert_eq!(rect_of(&first).3, viewport.height());

    // The next window splits the group instead of adding a strip item.
    let second = open_window(&workspace, &first);

    assert_eq!(workspace.child_count(), 4);
    assert_eq!(group.child_count(), 2);
    assert!((first.tiling_size() - 0.5).abs() < f32::EPSILON);
    assert!((second.tiling_size() - 0.5).abs() < f32::EPSILON);

    // The old windows didn't move.
    assert!((windows[0].tiling_size() - 0.9).abs() < f32::EPSILON);
  }

  #[test]
  fn tiling_group_inserts_after_the_focused_window() {
    let (workspace, windows, mut state) = mock_strip();

    switch(
      &workspace,
      WorkspaceLayout::Tiling,
      LayoutSwitchBehavior::Keep,
      &mut state,
    );

    let first = open_window(&workspace, &windows[0]);
    let second = open_window(&workspace, &first);
    let third = open_window(&workspace, &first);

    let group = first.parent().unwrap().as_split().cloned().unwrap();
    assert_eq!(group.layout_group(), Some(WorkspaceLayout::Tiling));

    let ids = group.children().iter().map(|c| c.id()).collect::<Vec<_>>();
    assert_eq!(ids, vec![first.id(), third.id(), second.id()]);
  }

  #[test]
  fn reuses_the_tail_group_only_for_the_same_layout() {
    let (workspace, windows, mut state) = mock_strip();

    switch(
      &workspace,
      WorkspaceLayout::Bsp,
      LayoutSwitchBehavior::Keep,
      &mut state,
    );
    let bsp_window = open_window(&workspace, &windows[0]);

    switch(
      &workspace,
      WorkspaceLayout::Tiling,
      LayoutSwitchBehavior::Keep,
      &mut state,
    );
    let tiling_window = open_window(&workspace, &windows[0]);

    // A different layout starts a new group.
    assert_ne!(
      bsp_window.parent().unwrap().id(),
      tiling_window.parent().unwrap().id()
    );
    assert_eq!(workspace.child_count(), 5);

    // Going back to a layout reuses the group only if it is the last one.
    switch(
      &workspace,
      WorkspaceLayout::Bsp,
      LayoutSwitchBehavior::Keep,
      &mut state,
    );
    let other_bsp_window = open_window(&workspace, &windows[0]);
    assert_ne!(
      other_bsp_window.parent().unwrap().id(),
      tiling_window.parent().unwrap().id()
    );
  }

  #[test]
  fn switching_back_to_scrolling_keeps_groups_and_adds_strip_items() {
    let (workspace, windows, mut state) = mock_strip();

    switch(
      &workspace,
      WorkspaceLayout::Bsp,
      LayoutSwitchBehavior::Keep,
      &mut state,
    );
    let grouped = open_window(&workspace, &windows[0]);

    switch(
      &workspace,
      WorkspaceLayout::Scrolling,
      LayoutSwitchBehavior::Keep,
      &mut state,
    );

    // The group stays a strip item.
    let group = grouped.parent().unwrap();
    assert_eq!(group.parent().unwrap().id(), workspace.id());
    assert_eq!(workspace.child_count(), 4);

    // New windows are plain strip items again.
    let (parent, _) = crate::layouts::insertion_target_for(
      &WindowState::Tiling,
      &grouped.clone().into(),
      &workspace,
    )
    .unwrap();

    assert_eq!(parent.id(), workspace.id());
  }

  #[test]
  fn reflow_dissolves_groups() {
    let (workspace, windows, mut state) = mock_strip();

    switch(
      &workspace,
      WorkspaceLayout::Bsp,
      LayoutSwitchBehavior::Keep,
      &mut state,
    );
    let first = open_window(&workspace, &windows[0]);
    let second = open_window(&workspace, &first);

    switch(
      &workspace,
      WorkspaceLayout::Tiling,
      LayoutSwitchBehavior::Reflow,
      &mut state,
    );

    assert!(!workspace.has_retained_strip());
    assert!(workspace
      .children()
      .iter()
      .all(|child| child.as_split().is_none()));
    assert_eq!(workspace.child_count(), 5);
    assert_eq!(first.parent().unwrap().id(), workspace.id());
    assert_eq!(second.parent().unwrap().id(), workspace.id());
  }

  #[test]
  fn group_vanishes_with_its_last_window() {
    let (workspace, windows, mut state) = mock_strip();

    switch(
      &workspace,
      WorkspaceLayout::Bsp,
      LayoutSwitchBehavior::Keep,
      &mut state,
    );
    let grouped = open_window(&workspace, &windows[0]);

    detach_container(grouped.clone().into()).unwrap();

    assert_eq!(workspace.child_count(), 3);
    assert!(workspace.has_retained_strip());

    // Closing an old window leaves the sizes of the others alone.
    detach_container(windows[0].clone().into()).unwrap();
    assert!((windows[1].tiling_size() - 0.9).abs() < f32::EPSILON);

    // Once nothing is left, there's no strip to retain.
    detach_container(windows[1].clone().into()).unwrap();
    detach_container(windows[2].clone().into()).unwrap();
    assert!(!workspace.has_retained_strip());
  }

  #[test]
  fn flattening_keeps_single_window_groups() {
    let (workspace, windows, mut state) = mock_strip();

    switch(
      &workspace,
      WorkspaceLayout::Tiling,
      LayoutSwitchBehavior::Keep,
      &mut state,
    );
    let grouped = open_window(&workspace, &windows[0]);

    // Even when the group is the only strip item.
    for window in &windows {
      detach_container(window.clone().into()).unwrap();
    }

    flatten_child_split_containers(&workspace.clone().into()).unwrap();

    let group = grouped.parent().unwrap();
    assert_eq!(group.parent().unwrap().id(), workspace.id());
    assert!(group.as_split().unwrap().is_layout_group());
  }

  #[test]
  fn empty_workspaces_switch_without_retaining_a_strip() {
    let (workspace, windows, mut state) = mock_strip();

    for window in &windows {
      detach_container(window.clone().into()).unwrap();
    }

    switch(
      &workspace,
      WorkspaceLayout::Bsp,
      LayoutSwitchBehavior::Keep,
      &mut state,
    );

    assert!(!workspace.has_retained_strip());
  }
}
