use anyhow::Context;
use wm_common::ScrollAlign;

use crate::{
  layouts, models::Container, traits::CommonGetters, wm_state::WmState,
};

/// Centers the strip item containing `container` in the viewport of its
/// scrolling workspace, regardless of the workspace's `scroll.align`
/// config.
///
/// The scroll offset is clamped to the valid range, so strip items at
/// either end of the strip stay flush with the viewport's edge. Has no
/// effect for workspaces that don't use the scrolling layout, or for
/// containers that aren't tiling descendants of their workspace.
pub fn center_focused(
  container: &Container,
  state: &mut WmState,
) -> anyhow::Result<()> {
  let workspace = container.workspace().context("No workspace.")?;

  if !layouts::supports_scrolling(&workspace) {
    tracing::info!(
      "Ignoring `center-focused`: workspace doesn't use the scrolling layout."
    );
    return Ok(());
  }

  let offset_changed = layouts::scrolling::scroll_descendant_into_view(
    &workspace,
    container,
    &ScrollAlign::Center,
  )?;

  if offset_changed {
    state.pending_sync.queue_container_to_redraw(workspace);
  }

  Ok(())
}

#[cfg(test)]
mod tests {
  use wm_common::{GapsConfig, ScrollConfig, TilingDirection};

  use super::*;
  use crate::test_utils::mock_scrolling_workspace;

  #[test]
  fn centers_focused_strip_item() {
    // Viewport is 1680px wide; each of the 5 items is 25% (420px), so
    // the strip can scroll by up to 420px.
    let (workspace, windows) = mock_scrolling_workspace(
      TilingDirection::Horizontal,
      5,
      0.25,
      &GapsConfig::default(),
      ScrollConfig::default(),
    );

    let mut state = WmState::mock();

    // The second item (420..840) is fully visible, so `minimal`
    // alignment wouldn't scroll. Centering moves its center (630) to the
    // viewport's center (840): the offset can't go below 0 though.
    center_focused(&windows[1].clone().into(), &mut state).unwrap();
    assert_eq!(workspace.scroll_offset(), 0);
    assert!(!state.pending_sync.has_changes());

    // The third item (840..1260) has its center at 1050.
    center_focused(&windows[2].clone().into(), &mut state).unwrap();
    assert_eq!(workspace.scroll_offset(), 210);
    assert!(state.pending_sync.has_changes());
  }

  #[test]
  fn clamps_to_end_of_strip() {
    let (workspace, windows) = mock_scrolling_workspace(
      TilingDirection::Horizontal,
      4,
      0.5,
      &GapsConfig::default(),
      ScrollConfig::default(),
    );

    let mut state = WmState::mock();

    center_focused(&windows[3].clone().into(), &mut state).unwrap();

    assert_eq!(
      workspace.scroll_offset(),
      layouts::scrolling::max_scroll_offset(&workspace).unwrap()
    );
  }
}
