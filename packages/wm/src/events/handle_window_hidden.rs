use tracing::info;
use wm_common::{DisplayState, HideMethod};
use wm_platform::NativeWindow;

use crate::{
  commands::window::unmanage_window,
  traits::{CommonGetters, WindowGetters},
  user_config::UserConfig,
  wm_state::WmState,
};

pub fn handle_window_hidden(
  native_window: &NativeWindow,
  state: &mut WmState,
  config: &UserConfig,
) -> anyhow::Result<()> {
  let found_window = state.window_from_native(native_window);

  if let Some(window) = found_window {
    info!("Window hidden: {window}");

    // Update the display state.
    if config.value.general.hide_method != HideMethod::PlaceInCorner
      && window.display_state() == DisplayState::Hiding
    {
      window.set_display_state(DisplayState::Hidden);
      return Ok(());
    }

    // A window that's waiting for its open animation is cloaked by the
    // window manager, so that it can't flash in place before the slide
    // parks it off-screen. The hidden event that the cloak produces is
    // not an external hide of the window (e.g. to the tray), so the
    // window must stay managed.
    let is_awaiting_reveal =
      state.animations.is_awaiting_reveal(window.id());

    // Unmanage the window if it's not in a display state transition. Also,
    // since window events are not 100% guaranteed to be in correct order,
    // we need to ignore events where the window is not actually hidden.
    if !is_awaiting_reveal
      && (config.value.general.hide_method == HideMethod::PlaceInCorner
        || window.display_state() == DisplayState::Shown)
      && !window.native().is_visible().unwrap_or(false)
    {
      unmanage_window(window, state)?;
    }
  }

  Ok(())
}
