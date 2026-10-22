use anyhow::Context;
use tracing::{info, warn};
use wm_common::WindowState;

use crate::{
  commands::container::{move_container_within_tree, replace_container},
  layouts,
  models::{Container, InsertionTarget, WindowContainer},
  traits::{CommonGetters, TilingSizeGetters, WindowGetters},
  user_config::UserConfig,
  wm_state::WmState,
};

/// Updates the state of a window.
///
/// Adds the window for redraw if there is a state change.
///
/// Returns the window after the state change.
pub fn update_window_state(
  window: WindowContainer,
  target_state: WindowState,
  state: &mut WmState,
  config: &UserConfig,
) -> anyhow::Result<WindowContainer> {
  if window.state() == target_state {
    return Ok(window);
  }

  info!("Updating window state: {:?}.", target_state);

  match target_state {
    WindowState::Tiling => set_tiling(&window, state, config),
    _ => set_non_tiling(window, target_state, state),
  }
}

/// Updates the state of a window to be `WindowState::Tiling`.
fn set_tiling(
  window: &WindowContainer,
  state: &mut WmState,
  config: &UserConfig,
) -> anyhow::Result<WindowContainer> {
  let window = window
    .as_non_tiling_window()
    .context("Invalid window state.")?
    .clone();

  let workspace =
    window.workspace().context("Window has no workspace.")?;

  // Check whether insertion target is still valid.
  let insertion_target =
    window.insertion_target().filter(|insertion_target| {
      insertion_target
        .target_parent
        .workspace()
        .is_some_and(|workspace| workspace.is_displayed())
    });

  // Get the position in the tree to insert the new tiling window. This
  // will be the window's previous tiling position if it has one, or
  // instead beside the last focused tiling window in the workspace.
  let (target_parent, target_index) = insertion_target
    .as_ref()
    .map(|insertion_target| {
      (
        insertion_target.target_parent.clone(),
        insertion_target.target_index,
      )
    })
    // Fallback to the last focused tiling window within the workspace.
    .or_else(|| {
      let focused_window = workspace
        .descendant_focus_order()
        .find(Container::is_tiling_window)?;

      Some((focused_window.parent()?, focused_window.index() + 1))
    })
    // Default to inserting at the end of the workspace.
    .unwrap_or((workspace.clone().into(), workspace.child_count()));

  let tiling_window = window.to_tiling(config.value.gaps.clone());

  // Replace the original window with the created tiling window.
  replace_container(
    &tiling_window.clone().into(),
    &window.parent().context("No parent.")?,
    window.index(),
  )?;

  move_container_within_tree(
    &tiling_window.clone().into(),
    &target_parent,
    target_index,
    state,
  )?;

  #[allow(clippy::cast_precision_loss)]
  if let Some(insertion_target) = &insertion_target {
    let size_scale = (insertion_target.prev_sibling_count + 1) as f32
      / (tiling_window.tiling_siblings().count() + 1) as f32;

    // Scale the window's previous size based on the current number of
    // siblings. E.g. if the window was 0.5 with 1 sibling, and now has 2
    // siblings, scale to 0.5 * (2/3) to maintain proportional sizing.
    let target_size = insertion_target.prev_tiling_size * size_scale;
    layouts::resize_tiling_container(
      &tiling_window.clone().into(),
      target_size,
    );
  }

  // While the window was non-tiling (e.g. fullscreen), the strip of a
  // scrolling workspace was shorter, which may have clamped the scroll
  // offset. The rescroll is deferred to the sync, so that it applies to
  // the final tree (e.g. after a drag-drop moves the window again).
  let target_workspace = tiling_window
    .workspace()
    .context("Window has no workspace.")?;

  state
    .pending_sync
    .queue_containers_to_redraw(target_parent.tiling_children())
    .queue_workspace_to_rescroll(target_workspace)
    .queue_workspace_to_reorder(workspace);

  Ok(tiling_window.into())
}

/// Updates the state of a window to be either `WindowState::Floating`,
/// `WindowState::Fullscreen`, or `WindowState::Minimized`.
fn set_non_tiling(
  window: WindowContainer,
  target_state: WindowState,
  state: &mut WmState,
) -> anyhow::Result<WindowContainer> {
  // A window can only be updated to a minimized state if it is
  // natively minimized.
  // TODO: Consider doing the same for maximized and fullscreen states.
  if target_state == WindowState::Minimized
    && !window.native_properties().is_minimized
  {
    info!("No window state update. Minimizing window.");

    // TODO: Instead of doing the platform call directly here, instead add
    // a `queue_state_change` method to `PendingSync`.
    if let Err(err) = window.native().minimize() {
      warn!("Failed to minimize window: {}", err);
    }

    return Ok(window);
  }

  let workspace = window.workspace().context("No workspace.")?;

  match window {
    WindowContainer::NonTilingWindow(window) => {
      let current_state = window.state();

      // Update the window's previous state if the discriminant changes.
      // TODO: Move out handling of active drag. Can then simplify calls to
      // `set_active_drag` in `handle_window_moved_or_resized_end`.
      if !current_state.is_same_state(&target_state)
        && window.active_drag().is_none()
      {
        window.set_prev_state(current_state);
        state.pending_sync.queue_workspace_to_reorder(workspace);
      }

      window.set_state(target_state);
      state.pending_sync.queue_container_to_redraw(window.clone());

      Ok(window.into())
    }
    WindowContainer::TilingWindow(window) => {
      let parent = window.parent().context("No parent")?;

      let non_tiling_window = window.to_non_tiling(
        target_state.clone(),
        Some(InsertionTarget {
          target_parent: parent.clone(),
          target_index: window.index(),
          prev_tiling_size: window.tiling_size(),
          prev_sibling_count: window.tiling_siblings().count(),
        }),
      );

      // Non-tiling windows should always be direct children of the
      // workspace.
      if parent != workspace.clone().into() {
        move_container_within_tree(
          &window.clone().into(),
          &workspace.clone().into(),
          workspace.child_count(),
          state,
        )?;
      }

      replace_container(
        &non_tiling_window.clone().into(),
        &workspace.clone().into(),
        window.index(),
      )?;

      state
        .pending_sync
        .queue_container_to_redraw(non_tiling_window.clone())
        .queue_containers_to_redraw(workspace.tiling_children())
        .queue_workspace_to_rescroll(workspace.clone())
        .queue_workspace_to_reorder(workspace);

      Ok(non_tiling_window.into())
    }
  }
}

#[cfg(test)]
mod tests {
  use wm_common::{
    FullscreenStateConfig, GapsConfig, ScrollConfig, TilingDirection,
  };

  use super::*;
  use crate::{
    commands::{
      container::{
        attach_container, move_container_within_tree,
        set_focused_descendant,
      },
      workspace::update_scroll_position,
    },
    models::{TilingWindow, Workspace},
    test_utils::mock_scrolling_workspace,
  };

  /// Creates a displayed scrolling workspace with three strip items, each
  /// half of the viewport, and focuses the window at `focused_index`.
  fn setup(
    axis: TilingDirection,
    focused_index: usize,
  ) -> (WmState, Workspace, Vec<TilingWindow>) {
    let (workspace, windows) = mock_scrolling_workspace(
      axis,
      3,
      0.5,
      &GapsConfig::default(),
      ScrollConfig::default(),
    );

    let state = WmState::mock();
    let monitor = workspace.monitor().unwrap();
    attach_container(
      &monitor.into(),
      &state.root_container.clone().into(),
      None,
    )
    .unwrap();

    set_focused_descendant(&windows[focused_index].clone().into(), None);

    (state, workspace, windows)
  }

  /// Runs the scroll step of a platform sync and clears the pending sync,
  /// like `platform_sync` does.
  fn sync(focused_container: &Container, state: &mut WmState) {
    update_scroll_position(focused_container, state).unwrap();
    state.pending_sync.clear();
  }

  /// Fullscreens the window, syncs, and returns it to tiling. The strip
  /// shrinks while the window is fullscreen, which clamps the offset.
  fn fullscreen_and_exit(
    window: &TilingWindow,
    focused_container: &Container,
    state: &mut WmState,
  ) -> WindowContainer {
    let config = UserConfig::mock();

    let fullscreen = update_window_state(
      window.clone().into(),
      WindowState::Fullscreen(FullscreenStateConfig::default()),
      state,
      &config,
    )
    .unwrap();

    sync(focused_container, state);

    update_window_state(fullscreen, WindowState::Tiling, state, &config)
      .unwrap()
  }

  #[test]
  fn restores_scroll_position_after_fullscreen_exit() {
    for axis in [TilingDirection::Horizontal, TilingDirection::Vertical] {
      let (mut state, workspace, windows) = setup(axis, 2);

      workspace.set_scroll_offset(
        layouts::scrolling::max_scroll_offset(&workspace).unwrap(),
      );
      assert!(workspace.scroll_offset() > 0);

      let focused = windows[2].clone().into();
      let window = fullscreen_and_exit(&windows[2], &focused, &mut state);
      assert_eq!(workspace.scroll_offset(), 0);

      sync(&window.clone().into(), &mut state);

      assert!(window.is_tiling_window());
      assert!(workspace.scroll_offset() > 0);
      assert_eq!(
        workspace.scroll_offset(),
        layouts::scrolling::max_scroll_offset(&workspace).unwrap()
      );
    }
  }

  #[test]
  fn keeps_focused_window_in_view_when_other_window_exits_fullscreen() {
    for axis in [TilingDirection::Horizontal, TilingDirection::Vertical] {
      let (mut state, workspace, windows) = setup(axis, 0);
      let focused = windows[0].clone().into();

      // An unfocused window exits fullscreen on its own (e.g. a video
      // ending), which must not scroll the focused window out of view.
      fullscreen_and_exit(&windows[2], &focused, &mut state);
      sync(&focused, &mut state);

      assert_eq!(workspace.scroll_offset(), 0);
    }
  }

  #[test]
  fn rescrolls_to_final_position_of_retiled_window() {
    for axis in [TilingDirection::Horizontal, TilingDirection::Vertical] {
      let (mut state, workspace, windows) = setup(axis, 2);

      workspace.set_scroll_offset(
        layouts::scrolling::max_scroll_offset(&workspace).unwrap(),
      );

      let focused = windows[2].clone().into();
      let window = fullscreen_and_exit(&windows[2], &focused, &mut state);

      // Moved again before the sync, like a drag-drop onto the start of
      // the strip.
      move_container_within_tree(
        &window.clone().into(),
        &workspace.clone().into(),
        0,
        &state,
      )
      .unwrap();

      sync(&window.clone().into(), &mut state);

      assert_eq!(window.index(), 0);
      assert_eq!(workspace.scroll_offset(), 0);
    }
  }
}
