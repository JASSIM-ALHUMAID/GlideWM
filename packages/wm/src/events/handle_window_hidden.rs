use tracing::info;
use wm_common::{DisplayState, HideMethod};
use wm_platform::NativeWindow;

use crate::{
  animation::is_cloaked_by_external_animation,
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

    // Animated windows are cloaked by the window manager while their
    // thumbnail animates in their place. The hidden event that the cloak
    // produces is not an external hide of the window (e.g. to the tray),
    // so the window must stay managed. Windows that are actually hidden
    // while they're animated lose their visible style. The same applies
    // to windows that an external tool cloaks for its own animation.
    let is_cloaked_by_animation =
      (state.animations.owns_cloak(window.id())
        && has_visible_style(&window.native()))
        || is_cloaked_by_external_animation(&window.native());

    // Unmanage the window if it's not in a display state transition. Also,
    // since window events are not 100% guaranteed to be in correct order,
    // we need to ignore events where the window is not actually hidden.
    if !is_cloaked_by_animation
      && (config.value.general.hide_method == HideMethod::PlaceInCorner
        || window.display_state() == DisplayState::Shown)
      && !window.native().is_visible().unwrap_or(false)
    {
      unmanage_window(window, state)?;
    }
  }

  Ok(())
}

/// Whether the window has the visible style, regardless of whether it's
/// cloaked.
#[cfg(target_os = "windows")]
fn has_visible_style(native_window: &NativeWindow) -> bool {
  use wm_platform::NativeWindowWindowsExt;

  native_window.has_visible_style()
}

/// Windows can't be cloaked on other platforms.
#[cfg(not(target_os = "windows"))]
fn has_visible_style(native_window: &NativeWindow) -> bool {
  native_window.is_visible().unwrap_or(false)
}
