//! Helpers for animating window frames.
//!
//! The helpers are intended to be used from a dedicated thread that
//! applies interpolated frames, while the main thread keeps processing
//! events.

#[cfg(target_os = "windows")]
use crate::{platform_impl, Rect};

/// Sets the frame of the window with the given handle.
///
/// Unlike [`NativeWindow::set_frame`], this doesn't block on the target
/// window's thread and omits `SWP_FRAMECHANGED`, since intermediate
/// animation frames shouldn't trigger a non-client frame recalculation.
/// No-op if the window no longer exists.
///
/// # Platform-specific
///
/// This function is only available on Windows.
#[cfg(target_os = "windows")]
pub fn set_window_frame_async(handle: isize, rect: &Rect) {
  platform_impl::set_window_frame_async(handle, rect);
}

/// Gets the current frame of the window with the given handle.
///
/// The returned rect is in the same coordinate space that
/// [`set_window_frame_async`] expects.
///
/// # Platform-specific
///
/// This function is only available on Windows.
#[cfg(target_os = "windows")]
#[must_use]
pub fn window_frame(handle: isize) -> Option<Rect> {
  platform_impl::window_frame(handle)
}

/// Blocks until the next composition frame of the desktop compositor.
///
/// Used to pace animations at the display refresh rate.
///
/// # Platform-specific
///
/// This function is only available on Windows.
#[cfg(target_os = "windows")]
pub fn dwm_flush() {
  platform_impl::dwm_flush();
}
