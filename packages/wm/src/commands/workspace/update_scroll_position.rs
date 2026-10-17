use crate::{models::Container, traits::CommonGetters, wm_state::WmState};

/// Updates scroll offsets of displayed scrolling workspaces.
///
/// If focus changed since the last sync, the focused container is
/// scrolled into view. Offsets of all displayed scrolling workspaces are
/// clamped to the valid range to handle strip contents changing (e.g. a
/// window being closed or moved to another workspace).
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
    if !workspace.is_scrolling() {
      continue;
    }

    let mut offset_changed = false;

    let is_focused_workspace = focused_workspace
      .as_ref()
      .is_some_and(|focused| focused.id() == workspace.id());

    if is_focused_workspace && state.pending_sync.needs_focus_update() {
      offset_changed |=
        workspace.ensure_descendant_visible(focused_container)?;
    }

    offset_changed |= workspace.clamp_scroll_offset()?;

    if offset_changed {
      state.pending_sync.queue_container_to_redraw(workspace);
    }
  }

  Ok(())
}
