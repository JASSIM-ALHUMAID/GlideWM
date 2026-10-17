use anyhow::Context;
#[cfg(target_os = "macos")]
use wm_common::try_warn;
use wm_platform::{MouseButton, MouseEvent, Point};

use crate::{
  commands::container::set_focused_descendant,
  layouts,
  models::WindowContainer,
  traits::{CommonGetters, PositionGetters},
  user_config::UserConfig,
  wm_state::WmState,
};
#[cfg(target_os = "macos")]
use crate::{
  events::handle_window_moved_or_resized_end, traits::WindowGetters,
};

pub fn handle_mouse_move(
  event: &MouseEvent,
  state: &mut WmState,
  config: &UserConfig,
) -> anyhow::Result<()> {
  // Ignore mouse move events if the WM is paused. The mouse listener
  // should anyways be disabled when the WM is paused, but this is just in
  // case any events slipped through while disabling.
  if state.is_paused {
    return Ok(());
  }

  // On macOS, detect when a window drag operation has ended by listening
  // to the release of left click.
  //
  // This cannot be used for Windows, since it leads to race conditions
  // where the mouse event comes in before the `MovedOrResized` event with
  // `is_interactive_end`. For example, if the user drags to maximize a
  // window, the WS_MAXIMIZED state is sometimes set after the mouse event.
  #[cfg(target_os = "macos")]
  if let MouseEvent::ButtonUp { button, .. } = event {
    if *button == MouseButton::Left {
      let active_drag_windows = state
        .windows()
        .into_iter()
        .filter(|window| window.active_drag().is_some());

      // Only one window should ever be actively dragged at a time, but
      // just in case, iterate over all active drag windows.
      for window in active_drag_windows {
        let new_rect = try_warn!(window.native().frame());

        window.update_native_properties(|properties| {
          properties.frame = new_rect;
        });

        handle_window_moved_or_resized_end(&window, state, config)?;
      }
    }

    return Ok(());
  }

  if let MouseEvent::Move {
    pressed_buttons,
    // LINT: `window_below_cursor` is only used on macOS.
    #[cfg_attr(not(target_os = "macos"), allow(unused_variables))]
    window_below_cursor,
    position,
    ..
  } = event
  {
    // Ignore event if left/right-click is down. Otherwise, this causes
    // focus to jitter when a window is being resized by its drag
    // handles. Also ignore if the OS focused window isn't the same as
    // the WM's focused window.
    if pressed_buttons.contains(&MouseButton::Left)
      || pressed_buttons.contains(&MouseButton::Right)
      || !state.is_focus_synced
      || !config.value.general.focus_follows_cursor
    {
      return Ok(());
    }

    let window_under_cursor = {
      #[cfg(target_os = "macos")]
      {
        window_below_cursor.and_then(|window_id| {
          use crate::traits::WindowGetters;

          state
            .windows()
            .into_iter()
            .find(|w| w.native().id() == window_id)
        })
      }
      #[cfg(target_os = "windows")]
      {
        state
          .dispatcher
          .window_from_point(position)?
          .and_then(|native| state.window_from_native(&native))
      }
    };

    // Ignore windows that are hidden due to being scrolled out of view
    // in a scrolling workspace.
    let window_under_cursor = window_under_cursor
      .filter(|window| is_within_visible_rect(window, position));

    // Set focus to whichever window is currently under the cursor.
    if let Some(window) = window_under_cursor {
      let focused_container =
        state.focused_container().context("No focused container.")?;

      if focused_container.id() != window.id() {
        set_focused_descendant(&window.as_container(), None);
        state.pending_sync.queue_focus_change();
      }
    } else {
      // Focus the monitor if no window is under the cursor.
      let cursor_monitor = state
        .monitor_at_point(position)
        .context("No monitor under cursor.")?;

      let focused_monitor = state
        .focused_container()
        .context("No focused container.")?
        .monitor()
        .context("Focused container has no monitor.")?;

      // Avoid setting focus to the same monitor.
      if cursor_monitor.id() != focused_monitor.id() {
        set_focused_descendant(&cursor_monitor.as_container(), None);
        state.pending_sync.queue_focus_change();
      }
    }
  }

  Ok(())
}

/// Whether the given point is within the part of the window that is
/// visibly rendered.
///
/// Tiling windows of a scrolling workspace are hidden and clamped just
/// beyond the viewport edge when they are scrolled out of view. Hit
/// tests must ignore them, so that cursors over a hidden (clamped)
/// window don't set focus to it.
fn is_within_visible_rect(
  window: &WindowContainer,
  point: &Point,
) -> bool {
  // Only the part of a tiling window that is within its workspace's
  // viewport is visible.
  let is_in_viewport = match window {
    WindowContainer::TilingWindow(_) => window
      .workspace()
      .filter(layouts::clips_children_to_viewport)
      .and_then(|workspace| workspace.to_rect().ok())
      .is_none_or(|viewport_rect| viewport_rect.contains_point(point)),
    WindowContainer::NonTilingWindow(_) => true,
  };

  if !is_in_viewport {
    return false;
  }

  window
    .to_rect()
    .is_ok_and(|rect| rect.contains_point(point))
}
