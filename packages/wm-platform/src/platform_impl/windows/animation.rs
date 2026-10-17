//! Windows implementation of the window frame animation helpers.
//!
//! See [`crate::animation`](crate::animation) for the public API.

use windows::Win32::{
  Foundation::{HWND, RECT},
  Graphics::Dwm::DwmFlush,
  UI::WindowsAndMessaging::{
    GetWindowRect, IsWindow, SetWindowPos, HWND_NOTOPMOST,
    SWP_ASYNCWINDOWPOS, SWP_NOACTIVATE, SWP_NOCOPYBITS,
    SWP_NOSENDCHANGING, SWP_NOZORDER,
  },
};

use crate::Rect;

/// Implements [`set_window_frame_async`](crate::animation::set_window_frame_async).
pub fn set_window_frame_async(handle: isize, rect: &Rect) {
  if !is_valid_handle(handle) {
    return;
  }

  let _ = unsafe {
    SetWindowPos(
      HWND(handle),
      HWND_NOTOPMOST,
      rect.x(),
      rect.y(),
      rect.width(),
      rect.height(),
      SWP_NOACTIVATE
        | SWP_NOZORDER
        | SWP_NOCOPYBITS
        | SWP_NOSENDCHANGING
        | SWP_ASYNCWINDOWPOS,
    )
  };
}

/// Implements [`window_frame`](crate::animation::window_frame).
pub fn window_frame(handle: isize) -> Option<Rect> {
  let mut rect = RECT::default();

  let is_read = unsafe {
    GetWindowRect(HWND(handle), std::ptr::from_mut(&mut rect).cast())
  }
  .is_ok();

  is_read.then(|| {
    Rect::from_ltrb(rect.left, rect.top, rect.right, rect.bottom)
  })
}

/// Implements [`dwm_flush`](crate::animation::dwm_flush).
pub fn dwm_flush() {
  // The result only reports whether the compositor had a frame to
  // flush; animations are paced by wall-clock time regardless.
  let _ = unsafe { DwmFlush() };
}

fn is_valid_handle(handle: isize) -> bool {
  unsafe { IsWindow(HWND(handle)) }.as_bool()
}
