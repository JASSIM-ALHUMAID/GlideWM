use wm_common::{WmEvent, WorkspaceLayout};

use crate::{
  models::Workspace,
  traits::{CommonGetters, TilingSizeGetters, MIN_TILING_SIZE},
  wm_state::WmState,
};

/// Switches the layout used by a workspace at runtime.
///
/// When switching to `Scrolling`, the sizes of the workspace's tiling
/// children are clamped to the valid range of viewport fractions and the
/// scroll offset is reset so that the focused descendant stays visible.
/// Switching back to `Tiling` keeps the existing sizes, which are then
/// reinterpreted as shares of the workspace (see the size semantics in
/// `docs/scrolling-layout.md`).
///
/// Queues a redraw of the workspace's children and emits a
/// `WorkspaceUpdated` event, since the layout is part of the workspace
/// config exposed over IPC.
pub fn set_workspace_layout(
  workspace: &Workspace,
  layout: &WorkspaceLayout,
  state: &mut WmState,
) -> anyhow::Result<()> {
  if workspace.config().layout == *layout {
    return Ok(());
  }

  let mut updated_config = workspace.config();
  updated_config.layout = layout.clone();
  workspace.set_config(updated_config);

  if *layout == WorkspaceLayout::Scrolling {
    // Sizes of strip items are fractions of the viewport. Clamp sizes
    // that were set as shares of the workspace (e.g. `1/3` with three
    // windows) into the valid range.
    for child in workspace.tiling_children() {
      if let Ok(tiling_child) = child.as_tiling_container() {
        let clamped_size =
          tiling_child.tiling_size().clamp(MIN_TILING_SIZE, 1.);
        tiling_child.set_tiling_size(clamped_size);
      }
    }

    // Reset the offset and scroll the focused descendant into view. This
    // is a no-op if the focused container is not in this workspace.
    workspace.set_scroll_offset(0);

    if let Some(focused_container) = state.focused_container() {
      workspace.ensure_descendant_visible(&focused_container)?;
    }
  }

  state
    .pending_sync
    .queue_containers_to_redraw(workspace.tiling_children());

  state.emit_event(WmEvent::WorkspaceUpdated {
    updated_workspace: workspace.to_dto()?,
  });

  Ok(())
}

#[cfg(test)]
mod tests {
  use wm_common::{GapsConfig, ScrollConfig, TilingDirection};

  use super::*;
  use crate::{
    commands::container::{attach_container, set_focused_descendant},
    models::{Monitor, TilingWindow, Workspace},
    test_utils::mock_scrolling_workspace,
    traits::{PositionGetters, TilingSizeGetters},
  };

  #[test]
  fn clamps_sizes_when_switching_to_scrolling() {
    let workspace = Workspace::mock().call();
    let first = TilingWindow::mock().call();
    let second = TilingWindow::mock().call();

    attach_container(
      &first.clone().into(),
      &workspace.clone().into(),
      None,
    )
    .unwrap();
    attach_container(
      &second.clone().into(),
      &workspace.clone().into(),
      None,
    )
    .unwrap();

    // Sizes that are out of range when interpreted as viewport fractions.
    first.set_tiling_size(0.005);
    second.set_tiling_size(0.75);

    // Attach the workspace to a mock monitor (needed for rect math).
    let _monitor =
      Monitor::mock().workspaces(vec![workspace.clone()]).call();

    let mut state = WmState::mock();

    set_workspace_layout(
      &workspace,
      &WorkspaceLayout::Scrolling,
      &mut state,
    )
    .unwrap();

    assert!(workspace.is_scrolling());
    assert!((first.tiling_size() - MIN_TILING_SIZE).abs() < f32::EPSILON);
    assert!((second.tiling_size() - 0.75).abs() < f32::EPSILON);
    assert!(state.pending_sync.has_changes());
  }

  #[test]
  fn resets_offset_and_scrolls_focused_into_view() {
    let workspace = Workspace::mock().call();
    let first = TilingWindow::mock().call();
    let second = TilingWindow::mock().call();

    attach_container(
      &first.clone().into(),
      &workspace.clone().into(),
      None,
    )
    .unwrap();
    attach_container(
      &second.clone().into(),
      &workspace.clone().into(),
      None,
    )
    .unwrap();

    // The strip is wider than the viewport, so the second window needs
    // scrolling into view.
    first.set_tiling_size(0.75);
    second.set_tiling_size(0.75);

    let monitor =
      Monitor::mock().workspaces(vec![workspace.clone()]).call();

    let mut state = WmState::mock();
    attach_container(
      &monitor.clone().into(),
      &state.root_container.clone().into(),
      None,
    )
    .unwrap();

    set_focused_descendant(&second.clone().into(), None);

    // Simulate a stale offset from before the layout switch.
    workspace.set_scroll_offset(9999);

    set_workspace_layout(
      &workspace,
      &WorkspaceLayout::Scrolling,
      &mut state,
    )
    .unwrap();

    // The offset was reset and re-ensured so that the focused window is
    // fully visible.
    assert_eq!(
      workspace.scroll_offset(),
      workspace.max_scroll_offset().unwrap()
    );

    let viewport_rect = workspace.to_rect().unwrap();
    let item_rect = second.to_rect().unwrap();

    assert!(item_rect.x() >= viewport_rect.x());
    assert!(item_rect.right <= viewport_rect.right);
  }

  #[test]
  fn keeps_sizes_when_switching_to_tiling() {
    let (workspace, windows) = mock_scrolling_workspace(
      TilingDirection::Horizontal,
      2,
      0.5,
      &GapsConfig::default(),
      ScrollConfig::default(),
    );

    let mut state = WmState::mock();

    set_workspace_layout(&workspace, &WorkspaceLayout::Tiling, &mut state)
      .unwrap();

    assert_eq!(workspace.config().layout, WorkspaceLayout::Tiling);

    // Sizes are reinterpreted as shares of the workspace, not changed.
    for window in &windows {
      assert!((window.tiling_size() - 0.5).abs() < f32::EPSILON);
    }

    // The scroll offset is left as-is; it is only meaningful while the
    // workspace uses the scrolling layout.
    assert_eq!(workspace.scroll_offset(), 0);
  }

  #[test]
  fn does_nothing_when_layout_is_unchanged() {
    let workspace = Workspace::mock().call();
    let _monitor =
      Monitor::mock().workspaces(vec![workspace.clone()]).call();

    let mut state = WmState::mock();

    set_workspace_layout(&workspace, &WorkspaceLayout::Tiling, &mut state)
      .unwrap();

    assert!(!state.pending_sync.has_changes());
  }
}
