use anyhow::Context;
use wm_common::{TilingDirection, WindowState};
use wm_platform::Direction;

use super::set_focused_descendant;
use crate::{
  models::{Container, TilingContainer},
  traits::{CommonGetters, TilingDirectionGetters, WindowGetters},
  wm_state::WmState,
};

pub fn focus_in_direction(
  origin_container: &Container,
  direction: &Direction,
  state: &mut WmState,
) -> anyhow::Result<()> {
  let focus_target = match origin_container {
    Container::TilingWindow(_) => {
      // If a suitable focus target isn't found in the current workspace,
      // attempt to find a workspace in the given direction.
      tiling_focus_target(origin_container, direction)?.map_or_else(
        || workspace_focus_target(origin_container, direction, state),
        |container| Ok(Some(container)),
      )?
    }
    Container::NonTilingWindow(ref non_tiling_window) => {
      match non_tiling_window.state() {
        WindowState::Floating(_) => {
          floating_focus_target(origin_container, direction)
        }
        WindowState::Fullscreen(_) => {
          workspace_focus_target(origin_container, direction, state)?
        }
        _ => None,
      }
    }
    Container::Workspace(_) => {
      workspace_focus_target(origin_container, direction, state)?
    }
    _ => None,
  };

  // Set focus to the target container.
  if let Some(focus_target) = focus_target {
    set_focused_descendant(&focus_target, None);
    state.pending_sync.queue_focus_change().queue_cursor_jump();
  }

  Ok(())
}

fn floating_focus_target(
  origin_container: &Container,
  direction: &Direction,
) -> Option<Container> {
  let is_floating = |sibling: &Container| {
    sibling.as_non_tiling_window().is_some_and(|window| {
      matches!(window.state(), WindowState::Floating(_))
    })
  };

  let mut floating_siblings =
    origin_container.siblings().filter(is_floating);

  // Wrap if next/previous floating window is not found.
  match direction {
    Direction::Left => origin_container
      .next_siblings()
      .find(is_floating)
      .or_else(|| floating_siblings.last()),
    Direction::Right => origin_container
      .prev_siblings()
      .find(is_floating)
      .or_else(|| floating_siblings.next()),
    // Cannot focus vertically from a floating window.
    _ => None,
  }
}

/// Gets a focus target within the current workspace. Traverse upwards from
/// the origin container to find an adjacent container that can be focused.
fn tiling_focus_target(
  origin_container: &Container,
  direction: &Direction,
) -> anyhow::Result<Option<Container>> {
  let tiling_direction = TilingDirection::from_direction(direction);
  let mut origin_or_ancestor = origin_container.clone();

  // Traverse upwards from the focused container. Stop searching when a
  // workspace is encountered.
  while !origin_or_ancestor.is_workspace() {
    let parent = origin_or_ancestor
      .parent()
      .and_then(|parent| parent.as_direction_container().ok())
      .context("No direction container.")?;

    // Skip if the tiling direction doesn't match.
    if parent.tiling_direction() != tiling_direction {
      origin_or_ancestor = parent.into();
      continue;
    }

    // Get the next/prev tiling sibling depending on the tiling direction.
    let focus_target = match direction {
      Direction::Up | Direction::Left => origin_or_ancestor
        .prev_siblings()
        .find_map(|c| c.as_tiling_container().ok()),
      _ => origin_or_ancestor
        .next_siblings()
        .find_map(|c| c.as_tiling_container().ok()),
    };

    match focus_target {
      Some(target) => {
        // Return once a suitable focus target is found.
        return Ok(match target {
          TilingContainer::TilingWindow(_) => Some(target.into()),
          TilingContainer::Split(split) => split
            .descendant_in_direction(&direction.inverse())
            .map(Into::into),
        });
      }
      None => origin_or_ancestor = parent.into(),
    }
  }

  Ok(None)
}

/// Gets a focus target outside of the current workspace in the given
/// direction.
///
/// This will descend into the workspace in the given direction, and will
/// always return a tiling container. This makes it different from the
/// `focus_workspace` command with `FocusWorkspaceTarget::Direction`.
fn workspace_focus_target(
  origin_container: &Container,
  direction: &Direction,
  state: &WmState,
) -> anyhow::Result<Option<Container>> {
  let monitor = origin_container.monitor().context("No monitor.")?;

  let target_workspace = state
    .monitor_in_direction(&monitor, direction)?
    .and_then(|monitor| monitor.displayed_workspace());

  let focused_fullscreen = target_workspace
    .as_ref()
    .and_then(|workspace| workspace.descendant_focus_order().next())
    .filter(|focused| match focused {
      Container::NonTilingWindow(window) => {
        matches!(window.state(), WindowState::Fullscreen(_))
      }
      _ => false,
    });

  let focus_target = focused_fullscreen
    .or_else(|| {
      target_workspace.as_ref().and_then(|workspace| {
        workspace
          .descendant_in_direction(&direction.inverse())
          .map(Into::into)
      })
    })
    .or(target_workspace.map(Into::into));

  Ok(focus_target)
}

#[cfg(test)]
mod tests {
  use uuid::Uuid;
  use wm_common::{GapsConfig, ScrollConfig, WorkspaceLayout};

  use super::*;
  use crate::{
    commands::container::{attach_container, set_focused_descendant},
    models::{SplitContainer, TilingWindow, Workspace},
    test_utils::mock_scrolling_workspace,
  };

  /// Gets the ID of the focus target when moving in a direction from the
  /// given container.
  fn focus_target_id(
    origin: &Container,
    direction: &Direction,
  ) -> Option<Uuid> {
    tiling_focus_target(origin, direction)
      .unwrap()
      .map(|container| container.id())
  }

  #[test]
  fn focuses_next_and_previous_strip_items() {
    let (_workspace, windows) = mock_scrolling_workspace(
      TilingDirection::Horizontal,
      3,
      0.5,
      &GapsConfig::default(),
      ScrollConfig::default(),
    );

    // Right/left move to the next/previous window in the strip.
    assert_eq!(
      focus_target_id(&windows[1].clone().into(), &Direction::Right),
      Some(windows[2].id())
    );
    assert_eq!(
      focus_target_id(&windows[1].clone().into(), &Direction::Left),
      Some(windows[0].id())
    );
    assert_eq!(
      focus_target_id(&windows[0].clone().into(), &Direction::Right),
      Some(windows[1].id())
    );
  }

  #[test]
  fn follows_scroll_axis_of_workspace() {
    let (_workspace, windows) = mock_scrolling_workspace(
      TilingDirection::Vertical,
      3,
      0.5,
      &GapsConfig::default(),
      ScrollConfig::default(),
    );

    // Down/up move to the next/previous window in a vertical strip.
    assert_eq!(
      focus_target_id(&windows[1].clone().into(), &Direction::Down),
      Some(windows[2].id())
    );
    assert_eq!(
      focus_target_id(&windows[1].clone().into(), &Direction::Up),
      Some(windows[0].id())
    );
  }

  #[test]
  fn ignores_cross_axis_directions() {
    let (_workspace, windows) = mock_scrolling_workspace(
      TilingDirection::Horizontal,
      3,
      0.5,
      &GapsConfig::default(),
      ScrollConfig::default(),
    );

    // Directions that don't match the scroll axis have no focus target
    // within the strip. The caller falls back to focusing the workspace
    // in that direction.
    assert!(tiling_focus_target(
      &windows[1].clone().into(),
      &Direction::Up
    )
    .unwrap()
    .is_none());
    assert!(tiling_focus_target(
      &windows[1].clone().into(),
      &Direction::Down
    )
    .unwrap()
    .is_none());

    let (_workspace, windows) = mock_scrolling_workspace(
      TilingDirection::Vertical,
      3,
      0.5,
      &GapsConfig::default(),
      ScrollConfig::default(),
    );

    assert!(tiling_focus_target(
      &windows[1].clone().into(),
      &Direction::Left
    )
    .unwrap()
    .is_none());
    assert!(tiling_focus_target(
      &windows[1].clone().into(),
      &Direction::Right
    )
    .unwrap()
    .is_none());
  }

  #[test]
  fn does_not_wrap_around_strip() {
    let (_workspace, windows) = mock_scrolling_workspace(
      TilingDirection::Horizontal,
      3,
      0.5,
      &GapsConfig::default(),
      ScrollConfig::default(),
    );

    // Moving left from the first window or right from the last window
    // doesn't wrap around the strip. The caller falls back to focusing
    // the workspace in that direction.
    assert!(tiling_focus_target(
      &windows[0].clone().into(),
      &Direction::Left
    )
    .unwrap()
    .is_none());
    assert!(tiling_focus_target(
      &windows[2].clone().into(),
      &Direction::Right
    )
    .unwrap()
    .is_none());
  }

  #[test]
  fn navigates_within_and_across_split_columns() {
    let workspace = Workspace::mock()
      .tiling_direction(TilingDirection::Horizontal)
      .layout(WorkspaceLayout::Scrolling)
      .scroll(ScrollConfig::default())
      .call();

    let left = TilingWindow::mock().call();
    let top = TilingWindow::mock().call();
    let bottom = TilingWindow::mock().call();
    let right = TilingWindow::mock().call();

    // A vertical split (i.e. a stacked column) between two strip items.
    let children: Vec<TilingContainer> =
      vec![top.clone().into(), bottom.clone().into()];
    let split = SplitContainer::mock()
      .tiling_direction(TilingDirection::Vertical)
      .tiling_containers(children)
      .call();

    attach_container(
      &left.clone().into(),
      &workspace.clone().into(),
      None,
    )
    .unwrap();
    attach_container(
      &split.clone().into(),
      &workspace.clone().into(),
      None,
    )
    .unwrap();
    attach_container(
      &right.clone().into(),
      &workspace.clone().into(),
      None,
    )
    .unwrap();

    // Vertical movement within the split is handled by the split itself.
    assert_eq!(
      focus_target_id(&top.clone().into(), &Direction::Down),
      Some(bottom.id())
    );

    // Moving out of the split crosses to the next/previous strip item.
    assert_eq!(
      focus_target_id(&bottom.clone().into(), &Direction::Right),
      Some(right.id())
    );
    assert_eq!(
      focus_target_id(&left.clone().into(), &Direction::Right),
      Some(top.id())
    );

    // Moving toward the split descends into its last-focused window.
    set_focused_descendant(&bottom.clone().into(), None);
    assert_eq!(
      focus_target_id(&right.clone().into(), &Direction::Left),
      Some(bottom.id())
    );
  }
}
