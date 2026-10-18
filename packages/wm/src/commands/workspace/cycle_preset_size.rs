use anyhow::Context;

use crate::{
  layouts,
  models::Container,
  traits::{CommonGetters, TilingDirectionGetters, TilingSizeGetters},
  wm_state::WmState,
};

/// Cycles the size of the strip item containing `container` through the
/// preset sizes of its scrolling workspace.
///
/// The presets are `scroll.preset_widths` while the scroll axis is
/// horizontal and `scroll.preset_heights` while it is vertical. The
/// resized strip item is kept in view. Has no effect for workspaces that
/// don't use the scrolling layout, or for containers that aren't tiling
/// descendants of their workspace (e.g. floating windows).
pub fn cycle_preset_size(
  container: &Container,
  reverse: bool,
  state: &mut WmState,
) -> anyhow::Result<()> {
  let workspace = container.workspace().context("No workspace.")?;

  if !layouts::supports_scrolling(&workspace) {
    tracing::info!(
      "Ignoring `cycle-preset-size`: workspace doesn't use the scrolling layout."
    );
    return Ok(());
  }

  let Some(strip_item) =
    layouts::scrolling::strip_item_of(&workspace, container)
  else {
    return Ok(());
  };

  let config = workspace.config();
  let presets = config.scroll.preset_sizes(&workspace.tiling_direction());

  let Some(target_size) = layouts::scrolling::next_preset(
    strip_item.tiling_size(),
    presets,
    reverse,
  ) else {
    return Ok(());
  };

  layouts::resize_tiling_container(&strip_item, target_size);

  // Growing the strip item can push it partially out of the viewport.
  layouts::scrolling::ensure_descendant_visible(&workspace, container)?;
  layouts::scrolling::clamp_scroll_offset(&workspace)?;

  state.pending_sync.queue_container_to_redraw(workspace);

  Ok(())
}

#[cfg(test)]
mod tests {
  use wm_common::{GapsConfig, ScrollConfig, TilingDirection};

  use super::*;
  use crate::{models::Workspace, test_utils::mock_scrolling_workspace};

  fn assert_size(container: &impl TilingSizeGetters, expected: f32) {
    assert!(
      (container.tiling_size() - expected).abs() < f32::EPSILON,
      "expected size {expected}, got {}",
      container.tiling_size()
    );
  }

  #[test]
  fn cycles_through_presets_of_scroll_axis() {
    let (_workspace, windows) = mock_scrolling_workspace(
      TilingDirection::Horizontal,
      2,
      0.5,
      &GapsConfig::default(),
      ScrollConfig {
        preset_widths: vec![0.25, 0.5, 1.0],
        preset_heights: vec![0.75],
        ..ScrollConfig::default()
      },
    );

    let mut state = WmState::mock();
    let window: Container = windows[0].clone().into();

    cycle_preset_size(&window, false, &mut state).unwrap();
    assert_size(&windows[0], 1.0);
    assert!(state.pending_sync.has_changes());

    // Wraps around to the smallest preset.
    cycle_preset_size(&window, false, &mut state).unwrap();
    assert_size(&windows[0], 0.25);

    // Reverse wraps around to the largest preset.
    cycle_preset_size(&window, true, &mut state).unwrap();
    assert_size(&windows[0], 1.0);

    // The sibling strip item is untouched.
    assert_size(&windows[1], 0.5);
  }

  #[test]
  fn uses_preset_heights_on_vertical_axis() {
    let (_workspace, windows) = mock_scrolling_workspace(
      TilingDirection::Vertical,
      2,
      0.5,
      &GapsConfig::default(),
      ScrollConfig {
        preset_widths: vec![0.25],
        preset_heights: vec![0.5, 0.75],
        ..ScrollConfig::default()
      },
    );

    let mut state = WmState::mock();

    cycle_preset_size(&windows[0].clone().into(), false, &mut state)
      .unwrap();

    assert_size(&windows[0], 0.75);
  }

  #[test]
  fn keeps_resized_item_in_view() {
    let (workspace, windows) = mock_scrolling_workspace(
      TilingDirection::Horizontal,
      2,
      0.5,
      &GapsConfig::default(),
      ScrollConfig {
        preset_widths: vec![1.0],
        ..ScrollConfig::default()
      },
    );

    let mut state = WmState::mock();

    // Growing the second item to the full viewport pushes its end out of
    // view; the viewport scrolls to keep it fully visible.
    cycle_preset_size(&windows[1].clone().into(), false, &mut state)
      .unwrap();

    assert_eq!(
      workspace.scroll_offset(),
      layouts::scrolling::max_scroll_offset(&workspace).unwrap()
    );
    assert!(workspace.scroll_offset() > 0);
  }

  #[test]
  fn ignores_non_scrolling_workspaces() {
    let workspace = Workspace::mock().call();
    let mut state = WmState::mock();

    cycle_preset_size(&workspace.clone().into(), false, &mut state)
      .unwrap();

    assert!(!state.pending_sync.has_changes());
  }
}
