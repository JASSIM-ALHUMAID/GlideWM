use tracing::info;
use wm_common::{DisplayState, HideMethod};
use wm_platform::NativeWindow;

use crate::{
  commands::window::manage_window, traits::WindowGetters,
  user_config::UserConfig, wm_state::WmState,
};

pub fn handle_window_shown(
  native_window: NativeWindow,
  state: &mut WmState,
  config: &mut UserConfig,
) -> anyhow::Result<()> {
  let found_window = state.window_from_native(&native_window);

  if let Some(window) = found_window {
    info!("Window shown: {window}");

    // A window that's hidden while its thumbnail animates in its place can
    // get shown by someone else (e.g. the shell uncloaks File Explorer
    // windows once they finish launching). It's hidden again until its
    // animation ends.
    if state.animations.reassert_cloak(&window) {
      return Ok(());
    }

    // Update display state if window is already managed.
    if config.value.general.hide_method != HideMethod::PlaceInCorner
      && window.display_state() == DisplayState::Showing
    {
      window.set_display_state(DisplayState::Shown);
    } else {
      state.pending_sync.queue_container_to_redraw(window);
    }
  } else if !state.ignored_windows.contains(&native_window) {
    // If the window is not managed and not explicitly ignored, manage it.
    manage_window(native_window, None, state, config)?;
  }

  Ok(())
}
