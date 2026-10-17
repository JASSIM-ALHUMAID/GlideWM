use anyhow::Context;
use wm_common::{TilingDirection, WindowState};
use wm_platform::{Direction, Rect};

use crate::{
  commands::container::{
    flatten_split_container, move_container_within_tree,
    set_focused_descendant,
  },
  layouts,
  models::{
    DirectionContainer, Monitor, NonTilingWindow, TilingContainer,
    TilingWindow, WindowContainer,
  },
  traits::{
    CommonGetters, PositionGetters, TilingDirectionGetters, WindowGetters,
  },
  user_config::UserConfig,
  wm_state::WmState,
};

/// The distance in pixels to snap the window to the monitor's edge.
const SNAP_DISTANCE: i32 = 15;

pub fn move_window_in_direction(
  window: WindowContainer,
  direction: &Direction,
  state: &mut WmState,
  config: &UserConfig,
) -> anyhow::Result<()> {
  match window {
    WindowContainer::TilingWindow(window) => {
      move_tiling_window(window, direction, state, config)
    }
    WindowContainer::NonTilingWindow(non_tiling_window) => {
      match non_tiling_window.state() {
        WindowState::Floating(_) => {
          move_floating_window(non_tiling_window, direction, state)
        }
        WindowState::Fullscreen(_) => move_to_workspace_in_direction(
          &non_tiling_window.into(),
          direction,
          state,
        ),
        _ => Ok(()),
      }
    }
  }
}

fn move_tiling_window(
  window_to_move: TilingWindow,
  direction: &Direction,
  state: &mut WmState,
  config: &UserConfig,
) -> anyhow::Result<()> {
  // Flatten the parent split container if it only contains the window.
  if let Some(split_parent) = window_to_move
    .parent()
    .and_then(|parent| parent.as_split().cloned())
  {
    if split_parent.child_count() == 1 {
      flatten_split_container(split_parent)?;
    }
  }

  let parent = window_to_move
    .direction_container()
    .context("No direction container.")?;

  let has_matching_tiling_direction = parent.tiling_direction()
    == TilingDirection::from_direction(direction);

  // Layouts can handle moves of their direct children themselves (e.g.
  // strip items of a scrolling workspace are reordered within the strip,
  // or moved to the workspace in the given direction when there is no
  // sibling in that direction). The workspace's tiling direction (i.e.
  // its scroll axis) is never inverted.
  if let Some(result) =
    layouts::move_child_in_direction(&window_to_move, direction, state)
  {
    return result;
  }

  // Attempt to swap or move the window into a sibling container.
  if has_matching_tiling_direction {
    if let Some(sibling) =
      tiling_sibling_in_direction(&window_to_move, direction)
    {
      return move_to_sibling_container(
        window_to_move,
        sibling,
        direction,
        state,
      );
    }
  }

  // Attempt to move the window to workspace in given direction.
  if (has_matching_tiling_direction
    || window_to_move.tiling_siblings().count() == 0)
    && parent.is_workspace()
  {
    return move_to_workspace_in_direction(
      &window_to_move.into(),
      direction,
      state,
    );
  }

  // The window cannot be moved within the parent container, so traverse
  // upwards to find an ancestor that has the correct tiling direction.
  let target_ancestor = parent.ancestors().find_map(|ancestor| {
    ancestor.as_direction_container().ok().filter(|ancestor| {
      ancestor.tiling_direction()
        == TilingDirection::from_direction(direction)
    })
  });

  match target_ancestor {
    // If there is no suitable ancestor, the layout makes room for the
    // window: tiling workspaces invert their tiling direction, while
    // strip-style layouts move the window to the workspace in the given
    // direction instead (see `layouts::move_without_target_ancestor`).
    None => layouts::move_without_target_ancestor(
      &window_to_move,
      direction,
      state,
      config,
    ),
    // Otherwise, move the container into the given ancestor. This could
    // simply be the container's direct parent.
    Some(target_ancestor) => insert_into_ancestor(
      &window_to_move,
      &target_ancestor,
      direction,
      state,
    ),
  }
}

/// Gets the next sibling `TilingWindow` or `SplitContainer` in the given
/// direction.
///
/// Also used by the layouts module to reorder strip items.
pub(crate) fn tiling_sibling_in_direction(
  window: &TilingWindow,
  direction: &Direction,
) -> Option<TilingContainer> {
  match direction {
    Direction::Up | Direction::Left => window
      .prev_siblings()
      .find_map(|sibling| sibling.as_tiling_container().ok()),
    _ => window
      .next_siblings()
      .find_map(|sibling| sibling.as_tiling_container().ok()),
  }
}

fn move_to_sibling_container(
  window_to_move: TilingWindow,
  target_sibling: TilingContainer,
  direction: &Direction,
  state: &mut WmState,
) -> anyhow::Result<()> {
  let parent = window_to_move.parent().context("No parent.")?;

  match target_sibling {
    TilingContainer::TilingWindow(sibling_window) => {
      // Swap the window with sibling in given direction.
      move_container_within_tree(
        &window_to_move.clone().into(),
        &parent,
        sibling_window.index(),
        state,
      )?;

      state
        .pending_sync
        .queue_container_to_redraw(sibling_window)
        .queue_container_to_redraw(window_to_move);
    }
    TilingContainer::Split(sibling_split) => {
      let sibling_descendant =
        sibling_split.descendant_in_direction(&direction.inverse());

      // Move the window into the sibling split container.
      if let Some(sibling_descendant) = sibling_descendant {
        let target_parent = sibling_descendant
          .direction_container()
          .context("No direction container.")?;

        let has_matching_tiling_direction =
          TilingDirection::from_direction(direction)
            == target_parent.tiling_direction();

        let target_index = match direction {
          Direction::Down | Direction::Right
            if has_matching_tiling_direction =>
          {
            sibling_descendant.index()
          }
          _ => sibling_descendant.index() + 1,
        };

        move_container_within_tree(
          &window_to_move.into(),
          &target_parent.clone().into(),
          target_index,
          state,
        )?;

        state
          .pending_sync
          .queue_container_to_redraw(target_parent)
          .queue_containers_to_redraw(parent.tiling_children());
      }
    }
  }

  Ok(())
}

/// Moves a window to the workspace (or monitor) in the given direction.
///
/// Also used by the layouts module to move strip items cross-axis.
pub(crate) fn move_to_workspace_in_direction(
  window_to_move: &WindowContainer,
  direction: &Direction,
  state: &mut WmState,
) -> anyhow::Result<()> {
  let parent = window_to_move.parent().context("No parent.")?;
  let workspace = window_to_move.workspace().context("No workspace.")?;
  let monitor = parent.monitor().context("No monitor.")?;

  let target_workspace = state
    .monitor_in_direction(&monitor, direction)?
    .and_then(|monitor| monitor.displayed_workspace());

  if let Some(target_workspace) = target_workspace {
    // Since the window is crossing monitors, adjustments might need to be
    // made because of DPI.
    if monitor.has_dpi_difference(&target_workspace.clone().into())? {
      window_to_move.set_has_pending_dpi_adjustment(true);
    }

    // Update floating placement since the window has to cross monitors.
    window_to_move.set_floating_placement(
      window_to_move
        .floating_placement()
        .translate_to_center(&target_workspace.to_rect()?),
    );

    if let WindowContainer::NonTilingWindow(window_to_move) =
      &window_to_move
    {
      window_to_move.set_insertion_target(None);
    }

    let target_index = match direction {
      Direction::Down | Direction::Right => 0,
      _ => target_workspace.child_count(),
    };

    // Focus should be reassigned within the original workspace after the
    // window is moved out. For example, if the focus order is 1. tiling
    // window and 2. fullscreen window, then we'd want to retain focus on a
    // tiling window on move.
    let focus_target = state.focus_target_after_removal(window_to_move);

    move_container_within_tree(
      &window_to_move.clone().into(),
      &target_workspace.clone().into(),
      target_index,
      state,
    )?;

    if let Some(focus_target) = focus_target {
      set_focused_descendant(
        &focus_target,
        Some(&workspace.clone().into()),
      );
    }

    state
      .pending_sync
      .queue_container_to_redraw(window_to_move.clone())
      .queue_containers_to_redraw(target_workspace.tiling_children())
      .queue_containers_to_redraw(parent.tiling_children())
      .queue_cursor_jump()
      .queue_workspace_to_reorder(target_workspace);
  }

  Ok(())
}

fn insert_into_ancestor(
  window_to_move: &TilingWindow,
  target_ancestor: &DirectionContainer,
  direction: &Direction,
  state: &mut WmState,
) -> anyhow::Result<()> {
  // Traverse upwards to find container whose parent is the target
  // ancestor. Then, depending on the direction, insert before or after
  // that container.
  let window_ancestor = window_to_move
    .ancestors()
    .find(|container| {
      container
        .parent()
        .is_some_and(|parent| parent == target_ancestor.clone().into())
    })
    .context("Window ancestor not found.")?;

  let target_index = match direction {
    Direction::Up | Direction::Left => window_ancestor.index(),
    _ => window_ancestor.index() + 1,
  };

  // Move the window into the container above.
  move_container_within_tree(
    &window_to_move.clone().into(),
    &target_ancestor.clone().into(),
    target_index,
    state,
  )?;

  state
    .pending_sync
    .queue_containers_to_redraw(target_ancestor.tiling_children());

  Ok(())
}

fn move_floating_window(
  window_to_move: NonTilingWindow,
  direction: &Direction,
  state: &mut WmState,
) -> anyhow::Result<()> {
  let new_position =
    new_floating_position(&window_to_move, direction, state)?;

  if let Some((position_rect, target_monitor)) = new_position {
    let monitor = window_to_move.monitor().context("No monitor.")?;

    // Mark window as needing DPI adjustment if it crosses monitors. The
    // handler for `PlatformEvent::LocationChanged` will update the
    // window's workspace if it goes out of bounds of its current
    // workspace.
    if monitor.id() != target_monitor.id()
      && monitor.has_dpi_difference(&target_monitor.into())?
    {
      window_to_move.set_has_pending_dpi_adjustment(true);
    }

    window_to_move.set_floating_placement(position_rect);
    state.pending_sync.queue_container_to_redraw(window_to_move);
  }

  Ok(())
}

/// Returns a tuple of the new floating position and the target monitor.
fn new_floating_position(
  window_to_move: &NonTilingWindow,
  direction: &Direction,
  state: &mut WmState,
) -> anyhow::Result<Option<(Rect, Monitor)>> {
  let monitor = window_to_move.monitor().context("No monitor.")?;
  let monitor_rect = monitor.native_properties().working_area;
  let window_pos = window_to_move.native_properties().frame;

  let is_on_monitor_edge = match direction {
    Direction::Up => window_pos.top == monitor_rect.top,
    Direction::Down => window_pos.bottom == monitor_rect.bottom,
    Direction::Left => window_pos.left == monitor_rect.left,
    Direction::Right => window_pos.right == monitor_rect.right,
  };

  // Window is on the edge of the monitor and should be moved to a
  // different monitor in the given direction.
  if is_on_monitor_edge {
    let next_monitor = state.monitor_in_direction(&monitor, direction)?;

    if let Some(next_monitor) = next_monitor {
      let monitor_rect = next_monitor.native().working_area()?.clone();

      let position = snap_to_monitor_edge(
        &window_pos,
        &monitor_rect,
        &direction.inverse(),
      )
      .clamp(&monitor_rect);

      return Ok(Some((position, next_monitor)));
    }

    return Ok(None);
  }

  let (monitor_length, window_length) = match direction {
    Direction::Up | Direction::Down => {
      (monitor_rect.height(), window_pos.height())
    }
    _ => (monitor_rect.width(), window_pos.width()),
  };

  let length_delta = monitor_length - window_length;

  // Calculate the distance the window should move based on the ratio of
  // the window's length to the monitor's length.
  #[allow(clippy::cast_precision_loss)]
  let move_distance = match window_length as f32 / monitor_length as f32 {
    x if (0.0..0.2).contains(&x) => length_delta / 5,
    x if (0.2..0.4).contains(&x) => length_delta / 4,
    x if (0.4..0.6).contains(&x) => length_delta / 3,
    _ => length_delta / 2,
  };

  // Snap the window to the current monitor's edge if it's within 15px of
  // it after the move.
  let should_snap_to_edge = match direction {
    Direction::Up => {
      window_pos.top - move_distance - SNAP_DISTANCE < monitor_rect.top
    }
    Direction::Down => {
      window_pos.bottom + move_distance + SNAP_DISTANCE
        > monitor_rect.bottom
    }
    Direction::Left => {
      window_pos.left - move_distance - SNAP_DISTANCE < monitor_rect.left
    }
    Direction::Right => {
      window_pos.right + move_distance + SNAP_DISTANCE > monitor_rect.right
    }
  };

  if should_snap_to_edge {
    let position =
      snap_to_monitor_edge(&window_pos, &monitor_rect, direction);

    return Ok(Some((position, monitor)));
  }

  // Snap the window to the current monitor's inverse edge if it's in
  // between two monitors or outside the bounds of the current monitor.
  let should_snap_to_inverse_edge = match direction {
    Direction::Up => window_pos.bottom > monitor_rect.bottom,
    Direction::Down => window_pos.top < monitor_rect.top,
    Direction::Left => window_pos.right > monitor_rect.right,
    Direction::Right => window_pos.left < monitor_rect.left,
  };

  let position = if should_snap_to_inverse_edge {
    snap_to_monitor_edge(&window_pos, &monitor_rect, &direction.inverse())
  } else {
    window_pos.translate_in_direction(direction, move_distance)
  };

  Ok(Some((position, monitor)))
}

fn snap_to_monitor_edge(
  window_pos: &Rect,
  monitor_rect: &Rect,
  edge: &Direction,
) -> Rect {
  let (x, y) = match edge {
    Direction::Up => (window_pos.x(), monitor_rect.top),
    Direction::Down => {
      (window_pos.x(), monitor_rect.bottom - window_pos.height())
    }
    Direction::Left => (monitor_rect.left, window_pos.y()),
    Direction::Right => {
      (monitor_rect.right - window_pos.width(), window_pos.y())
    }
  };

  window_pos.translate_to_coordinates(x, y)
}

#[cfg(test)]
mod tests {
  use uuid::Uuid;
  use wm_common::{
    GapsConfig, ScrollConfig, TilingDirection, WorkspaceLayout,
  };

  use super::*;
  use crate::{
    commands::container::attach_container,
    models::{SplitContainer, TilingWindow, Workspace},
    test_utils::{
      mock_scrolling_workspace, MOCK_MONITOR_HEIGHT, MOCK_MONITOR_WIDTH,
      MOCK_TASKBAR_HEIGHT,
    },
    traits::TilingSizeGetters,
    user_config::UserConfig,
    wm_state::WmState,
  };

  /// IDs of the top-level tiling children (i.e. strip items) of a
  /// workspace.
  fn strip_item_ids(workspace: &Workspace) -> Vec<Uuid> {
    workspace
      .tiling_children()
      .map(|child| child.id())
      .collect()
  }

  #[test]
  fn reorders_strip_items() {
    let (workspace, windows) = mock_scrolling_workspace(
      TilingDirection::Horizontal,
      3,
      0.5,
      &GapsConfig::default(),
      ScrollConfig::default(),
    );

    let mut state = WmState::mock();
    let config = UserConfig::mock();

    move_window_in_direction(
      windows[0].clone().into(),
      &Direction::Right,
      &mut state,
      &config,
    )
    .unwrap();

    assert_eq!(
      strip_item_ids(&workspace),
      vec![windows[1].id(), windows[0].id(), windows[2].id()]
    );

    // Reordering does not redistribute sizes.
    assert!((windows[0].tiling_size() - 0.5).abs() < f32::EPSILON);
    assert!((windows[1].tiling_size() - 0.5).abs() < f32::EPSILON);
  }

  #[test]
  fn cross_axis_move_keeps_scroll_axis() {
    let (workspace, windows) = mock_scrolling_workspace(
      TilingDirection::Horizontal,
      2,
      0.5,
      &GapsConfig::default(),
      ScrollConfig::default(),
    );

    let mut state = WmState::mock();
    let config = UserConfig::mock();

    // There is no monitor in the given direction, so the window stays put.
    move_window_in_direction(
      windows[0].clone().into(),
      &Direction::Up,
      &mut state,
      &config,
    )
    .unwrap();

    // The scroll axis is not inverted, and the strip is unchanged.
    assert_eq!(workspace.tiling_direction(), TilingDirection::Horizontal);
    assert_eq!(
      strip_item_ids(&workspace),
      vec![windows[0].id(), windows[1].id()]
    );
  }

  #[test]
  fn moves_to_adjacent_workspace_at_strip_end() {
    let source_workspace = Workspace::mock()
      .tiling_direction(TilingDirection::Horizontal)
      .layout(WorkspaceLayout::Scrolling)
      .scroll(ScrollConfig::default())
      .call();

    let first = TilingWindow::mock().call();
    let last = TilingWindow::mock().call();

    attach_container(
      &first.clone().into(),
      &source_workspace.clone().into(),
      None,
    )
    .unwrap();
    attach_container(
      &last.clone().into(),
      &source_workspace.clone().into(),
      None,
    )
    .unwrap();
    first.set_tiling_size(0.5);
    last.set_tiling_size(0.5);

    let target_workspace = Workspace::mock().name("2".to_string()).call();

    let source_monitor = Monitor::mock()
      .workspaces(vec![source_workspace.clone()])
      .call();
    let target_monitor = Monitor::mock()
      .bounds(Rect::from_xy(
        MOCK_MONITOR_WIDTH,
        0,
        MOCK_MONITOR_WIDTH,
        MOCK_MONITOR_HEIGHT,
      ))
      .working_area(Rect::from_xy(
        MOCK_MONITOR_WIDTH,
        0,
        MOCK_MONITOR_WIDTH,
        MOCK_MONITOR_HEIGHT - MOCK_TASKBAR_HEIGHT,
      ))
      .workspaces(vec![target_workspace.clone()])
      .call();

    let mut state = WmState::mock();
    for monitor in [source_monitor, target_monitor] {
      attach_container(
        &monitor.clone().into(),
        &state.root_container.clone().into(),
        None,
      )
      .unwrap();
    }

    let config = UserConfig::mock();

    // Moving the last strip item in the direction of the scroll axis moves
    // it to the displayed workspace of the monitor to the right.
    move_window_in_direction(
      last.clone().into(),
      &Direction::Right,
      &mut state,
      &config,
    )
    .unwrap();

    assert_eq!(last.workspace().unwrap().id(), target_workspace.id());
    assert_eq!(target_workspace.child_count(), 1);

    // The workspace it left keeps its scroll axis and the size of the
    // remaining strip item.
    assert_eq!(
      source_workspace.tiling_direction(),
      TilingDirection::Horizontal
    );
    assert_eq!(source_workspace.child_count(), 1);
    assert!((first.tiling_size() - 0.5).abs() < f32::EPSILON);
  }

  #[test]
  fn rescales_window_moved_out_of_column() {
    let workspace = Workspace::mock()
      .tiling_direction(TilingDirection::Horizontal)
      .layout(WorkspaceLayout::Scrolling)
      .scroll(ScrollConfig::default())
      .call();

    let top = TilingWindow::mock().call();
    let bottom = TilingWindow::mock().call();
    let right = TilingWindow::mock().call();

    // A vertical split (i.e. a stacked column) next to another strip item.
    let column = SplitContainer::mock()
      .tiling_direction(TilingDirection::Vertical)
      .tiling_containers(vec![top.clone().into(), bottom.clone().into()])
      .call();

    attach_container(
      &column.clone().into(),
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

    // Attach the workspace to a mock monitor (needed for rect math).
    let _monitor =
      Monitor::mock().workspaces(vec![workspace.clone()]).call();

    // Strip items are sized as fractions of the viewport.
    column.set_tiling_size(0.5);
    right.set_tiling_size(0.5);

    let mut state = WmState::mock();
    let config = UserConfig::mock();

    // The window leaves the column and becomes a strip item after it.
    move_window_in_direction(
      top.clone().into(),
      &Direction::Right,
      &mut state,
      &config,
    )
    .unwrap();

    assert_eq!(
      strip_item_ids(&workspace),
      vec![column.id(), top.id(), right.id()]
    );

    // The window's viewport fraction is preserved: half of a column that
    // is half of the viewport becomes a quarter of the viewport.
    assert!((top.tiling_size() - 0.25).abs() < f32::EPSILON);

    // The column is left with the remaining window, which fills it.
    assert_eq!(column.child_count(), 1);
    assert!((bottom.tiling_size() - 1.0).abs() < f32::EPSILON);
  }
}
