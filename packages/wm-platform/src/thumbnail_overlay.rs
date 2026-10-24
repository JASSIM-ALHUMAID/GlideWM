//! Overlay for drawing live thumbnails of windows.
//!
//! Used to animate windows without moving them: the window is hidden
//! (cloaked) and moved to its final position in one step, while a live
//! thumbnail of it is animated on the overlay. Every frame of the
//! animation is a single compositor call, independent of how fast the
//! window's application processes position updates.

#[cfg(target_os = "windows")]
use crate::platform_impl;
use crate::Rect;

/// ID of a thumbnail registered with a [`ThumbnailOverlay`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ThumbnailId(pub(crate) isize);

/// How a thumbnail is drawn on the overlay.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThumbnailFrame {
  /// Where the thumbnail is drawn, in screen coordinates.
  pub destination: Rect,

  /// Part of the source window that's drawn, relative to the window's
  /// top-left corner, in the coordinate space of [`window_frame`] (i.e.
  /// including invisible borders).
  pub source: Rect,

  /// Opacity of the thumbnail, from transparent (`0`) to opaque (`255`).
  pub opacity: u8,

  /// Whether the thumbnail is drawn at all.
  pub is_visible: bool,
}

/// A transparent, click-through window spanning all monitors that live
/// window thumbnails are drawn on.
///
/// The overlay is bound to the thread that created it and must be
/// dropped on that thread. Messages for the overlay are only processed
/// in [`ThumbnailOverlay::pump_messages`], so the overlay should only be
/// kept alive while it's in use.
///
/// # Platform-specific
///
/// This type is only available on Windows. It relies on DWM thumbnails,
/// which draw any top-level window (including cloaked ones) without
/// involving the window's application.
#[cfg(target_os = "windows")]
pub struct ThumbnailOverlay {
  inner: platform_impl::ThumbnailOverlay,
}

#[cfg(target_os = "windows")]
impl ThumbnailOverlay {
  /// Creates the overlay window and shows it above all other windows.
  ///
  /// The overlay doesn't draw anything until thumbnails are registered.
  pub fn new() -> crate::Result<Self> {
    Ok(Self {
      inner: platform_impl::ThumbnailOverlay::new()?,
    })
  }

  /// Resizes the overlay to the current bounds of the virtual screen.
  ///
  /// Must be called when the monitor setup changes.
  pub fn sync_bounds(&mut self) -> crate::Result<()> {
    self.inner.sync_bounds()
  }

  /// Registers a live thumbnail of the window with the given handle.
  ///
  /// The thumbnail isn't drawn until it's positioned with
  /// [`ThumbnailOverlay::update`]. Thumbnails are stacked in the order
  /// that they're registered (the last registered one is on top).
  pub fn register(&mut self, source: isize) -> crate::Result<ThumbnailId> {
    self.inner.register(source)
  }

  /// Positions a registered thumbnail. Takes effect on the next
  /// composition frame.
  pub fn update(
    &self,
    id: ThumbnailId,
    frame: &ThumbnailFrame,
  ) -> crate::Result<()> {
    self.inner.update(id, frame)
  }

  /// Removes a registered thumbnail. Takes effect on the next
  /// composition frame.
  pub fn unregister(&mut self, id: ThumbnailId) -> crate::Result<()> {
    self.inner.unregister(id)
  }

  /// Processes pending messages of the calling thread (including those
  /// for the overlay window).
  ///
  /// Must be called regularly while the overlay is alive, so that the
  /// overlay doesn't stall broadcasts that are sent to all windows.
  pub fn pump_messages(&self) {
    self.inner.pump_messages();
  }
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

/// Gets the current frame of the window with the given handle, in the
/// coordinate space that window positions are set in.
///
/// Returns `None` if the window no longer exists.
///
/// # Platform-specific
///
/// This function is only available on Windows.
#[cfg(target_os = "windows")]
#[must_use]
pub fn window_frame(handle: isize) -> Option<Rect> {
  platform_impl::window_frame(handle)
}

/// Whether the window with the given handle is cloaked (i.e. hidden by
/// the compositor while remaining logically visible).
///
/// Returns `false` if the window no longer exists.
///
/// # Platform-specific
///
/// This function is only available on Windows.
#[cfg(target_os = "windows")]
#[must_use]
pub fn is_window_cloaked(handle: isize) -> bool {
  platform_impl::is_window_cloaked(handle)
}
