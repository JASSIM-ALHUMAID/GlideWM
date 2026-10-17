//! Worker thread that applies animation frames.
//!
//! The worker owns all in-flight animations and repositions windows on
//! every composition frame, paced by the compositor's frame rate. Only
//! raw window handles are touched from this thread; all WM state remains
//! on the main thread.

// The worker is only spawned on Windows. On other platforms the module
// exists so that the command types compile.
#![cfg_attr(not(target_os = "windows"), allow(dead_code))]

use std::time::{Duration, Instant};

use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use uuid::Uuid;
use wm_platform::Rect;
#[cfg(target_os = "windows")]
use wm_platform::{dwm_flush, set_window_frame_async, window_frame};

use super::{bezier::CubicBezier, AnimationCompletion};

/// Request to start animating a window.
#[derive(Clone, Debug)]
pub struct StartRequest {
  /// ID of the window container that's animated.
  pub window_id: Uuid,

  /// Sequence number of the animation.
  pub sequence: u64,

  /// Handle of the native window to animate.
  pub handle: isize,

  /// Frame to start the animation from. If `None`, the window's current
  /// frame is captured when the animation starts.
  pub from: Option<Rect>,

  /// Frame to end the animation at.
  pub to: Rect,

  /// Duration of the animation.
  pub duration: Duration,

  /// Easing curve of the animation.
  pub curve: CubicBezier,
}

/// Command sent from the main thread to the animation worker.
#[derive(Clone, Debug)]
pub enum WorkerCommand {
  /// Starts animating a window. Replaces any animation that's in flight
  /// for the same window.
  Start(StartRequest),

  /// Stops the animation in flight for a window, if any.
  Cancel {
    /// ID of the window whose animation is cancelled.
    window_id: Uuid,
  },
}

/// An animation that the worker is applying.
#[derive(Debug)]
struct ActiveAnimation {
  window_id: Uuid,
  sequence: u64,
  handle: isize,
  from: Rect,
  to: Rect,
  started_at: Instant,
  duration: Duration,
  curve: CubicBezier,
}

/// Spawns the animation worker thread.
///
/// Returns `None` if the thread could not be spawned, in which case
/// window animations are unavailable.
pub fn spawn(
  completion_tx: UnboundedSender<AnimationCompletion>,
) -> Option<UnboundedSender<WorkerCommand>> {
  let (command_tx, command_rx) = mpsc::unbounded_channel();

  std::thread::Builder::new()
    .name("animation-worker".to_owned())
    .spawn(move || run(command_rx, &completion_tx))
    .map_err(|err| {
      tracing::error!("Failed to spawn animation worker: {err}");
    })
    .ok()?;

  Some(command_tx)
}

/// Runs the worker loop until every command sender is dropped.
fn run(
  mut command_rx: UnboundedReceiver<WorkerCommand>,
  completion_tx: &UnboundedSender<AnimationCompletion>,
) {
  let mut animations: Vec<ActiveAnimation> = Vec::new();

  loop {
    // Block until there's something to animate while idle.
    if animations.is_empty() {
      match command_rx.blocking_recv() {
        Some(command) => apply_command(command, &mut animations),
        None => return,
      }
    }

    // Apply pending commands to this frame, so that retargets and
    // cancellations take effect immediately.
    while let Ok(command) = command_rx.try_recv() {
      apply_command(command, &mut animations);
    }

    let now = Instant::now();

    animations.retain_mut(|animation| {
      let progress = (now - animation.started_at).as_secs_f64()
        / animation.duration.as_secs_f64();

      if progress >= 1.0 {
        apply_frame(animation.handle, &animation.to);

        let _ = completion_tx.send(AnimationCompletion {
          window_id: animation.window_id,
          sequence: animation.sequence,
        });

        return false;
      }

      let eased = animation.curve.ease(progress);
      apply_frame(
        animation.handle,
        &lerp_rect(&animation.from, &animation.to, eased),
      );

      true
    });

    // Block until the next composition frame, so that the animation is
    // paced at the display refresh rate.
    if !animations.is_empty() {
      flush_frames();
    }
  }
}

/// Applies a command to the in-flight animations.
fn apply_command(
  command: WorkerCommand,
  animations: &mut Vec<ActiveAnimation>,
) {
  match command {
    WorkerCommand::Start(request) => {
      // Retargeting a window restarts its animation from the window's
      // current frame, so that the retarget is seamless.
      animations
        .retain(|animation| animation.window_id != request.window_id);

      let from = request
        .from
        .or_else(|| get_frame(request.handle))
        .unwrap_or_else(|| request.to.clone());

      tracing::debug!(
        "Animating window (id={}): {:?} -> {:?} over {:?}",
        request.window_id,
        from,
        request.to,
        request.duration
      );

      animations.push(ActiveAnimation {
        window_id: request.window_id,
        sequence: request.sequence,
        handle: request.handle,
        from,
        to: request.to,
        started_at: Instant::now(),
        duration: request.duration,
        curve: request.curve,
      });
    }
    WorkerCommand::Cancel { window_id } => {
      animations.retain(|animation| animation.window_id != window_id);
    }
  }
}

/// Linearly interpolates between two frames.
fn lerp_rect(from: &Rect, to: &Rect, progress: f64) -> Rect {
  let lerp = |start: i32, end: i32| {
    let value =
      f64::from(start) + (f64::from(end) - f64::from(start)) * progress;

    // The interpolated values are within the range of monitor
    // coordinates, so the cast can't truncate.
    #[allow(clippy::cast_possible_truncation)]
    (value.round() as i32)
  };

  Rect::from_ltrb(
    lerp(from.left, to.left),
    lerp(from.top, to.top),
    lerp(from.right, to.right),
    lerp(from.bottom, to.bottom),
  )
}

/// Applies a frame to the window, without blocking on its thread.
#[cfg(target_os = "windows")]
fn apply_frame(handle: isize, rect: &Rect) {
  set_window_frame_async(handle, rect);
}

/// No-op on platforms that don't have animations.
#[cfg(not(target_os = "windows"))]
fn apply_frame(_handle: isize, _rect: &Rect) {}

/// Gets the current frame of a window.
#[cfg(target_os = "windows")]
fn get_frame(handle: isize) -> Option<Rect> {
  window_frame(handle)
}

/// No-op on platforms that don't have animations.
#[cfg(not(target_os = "windows"))]
fn get_frame(_handle: isize) -> Option<Rect> {
  None
}

/// Blocks until the next composition frame.
#[cfg(target_os = "windows")]
fn flush_frames() {
  dwm_flush();
}

/// No-op on platforms that don't have animations.
#[cfg(not(target_os = "windows"))]
fn flush_frames() {}
