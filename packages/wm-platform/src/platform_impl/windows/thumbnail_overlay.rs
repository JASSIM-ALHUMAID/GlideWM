//! Windows implementation of the thumbnail overlay.
//!
//! See [`crate::thumbnail_overlay`](crate::thumbnail_overlay) for the
//! public API.

use std::{marker::PhantomData, sync::Once};

use windows::{
  core::{w, PCWSTR},
  Win32::{
    Foundation::{BOOL, HWND, LPARAM, LRESULT, RECT, WPARAM},
    Graphics::Dwm::{
      DwmFlush, DwmQueryThumbnailSourceSize, DwmRegisterThumbnail,
      DwmUnregisterThumbnail, DwmUpdateThumbnailProperties,
      DWM_THUMBNAIL_PROPERTIES, DWM_TNP_OPACITY, DWM_TNP_RECTDESTINATION,
      DWM_TNP_RECTSOURCE, DWM_TNP_SOURCECLIENTAREAONLY, DWM_TNP_VISIBLE,
    },
    System::LibraryLoader::GetModuleHandleW,
    UI::WindowsAndMessaging::{
      CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW,
      GetSystemMetrics, GetWindowRect, PeekMessageW, RegisterClassW,
      SetWindowPos, ShowWindow, TranslateMessage, HTTRANSPARENT,
      HWND_TOPMOST, MSG, PM_REMOVE, SM_CXVIRTUALSCREEN,
      SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
      SWP_NOACTIVATE, SW_SHOWNOACTIVATE, WM_NCHITTEST, WNDCLASSW,
      WS_EX_NOACTIVATE, WS_EX_NOREDIRECTIONBITMAP, WS_EX_TOOLWINDOW,
      WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
    },
  },
};

use super::NativeWindow;
use crate::{Rect, ThumbnailFrame, ThumbnailId};

/// Class name of the overlay window.
const OVERLAY_CLASS_NAME: PCWSTR = w!("GlideWMThumbnailOverlay");

/// Implements [`ThumbnailOverlay`](crate::ThumbnailOverlay).
pub(crate) struct ThumbnailOverlay {
  handle: HWND,

  /// Top-left corner of the virtual screen, which the overlay's client
  /// area starts at. Thumbnail destinations are relative to it.
  origin: (i32, i32),

  /// The overlay window is bound to the thread that created it.
  _not_send: PhantomData<*const ()>,
}

impl ThumbnailOverlay {
  /// Implements [`ThumbnailOverlay::new`](crate::ThumbnailOverlay::new).
  pub(crate) fn new() -> crate::Result<Self> {
    register_overlay_class()?;

    let bounds = virtual_screen_bounds();

    // SAFETY: The window class is registered above, and all pointer
    // arguments are either null or point to static strings.
    let handle = unsafe {
      CreateWindowExW(
        WS_EX_NOREDIRECTIONBITMAP
          | WS_EX_TRANSPARENT
          | WS_EX_TOPMOST
          | WS_EX_TOOLWINDOW
          | WS_EX_NOACTIVATE,
        OVERLAY_CLASS_NAME,
        PCWSTR::null(),
        WS_POPUP,
        bounds.x(),
        bounds.y(),
        bounds.width(),
        bounds.height(),
        None,
        None,
        GetModuleHandleW(None)?,
        None,
      )
    };

    if handle.0 == 0 {
      return Err(crate::Error::Platform(
        "Failed to create thumbnail overlay window.".to_string(),
      ));
    }

    // SAFETY: `handle` is a valid window that was created above.
    unsafe { ShowWindow(handle, SW_SHOWNOACTIVATE) };

    Ok(Self {
      handle,
      origin: (bounds.x(), bounds.y()),
      _not_send: PhantomData,
    })
  }

  /// Implements
  /// [`ThumbnailOverlay::sync_bounds`](crate::ThumbnailOverlay::sync_bounds).
  pub(crate) fn sync_bounds(&mut self) -> crate::Result<()> {
    let bounds = virtual_screen_bounds();

    // SAFETY: `self.handle` is a valid window owned by this thread.
    unsafe {
      SetWindowPos(
        self.handle,
        HWND_TOPMOST,
        bounds.x(),
        bounds.y(),
        bounds.width(),
        bounds.height(),
        SWP_NOACTIVATE,
      )
    }?;

    self.origin = (bounds.x(), bounds.y());

    Ok(())
  }

  /// Implements
  /// [`ThumbnailOverlay::register`](crate::ThumbnailOverlay::register).
  pub(crate) fn register(
    &mut self,
    source: isize,
  ) -> crate::Result<ThumbnailId> {
    // SAFETY: Both handles are window handles; DWM validates them and
    // returns an error for invalid ones.
    let id = unsafe { DwmRegisterThumbnail(self.handle, HWND(source)) }?;

    Ok(ThumbnailId(id))
  }

  /// Implements
  /// [`ThumbnailOverlay::update`](crate::ThumbnailOverlay::update).
  pub(crate) fn update(
    &self,
    id: ThumbnailId,
    frame: &ThumbnailFrame,
  ) -> crate::Result<()> {
    let (origin_x, origin_y) = self.origin;

    let destination = RECT {
      left: frame.destination.left - origin_x,
      top: frame.destination.top - origin_y,
      right: frame.destination.right - origin_x,
      bottom: frame.destination.bottom - origin_y,
    };

    let source = RECT {
      left: frame.source.left,
      top: frame.source.top,
      right: frame.source.right,
      bottom: frame.source.bottom,
    };

    let properties = DWM_THUMBNAIL_PROPERTIES {
      dwFlags: DWM_TNP_RECTDESTINATION
        | DWM_TNP_RECTSOURCE
        | DWM_TNP_OPACITY
        | DWM_TNP_VISIBLE
        | DWM_TNP_SOURCECLIENTAREAONLY,
      rcDestination: destination,
      rcSource: source,
      opacity: frame.opacity,
      fVisible: BOOL::from(frame.is_visible),
      fSourceClientAreaOnly: BOOL::from(false),
    };

    // SAFETY: `properties` is a valid, fully initialized struct that
    // outlives the call.
    unsafe { DwmUpdateThumbnailProperties(id.0, &raw const properties) }?;

    Ok(())
  }

  /// Implements
  /// [`ThumbnailOverlay::source_size`](crate::ThumbnailOverlay::source_size).
  // LINT: Thumbnails belong to the overlay, so the method is scoped to
  // it even though DWM identifies thumbnails by ID alone.
  #[allow(clippy::unused_self)]
  pub(crate) fn source_size(
    &self,
    id: ThumbnailId,
  ) -> crate::Result<(i32, i32)> {
    // SAFETY: DWM validates the thumbnail ID and returns an error for
    // unknown ones.
    let size = unsafe { DwmQueryThumbnailSourceSize(id.0) }?;

    Ok((size.cx, size.cy))
  }

  /// Implements
  /// [`ThumbnailOverlay::unregister`](crate::ThumbnailOverlay::unregister).
  // LINT: Thumbnails belong to the overlay, so the method is scoped to
  // it even though DWM identifies thumbnails by ID alone.
  #[allow(clippy::unused_self)]
  pub(crate) fn unregister(
    &mut self,
    id: ThumbnailId,
  ) -> crate::Result<()> {
    // SAFETY: DWM validates the thumbnail ID and returns an error for
    // unknown ones.
    unsafe { DwmUnregisterThumbnail(id.0) }?;

    Ok(())
  }

  /// Implements
  /// [`ThumbnailOverlay::pump_messages`](crate::ThumbnailOverlay::pump_messages).
  // LINT: Messages are processed for the overlay's thread, which the
  // overlay is bound to.
  #[allow(clippy::unused_self)]
  pub(crate) fn pump_messages(&self) {
    let mut msg = MSG::default();

    // SAFETY: `msg` is a valid, writable `MSG` for every call.
    unsafe {
      while PeekMessageW(&raw mut msg, None, 0, 0, PM_REMOVE).as_bool() {
        let _ = TranslateMessage(&raw const msg);
        DispatchMessageW(&raw const msg);
      }
    }
  }
}

impl Drop for ThumbnailOverlay {
  fn drop(&mut self) {
    // SAFETY: `self.handle` is a valid window owned by this thread (the
    // type is `!Send`).
    if let Err(err) = unsafe { DestroyWindow(self.handle) } {
      tracing::warn!("Failed to destroy thumbnail overlay: {err}");
    }
  }
}

/// Implements [`dwm_flush`](crate::dwm_flush).
pub(crate) fn dwm_flush() {
  // SAFETY: `DwmFlush` takes no arguments. The result only reports
  // whether the compositor had a frame to flush.
  let _ = unsafe { DwmFlush() };
}

/// Implements [`window_frame`](crate::window_frame).
pub(crate) fn window_frame(handle: isize) -> Option<Rect> {
  let mut rect = RECT::default();

  // SAFETY: `rect` is a valid, writable `RECT`. Invalid handles make the
  // call fail instead of reading or writing out of bounds.
  let is_read =
    unsafe { GetWindowRect(HWND(handle), &raw mut rect) }.is_ok();

  is_read
    .then(|| Rect::from_ltrb(rect.left, rect.top, rect.right, rect.bottom))
}

/// Implements [`is_window_cloaked`](crate::is_window_cloaked).
pub(crate) fn is_window_cloaked(handle: isize) -> bool {
  NativeWindow::new(handle).is_cloaked().unwrap_or(false)
}

/// Bounds of the virtual screen (the bounding box of all monitors).
fn virtual_screen_bounds() -> Rect {
  // SAFETY: `GetSystemMetrics` has no preconditions.
  unsafe {
    Rect::from_xy(
      GetSystemMetrics(SM_XVIRTUALSCREEN),
      GetSystemMetrics(SM_YVIRTUALSCREEN),
      GetSystemMetrics(SM_CXVIRTUALSCREEN),
      GetSystemMetrics(SM_CYVIRTUALSCREEN),
    )
  }
}

/// Registers the overlay's window class once per process.
fn register_overlay_class() -> crate::Result<()> {
  static REGISTER: Once = Once::new();

  let instance = unsafe { GetModuleHandleW(None) }?;

  REGISTER.call_once(|| {
    let class = WNDCLASSW {
      lpszClassName: OVERLAY_CLASS_NAME,
      lpfnWndProc: Some(overlay_window_proc),
      hInstance: instance.into(),
      ..Default::default()
    };

    // SAFETY: `class` is a valid, fully initialized struct whose class
    // name points to a static string.
    if unsafe { RegisterClassW(&raw const class) } == 0 {
      tracing::warn!("Failed to register thumbnail overlay window class.");
    }
  });

  Ok(())
}

/// Window procedure of the overlay window.
///
/// The overlay is transparent to hit tests, so that clicks during an
/// animation reach the windows below it.
unsafe extern "system" fn overlay_window_proc(
  handle: HWND,
  message: u32,
  wparam: WPARAM,
  lparam: LPARAM,
) -> LRESULT {
  if message == WM_NCHITTEST {
    #[allow(clippy::cast_possible_wrap)]
    return LRESULT(HTTRANSPARENT as isize);
  }

  DefWindowProcW(handle, message, wparam, lparam)
}
