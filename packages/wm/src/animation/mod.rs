//! Animations for window positions.
//!
//! Animations are applied by a worker thread that repositions windows on
//! every composition frame. The main thread keeps computing the final
//! state of every window as usual, with two exceptions:
//!
//! - Windows that are being animated aren't repositioned by the regular
//!   redraw pass (`platform_sync`). Windows that are sliding out are
//!   hidden once their animation completes.
//! - Move events triggered by the animation frames are ignored, so that
//!   they aren't mistaken for user-initiated moves.
//!
//! The engine currently handles sliding windows when the displayed
//! workspace changes, and sliding windows in when they're first shown.

mod bezier;
mod worker;

use std::{
  collections::{HashMap, HashSet},
  time::{Duration, Instant},
};

use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use uuid::Uuid;
use wm_common::{
  AnimationDirection, DisplayState, WindowOpenAnimationStyle, WindowState,
  WorkspaceSwitchAnimationStyle,
};
use wm_platform::{Direction, Rect};

use crate::{
  layouts::WindowPlacement,
  models::{Monitor, WindowContainer},
  traits::{CommonGetters, WindowGetters},
  user_config::UserConfig,
};

use self::{
  bezier::CubicBezier,
  worker::{StartRequest, WorkerCommand},
};

/// Distance in pixels that a window slides beyond the monitor's edge, so
/// that the window (including its shadow) is fully off-screen at the
/// start/end of a slide.
const SLIDE_MARGIN_PX: i32 = 64;

/// Duration that move events are ignored for after an animation
/// completed. The OS can deliver move events for the last animation
/// frames after the animation already completed.
const POST_ANIMATION_GRACE: Duration = Duration::from_millis(200);

/// A switch of the displayed workspace on a monitor.
#[derive(Clone, Copy, Debug)]
pub struct WorkspaceSwitch {
  /// ID of the workspace that was displayed before the switch.
  from: Uuid,

  /// ID of the workspace that's now displayed.
  to: Uuid,

  /// Whether the switch moved forward in the monitor's workspace order.
  is_forward: bool,
}

/// Animation to apply for a window during the redraw pass.
#[derive(Debug)]
pub enum WindowAnimation {
  /// Window slides in from beyond an edge of its monitor. The caller is
  /// expected to position the window at `start_rect` and make it
  /// visible; the worker animates it to its final position.
  SlideIn {
    /// Frame to position the window at before the animation starts.
    start_rect: Rect,
  },

  /// Window slides out beyond an edge of its monitor and is hidden once
  /// the animation completes.
  SlideOut,
}

/// What an animated window is being animated for.
#[derive(Clone, Copy, Debug, PartialEq)]
enum AnimationKind {
  /// Window slides in when its workspace becomes displayed.
  SlideIn,

  /// Window slides out when its workspace stops being displayed.
  SlideOut,

  /// Window slides in when it's first shown (e.g. an application
  /// launching).
  WindowOpen,
}

/// State of an animation that's in flight.
#[derive(Debug)]
struct ActiveAnimation {
  /// Sequence number of the animation. Completions for a different
  /// sequence number belong to a replaced animation and are ignored.
  sequence: u64,

  kind: AnimationKind,

  /// Frame that the animation is tweening the window towards.
  target: Rect,
}

/// Completion of a window animation, emitted by the animation worker.
#[derive(Clone, Copy, Debug)]
pub struct AnimationCompletion {
  /// ID of the animated window.
  pub window_id: Uuid,

  /// Sequence number of the completed animation.
  pub sequence: u64,
}

/// Edge of a monitor that windows slide in from.
#[derive(Clone, Copy, Debug)]
enum SlideEdge {
  Left,
  Right,
  Top,
  Bottom,
}

impl SlideEdge {
  /// Resolves the edge that the incoming workspace slides in from for
  /// the given animation style and configured direction.
  ///
  /// `is_forward` is whether the switch moved forward in the monitor's
  /// workspace order (e.g. focused the next workspace).
  fn for_switch(
    style: WorkspaceSwitchAnimationStyle,
    direction: AnimationDirection,
    is_forward: bool,
  ) -> Self {
    // The vertical style (`slidevert`) slides along the y-axis and the
    // horizontal style (`slide`) slides along the x-axis. Directions
    // that don't match the style's axis are treated as `auto`.
    match style {
      WorkspaceSwitchAnimationStyle::SlideVert => match direction {
        AnimationDirection::Up => Self::Bottom,
        AnimationDirection::Down => Self::Top,
        AnimationDirection::Auto
        | AnimationDirection::Left
        | AnimationDirection::Right => {
          if is_forward {
            Self::Bottom
          } else {
            Self::Top
          }
        }
      },
      WorkspaceSwitchAnimationStyle::Slide
      | WorkspaceSwitchAnimationStyle::None => match direction {
        AnimationDirection::Left => Self::Left,
        AnimationDirection::Right => Self::Right,
        AnimationDirection::Auto
        | AnimationDirection::Up
        | AnimationDirection::Down => {
          if is_forward {
            Self::Right
          } else {
            Self::Left
          }
        }
      },
    }
  }

  /// Edge that the outgoing workspace slides out through. Windows travel
  /// in the same direction during a switch, so the outgoing workspace
  /// leaves through the opposite edge.
  fn opposite(self) -> Self {
    match self {
      Self::Left => Self::Right,
      Self::Right => Self::Left,
      Self::Top => Self::Bottom,
      Self::Bottom => Self::Top,
    }
  }

  fn to_direction(self) -> Direction {
    match self {
      Self::Left => Direction::Left,
      Self::Right => Direction::Right,
      Self::Top => Direction::Up,
      Self::Bottom => Direction::Down,
    }
  }
}

/// Animation state managed on the main thread.
pub struct Animations {
  /// Animations that are in flight, keyed by window ID.
  active: HashMap<Uuid, ActiveAnimation>,

  /// Windows to animate when they're first shown, populated when a
  /// window is managed. Used for the window open animation.
  pending_open: HashSet<Uuid>,

  /// The displayed workspace of each monitor as of the last sync pass.
  /// Used to detect workspace switches.
  displayed_workspaces: HashMap<Uuid, Uuid>,

  /// Timestamps of animations that recently completed or were cancelled,
  /// keyed by window ID.
  recently_finished: HashMap<Uuid, Instant>,

  /// Sequence number for the next animation.
  next_sequence: u64,

  /// Command sender for the animation worker. `None` if animations aren't
  /// supported on this platform or the worker failed to start.
  worker_tx: Option<UnboundedSender<WorkerCommand>>,
}

impl Animations {
  /// Creates the animation engine, along with the receiver for animation
  /// completions.
  ///
  /// Completed animations must be applied via
  /// `WindowManager::process_animation_completion`.
  #[must_use]
  pub fn new() -> (Self, UnboundedReceiver<AnimationCompletion>) {
    let (completion_tx, completion_rx) = mpsc::unbounded_channel();

    #[cfg(target_os = "windows")]
    let worker_tx = worker::spawn(completion_tx);

    #[cfg(not(target_os = "windows"))]
    let worker_tx = {
      let _ = completion_tx;
      None
    };

    let mut animations = Self::disabled();
    animations.worker_tx = worker_tx;

    (animations, completion_rx)
  }

  /// Creates a disabled engine, for tests and unsupported platforms.
  #[must_use]
  pub fn disabled() -> Self {
    Self {
      active: HashMap::new(),
      pending_open: HashSet::new(),
      displayed_workspaces: HashMap::new(),
      recently_finished: HashMap::new(),
      next_sequence: 0,
      worker_tx: None,
    }
  }

  /// Whether animations are enabled in the config and supported on this
  /// platform.
  fn is_available(&self, config: &UserConfig) -> bool {
    self.worker_tx.is_some() && config.value.general.animations.enabled
  }

  /// Marks a window to be animated when it's first shown. Used for the
  /// window open animation.
  ///
  /// Windows that exist when the WM starts aren't animated, so this is a
  /// no-op until the first sync pass has completed.
  pub fn mark_pending_open(&mut self, window_id: Uuid) {
    if self.displayed_workspaces.is_empty() {
      return;
    }

    self.pending_open.insert(window_id);
  }

  /// Detects workspace switches on each monitor and records the
  /// workspaces that are now displayed.
  ///
  /// Must be called on every sync pass before windows are redrawn. The
  /// first pass for a monitor (e.g. at startup) reports no switch.
  pub fn detect_switches(
    &mut self,
    monitors: &[Monitor],
  ) -> HashMap<Uuid, WorkspaceSwitch> {
    self
      .recently_finished
      .retain(|_, finished_at| finished_at.elapsed() < POST_ANIMATION_GRACE);

    let mut switches = HashMap::new();

    for monitor in monitors {
      let Some(workspace) = monitor.displayed_workspace() else {
        continue;
      };

      let workspace_id = workspace.id();

      let previous_id = self
        .displayed_workspaces
        .insert(monitor.id(), workspace_id);

      let Some(previous_id) = previous_id.filter(|id| *id != workspace_id)
      else {
        continue;
      };

      // Determine whether the switch moved forward in the monitor's
      // workspace order, so that the slide direction can follow it.
      let workspace_ids = monitor
        .children()
        .iter()
        .map(CommonGetters::id)
        .collect::<Vec<_>>();

      let is_forward = match (
        workspace_ids.iter().position(|id| *id == previous_id),
        workspace_ids.iter().position(|id| *id == workspace_id),
      ) {
        (Some(from_index), Some(to_index)) => to_index > from_index,
        // The direction can't be determined if either workspace is no
        // longer in the monitor's workspace list (e.g. it was removed
        // from the config). Default to a forward switch.
        _ => true,
      };

      switches.insert(
        monitor.id(),
        WorkspaceSwitch {
          from: previous_id,
          to: workspace_id,
          is_forward,
        },
      );
    }

    switches
  }

  /// Plans an animation for a window that's entering or leaving the
  /// displayed workspace of its monitor.
  ///
  /// Returns the animation to apply during the redraw pass, or `None` if
  /// the window shouldn't be animated.
  pub fn plan_switch_animation(
    &mut self,
    window: &WindowContainer,
    previous_display_state: &DisplayState,
    switches: &HashMap<Uuid, WorkspaceSwitch>,
    placement: &WindowPlacement,
    config: &UserConfig,
  ) -> Option<WindowAnimation> {
    let animation_config = &config.value.general.animations.workspace_switch;

    if !self.is_available(config)
      || animation_config.style == WorkspaceSwitchAnimationStyle::None
      || animation_config.duration_ms == 0
      || !Self::is_animatable_window(window, placement)
    {
      return None;
    }

    let (Some(workspace), Some(monitor)) =
      (window.workspace(), window.monitor())
    else {
      return None;
    };

    let switch = switches.get(&monitor.id())?;

    let display_state = window.display_state();

    let is_entering = workspace.id() == switch.to
      && matches!(
        previous_display_state,
        DisplayState::Hidden | DisplayState::Hiding
      )
      && matches!(display_state, DisplayState::Showing);

    let is_leaving = workspace.id() == switch.from
      && matches!(
        previous_display_state,
        DisplayState::Shown | DisplayState::Showing
      )
      && matches!(display_state, DisplayState::Hiding);

    if !is_entering && !is_leaving {
      return None;
    }

    let slide_edge = SlideEdge::for_switch(
      animation_config.style,
      animation_config.direction,
      switch.is_forward,
    );

    // The window is parked a margin beyond the monitor edge, so that
    // it's fully off-screen at the start/end of the slide.
    let monitor_bounds = monitor.native_properties().bounds;

    let edge_length = match slide_edge {
      SlideEdge::Left | SlideEdge::Right => monitor_bounds.width(),
      SlideEdge::Top | SlideEdge::Bottom => monitor_bounds.height(),
    };

    let slide_distance = edge_length + SLIDE_MARGIN_PX;

    let (from_rect, to_rect, kind, animation) = if is_entering {
      let start_rect = placement.rect.translate_in_direction(
        &slide_edge.to_direction(),
        slide_distance,
      );

      (
        Some(start_rect.clone()),
        placement.rect.clone(),
        AnimationKind::SlideIn,
        WindowAnimation::SlideIn { start_rect },
      )
    } else {
      let end_rect = placement.rect.translate_in_direction(
        &slide_edge.opposite().to_direction(),
        slide_distance,
      );

      (
        None,
        end_rect,
        AnimationKind::SlideOut,
        WindowAnimation::SlideOut,
      )
    };

    self.start_animation(
      window,
      from_rect,
      to_rect,
      Duration::from_millis(u64::from(animation_config.duration_ms)),
      CubicBezier::from(&animation_config.curve),
      kind,
    );

    Some(animation)
  }

  /// Plans an animation for a window that's being shown for the first
  /// time (e.g. an application that's launched).
  ///
  /// Returns the animation to apply during the redraw pass, or `None` if
  /// the window shouldn't be animated.
  pub fn plan_window_open(
    &mut self,
    window: &WindowContainer,
    placement: &WindowPlacement,
    config: &UserConfig,
  ) -> Option<WindowAnimation> {
    // The flag is consumed on the first redraw pass the window goes
    // through, so that a window that isn't actually shown during this
    // pass (e.g. it's on a workspace that isn't displayed) doesn't get
    // animated later.
    if !self.pending_open.remove(&window.id()) {
      return None;
    }

    if let Some(reason) =
      self.open_slide_skip_reason(window, placement, config)
    {
      tracing::debug!("Not animating window open for {window}: {reason}");
      return None;
    }

    let animation_config = &config.value.general.animations.window_open;

    let monitor = window.monitor()?;

    // The window slides in from the right edge of its monitor.
    let slide_edge = SlideEdge::Right;

    let slide_distance =
      monitor.native_properties().bounds.width() + SLIDE_MARGIN_PX;

    let start_rect = placement
      .rect
      .translate_in_direction(&slide_edge.to_direction(), slide_distance);

    tracing::debug!("Animating window open for {window}");

    self.start_animation(
      window,
      Some(start_rect.clone()),
      placement.rect.clone(),
      Duration::from_millis(u64::from(animation_config.duration_ms)),
      CubicBezier::from(&animation_config.curve),
      AnimationKind::WindowOpen,
    );

    Some(WindowAnimation::SlideIn { start_rect })
  }

  /// Why a newly opened window shouldn't get an open slide, if any.
  ///
  /// The `pending_open` flag is consumed regardless of the outcome, so a
  /// window that isn't animated on its first redraw pass is never
  /// animated later.
  fn open_slide_skip_reason(
    &self,
    window: &WindowContainer,
    placement: &WindowPlacement,
    config: &UserConfig,
  ) -> Option<&'static str> {
    let animation_config = &config.value.general.animations.window_open;

    if !self.is_available(config) {
      return Some("animations are unavailable");
    }

    if animation_config.style == WindowOpenAnimationStyle::None {
      return Some("window open animations are disabled in config");
    }

    if animation_config.duration_ms == 0 {
      return Some("window open animation duration is zero");
    }

    // New windows start in the `Shown` display state (the OS window
    // already exists by the time it's managed), so the check is on
    // whether the window will be visible after this pass.
    if !matches!(
      window.display_state(),
      DisplayState::Showing | DisplayState::Shown
    ) {
      return Some("window won't be visible after this pass");
    }

    if !Self::is_animatable_window(window, placement) {
      return Some("window can't be animated");
    }

    None
  }

  /// Whether the window is eligible for position animations.
  fn is_animatable_window(
    window: &WindowContainer,
    placement: &WindowPlacement,
  ) -> bool {
    // Only tiled windows are animated for now. Floating windows would
    // need their saved placement restored after the animation, and
    // fullscreen/minimized windows aren't positioned by the WM.
    window.state() == WindowState::Tiling
      && !placement.is_scrolled_out
      && window.active_drag().is_none()
  }

  /// Registers an animation with the worker thread.
  fn start_animation(
    &mut self,
    window: &WindowContainer,
    // LINT: `from_rect` is only used on Windows.
    #[cfg_attr(not(target_os = "windows"), allow(unused_variables))]
    from_rect: Option<Rect>,
    // LINT: `to_rect` is only used on Windows.
    #[cfg_attr(not(target_os = "windows"), allow(unused_variables))]
    to_rect: Rect,
    // LINT: `duration` is only used on Windows.
    #[cfg_attr(not(target_os = "windows"), allow(unused_variables))]
    duration: Duration,
    // LINT: `curve` is only used on Windows.
    #[cfg_attr(not(target_os = "windows"), allow(unused_variables))]
    curve: CubicBezier,
    kind: AnimationKind,
  ) {
    let window_id = window.id();

    let sequence = self.next_sequence;
    self.next_sequence += 1;

    #[cfg(target_os = "windows")]
    {
      let Some(worker_tx) = &self.worker_tx else {
        return;
      };

      let _ = worker_tx.send(WorkerCommand::Start(StartRequest {
        window_id,
        sequence,
        handle: window.native().id().0,
        from: from_rect,
        to: to_rect.clone(),
        duration,
        curve,
      }));
    }

    self.active.insert(
      window_id,
      ActiveAnimation {
        sequence,
        kind,
        target: to_rect,
      },
    );
  }

  /// Keeps the open animation of a window alive when the layout moves
  /// the window while the slide is still in flight.
  ///
  /// A window that opens in a scrolling workspace can be laid out
  /// differently within the same sync pass or the next one, because the
  /// strip re-flows and scrolls when the window is inserted. Instead of
  /// cancelling the slide and snapping the window to the new placement,
  /// the slide is retargeted to it from the window's current frame.
  ///
  /// Returns whether the animation worker owns the window's position,
  /// meaning that the redraw pass must not reposition the window.
  pub fn retarget_open_animation(
    &mut self,
    window: &WindowContainer,
    placement: &WindowPlacement,
    config: &UserConfig,
  ) -> bool {
    let Some(active) = self.active.get(&window.id()) else {
      return false;
    };

    if active.kind != AnimationKind::WindowOpen {
      return false;
    }

    let target = active.target.clone();

    // If the window can no longer be animated (e.g. it was hidden again
    // or scrolled out of view mid-slide), let the regular redraw pass
    // take over.
    if !self.is_available(config)
      || !Self::is_animatable_window(window, placement)
    {
      return false;
    }

    if target == placement.rect {
      // The placement didn't change; the window is already sliding
      // towards it.
      return true;
    }

    let animation_config = &config.value.general.animations.window_open;

    tracing::debug!("Retargeting window open animation: {window}");

    // Start from the window's current frame so that the slide continues
    // seamlessly.
    self.start_animation(
      window,
      None,
      placement.rect.clone(),
      Duration::from_millis(u64::from(animation_config.duration_ms)),
      CubicBezier::from(&animation_config.curve),
      AnimationKind::WindowOpen,
    );

    true
  }

  /// Cancels the animation that's in flight for the window, if any.
  pub fn cancel(&mut self, window: &WindowContainer) {
    let window_id = window.id();

    if self.active.remove(&window_id).is_none() {
      return;
    }

    self.recently_finished.insert(window_id, Instant::now());

    #[cfg(target_os = "windows")]
    if let Some(worker_tx) = &self.worker_tx {
      let _ = worker_tx.send(WorkerCommand::Cancel { window_id });
    }
  }

  /// Whether a move event for the window can be ignored because an
  /// animation is responsible for its position.
  pub fn should_ignore_move_event(&self, window: &WindowContainer) -> bool {
    let window_id = window.id();

    if self.active.contains_key(&window_id) {
      return true;
    }

    if !self
      .recently_finished
      .get(&window_id)
      .is_some_and(|finished_at| {
        finished_at.elapsed() < POST_ANIMATION_GRACE
      })
    {
      return false;
    }

    // Ignore the trailing events of the animation, except for the ones
    // that finalize the display state: windows that are sliding out
    // still need to be detected as hidden when they reach the hide
    // corner, and windows that slid in are marked as shown when they
    // reach their final position.
    !matches!(
      window.display_state(),
      DisplayState::Hiding | DisplayState::Showing
    )
  }

  /// Applies a completed animation.
  ///
  /// Returns the window that needs to be redrawn with its final display
  /// state applied, if any.
  pub fn complete(
    &mut self,
    completion: &AnimationCompletion,
  ) -> Option<Uuid> {
    let is_slide_out = {
      let animation = self.active.get(&completion.window_id)?;

      // Ignore completions of animations that were replaced by a newer
      // one.
      if animation.sequence != completion.sequence {
        return None;
      }

      animation.kind == AnimationKind::SlideOut
    };

    self.active.remove(&completion.window_id);
    self
      .recently_finished
      .insert(completion.window_id, Instant::now());

    // Windows that slid out need to be redrawn so that they're hidden
    // (cloaked or moved to the hide corner) by the regular redraw pass.
    is_slide_out.then_some(completion.window_id)
  }
}
