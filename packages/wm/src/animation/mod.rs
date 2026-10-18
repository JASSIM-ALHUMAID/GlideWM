//! Window animations.
//!
//! Windows are animated via live compositor thumbnails rather than by
//! moving the real windows frame by frame: an animated window is hidden
//! (cloaked) and moved to its final position in one step, while a worker
//! thread animates the window's thumbnail on a click-through overlay
//! (see `worker` for the protocol). Once the animation ends, the window
//! is shown again underneath its thumbnail and the thumbnail is removed.
//!
//! This keeps animations smooth regardless of how fast applications
//! process position updates, and real windows never visibly jump.
//!
//! The main thread (this module) decides which windows are animated
//! during the redraw pass (see `planner`), and is the only writer of the
//! cloak state of windows that it animates.
//!
//! Animations require the `cloak` hide method: windows hidden by other
//! methods aren't drawn by the compositor, so they have no thumbnail to
//! animate.

mod bezier;
mod geometry;
mod planner;
mod timeline;
mod worker;

use std::collections::{HashMap, HashSet};

use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use uuid::Uuid;
use wm_common::{
  DisplayState, HideMethod, WindowOpenAnimationStyle, WindowState,
};
use wm_platform::{Rect, WindowId, WindowZOrder};

use self::{
  planner::{PlanInput, Start, SwitchRole, Target, Visibility},
  timeline::{Frame, Timing},
  worker::{BeginRequest, StartFrame, WorkerCommand},
};
use crate::{
  layouts::WindowPlacement,
  models::{Monitor, WindowContainer},
  traits::{CommonGetters, WindowGetters},
  user_config::UserConfig,
};

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

/// Update emitted by the animation worker for the main thread to apply
/// (see `Animations::apply_update`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnimationUpdate {
  /// The thumbnail covers the visible window, so the window can be
  /// hidden and moved to its final position.
  Covered { window_id: Uuid, sequence: u64 },

  /// The animation ended with the window visible, so the window can be
  /// shown underneath its thumbnail.
  Finished { window_id: Uuid, sequence: u64 },

  /// The animation is over and its thumbnail was removed.
  Removed { window_id: Uuid, sequence: u64 },

  /// The animation couldn't continue and its thumbnail was removed. The
  /// window's final state must be applied without animating it.
  Aborted { window_id: Uuid, sequence: u64 },
}

impl AnimationUpdate {
  /// ID of the animated window.
  #[must_use]
  pub fn window_id(&self) -> Uuid {
    match self {
      Self::Covered { window_id, .. }
      | Self::Finished { window_id, .. }
      | Self::Removed { window_id, .. }
      | Self::Aborted { window_id, .. } => *window_id,
    }
  }

  /// Sequence number of the animation.
  fn sequence(&self) -> u64 {
    match self {
      Self::Covered { sequence, .. }
      | Self::Finished { sequence, .. }
      | Self::Removed { sequence, .. }
      | Self::Aborted { sequence, .. } => *sequence,
    }
  }
}

/// How the redraw pass handles a window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ownership {
  /// The window isn't animated: the redraw pass positions it and applies
  /// its visibility as usual.
  None,

  /// The window is animated and hidden: the redraw pass positions it,
  /// but leaves its visibility to the animation.
  PositionOnly,

  /// The window is animated and still visible: it's positioned once its
  /// thumbnail covers it (see `AnimationUpdate::Covered`). The redraw
  /// pass leaves it alone.
  Deferred,
}

/// Position to apply to a window once its thumbnail covers it.
#[derive(Clone, Debug)]
pub struct DeferredPosition {
  pub rect: Rect,
  pub z_order: WindowZOrder,
}

/// What the caller of `Animations::apply_update` needs to do.
#[derive(Debug, Default)]
pub struct UpdateOutcome {
  /// Position to apply to the window.
  pub position: Option<DeferredPosition>,

  /// Whether the window needs to be redrawn (e.g. to apply its final
  /// state after an aborted animation).
  pub needs_redraw: bool,
}

/// A window's transition during a redraw pass.
pub struct RedrawInput<'a> {
  pub placement: &'a WindowPlacement,
  pub previous_display_state: &'a DisplayState,

  /// Whether the window itself is shown after the redraw. Unlike the
  /// placement, this is `true` for windows that are parked off-screen.
  pub is_visible: bool,

  pub switches: &'a HashMap<Uuid, WorkspaceSwitch>,
  pub z_order: &'a WindowZOrder,

  /// Viewport of the window's workspace, which windows scrolling into or
  /// out of view are clipped to.
  pub viewport: Option<Rect>,
}

/// Phase of an animation, as seen by the main thread.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SlotPhase {
  /// Waiting for the thumbnail to cover the visible window.
  Covering,

  /// The window is hidden and its thumbnail is animating.
  Running,

  /// The window was shown again; waiting for its thumbnail to be
  /// removed.
  Revealing,
}

/// An animation in flight, as seen by the main thread.
#[derive(Debug)]
struct Slot {
  sequence: u64,
  handle: WindowId,
  phase: SlotPhase,
  target: Target,

  /// Position to apply once the thumbnail covers the window.
  deferred_position: Option<DeferredPosition>,

  /// Whether the window is shown once its thumbnail is removed, although
  /// the animation ends out of view (e.g. a window that's parked
  /// off-screen when it's scrolled out of view).
  reveal_on_removal: bool,
}

/// Animation state managed on the main thread.
pub struct Animations {
  /// Animations in flight, keyed by window ID.
  slots: HashMap<Uuid, Slot>,

  /// Windows that are animated when they're first shown, populated when
  /// a window is managed.
  pending_open: HashSet<Uuid>,

  /// Windows that were cloaked when they were managed, so that they
  /// can't flash in place before their open animation starts. Keyed by
  /// window ID.
  cloaked_for_open: HashMap<Uuid, WindowId>,

  /// Windows that were scrolled out of the viewport of their workspace
  /// as of their last redraw, along with where they appeared (clamped
  /// just beyond the viewport's edge).
  scrolled_out: HashMap<Uuid, Rect>,

  /// Windows whose next redraw must not be animated (e.g. after an
  /// aborted animation).
  suppressed: HashSet<Uuid>,

  /// The displayed workspace of each monitor as of the last sync pass.
  /// Used to detect workspace switches.
  displayed_workspaces: HashMap<Uuid, Uuid>,

  /// Whether the current sync pass is the first one. Windows that exist
  /// when the WM starts are tiled without animating them.
  is_initial_pass: bool,

  /// Number of the current sync pass. Animations begun in the same pass
  /// form a group whose tweens start together.
  sync_pass: u64,

  /// Sequence number for the next animation.
  next_sequence: u64,

  /// Command sender for the animation worker. `None` if animations aren't
  /// supported on this platform or the worker failed to start.
  worker_tx: Option<UnboundedSender<WorkerCommand>>,

  /// Cloaks or uncloaks a window. Replaced in tests.
  cloak: CloakFn,
}

/// Function that cloaks (`true`) or uncloaks (`false`) a window.
type CloakFn = fn(WindowId, bool) -> anyhow::Result<()>;

impl Animations {
  /// Creates the animation engine, along with the receiver for updates
  /// from the animation worker.
  ///
  /// Updates must be applied via
  /// `WindowManager::process_animation_update`.
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
      slots: HashMap::new(),
      pending_open: HashSet::new(),
      cloaked_for_open: HashMap::new(),
      scrolled_out: HashMap::new(),
      suppressed: HashSet::new(),
      displayed_workspaces: HashMap::new(),
      is_initial_pass: true,
      sync_pass: 0,
      next_sequence: 0,
      worker_tx: None,
      cloak: set_cloaked,
    }
  }

  /// Whether animations are enabled in the config and supported with the
  /// current config and platform.
  fn is_available(&self, config: &UserConfig) -> bool {
    self.worker_tx.is_some()
      && config.value.general.animations.enabled
      && config.value.general.hide_method == HideMethod::Cloak
  }

  /// Logs a warning if animations are enabled in the config, but can't
  /// be used with it.
  pub fn warn_if_unsupported(config: &UserConfig) {
    let general = &config.value.general;

    if general.animations.enabled
      && general.hide_method != HideMethod::Cloak
    {
      tracing::warn!(
        "Window animations require `general.hide_method: 'cloak'` and \
         are disabled."
      );
    }
  }

  /// Marks a window to be animated when it's first shown. Used for the
  /// window open animation.
  ///
  /// Windows that exist when the WM starts aren't animated, so this is a
  /// no-op until the first sync pass has completed.
  pub fn mark_pending_open(
    &mut self,
    window: &WindowContainer,
    config: &UserConfig,
  ) {
    if self.displayed_workspaces.is_empty() {
      return;
    }

    let open_config = &config.value.general.animations.window_open;

    // Apps are allowed to ignore position updates while fullscreen (e.g.
    // games), so fullscreen windows aren't animated on open.
    let will_animate = self.is_available(config)
      && !matches!(
        window.state(),
        WindowState::Minimized | WindowState::Fullscreen(_)
      )
      && open_config.style != WindowOpenAnimationStyle::None
      && open_config.duration_ms > 0;

    if !will_animate {
      return;
    }

    // Cloak the window until its open animation starts, so that it can't
    // flash in place in the meantime.
    if let Err(err) = (self.cloak)(window.native().id(), true) {
      tracing::warn!(
        "Failed to cloak window for its open animation: {err}"
      );
      return;
    }

    self.pending_open.insert(window.id());
    self
      .cloaked_for_open
      .insert(window.id(), window.native().id());
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
    self.is_initial_pass = self.displayed_workspaces.is_empty();
    self.sync_pass += 1;

    let mut switches = HashMap::new();

    for monitor in monitors {
      let Some(workspace) = monitor.displayed_workspace() else {
        continue;
      };

      let workspace_id = workspace.id();

      let previous_id =
        self.displayed_workspaces.insert(monitor.id(), workspace_id);

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

  /// Decides how a window is animated during the redraw pass, and starts
  /// or redirects its animation accordingly.
  ///
  /// Must be called for every redrawn window, after its display state
  /// transitioned. Returns how the redraw pass must handle the window.
  pub fn plan_redraw(
    &mut self,
    window: &WindowContainer,
    input: &RedrawInput,
    config: &UserConfig,
  ) -> Ownership {
    let window_id = window.id();

    let scrolled_out_rect = if input.placement.is_scrolled_out {
      self
        .scrolled_out
        .insert(window_id, input.placement.rect.clone())
    } else {
      self.scrolled_out.remove(&window_id)
    };

    let is_opening = self.pending_open.remove(&window_id);
    let is_suppressed = self.suppressed.remove(&window_id);

    let can_animate = !is_suppressed
      && !self.is_initial_pass
      && self.is_available(config)
      && window.state() != WindowState::Minimized
      && window.active_drag().is_none();

    let plan_input = can_animate
      .then(|| {
        self.plan_input(window, input, is_opening, scrolled_out_rect)
      })
      .flatten();

    let Some(plan_input) = plan_input else {
      self.stop(window_id);
      return Ownership::None;
    };

    // A window that's being revealed is already shown at its previous
    // target. It's left alone unless it's going somewhere else.
    if let Some(slot) = self.slots.get(&window_id) {
      if slot.phase == SlotPhase::Revealing {
        if slot.target.to.rect == plan_input.target_rect
          && slot.target.end_visible == plan_input.after.is_visible
        {
          return Ownership::Deferred;
        }

        self.stop(window_id);
        return self.plan_redraw_with(window, input, config, is_opening);
      }
    }

    let plan =
      planner::plan(&plan_input, &config.value.general.animations);

    if self.slots.contains_key(&window_id) {
      return self.retarget(window_id, &plan_input, plan, input, config);
    }

    let Some(plan) = plan else {
      // A window that was cloaked for an open animation that doesn't
      // play gets its visibility applied by the redraw pass.
      self.cloaked_for_open.remove(&window_id);
      return Ownership::None;
    };

    self.start(window, plan, input, is_opening)
  }

  /// Plans a redraw for a window whose revealing animation was just
  /// stopped, from where the window now is.
  fn plan_redraw_with(
    &mut self,
    window: &WindowContainer,
    input: &RedrawInput,
    config: &UserConfig,
    is_opening: bool,
  ) -> Ownership {
    let Some(plan_input) =
      self.plan_input(window, input, is_opening, None)
    else {
      return Ownership::None;
    };

    match planner::plan(&plan_input, &config.value.general.animations) {
      Some(plan) => self.start(window, plan, input, is_opening),
      None => Ownership::None,
    }
  }

  /// Describes a window's transition for the planner.
  ///
  /// `scrolled_out_rect` is where the window appeared if it was scrolled
  /// out of view before the redraw. Returns `None` if the window has no
  /// workspace or monitor.
  fn plan_input(
    &self,
    window: &WindowContainer,
    input: &RedrawInput,
    is_opening: bool,
    scrolled_out_rect: Option<Rect>,
  ) -> Option<PlanInput> {
    let was_scrolled_out = scrolled_out_rect.is_some();

    let workspace = window.workspace()?;
    let monitor = window.monitor()?;

    let switch_role =
      input.switches.get(&monitor.id()).and_then(|switch| {
        if workspace.id() == switch.to {
          Some(SwitchRole::Entering {
            is_forward: switch.is_forward,
          })
        } else if workspace.id() == switch.from {
          Some(SwitchRole::Leaving {
            is_forward: switch.is_forward,
          })
        } else {
          None
        }
      });

    let slot = self.slots.get(&window.id());

    // An animated window is where its animation is heading. A window
    // that's opening was cloaked when it was managed, so it was never
    // visible.
    let (was_visible, current_rect) = match slot {
      Some(slot) => (slot.target.end_visible, slot.target.to.rect.clone()),
      None => (
        !is_opening
          && !was_scrolled_out
          && matches!(
            input.previous_display_state,
            DisplayState::Shown | DisplayState::Showing
          ),
        scrolled_out_rect.unwrap_or_else(|| positioned_frame(window)),
      ),
    };

    let will_be_visible = !input.placement.is_scrolled_out
      && matches!(
        window.display_state(),
        DisplayState::Shown | DisplayState::Showing
      );

    Some(PlanInput {
      is_opening,
      switch_role,
      before: Visibility {
        is_visible: was_visible,
        is_scrolled_out: was_scrolled_out,
      },
      after: Visibility {
        is_visible: will_be_visible,
        is_scrolled_out: input.placement.is_scrolled_out,
      },
      current_rect,
      target_rect: input.placement.rect.clone(),
      viewport: input.viewport.clone(),
      monitor_bounds: monitor.native_properties().bounds,
      monitor_work_area: monitor.native_properties().working_area,
    })
  }

  /// Starts a new animation for a window.
  fn start(
    &mut self,
    window: &WindowContainer,
    plan: planner::Plan,
    input: &RedrawInput,
    is_opening: bool,
  ) -> Ownership {
    let window_id = window.id();
    let handle = window.native().id();

    let (start, phase, ownership) = match plan.start {
      Start::Cover => {
        (StartFrame::Cover, SlotPhase::Covering, Ownership::Deferred)
      }
      Start::Hidden { from } => {
        // The window must already be hidden: windows that are opening
        // were cloaked when they were managed, and windows of hidden
        // workspaces or scrolled out of view are cloaked by the hide
        // method.
        if is_opening && !self.cloaked_for_open.contains_key(&window_id) {
          return Ownership::None;
        }

        // Windows that are parked off-screen are shown, so they're hidden
        // before being moved into place.
        let is_shown = !is_opening
          && matches!(
            input.previous_display_state,
            DisplayState::Shown | DisplayState::Showing
          );

        if is_shown {
          if let Err(err) = (self.cloak)(handle, true) {
            tracing::warn!("Failed to cloak window for animation: {err}");
            return Ownership::None;
          }
        }

        let start = StartFrame::Hidden {
          from,
          settle: input.placement.window_rect().clone(),
        };

        (start, SlotPhase::Running, Ownership::PositionOnly)
      }
    };

    // The animation takes over the cloak of an opening window.
    self.cloaked_for_open.remove(&window_id);

    let sequence = self.next_sequence;
    self.next_sequence += 1;

    tracing::debug!("Animating window: {window}");

    self.send(WorkerCommand::Begin(BeginRequest {
      window_id,
      sequence,
      handle,
      group: self.sync_pass,
      start,
      target: plan.target.clone(),
    }));

    let deferred_position =
      (phase == SlotPhase::Covering).then(|| DeferredPosition {
        rect: input.placement.window_rect().clone(),
        z_order: input.z_order.clone(),
      });

    let reveal_on_removal = !plan.target.end_visible && input.is_visible;

    self.slots.insert(
      window_id,
      Slot {
        sequence,
        handle,
        phase,
        target: plan.target,
        deferred_position,
        reveal_on_removal,
      },
    );

    ownership
  }

  /// Redirects the animation in flight for a window to its new
  /// placement.
  fn retarget(
    &mut self,
    window_id: Uuid,
    plan_input: &PlanInput,
    plan: Option<planner::Plan>,
    input: &RedrawInput,
    config: &UserConfig,
  ) -> Ownership {
    // Transitions that wouldn't be animated on their own (e.g. the
    // window being moved to a hidden workspace) still continue the
    // animation in flight, so that the window doesn't snap.
    let target = plan.map_or_else(
      || {
        let move_config = &config.value.general.animations.window_move;
        let rect = plan_input.target_rect.clone();

        Target {
          to: if plan_input.after.is_visible {
            Frame::opaque(rect)
          } else {
            Frame::transparent(rect)
          },
          clip: None,
          timing: Timing::from_config(
            move_config.duration_ms,
            &move_config.curve,
          ),
          end_visible: plan_input.after.is_visible,
        }
      },
      |plan| plan.target,
    );

    let Some(slot) = self.slots.get_mut(&window_id) else {
      return Ownership::None;
    };

    if slot.target.to != target.to
      || slot.target.end_visible != target.end_visible
    {
      tracing::debug!("Retargeting window animation (id={window_id}).");

      let sequence = slot.sequence;
      slot.target = target.clone();

      self.send(WorkerCommand::Retarget {
        window_id,
        sequence,
        target,
        window_rect: input.placement.window_rect().clone(),
      });
    }

    let Some(slot) = self.slots.get_mut(&window_id) else {
      return Ownership::None;
    };

    slot.reveal_on_removal = !slot.target.end_visible && input.is_visible;

    match slot.phase {
      SlotPhase::Covering => {
        slot.deferred_position = Some(DeferredPosition {
          rect: input.placement.window_rect().clone(),
          z_order: input.z_order.clone(),
        });

        Ownership::Deferred
      }
      SlotPhase::Running | SlotPhase::Revealing => Ownership::PositionOnly,
    }
  }

  /// Applies an update from the animation worker.
  ///
  /// `window` is the animated window, or `None` if it's no longer
  /// managed.
  pub fn apply_update(
    &mut self,
    update: AnimationUpdate,
    window: Option<&WindowContainer>,
  ) -> UpdateOutcome {
    let window_id = update.window_id();

    let Some(slot) = self.slots.get_mut(&window_id) else {
      return UpdateOutcome::default();
    };

    // Updates for replaced animations are ignored.
    if slot.sequence != update.sequence() {
      return UpdateOutcome::default();
    }

    if window.is_none() {
      self.stop(window_id);
      return UpdateOutcome::default();
    }

    match update {
      AnimationUpdate::Covered { sequence, .. } => {
        if slot.phase != SlotPhase::Covering {
          return UpdateOutcome::default();
        }

        // The thumbnail covers the window, so the window can be hidden
        // underneath it without anything changing on screen.
        if let Err(err) = (self.cloak)(slot.handle, true) {
          tracing::warn!("Failed to cloak animated window: {err}");
          return self.abort(window_id);
        }

        slot.phase = SlotPhase::Running;
        let position = slot.deferred_position.take();
        let needs_redraw = position.is_none();

        self.send(WorkerCommand::Release {
          window_id,
          sequence,
        });

        UpdateOutcome {
          position,
          needs_redraw,
        }
      }
      AnimationUpdate::Finished { sequence, .. } => {
        slot.phase = SlotPhase::Revealing;

        let cloak_result = (self.cloak)(slot.handle, false);

        self.send(WorkerCommand::Revealed {
          window_id,
          sequence,
        });

        if let Err(err) = cloak_result {
          tracing::warn!("Failed to reveal animated window: {err}");
          self.suppressed.insert(window_id);

          return UpdateOutcome {
            position: None,
            needs_redraw: true,
          };
        }

        UpdateOutcome::default()
      }
      AnimationUpdate::Removed { .. } => {
        let (handle, reveal_on_removal, end_visible) =
          (slot.handle, slot.reveal_on_removal, slot.target.end_visible);
        self.slots.remove(&window_id);

        // A window that was already hidden by its animation doesn't emit
        // a hidden event when its hide completes, so the hide is
        // finalized here.
        if let Some(window) = window {
          if !end_visible && window.display_state() == DisplayState::Hiding
          {
            window.set_display_state(DisplayState::Hidden);
          }
        }

        if reveal_on_removal {
          if let Err(err) = (self.cloak)(handle, false) {
            tracing::warn!("Failed to reveal parked window: {err}");
            self.suppressed.insert(window_id);

            return UpdateOutcome {
              position: None,
              needs_redraw: true,
            };
          }
        }

        UpdateOutcome::default()
      }
      AnimationUpdate::Aborted { .. } => self.abort(window_id),
    }
  }

  /// Stops the animation of a window and makes sure that its final
  /// state gets applied by a redraw without animating it.
  fn abort(&mut self, window_id: Uuid) -> UpdateOutcome {
    self.stop(window_id);
    self.suppressed.insert(window_id);

    UpdateOutcome {
      position: None,
      needs_redraw: true,
    }
  }

  /// Stops the animation of a window, if any, and removes its
  /// thumbnail. The window's visibility is left to the caller.
  fn stop(&mut self, window_id: Uuid) {
    self.cloaked_for_open.remove(&window_id);

    if self.slots.remove(&window_id).is_some() {
      self.send(WorkerCommand::Cancel { window_id });
    }
  }

  /// Stops the animation of a window, e.g. because the user started
  /// dragging it.
  ///
  /// Returns whether the window was animated, in which case it must be
  /// redrawn to apply its final state.
  pub fn interrupt(&mut self, window_id: Uuid) -> bool {
    if !self.slots.contains_key(&window_id) {
      return false;
    }

    self.abort(window_id);
    true
  }

  /// Stops animating a window that's no longer managed, and shows it if
  /// it was hidden by an animation (the window stays alive, e.g. when
  /// it's hidden to the tray).
  pub fn release_window(&mut self, window: &WindowContainer) {
    let window_id = window.id();
    let owned_cloak = self.owns_cloak(window_id);

    self.stop(window_id);
    self.pending_open.remove(&window_id);
    self.scrolled_out.remove(&window_id);
    self.suppressed.remove(&window_id);

    if owned_cloak {
      if let Err(err) = (self.cloak)(window.native().id(), false) {
        tracing::warn!("Failed to uncloak released window: {err}");
      }
    }
  }

  /// Whether the window is cloaked by an animation (or waiting for its
  /// open animation).
  ///
  /// Cloaking a window produces window events, which shouldn't be
  /// mistaken for the window being hidden externally (e.g. to the tray).
  pub fn owns_cloak(&self, window_id: Uuid) -> bool {
    self.slots.contains_key(&window_id)
      || self.cloaked_for_open.contains_key(&window_id)
  }

  /// Notifies the animation worker that the monitor setup changed.
  pub fn displays_changed(&self) {
    self.send(WorkerCommand::DisplaysChanged);
  }

  fn send(&self, command: WorkerCommand) {
    if let Some(worker_tx) = &self.worker_tx {
      let _ = worker_tx.send(command);
    }
  }
}

impl Drop for Animations {
  /// Shows windows that are hidden by an animation, so that they aren't
  /// left invisible when the WM exits.
  fn drop(&mut self) {
    let handles = self
      .slots
      .values()
      .map(|slot| slot.handle)
      .chain(self.cloaked_for_open.values().copied());

    for handle in handles {
      if let Err(err) = (self.cloak)(handle, false) {
        tracing::warn!("Failed to uncloak animated window: {err}");
      }
    }
  }
}

/// Current frame of a window, in the coordinates that windows are
/// positioned in (i.e. including the window's invisible borders).
///
/// `NativeWindowProperties::frame` is the window's visible frame, which
/// doesn't match the placement rects that windows are positioned at.
#[cfg(target_os = "windows")]
fn positioned_frame(window: &WindowContainer) -> Rect {
  wm_platform::window_frame(window.native().id().0)
    .unwrap_or_else(|| window.native_properties().frame)
}

/// Current frame of a window. Windows are positioned at their visible
/// frame on platforms other than Windows.
#[cfg(not(target_os = "windows"))]
fn positioned_frame(window: &WindowContainer) -> Rect {
  window.native_properties().frame
}

/// Cloaks or uncloaks the window with the given handle.
#[cfg(target_os = "windows")]
fn set_cloaked(handle: WindowId, cloaked: bool) -> anyhow::Result<()> {
  use wm_platform::{NativeWindow, NativeWindowWindowsExt};

  Ok(NativeWindow::from_handle(handle.0).set_cloaked(cloaked)?)
}

/// No-op on platforms without animations.
#[cfg(not(target_os = "windows"))]
#[allow(clippy::unnecessary_wraps)]
fn set_cloaked(_handle: WindowId, _cloaked: bool) -> anyhow::Result<()> {
  Ok(())
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::{
    commands::container::attach_container,
    models::{TilingWindow, Workspace},
    test_utils::mock_window_rect,
  };

  /// Animations with a worker channel that's read by the test, and a
  /// no-op cloak.
  fn animations() -> (Animations, UnboundedReceiver<WorkerCommand>) {
    let (worker_tx, worker_rx) = mpsc::unbounded_channel();

    let mut animations = Animations::disabled();
    animations.worker_tx = Some(worker_tx);
    animations.cloak = |_, _| Ok(());

    (animations, worker_rx)
  }

  fn config() -> UserConfig {
    let mut config = UserConfig::mock();
    config.value.general.animations.enabled = true;
    config.value.general.hide_method = HideMethod::Cloak;
    config
  }

  /// A shown tiling window on the displayed workspace of a monitor, with
  /// the initial sync pass already done.
  fn window(animations: &mut Animations) -> WindowContainer {
    let workspace = Workspace::mock().call();
    let window = TilingWindow::mock().call();

    attach_container(
      &window.clone().into(),
      &workspace.clone().into(),
      None,
    )
    .unwrap();

    let monitor = Monitor::mock().workspaces(vec![workspace]).call();

    animations.detect_switches(std::slice::from_ref(&monitor));
    animations.detect_switches(&[monitor]);

    window.into()
  }

  /// Redraws a shown window at the given rect.
  fn redraw(
    animations: &mut Animations,
    window: &WindowContainer,
    rect: Rect,
    config: &UserConfig,
  ) -> Ownership {
    animations.plan_redraw(
      window,
      &RedrawInput {
        placement: &WindowPlacement {
          rect,
          is_scrolled_out: false,
          parked_rect: None,
        },
        previous_display_state: &DisplayState::Shown,
        is_visible: true,
        switches: &HashMap::new(),
        z_order: &WindowZOrder::Normal,
        viewport: None,
      },
      config,
    )
  }

  fn moved_rect() -> Rect {
    mock_window_rect().translate_to_coordinates(500, 0)
  }

  fn commands(
    worker_rx: &mut UnboundedReceiver<WorkerCommand>,
  ) -> Vec<WorkerCommand> {
    std::iter::from_fn(|| worker_rx.try_recv().ok()).collect()
  }

  /// Starts a move animation and returns its sequence number.
  fn start_move(
    animations: &mut Animations,
    worker_rx: &mut UnboundedReceiver<WorkerCommand>,
    window: &WindowContainer,
  ) -> u64 {
    assert_eq!(
      redraw(animations, window, moved_rect(), &config()),
      Ownership::Deferred
    );

    let commands_sent = commands(worker_rx);
    let [WorkerCommand::Begin(begin)] = commands_sent.as_slice() else {
      panic!("expected a single begin command, got {commands_sent:?}");
    };

    assert!(matches!(begin.start, StartFrame::Cover));
    begin.sequence
  }

  #[test]
  fn skips_initial_sync_pass() {
    let (mut animations, mut worker_rx) = animations();

    let workspace = Workspace::mock().call();
    let window = TilingWindow::mock().call();
    attach_container(
      &window.clone().into(),
      &workspace.clone().into(),
      None,
    )
    .unwrap();
    let monitor = Monitor::mock().workspaces(vec![workspace]).call();

    animations.detect_switches(&[monitor]);

    assert_eq!(
      redraw(&mut animations, &window.into(), moved_rect(), &config()),
      Ownership::None
    );
    assert!(commands(&mut worker_rx).is_empty());
  }

  #[test]
  fn covers_moves_and_reveals_window() {
    let (mut animations, mut worker_rx) = animations();
    let window = window(&mut animations);

    let sequence = start_move(&mut animations, &mut worker_rx, &window);
    assert!(animations.owns_cloak(window.id()));

    // The window is positioned once its thumbnail covers it.
    let outcome = animations.apply_update(
      AnimationUpdate::Covered {
        window_id: window.id(),
        sequence,
      },
      Some(&window),
    );
    assert_eq!(
      outcome.position.map(|position| position.rect),
      Some(moved_rect())
    );
    assert!(matches!(
      commands(&mut worker_rx).as_slice(),
      [WorkerCommand::Release { .. }]
    ));

    // Redraws while the window is hidden only position it.
    assert_eq!(
      redraw(&mut animations, &window, moved_rect(), &config()),
      Ownership::PositionOnly
    );
    assert!(commands(&mut worker_rx).is_empty());

    animations.apply_update(
      AnimationUpdate::Finished {
        window_id: window.id(),
        sequence,
      },
      Some(&window),
    );
    assert!(matches!(
      commands(&mut worker_rx).as_slice(),
      [WorkerCommand::Revealed { .. }]
    ));

    // The redraw that the reveal triggers leaves the window alone.
    assert_eq!(
      redraw(&mut animations, &window, moved_rect(), &config()),
      Ownership::Deferred
    );
    assert!(commands(&mut worker_rx).is_empty());

    animations.apply_update(
      AnimationUpdate::Removed {
        window_id: window.id(),
        sequence,
      },
      Some(&window),
    );
    assert!(!animations.owns_cloak(window.id()));
  }

  #[test]
  fn retargets_animation_in_flight() {
    let (mut animations, mut worker_rx) = animations();
    let window = window(&mut animations);

    start_move(&mut animations, &mut worker_rx, &window);

    let retargeted_rect =
      mock_window_rect().translate_to_coordinates(800, 0);

    assert_eq!(
      redraw(&mut animations, &window, retargeted_rect.clone(), &config()),
      Ownership::Deferred
    );

    let commands_sent = commands(&mut worker_rx);
    let [WorkerCommand::Retarget { target, .. }] =
      commands_sent.as_slice()
    else {
      panic!("expected a single retarget command, got {commands_sent:?}");
    };
    assert_eq!(target.to.rect, retargeted_rect);
  }

  #[test]
  fn applies_final_state_after_abort() {
    let (mut animations, mut worker_rx) = animations();
    let window = window(&mut animations);

    let sequence = start_move(&mut animations, &mut worker_rx, &window);

    let outcome = animations.apply_update(
      AnimationUpdate::Aborted {
        window_id: window.id(),
        sequence,
      },
      Some(&window),
    );

    assert!(outcome.needs_redraw);
    assert!(!animations.owns_cloak(window.id()));

    // The redraw that applies the final state isn't animated.
    assert_eq!(
      redraw(&mut animations, &window, moved_rect(), &config()),
      Ownership::None
    );
  }

  #[test]
  fn ignores_updates_for_replaced_animations() {
    let (mut animations, mut worker_rx) = animations();
    let window = window(&mut animations);

    let sequence = start_move(&mut animations, &mut worker_rx, &window);

    let outcome = animations.apply_update(
      AnimationUpdate::Covered {
        window_id: window.id(),
        sequence: sequence + 1,
      },
      Some(&window),
    );

    assert!(outcome.position.is_none());
    assert!(commands(&mut worker_rx).is_empty());
  }

  #[test]
  fn requires_cloak_hide_method() {
    let (mut animations, mut worker_rx) = animations();
    let window = window(&mut animations);

    let mut config = config();
    config.value.general.hide_method = HideMethod::Hide;

    assert_eq!(
      redraw(&mut animations, &window, moved_rect(), &config),
      Ownership::None
    );
    assert!(commands(&mut worker_rx).is_empty());
  }

  thread_local! {
    /// Cloak calls made by `recording_cloak`, as `cloaked` values.
    static CLOAK_CALLS: std::cell::RefCell<Vec<bool>> =
      const { std::cell::RefCell::new(Vec::new()) };
  }

  #[allow(clippy::unnecessary_wraps)]
  fn recording_cloak(
    _handle: WindowId,
    cloaked: bool,
  ) -> anyhow::Result<()> {
    CLOAK_CALLS.with(|calls| calls.borrow_mut().push(cloaked));
    Ok(())
  }

  #[test]
  fn reveals_parked_window_after_scrolling_it_out() {
    let (mut animations, mut worker_rx) = animations();
    animations.cloak = recording_cloak;
    let window = window(&mut animations);

    let clamped_rect =
      mock_window_rect().translate_to_coordinates(2000, 0);
    let parked_rect = mock_window_rect().translate_to_coordinates(9000, 0);

    let ownership = animations.plan_redraw(
      &window,
      &RedrawInput {
        placement: &WindowPlacement {
          rect: clamped_rect,
          is_scrolled_out: true,
          parked_rect: Some(parked_rect.clone()),
        },
        previous_display_state: &DisplayState::Shown,
        is_visible: true,
        switches: &HashMap::new(),
        z_order: &WindowZOrder::Normal,
        viewport: None,
      },
      &config(),
    );
    assert_eq!(ownership, Ownership::Deferred);

    let commands_sent = commands(&mut worker_rx);
    let [WorkerCommand::Begin(begin)] = commands_sent.as_slice() else {
      panic!("expected a single begin command, got {commands_sent:?}");
    };
    assert!(!begin.target.end_visible);
    let sequence = begin.sequence;

    // The window is hidden underneath its thumbnail and moved to where
    // it's parked.
    let outcome = animations.apply_update(
      AnimationUpdate::Covered {
        window_id: window.id(),
        sequence,
      },
      Some(&window),
    );
    assert_eq!(
      outcome.position.map(|position| position.rect),
      Some(parked_rect)
    );

    // Once the thumbnail has slid out of view, the parked window is shown
    // again, so that it stays listed in Alt+Tab.
    animations.apply_update(
      AnimationUpdate::Removed {
        window_id: window.id(),
        sequence,
      },
      Some(&window),
    );

    assert_eq!(
      CLOAK_CALLS.with(std::cell::RefCell::take),
      vec![true, false]
    );
  }

  #[test]
  fn releases_window_unmanaged_mid_animation() {
    let (mut animations, mut worker_rx) = animations();
    let window = window(&mut animations);

    start_move(&mut animations, &mut worker_rx, &window);

    animations.release_window(&window);

    assert!(!animations.owns_cloak(window.id()));
    assert!(matches!(
      commands(&mut worker_rx).as_slice(),
      [WorkerCommand::Cancel { .. }]
    ));
  }
}
