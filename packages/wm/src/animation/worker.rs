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

use super::{bezier::CubicBezier, AnimationUpdate};

/// How long the worker waits for a window to reach its parked frame
/// before starting the slide without confirmation.
///
/// A window that's starting up can take a long time to process position
/// updates (the app's thread is busy initializing), and the window is
/// cloaked while it waits, so the timeout is generous.
const PARK_CONFIRM_TIMEOUT: Duration = Duration::from_millis(1000);

/// How long the worker waits for the main thread to confirm that a
/// parked window was revealed before starting the slide without
/// confirmation.
///
/// The confirmation is sent right after the cloak is lifted, which
/// normally takes well under a frame. The timeout only bounds the
/// damage if the main thread is overloaded.
const REVEAL_CONFIRM_TIMEOUT: Duration = Duration::from_millis(500);

/// How long an open slide frame may stay unconfirmed before the worker
/// stops pacing on it.
///
/// Without a cutoff, frames queued on a stalled window (one whose thread
/// isn't processing position updates) would pile up and then play back
/// as a burst. Giving up pauses the tween clock by the stall time, so
/// the slide resumes from where it left off once the window catches up.
const FRAME_STALL_LIMIT: Duration = Duration::from_millis(250);

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

  /// Whether this is the open slide of a window that's shown for the
  /// first time. Open slides wait for the window to be parked before
  /// starting their tween, and are paced by the window's frame
  /// consumption (see `ActiveAnimation::advance`).
  pub is_window_open: bool,
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

  /// Confirms that a parked window was revealed, so that its open slide
  /// can start its tween.
  Revealed {
    /// ID of the window that was revealed.
    window_id: Uuid,

    /// Sequence number of the animation that requested the reveal.
    /// Confirmations for replaced animations are ignored.
    sequence: u64,
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
  duration: Duration,
  curve: CubicBezier,
  is_window_open: bool,
  phase: Phase,

  /// Frame that was last applied to the window, along with when it was
  /// applied. Open slides only apply their next frame once the window
  /// processed the previous one (see `advance`).
  awaiting: Option<(Rect, Instant)>,
}

/// Phase of an animation that the worker is applying.
#[derive(Clone, Copy, Debug)]
enum Phase {
  /// The park frame was applied, but the window may not have processed
  /// it yet. Position updates are queued to the window's thread, which
  /// is often busy while the app is starting up, so the window can sit
  /// in its old spot for a while after the park frame was applied. Only
  /// open slides are prepared this way: the tween clock doesn't start
  /// until the window is parked off-screen.
  Preparing {
    /// When the park frame was applied.
    applied_at: Instant,
  },

  /// The park frame landed and the window's reveal was requested from
  /// the main thread. The tween clock doesn't start until the reveal is
  /// confirmed (see `WorkerCommand::Revealed`), so that the slide is
  /// visible from its first frame.
  Revealing {
    /// When the reveal was requested.
    sent_at: Instant,
  },

  /// The tween is in flight.
  Running {
    /// When the tween clock started.
    started_at: Instant,
  },
}

/// Spawns the animation worker thread.
///
/// Returns `None` if the thread could not be spawned, in which case
/// window animations are unavailable.
pub fn spawn(
  update_tx: UnboundedSender<AnimationUpdate>,
) -> Option<UnboundedSender<WorkerCommand>> {
  let (command_tx, command_rx) = mpsc::unbounded_channel();

  std::thread::Builder::new()
    .name("animation-worker".to_owned())
    .spawn(move || run(command_rx, &update_tx))
    .map_err(|err| {
      tracing::error!("Failed to spawn animation worker: {err}");
    })
    .ok()?;

  Some(command_tx)
}

/// Runs the worker loop until every command sender is dropped.
fn run(
  mut command_rx: UnboundedReceiver<WorkerCommand>,
  update_tx: &UnboundedSender<AnimationUpdate>,
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

    animations.retain_mut(|animation| animation.advance(update_tx));

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
      // A retarget replaces the window's in-flight animation. Take it
      // out so that it can be inspected below.
      let replaced = animations
        .iter()
        .position(|animation| animation.window_id == request.window_id)
        .map(|index| animations.remove(index));

      // A retarget that arrives while an open slide is still waiting for
      // its park frame keeps the park frame and the wait: the window is
      // cloaked while it waits, so shifting the park would only move an
      // invisible window, and reusing the phase means the wait can't be
      // restarted by repeated retargets.
      let parked_retarget = replaced.as_ref().filter(|animation| {
        request.from.is_none()
          && animation.is_window_open
          && matches!(animation.phase, Phase::Preparing { .. })
      });

      let now = Instant::now();

      let (from, phase) = if let Some(parked) = parked_retarget {
        (parked.from.clone(), parked.phase)
      } else if let Some(from) = request.from {
        // The animation starts from a frame that the main thread
        // computed. Open slides park the window off-screen and wait for
        // the park frame to land, so that the slide isn't half over by
        // the time the window is actually off-screen. Other animations
        // (e.g. a workspace switch slide) start tweening right away.
        let phase = if request.is_window_open {
          Phase::Preparing { applied_at: now }
        } else {
          Phase::Running { started_at: now }
        };

        (from, phase)
      } else {
        // A retarget that didn't arrive during the park wait continues
        // from the window's current frame, so that the slide isn't
        // interrupted.
        let from = get_frame(request.handle)
          .unwrap_or_else(|| request.to.clone());

        (from, Phase::Running { started_at: now })
      };

      tracing::debug!(
        "Animating window (id={}): {:?} -> {:?} over {:?}",
        request.window_id,
        from,
        request.to,
        request.duration
      );

      // Position the window at the start frame. For open slides this is
      // the park frame, which the window is visible at until the app's
      // thread processes the first position update.
      if matches!(phase, Phase::Preparing { .. }) {
        apply_frame(request.handle, &from);
      }

      animations.push(ActiveAnimation {
        window_id: request.window_id,
        sequence: request.sequence,
        handle: request.handle,
        from,
        to: request.to,
        duration: request.duration,
        curve: request.curve,
        is_window_open: request.is_window_open,
        phase,
        awaiting: None,
      });
    }
    WorkerCommand::Cancel { window_id } => {
      animations.retain(|animation| animation.window_id != window_id);
    }
    WorkerCommand::Revealed { window_id, sequence } => {
      let Some(animation) = animations
        .iter_mut()
        .find(|animation| animation.window_id == window_id)
      else {
        return;
      };

      // Confirmations for replaced animations are ignored.
      if animation.sequence != sequence {
        return;
      }

      if matches!(animation.phase, Phase::Revealing { .. }) {
        animation.phase = Phase::Running {
          started_at: Instant::now(),
        };
      }
    }
  }
}

impl ActiveAnimation {
  /// Applies the animation's frame for the current composition frame.
  ///
  /// Returns whether the animation is still in flight.
  fn advance(
    &mut self,
    update_tx: &UnboundedSender<AnimationUpdate>,
  ) -> bool {
    let mut started_at = match self.phase {
      Phase::Preparing { applied_at } => {
        // The window is parked once its frame matches `from` (the frame
        // it was parked at; off-screen for slides).
        let is_parked = frame_matches(self.handle, &self.from);

        if !is_parked {
          if applied_at.elapsed() < PARK_CONFIRM_TIMEOUT {
            return true;
          }

          tracing::debug!(
            "Window was not parked in time to start its slide: {}",
            self.window_id
          );
        }

        // The window is off-screen (or the wait was given up), so it can
        // be revealed. The tween clock doesn't start until the main
        // thread confirms the reveal, so that the slide isn't already
        // partway through by the time the window is actually visible.
        let _ = update_tx.send(AnimationUpdate::RevealCloakedWindow {
          window_id: self.window_id,
          sequence: self.sequence,
        });

        self.phase = Phase::Revealing {
          sent_at: Instant::now(),
        };

        return true;
      }
      Phase::Revealing { sent_at } => {
        if sent_at.elapsed() < REVEAL_CONFIRM_TIMEOUT {
          return true;
        }

        tracing::debug!(
          "Window reveal was not confirmed in time to start its slide: {}",
          self.window_id
        );

        let started_at = Instant::now();
        self.phase = Phase::Running { started_at };

        started_at
      }
      Phase::Running { started_at } => started_at,
    };

    // An open slide's next frame is only applied once the previous one
    // landed. An app that's busy starting up processes position updates
    // late, and queueing frames on its thread makes them play back as a
    // burst after the app is done starting up.
    if let Some((rect, applied_at)) = &self.awaiting {
      if !frame_matches(self.handle, rect) {
        if applied_at.elapsed() < FRAME_STALL_LIMIT {
          return true;
        }

        tracing::debug!(
          "Slide frame was not applied in time: {}",
          self.window_id
        );

        // Pause the tween clock by the stall time, so that the slide
        // resumes from where it left off instead of skipping ahead to
        // the end once the app works through its queue of frames.
        started_at += applied_at.elapsed();
        self.phase = Phase::Running { started_at };
      }

      self.awaiting = None;
    }

    let progress =
      started_at.elapsed().as_secs_f64() / self.duration.as_secs_f64();

    if progress >= 1.0 {
      apply_frame(self.handle, &self.to);

      let _ = update_tx.send(AnimationUpdate::Completed {
        window_id: self.window_id,
        sequence: self.sequence,
      });

      return false;
    }

    // Window slides are translations; an overshooting curve (meant for
    // scale-style animations) would read as the window bouncing off its
    // slot, so the eased value is clamped to the slide's endpoints.
    let eased = self.curve.ease(progress).clamp(0.0, 1.0);
    let rect = lerp_rect(&self.from, &self.to, eased);
    apply_frame(self.handle, &rect);

    if self.is_window_open {
      self.awaiting = Some((rect, Instant::now()));
    }

    true
  }
}

/// Whether the window's frame is currently at the given frame.
///
/// Position updates are queued to the window's thread, so a frame that
/// was applied to a busy thread doesn't show up in the window's frame
/// until the thread processes it. Each edge is allowed a small
/// tolerance, because apps can nudge the geometry they're given (e.g.
/// rounding sizes to DPI).
#[cfg(target_os = "windows")]
fn frame_matches(handle: isize, rect: &Rect) -> bool {
  /// Maximum distance an edge may be from the applied frame.
  const TOLERANCE_PX: i32 = 2;

  window_frame(handle).is_some_and(|frame| {
    (frame.left - rect.left).abs() <= TOLERANCE_PX
      && (frame.top - rect.top).abs() <= TOLERANCE_PX
      && (frame.right - rect.right).abs() <= TOLERANCE_PX
      && (frame.bottom - rect.bottom).abs() <= TOLERANCE_PX
  })
}

/// No-op on platforms that don't have animations.
#[cfg(not(target_os = "windows"))]
fn frame_matches(_handle: isize, _rect: &Rect) -> bool {
  true
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
