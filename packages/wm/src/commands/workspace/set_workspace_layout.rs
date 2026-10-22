use wm_common::{LayoutSwitchBehavior, WmEvent, WorkspaceLayout};

use crate::{
  layouts, models::Workspace, traits::CommonGetters, wm_state::WmState,
};

/// Switches the layout used by a workspace at runtime.
///
/// Layout-specific state updates are applied via
/// [`layouts::on_layout_changed`] (e.g. switching to scrolling clamps
/// sizes to the valid range of viewport fractions and resets the scroll
/// offset so that the focused descendant stays visible). Switching back
/// to tiling rescales the existing sizes so that the children fill the
/// viewport; the sizes were viewport fractions, which are rescaled into
/// shares of the workspace (see the size semantics in
/// `docs/scrolling-layout.md`).
///
/// `behavior` decides whether the existing children are rearranged into
/// the new layout (see [`layouts::on_layout_changed`]).
///
/// The updated config is recorded as a session override (see
/// [`WmState::set_workspace_config_override`]), so that the switch
/// survives the workspace being destroyed while empty.
///
/// Queues a redraw of the workspace's children and emits a
/// `WorkspaceUpdated` event, since the layout is part of the workspace
/// config exposed over IPC.
pub fn set_workspace_layout(
  workspace: &Workspace,
  layout: &WorkspaceLayout,
  behavior: LayoutSwitchBehavior,
  state: &mut WmState,
) -> anyhow::Result<()> {
  let prev_layout = workspace.config().layout;

  if prev_layout == *layout {
    return Ok(());
  }

  let mut updated_config = workspace.config();
  updated_config.layout = layout.clone();
  workspace.set_config(updated_config.clone());
  state.set_workspace_config_override(updated_config);

  layouts::on_layout_changed(
    workspace,
    &prev_layout,
    layout,
    behavior,
    state,
  )?;

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
      LayoutSwitchBehavior::Reflow,
      &mut state,
    )
    .unwrap();

    assert_eq!(workspace.layout(), WorkspaceLayout::Scrolling);
    assert!(
      (first.tiling_size() - ScrollConfig::MIN_ITEM_SIZE).abs()
        < f32::EPSILON
    );
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
      LayoutSwitchBehavior::Reflow,
      &mut state,
    )
    .unwrap();

    // The offset was reset and re-ensured so that the focused window is
    // fully visible.
    assert_eq!(
      workspace.scroll_offset(),
      layouts::scrolling::max_scroll_offset(&workspace).unwrap()
    );

    let viewport_rect = workspace.to_rect().unwrap();
    let item_rect = second.to_rect().unwrap();

    assert!(item_rect.x() >= viewport_rect.x());
    assert!(item_rect.right <= viewport_rect.right);
  }

  #[test]
  fn rescales_sizes_when_switching_to_tiling() {
    let (workspace, windows) = mock_scrolling_workspace(
      TilingDirection::Horizontal,
      2,
      0.7,
      &GapsConfig::default(),
      ScrollConfig::default(),
    );

    let mut state = WmState::mock();

    set_workspace_layout(
      &workspace,
      &WorkspaceLayout::Tiling,
      LayoutSwitchBehavior::Reflow,
      &mut state,
    )
    .unwrap();

    assert_eq!(workspace.config().layout, WorkspaceLayout::Tiling);

    // Viewport fractions that sum to more than the full viewport are
    // rescaled into shares that fill it (0.7 and 0.7 become 0.5 and
    // 0.5), so no window sits past the edge of the viewport.
    for window in &windows {
      assert!((window.tiling_size() - 0.5).abs() < f32::EPSILON);
    }

    // The scroll offset is left as-is; it is only meaningful while the
    // workspace uses the scrolling layout.
    assert_eq!(workspace.scroll_offset(), 0);
  }

  #[test]
  fn records_session_config_override() {
    let (workspace, _windows) = mock_scrolling_workspace(
      TilingDirection::Horizontal,
      1,
      0.5,
      &GapsConfig::default(),
      ScrollConfig::default(),
    );

    let mut state = WmState::mock();

    set_workspace_layout(
      &workspace,
      &WorkspaceLayout::Tiling,
      LayoutSwitchBehavior::Reflow,
      &mut state,
    )
    .unwrap();

    // The updated config is kept as a session override, so that the
    // switch survives the workspace being destroyed while empty.
    let override_config =
      state.workspace_config_overrides.get("1").unwrap();

    assert_eq!(override_config.layout, WorkspaceLayout::Tiling);
  }

  #[test]
  fn does_nothing_when_layout_is_unchanged() {
    let workspace = Workspace::mock().call();
    let _monitor =
      Monitor::mock().workspaces(vec![workspace.clone()]).call();

    let mut state = WmState::mock();

    set_workspace_layout(
      &workspace,
      &WorkspaceLayout::Tiling,
      LayoutSwitchBehavior::Reflow,
      &mut state,
    )
    .unwrap();

    assert!(!state.pending_sync.has_changes());
  }

  #[test]
  fn runtime_switch_uses_resolved_scroll_options() {
    let workspace = Workspace::mock().call();
    let _monitor =
      Monitor::mock().workspaces(vec![workspace.clone()]).call();

    // The workspace's resolved scroll options are used for the strip
    // items that are added after the switch.
    let mut config = workspace.config();
    config.scroll.window_width = 0.8;
    workspace.set_config(config);

    let mut state = WmState::mock();

    set_workspace_layout(
      &workspace,
      &WorkspaceLayout::Scrolling,
      LayoutSwitchBehavior::Reflow,
      &mut state,
    )
    .unwrap();

    let size = layouts::default_child_size(&workspace).unwrap();

    assert!((size - 0.8).abs() < f32::EPSILON);
  }
}
