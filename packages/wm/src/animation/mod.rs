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
#[cfg(target_os = "windows")]
use wm_platform::NativeWindowWindowsExt;
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
  /// visible; the worker animates it to its final position. A window
  /// that's cloaked until its open slide is parked is revealed by the
  /// worker instead (see `AnimationUpdate::RevealCloakedWindow`).
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

/// Update emitted by the animation worker for the main thread to apply.
#[derive(Clone, Copy, Debug)]
pub enum AnimationUpdate {
  /// A window animation completed.
  Completed {
    /// ID of the animated window.
    window_id: Uuid,

    /// Sequence number of the completed animation.
    sequence: u64,
  },

  /// The worker parked a window that's sliding in for the first time
  /// (`AnimationKind::WindowOpen`). The cloak that hid the window while
  /// it was waiting to be parked can be lifted now.
  ///
  /// The cloak must be lifted on the main thread, because cloaking uses
  /// shell COM objects. The main thread confirms the reveal once the
  /// cloak is lifted (see `Animations::confirm_reveal`), which starts
  /// the window's slide.
  RevealCloakedWindow {
    /// ID of the window to reveal.
    window_id: Uuid,

    /// Sequence number of the animation that requested the reveal.
    sequence: u64,
  },
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
    // The vertical style (`slide_vert`) slides along the y-axis and the
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

/// Gets the current frame of a window from the OS.
///
/// Used to retarget a slide from where the window currently is, instead
/// of from where it was originally planned to start.
#[cfg(target_os = "windows")]
fn get_window_frame(window: &WindowContainer) -> Option<Rect> {
  wm_platform::window_frame(window.native().id().0)
}

/// Always `None` on platforms without window animations.
#[cfg(not(target_os = "windows"))]
fn get_window_frame(_window: &WindowContainer) -> Option<Rect> {
  None
}

/// Animation state managed on the main thread.
pub struct Animations {
  /// Animations that are in flight, keyed by window ID.
  active: HashMap<Uuid, ActiveAnimation>,

  /// Windows to animate when they're first shown, populated when a
  /// window is managed. Used for the window open animation.
  pending_open: HashSet<Uuid>,

  /// Windows that are currently cloaked for their open animation.
  ///
  /// A window that's going to slide in when it's first shown is cloaked
  /// as soon as it's managed, so that it can't flash in place before the
  /// animation parks it off-screen. The cloak is lifted once the window
  /// is parked (reported by the animation worker), or when the window
  /// stops being animated (e.g. its open slide is skipped or cancelled).
  cloaked_for_open: HashSet<Uuid>,

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
  /// Creates the animation engine, along with the receiver for updates
  /// from the animation worker.
  ///
  /// Updates must be applied via `WindowManager::process_animation_update`.
  #[must_use]
  pub fn new() -> (Self, UnboundedReceiver<AnimationUpdate>) {
    let (update_tx, update_rx) = mpsc::unbounded_channel();

    #[cfg(target_os = "windows")]
    let worker_tx = worker::spawn(update_tx);

    #[cfg(not(target_os = "windows"))]
    let worker_tx = {
      let _ = update_tx;
      None
    };

    let mut animations = Self::disabled();
    animations.worker_tx = worker_tx;

    (animations, update_rx)
  }

  /// Creates a disabled engine, for tests and unsupported platforms.
  #[must_use]
  pub fn disabled() -> Self {
    Self {
      active: HashMap::new(),
      pending_open: HashSet::new(),
      cloaked_for_open: HashSet::new(),
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
  pub fn mark_pending_open(
    &mut self,
    window: &WindowContainer,
    // LINT: `config` is only used on Windows.
    #[cfg_attr(not(target_os = "windows"), allow(unused_variables))]
    config: &UserConfig,
  ) {
    if self.displayed_workspaces.is_empty() {
      return;
    }

    self.pending_open.insert(window.id());

    #[cfg(target_os = "windows")]
    self.cloak_for_open(window, config);
  }

  /// Cloaks a newly managed window until its open animation parks it
  /// off-screen, so that the window can't flash in place before the
  /// slide starts.
  ///
  /// Only windows that are going to slide in are cloaked. The cloak is
  /// lifted once the animation worker reports that the window is parked
  /// (`AnimationUpdate::RevealCloakedWindow`), or when the window stops
  /// being animated with the cloak still on (see `lift_window_cloak`).
  #[cfg(target_os = "windows")]
  fn cloak_for_open(&mut self, window: &WindowContainer, config: &UserConfig) {
    let animation_config = &config.value.general.animations.window_open;

    let will_slide_in = self.is_available(config)
      && !matches!(
        window.state(),
        WindowState::Minimized | WindowState::Fullscreen(_)
      )
      && animation_config.style != WindowOpenAnimationStyle::None
      && animation_config.duration_ms > 0;

    if !will_slide_in {
      return;
    }

    if let Err(err) = window.native().set_cloaked(true) {
      tracing::warn!("Failed to cloak window for its open animation: {err}");
      return;
    }

    self.cloaked_for_open.insert(window.id());
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
      let edge_rect = placement.rect.translate_in_direction(
        &slide_edge.to_direction(),
        slide_distance,
      );

      // A window that's already mid-slide (e.g. its workspace was
      // switched away from and back before the slide finished) resumes
      // from its current frame, instead of snapping back to the edge.
      let start_rect = if self.active.contains_key(&window.id()) {
        get_window_frame(window).unwrap_or(edge_rect)
      } else {
        edge_rect
      };

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

      // The window was cloaked when it was managed, in case it would
      // slide in. It won't, so lift the cloak before the regular redraw
      // pass positions the window.
      self.lift_window_cloak(window);

      return None;
    }

    let animation_config = &config.value.general.animations.window_open;

    // The monitor is needed to compute the frame that the window is
    // parked at. Lift the cloak before bailing out if there's none yet,
    // so that the window isn't left invisible.
    let Some(monitor) = window.monitor() else {
      self.lift_window_cloak(window);

      return None;
    };

    // `slide` parks the window beyond the right edge of the monitor
    // and `slide_vert` below the bottom edge so that it rises into
    // place. The window is parked a margin beyond the edge, so that
    // it's fully off-screen at the start of the slide. (`none` was
    // filtered out above, in `open_slide_skip_reason`.)
    let slide_edge = match animation_config.style {
      WindowOpenAnimationStyle::SlideVert => SlideEdge::Bottom,
      WindowOpenAnimationStyle::Slide
      | WindowOpenAnimationStyle::None => SlideEdge::Right,
    };

    let monitor_bounds = monitor.native_properties().bounds;

    let slide_distance = match slide_edge {
      SlideEdge::Left | SlideEdge::Right => monitor_bounds.width(),
      SlideEdge::Top | SlideEdge::Bottom => monitor_bounds.height(),
    } + SLIDE_MARGIN_PX;

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

    // Apps are allowed to ignore position updates while fullscreen, so
    // the window could sit parked off-screen (and cloaked) until the
    // park times out. Fullscreen windows are left to the switch
    // animation instead.
    if matches!(window.state(), WindowState::Fullscreen(_)) {
      return Some("fullscreen windows aren't animated on open");
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
    // Minimized windows aren't positioned by the WM, tiling windows that
    // are scrolled out of the viewport are already parked at the edge,
    // and dragged or resized windows follow the cursor. Tiling, floating,
    // and fullscreen windows are all animated.
    window.state() != WindowState::Minimized
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

    // A window that's still waiting for its open slide loses its cloak
    // when any other animation takes over: the reveal is only requested
    // by the in-flight open slide, which was just replaced. The pending
    // open flag is dropped too, so that a later redraw pass doesn't
    // start a second open slide for the same window.
    if kind != AnimationKind::WindowOpen {
      self.pending_open.remove(&window_id);
      self.lift_window_cloak(window);
    }

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
        is_window_open: kind == AnimationKind::WindowOpen,
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

    // A window that's transitioning to fullscreen is handed back to the
    // regular redraw pass too: fullscreen windows aren't animated on
    // open (see `open_slide_skip_reason`).
    if matches!(window.state(), WindowState::Fullscreen(_)) {
      return false;
    }

    if target == placement.rect {
      // The placement didn't change; the window is already sliding
      // towards it.
      return true;
    }

    let animation_config = &config.value.general.animations.window_open;

    tracing::debug!("Retargeting window open animation: {window}");

    // Let the worker continue the slide from the window's current frame,
    // or from the in-flight park frame if the window is still being
    // parked, so that the retarget is seamless.
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

    // The regular redraw pass owns the window's position from here on,
    // so the window must be visible again.
    self.lift_window_cloak(window);

    #[cfg(target_os = "windows")]
    if let Some(worker_tx) = &self.worker_tx {
      let _ = worker_tx.send(WorkerCommand::Cancel { window_id });
    }
  }

  /// Lifts the cloak that hid a window while its open animation was
  /// being prepared, if the cloak is still on.
  ///
  /// Called when the animation worker reports that the window was parked
  /// (`AnimationUpdate::RevealCloakedWindow`), and when the window stops
  /// being animated with the cloak still on: the open slide was skipped,
  /// cancelled, or replaced by another animation, or the window was
  /// unmanaged. The cloak must not be left on a window, because cloaked
  /// windows can't be shown by the regular redraw pass.
  pub fn lift_window_cloak(&mut self, window: &WindowContainer) {
    if !self.cloaked_for_open.remove(&window.id()) {
      return;
    }

    #[cfg(target_os = "windows")]
    if let Err(err) = window.native().set_cloaked(false) {
      tracing::warn!("Failed to lift cloak for window: {err}");
    }
  }

  /// Confirms to the animation worker that a window's cloak was lifted,
  /// so that the window's open slide can start.
  ///
  /// Sent while processing `AnimationUpdate::RevealCloakedWindow`, after
  /// the cloak is lifted. The worker holds the slide's clock until the
  /// confirmation, so that the slide isn't already partway through by
  /// the time the window is visible.
  pub fn confirm_reveal(
    &self,
    // LINT: `window_id` and `sequence` are only used on Windows.
    #[cfg_attr(not(target_os = "windows"), allow(unused_variables))]
    window_id: Uuid,
    #[cfg_attr(not(target_os = "windows"), allow(unused_variables))]
    sequence: u64,
  ) {
    #[cfg(target_os = "windows")]
    if let Some(worker_tx) = &self.worker_tx {
      let _ = worker_tx.send(WorkerCommand::Revealed {
        window_id,
        sequence,
      });
    }
  }

  /// Whether the window is cloaked by the WM while waiting for its open
  /// animation to park it.
  ///
  /// Cloaking a window produces window events, which shouldn't be
  /// mistaken for the window being hidden externally (e.g. to the tray).
  pub fn is_awaiting_reveal(&self, window_id: Uuid) -> bool {
    self.cloaked_for_open.contains(&window_id)
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
  pub fn complete(&mut self, window_id: Uuid, sequence: u64) -> Option<Uuid> {
    let is_slide_out = {
      let animation = self.active.get(&window_id)?;

      // Ignore completions of animations that were replaced by a newer
      // one.
      if animation.sequence != sequence {
        return None;
      }

      animation.kind == AnimationKind::SlideOut
    };

    self.active.remove(&window_id);
    self.recently_finished.insert(window_id, Instant::now());

    // Windows that slid out need to be redrawn so that they're hidden
    // (cloaked or moved to the hide corner) by the regular redraw pass.
    is_slide_out.then_some(window_id)
  }
}
