//! Worker that draws animation frames.
//!
//! The worker owns the thumbnails of all animated windows and updates
//! them on every composition frame. Real windows are never moved by the
//! worker: the main thread hides (cloaks) an animated window and moves
//! it to its final position in one step, while the worker animates the
//! window's thumbnail. Only raw window handles are touched from the
//! worker thread; all WM state remains on the main thread.
//!
//! # Protocol
//!
//! Animations of windows that are visible when the animation starts
//! (`StartFrame::Cover`):
//!
//! 1. `Begin`: the thumbnail is drawn over the window at its current
//!    frame. Once that frame is composed, `Covered` is reported.
//! 2. The main thread hides the window, moves it to its final position,
//!    and sends `Release`. The tween starts.
//!
//! Animations of windows that are hidden when the animation starts
//! (`StartFrame::Hidden`): the main thread moves the hidden window to
//! its final position and sends `Begin`. The worker waits for the move
//! to land (so that the thumbnail shows the window at its final size),
//! then starts the tween.
//!
//! When the tween ends with the window visible, `Finished` is reported;
//! the main thread shows the window and sends `Revealed`. The worker
//! waits for the window to be shown on screen, then removes the
//! thumbnail and reports `Removed`. When the tween ends with the window
//! hidden, the thumbnail is removed right away.
//!
//! Animations that are begun in the same sync pass form a group (e.g. the
//! windows of a workspace switch, or the windows reflowing around a new
//! window). The tweens of a group start together once every animation of
//! the group is ready, so that the windows move in unison.
//!
//! Every wait has a timeout. When an animation can't continue (e.g. a
//! thumbnail can't be registered), `Aborted` is reported and the main
//! thread applies the window's final state without animating it.

use std::{
  collections::HashSet,
  time::{Duration, Instant},
};

use uuid::Uuid;
use wm_platform::{Rect, ThumbnailFrame, WindowId};

use super::{
  geometry::clip_thumbnail,
  planner::Target,
  timeline::{Frame, Tween},
  AnimationUpdate,
};

/// How long the worker waits for the main thread to hide a covered
/// window before giving up on the animation.
const RELEASE_TIMEOUT: Duration = Duration::from_millis(500);

/// How long the worker waits for a hidden window to reach its final
/// position before starting its animation regardless.
///
/// Windows of busy applications can take a while to process position
/// updates. The thumbnail shows the window wherever it currently is, so
/// starting the animation early only means that the window's content
/// visibly resizes mid-animation.
const SETTLE_TIMEOUT: Duration = Duration::from_millis(250);

/// How long the worker waits for the main thread to show a window whose
/// animation finished, before giving up on the animation.
const REVEAL_TIMEOUT: Duration = Duration::from_millis(500);

/// How long the worker waits for a shown window to be uncloaked by the
/// compositor before removing its thumbnail regardless.
const UNCLOAK_TIMEOUT: Duration = Duration::from_millis(250);

/// Maximum distance that an edge of a window may be from its expected
/// position for the window to count as positioned. Applications can
/// nudge the geometry they're given (e.g. rounding sizes to DPI).
const SETTLE_TOLERANCE_PX: i32 = 2;

/// Number of consecutive frames that a hidden window's frame must stay
/// unchanged to count as positioned, when it doesn't match the expected
/// position (e.g. an application that enforces a minimum size or snaps
/// its size to a grid).
const SETTLE_STABLE_FRAMES: u32 = 3;

/// How an animation starts.
#[derive(Clone, Debug)]
pub enum StartFrame {
  /// The window is visible: its thumbnail first covers it at its current
  /// frame.
  Cover,

  /// The window is hidden and being moved to `settle`: its thumbnail
  /// starts at `from` once the move landed.
  Hidden { from: Frame, settle: Rect },
}

/// Request to start animating a window.
#[derive(Clone, Debug)]
pub struct BeginRequest {
  /// ID of the window container that's animated.
  pub window_id: Uuid,

  /// Sequence number of the animation. Commands and updates for a
  /// different sequence number belong to a replaced animation.
  pub sequence: u64,

  /// Handle of the native window to animate.
  pub handle: WindowId,

  /// Group of the animation. The tweens of a group start together.
  pub group: u64,

  pub start: StartFrame,
  pub target: Target,
}

/// Command sent from the main thread to the animation worker.
#[derive(Clone, Debug)]
pub enum WorkerCommand {
  /// Starts animating a window. Replaces any animation that's in flight
  /// for the same window.
  Begin(BeginRequest),

  /// The covered window was hidden and moved to its final position, so
  /// its tween can start.
  Release { window_id: Uuid, sequence: u64 },

  /// Redirects the animation of a window to a new target, continuing
  /// from the window's current frame.
  Retarget {
    window_id: Uuid,
    sequence: u64,
    target: Target,

    /// Where the (hidden) window itself is being moved to.
    window_rect: Rect,
  },

  /// The window whose animation finished was shown.
  Revealed { window_id: Uuid, sequence: u64 },

  /// Stops the animation of a window and removes its thumbnail right
  /// away.
  Cancel { window_id: Uuid },

  /// The monitor setup changed.
  DisplaysChanged,
}

/// Compositor operations that the worker draws animations with.
///
/// Implemented with DWM thumbnails on Windows, and with a fake in
/// tests.
pub trait ThumbnailBackend {
  /// ID of a registered thumbnail.
  type Id: Copy + std::fmt::Debug;

  /// Registers a thumbnail of the window with the given handle.
  fn register(&mut self, handle: WindowId) -> anyhow::Result<Self::Id>;

  /// Positions a registered thumbnail.
  fn update(
    &mut self,
    id: Self::Id,
    frame: &ThumbnailFrame,
  ) -> anyhow::Result<()>;

  /// Size of the thumbnail's source window, as `(width, height)`.
  fn source_size(&self, id: Self::Id) -> Option<(i32, i32)>;

  /// Removes a registered thumbnail.
  fn unregister(&mut self, id: Self::Id);

  /// Current frame of the window with the given handle.
  fn window_frame(&self, handle: WindowId) -> Option<Rect>;

  /// Whether the window with the given handle is cloaked.
  fn is_cloaked(&self, handle: WindowId) -> bool;

  /// Blocks until the next composition frame.
  fn end_frame(&mut self);

  /// Releases resources that are only needed while animating. Called
  /// once no thumbnails are registered.
  fn release_resources(&mut self);

  /// Adapts to a change of the monitor setup.
  fn sync_displays(&mut self);
}

/// Phase of an animation that the worker is drawing.
#[derive(Clone, Debug)]
enum Phase {
  /// The thumbnail covers the visible window at `start`, waiting for the
  /// main thread to hide the window.
  Covering {
    start: Frame,

    /// Composition frame that the thumbnail was first drawn in.
    drawn_in_frame: u64,

    /// Whether `Covered` was reported.
    is_reported: bool,

    since: Instant,
  },

  /// Waiting for the hidden window to reach its final position.
  Settling {
    from: Frame,
    settle: Rect,
    since: Instant,

    /// Frame of the window as of the previous composition frame.
    last_frame: Option<Rect>,

    /// Number of consecutive frames that the window's frame didn't
    /// change.
    stable_frames: u32,
  },

  /// The thumbnail is drawn at `start`, waiting for the other animations
  /// of its group to be ready.
  Ready { start: Frame },

  /// The tween is in flight.
  Running { tween: Tween },

  /// The tween ended with the window visible; waiting for the main
  /// thread to show the window.
  AwaitingReveal { since: Instant },

  /// The window was shown; waiting for the compositor to uncloak it.
  AwaitingUncloak { since: Instant },

  /// The window is on screen; the thumbnail is removed once a
  /// composition frame after `frame` completed.
  Draining { frame: u64 },
}

/// An animation that the worker is drawing.
#[derive(Debug)]
struct Animation<Id> {
  window_id: Uuid,
  sequence: u64,
  handle: WindowId,
  group: u64,
  target: Target,
  thumbnail: Option<Id>,
  phase: Phase,
}

/// State of the animation worker.
pub struct Worker<B: ThumbnailBackend> {
  backend: B,
  animations: Vec<Animation<B::Id>>,

  /// Number of composition frames that were completed.
  frame: u64,
}

impl<B: ThumbnailBackend> Worker<B> {
  pub fn new(backend: B) -> Self {
    Self {
      backend,
      animations: Vec::new(),
      frame: 0,
    }
  }

  /// Whether no animations are in flight.
  pub fn is_idle(&self) -> bool {
    self.animations.is_empty()
  }

  /// Applies a command from the main thread.
  pub fn handle(
    &mut self,
    command: WorkerCommand,
    now: Instant,
    emit: &mut impl FnMut(AnimationUpdate),
  ) {
    match command {
      WorkerCommand::Begin(request) => self.begin(request, now, emit),
      WorkerCommand::Release {
        window_id,
        sequence,
      } => {
        if let Some(animation) = self.find(window_id, sequence) {
          if let Phase::Covering { start, .. } = &animation.phase {
            animation.phase = Phase::Ready {
              start: start.clone(),
            };
          }
        }
      }
      WorkerCommand::Retarget {
        window_id,
        sequence,
        target,
        window_rect,
      } => {
        if let Some(animation) = self.find(window_id, sequence) {
          match &mut animation.phase {
            Phase::Running { tween } => {
              tween.retarget(now, target.to.clone(), target.timing);
            }
            // A settling window waits for its new position instead.
            Phase::Settling { settle, .. } => *settle = window_rect,
            _ => {}
          }

          // Animations whose tween hasn't started pick up the new target
          // once it starts.
          animation.target = target;
        }
      }
      WorkerCommand::Revealed {
        window_id,
        sequence,
      } => {
        if let Some(animation) = self.find(window_id, sequence) {
          if matches!(animation.phase, Phase::AwaitingReveal { .. }) {
            animation.phase = Phase::AwaitingUncloak { since: now };
          }
        }
      }
      WorkerCommand::Cancel { window_id } => self.remove(window_id),
      WorkerCommand::DisplaysChanged => self.backend.sync_displays(),
    }
  }

  /// Draws the current frame of every animation, and reports animations
  /// that changed phase.
  pub fn tick(
    &mut self,
    now: Instant,
    emit: &mut impl FnMut(AnimationUpdate),
  ) {
    self.start_ready_groups(now);

    let mut index = 0;

    while index < self.animations.len() {
      if let Some(update) = self.advance(index, now, emit) {
        let animation = self.animations.remove(index);

        if let Some(thumbnail) = animation.thumbnail {
          self.backend.unregister(thumbnail);
        }

        emit(update);
      } else {
        index += 1;
      }
    }

    if self.is_idle() {
      self.backend.release_resources();
    }
  }

  /// Starts the tweens of the groups whose animations are all ready.
  fn start_ready_groups(&mut self, now: Instant) {
    let pending_groups = self
      .animations
      .iter()
      .filter(|animation| {
        matches!(
          animation.phase,
          Phase::Covering { .. } | Phase::Settling { .. }
        )
      })
      .map(|animation| animation.group)
      .collect::<HashSet<_>>();

    for animation in &mut self.animations {
      if pending_groups.contains(&animation.group) {
        continue;
      }

      if let Phase::Ready { start } = &animation.phase {
        tracing::debug!(
          "Starting window animation (id={}, group={}).",
          animation.window_id,
          animation.group
        );

        animation.phase = Phase::Running {
          tween: Tween::new(
            start.clone(),
            animation.target.to.clone(),
            now,
            animation.target.timing,
          ),
        };
      }
    }
  }

  /// Blocks until the next composition frame.
  pub fn end_frame(&mut self) {
    self.backend.end_frame();
    self.frame += 1;
  }

  /// Removes all animations and their thumbnails.
  pub fn clear(&mut self) {
    for animation in self.animations.drain(..) {
      if let Some(thumbnail) = animation.thumbnail {
        self.backend.unregister(thumbnail);
      }
    }

    self.backend.release_resources();
  }

  fn find(
    &mut self,
    window_id: Uuid,
    sequence: u64,
  ) -> Option<&mut Animation<B::Id>> {
    self.animations.iter_mut().find(|animation| {
      animation.window_id == window_id && animation.sequence == sequence
    })
  }

  /// Removes the animation of a window and its thumbnail, if any.
  fn remove(&mut self, window_id: Uuid) {
    let backend = &mut self.backend;

    self.animations.retain(|animation| {
      if animation.window_id != window_id {
        return true;
      }

      if let Some(thumbnail) = animation.thumbnail {
        backend.unregister(thumbnail);
      }

      false
    });

    if self.is_idle() {
      self.backend.release_resources();
    }
  }

  fn begin(
    &mut self,
    request: BeginRequest,
    now: Instant,
    emit: &mut impl FnMut(AnimationUpdate),
  ) {
    self.remove(request.window_id);

    let aborted = AnimationUpdate::Aborted {
      window_id: request.window_id,
      sequence: request.sequence,
    };

    let (phase, thumbnail) = match request.start {
      StartFrame::Cover => {
        // Cover the window exactly where it currently is, so that hiding
        // the window underneath doesn't change what's on screen.
        let Some(current) = self.backend.window_frame(request.handle)
        else {
          emit(aborted);
          return;
        };

        let thumbnail = match self.backend.register(request.handle) {
          Ok(thumbnail) => thumbnail,
          Err(err) => {
            tracing::warn!("Failed to register window thumbnail: {err}");
            emit(aborted);
            return;
          }
        };

        let start = Frame::opaque(current);

        if let Err(err) = self.draw(thumbnail, &start, None) {
          tracing::warn!("Failed to draw window thumbnail: {err}");
          self.backend.unregister(thumbnail);
          emit(aborted);
          return;
        }

        let phase = Phase::Covering {
          start,
          drawn_in_frame: self.frame,
          is_reported: false,
          since: now,
        };

        (phase, Some(thumbnail))
      }
      StartFrame::Hidden { from, settle } => (
        Phase::Settling {
          from,
          settle,
          since: now,
          last_frame: None,
          stable_frames: 0,
        },
        None,
      ),
    };

    tracing::debug!(
      "Animating window (id={}, group={}) to {:?}.",
      request.window_id,
      request.group,
      request.target.to
    );

    self.animations.push(Animation {
      window_id: request.window_id,
      sequence: request.sequence,
      handle: request.handle,
      group: request.group,
      target: request.target,
      thumbnail,
      phase,
    });
  }

  /// Advances the animation at `index` to the given time.
  ///
  /// Returns the final update of the animation if it's over, in which
  /// case the caller removes it.
  fn advance(
    &mut self,
    index: usize,
    now: Instant,
    emit: &mut impl FnMut(AnimationUpdate),
  ) -> Option<AnimationUpdate> {
    let frame = self.frame;
    let animation = &mut self.animations[index];

    let window_id = animation.window_id;
    let sequence = animation.sequence;
    let aborted = AnimationUpdate::Aborted {
      window_id,
      sequence,
    };

    match &mut animation.phase {
      Phase::Covering {
        drawn_in_frame,
        is_reported,
        since,
        ..
      } => {
        if !*is_reported && frame > *drawn_in_frame {
          *is_reported = true;
          emit(AnimationUpdate::Covered {
            window_id,
            sequence,
          });
        }

        if now.saturating_duration_since(*since) > RELEASE_TIMEOUT {
          tracing::warn!("Covered window was not released in time.");
          return Some(aborted);
        }

        None
      }
      Phase::Settling {
        from,
        settle,
        since,
        last_frame,
        stable_frames,
      } => {
        let current_frame = self.backend.window_frame(animation.handle);

        let is_at_target = current_frame
          .as_ref()
          .is_some_and(|frame| is_near(frame, settle));

        if current_frame.is_some() && current_frame == *last_frame {
          *stable_frames += 1;
        } else {
          *stable_frames = 0;
        }

        *last_frame = current_frame;

        let is_timed_out =
          now.saturating_duration_since(*since) >= SETTLE_TIMEOUT;

        if !is_at_target && *stable_frames < SETTLE_STABLE_FRAMES {
          if !is_timed_out {
            return None;
          }

          tracing::debug!(
            "Window didn't reach its position in time: {window_id}"
          );
        }

        let from = from.clone();
        let handle = animation.handle;
        let clip = animation.target.clip.clone();

        let thumbnail = match self.backend.register(handle) {
          Ok(thumbnail) => thumbnail,
          Err(err) => {
            tracing::warn!("Failed to register window thumbnail: {err}");
            return Some(aborted);
          }
        };

        self.animations[index].thumbnail = Some(thumbnail);

        if let Err(err) = self.draw(thumbnail, &from, clip.as_ref()) {
          tracing::warn!("Failed to draw window thumbnail: {err}");
          return Some(aborted);
        }

        self.animations[index].phase = Phase::Ready { start: from };

        None
      }
      Phase::Ready { .. } => None,
      Phase::Running { .. } => self.draw_running(index, now, emit),
      Phase::AwaitingReveal { since } => {
        if now.saturating_duration_since(*since) > REVEAL_TIMEOUT {
          tracing::warn!("Window was not revealed in time.");
          return Some(aborted);
        }

        None
      }
      Phase::AwaitingUncloak { since } => {
        // The window may be uncloaked without being composed yet. The
        // thumbnail is kept until the next frame completes, so that the
        // swap from the thumbnail to the window is seamless.
        if !self.backend.is_cloaked(animation.handle)
          || now.saturating_duration_since(*since) > UNCLOAK_TIMEOUT
        {
          animation.phase = Phase::Draining { frame };
        }

        None
      }
      Phase::Draining { frame: drain_frame } => (frame > *drain_frame)
        .then_some(AnimationUpdate::Removed {
          window_id,
          sequence,
        }),
    }
  }

  /// Draws the tween of the running animation at `index`, and moves the
  /// animation on once the tween is done.
  ///
  /// Returns the final update of the animation if it's over.
  fn draw_running(
    &mut self,
    index: usize,
    now: Instant,
    emit: &mut impl FnMut(AnimationUpdate),
  ) -> Option<AnimationUpdate> {
    let animation = &self.animations[index];

    let (Phase::Running { tween }, Some(thumbnail)) =
      (&animation.phase, animation.thumbnail)
    else {
      return None;
    };

    let frame = tween.sample(now);
    let is_finished = tween.is_finished(now);
    let window_id = animation.window_id;
    let sequence = animation.sequence;
    let end_visible = animation.target.end_visible;
    let clip = animation.target.clip.clone();

    if let Err(err) = self.draw(thumbnail, &frame, clip.as_ref()) {
      tracing::warn!("Failed to draw window thumbnail: {err}");

      return Some(AnimationUpdate::Aborted {
        window_id,
        sequence,
      });
    }

    if !is_finished {
      return None;
    }

    if !end_visible {
      return Some(AnimationUpdate::Removed {
        window_id,
        sequence,
      });
    }

    self.animations[index].phase = Phase::AwaitingReveal { since: now };

    emit(AnimationUpdate::Finished {
      window_id,
      sequence,
    });

    None
  }

  /// Draws a thumbnail at the given frame, clipped to `clip`.
  fn draw(
    &mut self,
    thumbnail: B::Id,
    frame: &Frame,
    clip: Option<&Rect>,
  ) -> anyhow::Result<()> {
    let source_size = self
      .backend
      .source_size(thumbnail)
      .unwrap_or((frame.rect.width(), frame.rect.height()));

    // Opacity is within `0.0..=1.0`, so the cast can't truncate.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let opacity = (frame.opacity * 255.0).round() as u8;

    let thumbnail_frame =
      match clip_thumbnail(&frame.rect, source_size, clip) {
        Some(clipped) => ThumbnailFrame {
          destination: clipped.destination,
          source: clipped.source,
          opacity,
          is_visible: opacity > 0,
        },
        None => ThumbnailFrame {
          destination: frame.rect.clone(),
          source: Rect::from_xy(0, 0, source_size.0, source_size.1),
          opacity,
          is_visible: false,
        },
      };

    self.backend.update(thumbnail, &thumbnail_frame)
  }
}

/// Whether every edge of `rect` is within the settle tolerance of
/// `expected`.
fn is_near(rect: &Rect, expected: &Rect) -> bool {
  (rect.left - expected.left).abs() <= SETTLE_TOLERANCE_PX
    && (rect.top - expected.top).abs() <= SETTLE_TOLERANCE_PX
    && (rect.right - expected.right).abs() <= SETTLE_TOLERANCE_PX
    && (rect.bottom - expected.bottom).abs() <= SETTLE_TOLERANCE_PX
}

/// Spawns the animation worker thread, drawing with DWM thumbnails.
///
/// Returns `None` if the thread could not be spawned, in which case
/// window animations are unavailable.
#[cfg(target_os = "windows")]
pub fn spawn(
  update_tx: tokio::sync::mpsc::UnboundedSender<AnimationUpdate>,
) -> Option<tokio::sync::mpsc::UnboundedSender<WorkerCommand>> {
  let (command_tx, command_rx) = tokio::sync::mpsc::unbounded_channel();

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
#[cfg(target_os = "windows")]
fn run(
  mut command_rx: tokio::sync::mpsc::UnboundedReceiver<WorkerCommand>,
  update_tx: &tokio::sync::mpsc::UnboundedSender<AnimationUpdate>,
) {
  let mut worker = Worker::new(dwm::DwmBackend::default());
  let mut emit = |update| {
    let _ = update_tx.send(update);
  };

  loop {
    // Block until there's something to animate while idle. The overlay
    // window is released while idle, so no messages need processing.
    if worker.is_idle() {
      match command_rx.blocking_recv() {
        Some(command) => worker.handle(command, Instant::now(), &mut emit),
        None => break,
      }
    }

    // Apply pending commands to this frame, so that retargets and
    // cancellations take effect immediately.
    loop {
      match command_rx.try_recv() {
        Ok(command) => worker.handle(command, Instant::now(), &mut emit),
        Err(tokio::sync::mpsc::error::TryRecvError::Empty) => break,
        Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
          worker.clear();
          return;
        }
      }
    }

    worker.tick(Instant::now(), &mut emit);

    // Pace the animation at the display refresh rate.
    if !worker.is_idle() {
      worker.end_frame();
    }
  }

  worker.clear();
}

/// Thumbnail backend using DWM thumbnails.
#[cfg(target_os = "windows")]
mod dwm {
  use wm_platform::{
    dwm_flush, is_window_cloaked, window_frame, Rect, ThumbnailFrame,
    ThumbnailId, ThumbnailOverlay, WindowId,
  };

  use super::ThumbnailBackend;

  /// Draws thumbnails on a [`ThumbnailOverlay`], which is created on
  /// demand and released while no animations are in flight.
  #[derive(Default)]
  pub struct DwmBackend {
    overlay: Option<ThumbnailOverlay>,
  }

  impl ThumbnailBackend for DwmBackend {
    type Id = ThumbnailId;

    fn register(
      &mut self,
      handle: WindowId,
    ) -> anyhow::Result<ThumbnailId> {
      let overlay = match &mut self.overlay {
        Some(overlay) => overlay,
        None => self.overlay.insert(ThumbnailOverlay::new()?),
      };

      Ok(overlay.register(handle.0)?)
    }

    fn update(
      &mut self,
      id: ThumbnailId,
      frame: &ThumbnailFrame,
    ) -> anyhow::Result<()> {
      let overlay = self
        .overlay
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("No thumbnail overlay."))?;

      Ok(overlay.update(id, frame)?)
    }

    fn source_size(&self, id: ThumbnailId) -> Option<(i32, i32)> {
      self.overlay.as_ref()?.source_size(id).ok()
    }

    fn unregister(&mut self, id: ThumbnailId) {
      if let Some(overlay) = &mut self.overlay {
        if let Err(err) = overlay.unregister(id) {
          tracing::warn!("Failed to unregister window thumbnail: {err}");
        }
      }
    }

    fn window_frame(&self, handle: WindowId) -> Option<Rect> {
      window_frame(handle.0)
    }

    fn is_cloaked(&self, handle: WindowId) -> bool {
      is_window_cloaked(handle.0)
    }

    fn end_frame(&mut self) {
      if let Some(overlay) = &self.overlay {
        overlay.pump_messages();
      }

      dwm_flush();
    }

    fn release_resources(&mut self) {
      self.overlay = None;
    }

    fn sync_displays(&mut self) {
      if let Some(overlay) = &mut self.overlay {
        if let Err(err) = overlay.sync_bounds() {
          tracing::warn!("Failed to resize thumbnail overlay: {err}");
        }
      }
    }
  }
}

#[cfg(test)]
mod tests {
  use std::collections::HashMap;

  use super::*;
  use crate::animation::{bezier::CubicBezier, timeline::Timing};

  /// Records thumbnail operations instead of drawing.
  #[derive(Default)]
  struct FakeBackend {
    next_id: u32,
    registered: HashMap<u32, WindowId>,
    updates: Vec<(u32, ThumbnailFrame)>,
    frames: HashMap<WindowId, Rect>,
    cloaked: HashMap<WindowId, bool>,
    released: u32,
  }

  impl ThumbnailBackend for FakeBackend {
    type Id = u32;

    fn register(&mut self, handle: WindowId) -> anyhow::Result<u32> {
      self.next_id += 1;
      self.registered.insert(self.next_id, handle);
      Ok(self.next_id)
    }

    fn update(
      &mut self,
      id: u32,
      frame: &ThumbnailFrame,
    ) -> anyhow::Result<()> {
      self.updates.push((id, frame.clone()));
      Ok(())
    }

    fn source_size(&self, id: u32) -> Option<(i32, i32)> {
      let handle = self.registered.get(&id)?;
      let frame = self.frames.get(handle)?;
      Some((frame.width(), frame.height()))
    }

    fn unregister(&mut self, id: u32) {
      self.registered.remove(&id);
    }

    fn window_frame(&self, handle: WindowId) -> Option<Rect> {
      self.frames.get(&handle).cloned()
    }

    fn is_cloaked(&self, handle: WindowId) -> bool {
      self.cloaked.get(&handle).copied().unwrap_or(false)
    }

    fn end_frame(&mut self) {}

    fn release_resources(&mut self) {
      self.released += 1;
    }

    fn sync_displays(&mut self) {}
  }

  const HANDLE: WindowId = WindowId(42);

  fn rect(x: i32) -> Rect {
    Rect::from_xy(x, 0, 100, 100)
  }

  fn target(x: i32, end_visible: bool) -> Target {
    Target {
      to: Frame::opaque(rect(x)),
      clip: None,
      timing: Timing {
        duration: Duration::from_millis(100),
        curve: CubicBezier::new(0.0, 0.0, 1.0, 1.0),
      },
      end_visible,
    }
  }

  fn begin(
    window_id: Uuid,
    start: StartFrame,
    target: Target,
  ) -> WorkerCommand {
    WorkerCommand::Begin(BeginRequest {
      window_id,
      sequence: 1,
      handle: HANDLE,
      group: 1,
      start,
      target,
    })
  }

  /// Runs the worker like the worker thread does: commands, then a tick,
  /// then the end of the composition frame.
  struct Harness {
    worker: Worker<FakeBackend>,
    updates: Vec<AnimationUpdate>,
    now: Instant,
  }

  impl Harness {
    fn new() -> Self {
      let mut backend = FakeBackend::default();
      backend.frames.insert(HANDLE, rect(0));

      Self {
        worker: Worker::new(backend),
        updates: Vec::new(),
        now: Instant::now(),
      }
    }

    fn send(&mut self, command: WorkerCommand) {
      let updates = &mut self.updates;
      self
        .worker
        .handle(command, self.now, &mut |update| updates.push(update));
    }

    fn frame(&mut self, elapsed_ms: u64) {
      self.now += Duration::from_millis(elapsed_ms);
      let updates = &mut self.updates;
      self
        .worker
        .tick(self.now, &mut |update| updates.push(update));
      self.worker.end_frame();
    }

    fn take_updates(&mut self) -> Vec<AnimationUpdate> {
      std::mem::take(&mut self.updates)
    }

    fn last_destination(&self) -> Rect {
      self
        .worker
        .backend
        .updates
        .last()
        .map(|(_, frame)| frame.destination.clone())
        .unwrap()
    }
  }

  #[test]
  fn covers_moves_and_reveals_visible_window() {
    let id = Uuid::new_v4();
    let mut harness = Harness::new();

    harness.send(begin(id, StartFrame::Cover, target(500, true)));

    // The thumbnail covers the window where it currently is.
    assert_eq!(harness.last_destination(), rect(0));
    assert_eq!(harness.take_updates(), vec![]);

    // `Covered` is reported once the covering frame was composed.
    harness.frame(0);
    harness.frame(16);
    assert_eq!(
      harness.take_updates(),
      vec![AnimationUpdate::Covered {
        window_id: id,
        sequence: 1
      }]
    );

    // The main thread hides the window and releases the animation.
    harness.worker.backend.cloaked.insert(HANDLE, true);
    harness.send(WorkerCommand::Release {
      window_id: id,
      sequence: 1,
    });
    harness.frame(0);

    harness.frame(50);
    assert_eq!(harness.last_destination(), rect(250));

    harness.frame(50);
    assert_eq!(harness.last_destination(), rect(500));
    assert_eq!(
      harness.take_updates(),
      vec![AnimationUpdate::Finished {
        window_id: id,
        sequence: 1
      }]
    );

    // The thumbnail stays until the shown window is on screen.
    harness.send(WorkerCommand::Revealed {
      window_id: id,
      sequence: 1,
    });
    harness.frame(16);
    assert_eq!(harness.worker.backend.registered.len(), 1);

    // Once uncloaked, the thumbnail is kept for one more composed frame.
    harness.worker.backend.cloaked.insert(HANDLE, false);
    harness.frame(16);
    assert_eq!(harness.worker.backend.registered.len(), 1);
    harness.frame(16);

    assert!(harness.worker.backend.registered.is_empty());
    assert!(harness.worker.is_idle());
    assert_eq!(
      harness.take_updates(),
      vec![AnimationUpdate::Removed {
        window_id: id,
        sequence: 1
      }]
    );
  }

  #[test]
  fn removes_thumbnail_when_window_ends_hidden() {
    let id = Uuid::new_v4();
    let mut harness = Harness::new();

    harness.send(begin(id, StartFrame::Cover, target(-500, false)));
    harness.frame(0);
    harness.frame(16);
    harness.send(WorkerCommand::Release {
      window_id: id,
      sequence: 1,
    });
    harness.frame(0);
    harness.take_updates();

    harness.frame(100);

    assert!(harness.worker.backend.registered.is_empty());
    assert_eq!(
      harness.take_updates(),
      vec![AnimationUpdate::Removed {
        window_id: id,
        sequence: 1
      }]
    );
  }

  #[test]
  fn waits_for_hidden_window_to_settle() {
    let id = Uuid::new_v4();
    let mut harness = Harness::new();

    harness.send(begin(
      id,
      StartFrame::Hidden {
        from: Frame::opaque(rect(2000)),
        settle: rect(300),
      },
      target(300, true),
    ));

    // The window hasn't reached its final position yet.
    harness.frame(16);
    assert!(harness.worker.backend.registered.is_empty());

    harness.worker.backend.frames.insert(HANDLE, rect(300));
    harness.frame(16);

    assert_eq!(harness.worker.backend.registered.len(), 1);
    assert_eq!(harness.last_destination(), rect(2000));
  }

  #[test]
  fn retargeted_settling_window_waits_for_new_position() {
    let id = Uuid::new_v4();
    let mut harness = Harness::new();

    harness.send(begin(
      id,
      StartFrame::Hidden {
        from: Frame::opaque(rect(2000)),
        settle: rect(300),
      },
      target(300, true),
    ));

    harness.send(WorkerCommand::Retarget {
      window_id: id,
      sequence: 1,
      target: target(600, true),
      window_rect: rect(600),
    });

    // The old position no longer counts as settled.
    harness.worker.backend.frames.insert(HANDLE, rect(300));
    harness.frame(16);
    assert!(harness.worker.backend.registered.is_empty());

    harness.worker.backend.frames.insert(HANDLE, rect(600));
    harness.frame(16);
    assert_eq!(harness.worker.backend.registered.len(), 1);
  }

  #[test]
  fn retargets_running_animation() {
    let id = Uuid::new_v4();
    let mut harness = Harness::new();

    harness.send(begin(id, StartFrame::Cover, target(500, true)));
    harness.frame(0);
    harness.frame(16);
    harness.send(WorkerCommand::Release {
      window_id: id,
      sequence: 1,
    });
    harness.frame(0);
    harness.frame(50);
    assert_eq!(harness.last_destination(), rect(250));

    harness.send(WorkerCommand::Retarget {
      window_id: id,
      sequence: 1,
      target: target(1250, true),
      window_rect: rect(1250),
    });

    // Continues from the current frame towards the new target.
    harness.frame(50);
    assert_eq!(harness.last_destination(), rect(750));
  }

  #[test]
  fn starts_tweens_of_group_together() {
    let covered_id = Uuid::new_v4();
    let entering_id = Uuid::new_v4();
    let entering_handle = WindowId(7);
    let mut harness = Harness::new();
    harness
      .worker
      .backend
      .frames
      .insert(entering_handle, rect(300));

    harness.send(begin(
      covered_id,
      StartFrame::Cover,
      target(-500, false),
    ));
    harness.send(WorkerCommand::Begin(BeginRequest {
      window_id: entering_id,
      sequence: 1,
      handle: entering_handle,
      group: 1,
      start: StartFrame::Hidden {
        from: Frame::opaque(rect(1000)),
        settle: rect(300),
      },
      target: target(300, true),
    }));

    // The entering window is ready right away, but waits for the covered
    // window of its group to be released.
    harness.frame(16);
    harness.frame(50);
    assert_eq!(harness.last_destination(), rect(1000));

    harness.send(WorkerCommand::Release {
      window_id: covered_id,
      sequence: 1,
    });
    harness.frame(0);
    harness.frame(50);

    // Both tweens are halfway done.
    let destinations = harness
      .worker
      .backend
      .updates
      .iter()
      .rev()
      .take(2)
      .map(|(_, frame)| frame.destination.clone())
      .collect::<Vec<_>>();

    assert!(destinations.contains(&rect(-250)));
    assert!(destinations.contains(&rect(650)));
  }

  #[test]
  fn starts_settled_window_with_adjusted_size() {
    let id = Uuid::new_v4();
    let mut harness = Harness::new();

    // The application keeps its window slightly larger than requested.
    harness
      .worker
      .backend
      .frames
      .insert(HANDLE, Rect::from_xy(300, 0, 120, 100));

    harness.send(begin(
      id,
      StartFrame::Hidden {
        from: Frame::opaque(rect(2000)),
        settle: rect(300),
      },
      target(300, true),
    ));

    for _ in 0..SETTLE_STABLE_FRAMES {
      harness.frame(16);
      assert!(harness.worker.backend.registered.is_empty());
    }

    // Once the frame is stable, the window counts as positioned well
    // before the settle timeout.
    harness.frame(16);
    assert_eq!(harness.worker.backend.registered.len(), 1);
  }

  #[test]
  fn cancel_removes_thumbnail() {
    let id = Uuid::new_v4();
    let mut harness = Harness::new();

    harness.send(begin(id, StartFrame::Cover, target(500, true)));
    assert_eq!(harness.worker.backend.registered.len(), 1);

    harness.send(WorkerCommand::Cancel { window_id: id });

    assert!(harness.worker.backend.registered.is_empty());
    assert!(harness.worker.is_idle());
  }

  #[test]
  fn aborts_when_not_released_in_time() {
    let id = Uuid::new_v4();
    let mut harness = Harness::new();

    harness.send(begin(id, StartFrame::Cover, target(500, true)));
    harness.frame(0);
    harness.frame(16);
    harness.take_updates();

    harness.frame(600);

    assert!(harness.worker.backend.registered.is_empty());
    assert_eq!(
      harness.take_updates(),
      vec![AnimationUpdate::Aborted {
        window_id: id,
        sequence: 1
      }]
    );
  }

  #[test]
  fn ignores_commands_for_replaced_animations() {
    let id = Uuid::new_v4();
    let mut harness = Harness::new();

    harness.send(begin(id, StartFrame::Cover, target(500, true)));
    harness.frame(0);
    harness.frame(16);

    // A release for another sequence doesn't start the tween.
    harness.send(WorkerCommand::Release {
      window_id: id,
      sequence: 7,
    });
    harness.frame(50);

    assert_eq!(harness.last_destination(), rect(0));
  }

  #[test]
  fn aborts_when_window_is_gone() {
    let id = Uuid::new_v4();
    let mut harness = Harness::new();
    harness.worker.backend.frames.clear();

    harness.send(begin(id, StartFrame::Cover, target(500, true)));

    assert!(harness.worker.is_idle());
    assert_eq!(
      harness.take_updates(),
      vec![AnimationUpdate::Aborted {
        window_id: id,
        sequence: 1
      }]
    );
  }
}
