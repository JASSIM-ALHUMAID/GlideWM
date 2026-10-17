use crate::{models::Container, traits::CommonGetters, wm_state::WmState};

/// Updates the scroll offset of the focused container's workspace if the
/// workspace uses the scrolling layout.
///
/// If focus changed since the last sync, the focused container is
/// scrolled into view. The offset is also clamped to the valid range to
/// handle strip contents changing (e.g. a window being closed or moved
/// to another workspace).
///
/// Queues a redraw of the workspace if the scroll offset changed.
pub fn update_scroll_position(
  focused_container: &Container,
  state: &mut WmState,
) -> anyhow::Result<()> {
  let Some(workspace) = focused_container.workspace() else {
    return Ok(());
  };

  if !workspace.is_scrolling() {
    return Ok(());
  }

  let mut offset_changed = false;

  if state.pending_sync.needs_focus_update() {
    offset_changed |=
      workspace.ensure_descendant_visible(focused_container)?;
  }

  offset_changed |= workspace.clamp_scroll_offset()?;

  if offset_changed {
    state.pending_sync.queue_container_to_redraw(workspace);
  }

  Ok(())
}
