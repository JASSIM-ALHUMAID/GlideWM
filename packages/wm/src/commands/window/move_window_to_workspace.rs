use anyhow::Context;
use tracing::info;
use wm_common::WindowState;

use crate::{
  commands::{
    container::{move_container_within_tree, set_focused_descendant},
    workspace::activate_workspace,
  },
  models::{WindowContainer, Workspace, WorkspaceTarget},
  traits::{CommonGetters, PositionGetters, WindowGetters},
  user_config::UserConfig,
  wm_state::WmState,
};

pub fn move_window_to_workspace(
  window: WindowContainer,
  target: WorkspaceTarget,
  state: &mut WmState,
  config: &UserConfig,
) -> anyhow::Result<()> {
  let current_workspace = window.workspace().context("No workspace.")?;
  let current_monitor =
    current_workspace.monitor().context("No monitor.")?;

  let (target_workspace_name, target_workspace) =
    state.workspace_by_target(&current_workspace, target, config)?;

  // Retrieve or activate the target workspace by its name.
  let target_workspace = match target_workspace {
    Some(_) => anyhow::Ok(target_workspace),
    _ => match target_workspace_name {
      Some(name) => {
        activate_workspace(Some(&name), None, state, config)?;

        Ok(state.workspace_by_name(&name))
      }
      _ => Ok(None),
    },
  }?;

  if let Some(target_workspace) = target_workspace {
    if target_workspace.id() == current_workspace.id() {
      return Ok(());
    }

    info!(
      "Moving window to workspace: '{}'.",
      target_workspace.config().name
    );

    let target_monitor =
      target_workspace.monitor().context("No monitor.")?;

    // Since target workspace could be on a different monitor, adjustments
    // might need to be made because of DPI.
    if current_monitor
      .has_dpi_difference(&target_monitor.clone().into())?
    {
      window.set_has_pending_dpi_adjustment(true);
    }

    // Update floating placement if the window has to cross monitors.
    if target_monitor.id() != current_monitor.id() {
      window.set_floating_placement(
        window
          .floating_placement()
          .translate_to_center(&target_workspace.to_rect()?),
      );
    }

    if let WindowContainer::NonTilingWindow(window) = &window {
      window.set_insertion_target(None);
    }

    // Focus target is `None` if the window is not focused.
    let focus_target = state.focus_target_after_removal(&window);

    let focus_reset_target = if target_workspace.is_displayed() {
      None
    } else {
      target_monitor.descendant_focus_order().next()
    };

    let insertion_sibling = target_workspace
      .descendant_focus_order()
      .filter_map(|descendant| descendant.as_window_container().ok())
      .find(|descendant| descendant.state() == WindowState::Tiling);

    // Insert the window into the target workspace.
    insert_window_into_workspace(
      &window,
      &target_workspace,
      insertion_sibling,
      state,
    )?;

    // When moving a focused window within the tree to another workspace,
    // the target workspace will get displayed. If moving the window e.g.
    // from monitor 1 -> 2, and the target workspace is hidden on that
    // monitor, we want to reset focus to the workspace that was displayed
    // on that monitor.
    if let Some(focus_reset_target) = focus_reset_target {
      set_focused_descendant(
        &focus_reset_target,
        Some(&target_monitor.into()),
      );
    }

    // Retain focus within the workspace from where the window was moved.
    if let Some(focus_target) = focus_target {
      set_focused_descendant(&focus_target, None);
      state.pending_sync.queue_focus_change();
    }

    match window {
      WindowContainer::NonTilingWindow(_) => {
        state.pending_sync.queue_container_to_redraw(window);
      }
      WindowContainer::TilingWindow(_) => {
        state
          .pending_sync
          .queue_containers_to_redraw(current_workspace.tiling_children())
          .queue_containers_to_redraw(target_workspace.tiling_children());
      }
    }

    state
      .pending_sync
      .queue_workspace_to_reorder(target_workspace);
  }

  Ok(())
}

/// Inserts the window into the target workspace.
///
/// When the target workspace is scrolling, tiling windows are inserted at
/// the strip level directly after the strip item that contains
/// `insertion_sibling` (or appended to the end of the strip if the target
/// workspace has no tiling windows). Columns are never entered, so the
/// window always becomes a strip item of its own.
///
/// Otherwise, the window is inserted into the sibling's parent at the
/// sibling's index + 1, or appended to the workspace if there is no
/// sibling to insert after.
fn insert_window_into_workspace(
  window: &WindowContainer,
  target_workspace: &Workspace,
  insertion_sibling: Option<WindowContainer>,
  state: &mut WmState,
) -> anyhow::Result<()> {
  if window.is_tiling_window() && target_workspace.is_scrolling() {
    let insert_index = insertion_sibling
      .and_then(|sibling| {
        target_workspace.strip_item_index(&sibling.into())
      })
      .map_or(target_workspace.child_count(), |index| index + 1);

    return move_container_within_tree(
      &window.clone().into(),
      &target_workspace.clone().into(),
      insert_index,
      state,
    );
  }

  match (window.is_tiling_window(), insertion_sibling) {
    (true, Some(insertion_sibling)) => move_container_within_tree(
      &window.clone().into(),
      &insertion_sibling.clone().parent().context("No parent.")?,
      insertion_sibling.index() + 1,
      state,
    ),
    _ => move_container_within_tree(
      &window.clone().into(),
      &target_workspace.clone().into(),
      target_workspace.child_count(),
      state,
    ),
  }
}

#[cfg(test)]
mod tests {
  use wm_common::{ScrollConfig, TilingDirection, WorkspaceLayout};
  use wm_platform::Rect;

  use super::*;
  use crate::{
    commands::container::attach_container,
    models::{Monitor, SplitContainer, TilingWindow, Workspace},
    test_utils::{
      MOCK_MONITOR_HEIGHT, MOCK_MONITOR_WIDTH, MOCK_TASKBAR_HEIGHT,
    },
    traits::TilingSizeGetters,
    user_config::UserConfig,
    wm_state::WmState,
  };

  #[test]
  fn inserts_strip_item_after_column_with_focused_window() {
    // Source workspace with a single tiling window.
    let source_workspace = Workspace::mock().call();
    let window = TilingWindow::mock().call();

    attach_container(
      &window.clone().into(),
      &source_workspace.clone().into(),
      None,
    )
    .unwrap();
    window.set_tiling_size(0.75);

    // Target scrolling workspace with a window and a column of two
    // windows.
    let target_workspace = Workspace::mock()
      .name("2".to_string())
      .tiling_direction(TilingDirection::Horizontal)
      .layout(WorkspaceLayout::Scrolling)
      .scroll(ScrollConfig::default())
      .call();

    let first = TilingWindow::mock().call();
    let top = TilingWindow::mock().call();
    let bottom = TilingWindow::mock().call();

    let column = SplitContainer::mock()
      .tiling_direction(TilingDirection::Vertical)
      .tiling_containers(vec![top.clone().into(), bottom.clone().into()])
      .call();

    attach_container(
      &first.clone().into(),
      &target_workspace.clone().into(),
      None,
    )
    .unwrap();
    attach_container(
      &column.clone().into(),
      &target_workspace.clone().into(),
      None,
    )
    .unwrap();
    first.set_tiling_size(0.5);
    column.set_tiling_size(0.5);

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

    // Focus the window inside the column of the target workspace.
    set_focused_descendant(&bottom.clone().into(), None);

    let config = UserConfig::mock();

    move_window_to_workspace(
      window.clone().into(),
      WorkspaceTarget::Name("2".to_string()),
      &mut state,
      &config,
    )
    .unwrap();

    // The window becomes a strip item directly after the column that
    // contains the focused window.
    let strip_item_ids = target_workspace
      .tiling_children()
      .map(|child| child.id())
      .collect::<Vec<_>>();

    assert_eq!(strip_item_ids, vec![first.id(), column.id(), window.id()]);

    // The window's viewport fraction is preserved.
    assert!((window.tiling_size() - 0.75).abs() < f32::EPSILON);
  }
}
