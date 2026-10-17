use anyhow::Context;
use wm_common::{TilingDirection, WmEvent};

use super::{flatten_split_container, wrap_in_split_container};
use crate::{
  layouts,
  models::{Container, DirectionContainer, SplitContainer, TilingWindow},
  traits::{CommonGetters, TilingDirectionGetters},
  user_config::UserConfig,
  wm_state::WmState,
};

pub fn toggle_tiling_direction(
  container: Container,
  state: &mut WmState,
  config: &UserConfig,
) -> anyhow::Result<()> {
  let direction_container = match container {
    Container::TilingWindow(tiling_window) => {
      toggle_window_direction(tiling_window, config)
    }
    Container::Workspace(workspace) => {
      workspace
        .set_tiling_direction(workspace.tiling_direction().inverse());

      Ok(workspace.into())
    }
    // Can only toggle tiling direction from a tiling window or workspace.
    _ => return Ok(()),
  }?;

  state.emit_event(WmEvent::TilingDirectionChanged {
    direction_container: direction_container.to_dto()?,
    new_tiling_direction: direction_container.tiling_direction(),
  });

  Ok(())
}

fn toggle_window_direction(
  tiling_window: TilingWindow,
  config: &UserConfig,
) -> anyhow::Result<DirectionContainer> {
  let parent = tiling_window
    .direction_container()
    .context("No direction container.")?;

  // Some layouts handle toggles of their direct children themselves (e.g.
  // toggling a strip item of a scrolling workspace flips the workspace's
  // scroll axis instead of wrapping the item in a split container).
  if let Some(direction_container) =
    layouts::toggle_child_direction(&parent)
  {
    return Ok(direction_container);
  }

  // If the window is an only child, then either change the tiling
  // direction of its parent workspace or flatten its parent split
  // container.
  if tiling_window.tiling_siblings().count() == 0 {
    return match parent {
      DirectionContainer::Workspace(workspace) => {
        workspace
          .set_tiling_direction(workspace.tiling_direction().inverse());

        Ok(workspace.into())
      }
      DirectionContainer::Split(split_container) => {
        flatten_split_container(split_container.clone())?;

        tiling_window
          .direction_container()
          .context("No direction container.")
      }
    };
  }

  // Create a new split container to wrap the window.
  let split_container = SplitContainer::new(
    parent.tiling_direction().inverse(),
    config.value.gaps.clone(),
  );

  wrap_in_split_container(
    &split_container,
    &parent.into(),
    &[tiling_window.into()],
  )?;

  Ok(split_container.into())
}

pub fn set_tiling_direction(
  container: Container,
  state: &mut WmState,
  config: &UserConfig,
  tiling_direction: &TilingDirection,
) -> anyhow::Result<()> {
  let direction_container = container
    .direction_container()
    .context("No direction container.")?;

  if direction_container.tiling_direction() == *tiling_direction {
    Ok(())
  } else {
    toggle_tiling_direction(container, state, config)
  }
}

#[cfg(test)]
mod tests {
  use wm_common::{GapsConfig, ScrollConfig};

  use super::*;
  use crate::{
    models::{Monitor, Workspace},
    test_utils::mock_scrolling_workspace,
    user_config::UserConfig,
    wm_state::WmState,
  };

  #[test]
  fn flips_scroll_axis_of_strip_item() {
    let (workspace, windows) = mock_scrolling_workspace(
      TilingDirection::Horizontal,
      2,
      0.5,
      &GapsConfig::default(),
      ScrollConfig::default(),
    );

    let mut state = WmState::mock();
    let config = UserConfig::mock();

    toggle_tiling_direction(
      windows[0].clone().into(),
      &mut state,
      &config,
    )
    .unwrap();

    // The scroll axis is flipped, and the window is not wrapped in a
    // split container.
    assert_eq!(workspace.tiling_direction(), TilingDirection::Vertical);
    assert_eq!(workspace.child_count(), 2);
    assert_eq!(windows[0].parent().unwrap().id(), workspace.id());
  }

  #[test]
  fn inverts_direction_of_tiling_workspace() {
    let workspace = Workspace::mock().call();
    let _monitor =
      Monitor::mock().workspaces(vec![workspace.clone()]).call();

    let mut state = WmState::mock();
    let config = UserConfig::mock();

    toggle_tiling_direction(workspace.clone().into(), &mut state, &config)
      .unwrap();

    assert_eq!(workspace.tiling_direction(), TilingDirection::Vertical);
  }
}
