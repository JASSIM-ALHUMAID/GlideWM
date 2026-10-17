use wm_common::TilingDirection;
use wm_platform::{Direction, LengthUnit, LengthValue};

use crate::{
  layouts, models::Workspace, traits::TilingDirectionGetters,
  wm_state::WmState,
};

/// Amount scrolled when the `scroll` command is invoked without an
/// explicit amount, as a fraction of the viewport.
const DEFAULT_SCROLL_AMOUNT: LengthValue = LengthValue {
  amount: 0.1,
  unit: LengthUnit::Percentage,
};

/// Offsets the scroll position of a scrolling workspace without changing
/// focus.
///
/// Has no effect for workspaces that don't use the scrolling layout or
/// for directions perpendicular to the scroll axis. The offset is clamped
/// to the valid range, so scrolling never goes past either end of the
/// strip. The next focus change re-ensures the focused strip item is
/// visible, which may undo scrolling away from it.
pub fn scroll_workspace(
  workspace: &Workspace,
  direction: &Direction,
  amount: Option<&LengthValue>,
  state: &mut WmState,
) -> anyhow::Result<()> {
  if !layouts::supports_scrolling(workspace) {
    return Ok(());
  }

  // Only directions along the scroll axis scroll the strip.
  if TilingDirection::from_direction(direction)
    != workspace.tiling_direction()
  {
    return Ok(());
  }

  let viewport_extent = layouts::scrolling::viewport_extent(workspace)?;

  // Pixel amounts are interpreted as physical pixels; percentage amounts
  // as a fraction of the viewport extent.
  let scroll_distance = amount
    .unwrap_or(&DEFAULT_SCROLL_AMOUNT)
    .to_px(viewport_extent, None);

  let new_offset = match direction {
    Direction::Up | Direction::Left => {
      workspace.scroll_offset() - scroll_distance
    }
    _ => workspace.scroll_offset() + scroll_distance,
  };

  let offset_changed = workspace.set_scroll_offset(new_offset);
  let offset_changed =
    layouts::scrolling::clamp_scroll_offset(workspace)? || offset_changed;

  if offset_changed {
    state
      .pending_sync
      .queue_container_to_redraw(workspace.clone());
  }

  Ok(())
}

#[cfg(test)]
mod tests {
  use wm_common::{GapsConfig, ScrollConfig, TilingDirection};
  use wm_platform::LengthUnit;

  use super::*;
  use crate::{models::Workspace, test_utils::mock_scrolling_workspace};

  #[test]
  fn scrolls_by_amount_along_axis() {
    let (workspace, _windows) = mock_scrolling_workspace(
      TilingDirection::Horizontal,
      3,
      0.5,
      &GapsConfig::default(),
      ScrollConfig::default(),
    );

    let mut state = WmState::mock();

    // The default amount is 10% of the viewport extent.
    scroll_workspace(&workspace, &Direction::Right, None, &mut state)
      .unwrap();

    assert_eq!(workspace.scroll_offset(), 168);
    assert!(state.pending_sync.has_changes());

    // Percentage amounts are relative to the viewport extent.
    scroll_workspace(
      &workspace,
      &Direction::Right,
      Some(&LengthValue {
        amount: 0.25,
        unit: LengthUnit::Percentage,
      }),
      &mut state,
    )
    .unwrap();

    assert_eq!(workspace.scroll_offset(), 168 + 420);

    // Pixel amounts are physical pixels.
    scroll_workspace(
      &workspace,
      &Direction::Left,
      Some(&LengthValue::from_px(100)),
      &mut state,
    )
    .unwrap();

    assert_eq!(workspace.scroll_offset(), 168 + 420 - 100);
  }

  #[test]
  fn clamps_at_strip_ends() {
    let (workspace, _windows) = mock_scrolling_workspace(
      TilingDirection::Horizontal,
      3,
      0.5,
      &GapsConfig::default(),
      ScrollConfig::default(),
    );

    let mut state = WmState::mock();

    // Scrolling past the end of the strip clamps to the maximum offset.
    scroll_workspace(
      &workspace,
      &Direction::Right,
      Some(&LengthValue::from_px(10_000)),
      &mut state,
    )
    .unwrap();

    assert_eq!(workspace.scroll_offset(), 840);
    assert_eq!(
      workspace.scroll_offset(),
      layouts::scrolling::max_scroll_offset(&workspace).unwrap()
    );

    // Scrolling past the start of the strip clamps to 0.
    scroll_workspace(
      &workspace,
      &Direction::Left,
      Some(&LengthValue::from_px(10_000)),
      &mut state,
    )
    .unwrap();

    assert_eq!(workspace.scroll_offset(), 0);
  }

  #[test]
  fn ignores_cross_axis_and_non_scrolling_workspaces() {
    let (workspace, _windows) = mock_scrolling_workspace(
      TilingDirection::Horizontal,
      2,
      0.5,
      &GapsConfig::default(),
      ScrollConfig::default(),
    );

    let mut state = WmState::mock();

    // Directions perpendicular to the scroll axis are ignored.
    scroll_workspace(&workspace, &Direction::Up, None, &mut state)
      .unwrap();

    assert_eq!(workspace.scroll_offset(), 0);
    assert!(!state.pending_sync.has_changes());

    // Workspaces that don't use the scrolling layout are ignored.
    let tiling_workspace = Workspace::mock().call();

    scroll_workspace(
      &tiling_workspace,
      &Direction::Right,
      None,
      &mut state,
    )
    .unwrap();

    assert_eq!(tiling_workspace.scroll_offset(), 0);
    assert!(!state.pending_sync.has_changes());

    // A vertical workspace scrolls along its own axis.
    let (vertical_workspace, _windows) = mock_scrolling_workspace(
      TilingDirection::Vertical,
      3,
      0.5,
      &GapsConfig::default(),
      ScrollConfig::default(),
    );

    scroll_workspace(
      &vertical_workspace,
      &Direction::Left,
      None,
      &mut state,
    )
    .unwrap();

    assert_eq!(vertical_workspace.scroll_offset(), 0);

    scroll_workspace(
      &vertical_workspace,
      &Direction::Down,
      None,
      &mut state,
    )
    .unwrap();

    assert_eq!(vertical_workspace.scroll_offset(), 100);
  }
}
