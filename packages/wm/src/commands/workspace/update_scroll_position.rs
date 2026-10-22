use crate::{
  layouts, models::Container, traits::CommonGetters, wm_state::WmState,
};

/// Updates scroll offsets of displayed scrolling workspaces.
///
/// If focus changed since the last sync, the focused container is
/// scrolled into view. Workspaces queued for a rescroll (strip contents
/// changed without a focus change, e.g. a window exiting fullscreen) get
/// their focused descendant scrolled back into view. Offsets of all
/// displayed scrolling workspaces are clamped to the valid range to
/// handle strip contents changing (e.g. a window being closed or moved to
/// another workspace).
///
/// Queues a redraw of any workspace whose scroll offset changed.
pub fn update_scroll_position(
  focused_container: &Container,
  state: &mut WmState,
) -> anyhow::Result<()> {
  let focused_workspace = focused_container.workspace();

  let displayed_workspaces = state
    .monitors()
    .into_iter()
    .filter_map(|monitor| monitor.displayed_workspace())
    .collect::<Vec<_>>();

  for workspace in displayed_workspaces {
    if !layouts::supports_scrolling(&workspace) {
      continue;
    }

    let is_focused_workspace = focused_workspace
      .as_ref()
      .is_some_and(|focused| focused.id() == workspace.id());

    // The focused container is the focused descendant of the focused
    // workspace, so both cases scroll the same container into view there.
    let scroll_target =
      if is_focused_workspace && state.pending_sync.needs_focus_update() {
        Some(focused_container.clone())
      } else if state.pending_sync.needs_rescroll(&workspace) {
        workspace.descendant_focus_order().next()
      } else {
        None
      };

    let mut offset_changed = false;

    if let Some(scroll_target) = scroll_target {
      offset_changed |= layouts::scrolling::ensure_descendant_visible(
        &workspace,
        &scroll_target,
      )?;
    }

    offset_changed |= layouts::scrolling::clamp_scroll_offset(&workspace)?;

    if offset_changed {
      state.pending_sync.queue_container_to_redraw(workspace);
    }
  }

  Ok(())
}
