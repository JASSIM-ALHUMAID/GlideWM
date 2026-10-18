//! Decides how a window is animated when it's redrawn.
//!
//! The planner is pure: it maps a window's transition during a redraw
//! (where it was, where it's going, whether it's becoming visible or
//! hidden) to the animation that plays for it.

use wm_common::{
  AnimationsConfig, WindowOpenAnimationStyle,
  WorkspaceSwitchAnimationStyle,
};
use wm_platform::Rect;

use super::{
  geometry::{
    beyond_edge, intersection, scale_about_center, SlideEdge, POPIN_SCALE,
  },
  timeline::{Frame, Timing},
};

/// Distance in pixels below which a change of a window's position or size
/// isn't animated. Such changes come from rounding (e.g. a window's
/// shadow borders being measured after it's first positioned), and
/// animating them would only flicker the window.
const MIN_MOVE_DISTANCE_PX: i32 = 4;

/// Role of a window in a switch of the displayed workspace on its
/// monitor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SwitchRole {
  /// The window's workspace is becoming displayed.
  Entering {
    /// Whether the switch moved forward in the monitor's workspace
    /// order.
    is_forward: bool,
  },

  /// The window's workspace stops being displayed.
  Leaving {
    /// Whether the switch moved forward in the monitor's workspace
    /// order.
    is_forward: bool,
  },
}

/// Whether a window is visible at a point in time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Visibility {
  /// Whether the window is visible.
  pub is_visible: bool,

  /// Whether the window is scrolled out of the viewport of its
  /// (scrolling) workspace, which hides it.
  pub is_scrolled_out: bool,
}

/// A window's transition during a redraw.
#[derive(Clone, Debug)]
pub struct PlanInput {
  /// Whether the window is shown for the first time (e.g. an
  /// application that's launched).
  pub is_opening: bool,

  /// Role of the window in a workspace switch on its monitor, if any.
  pub switch_role: Option<SwitchRole>,

  /// Visibility of the window before the redraw.
  pub before: Visibility,

  /// Visibility of the window after the redraw.
  pub after: Visibility,

  /// Where the window is before the redraw.
  pub current_rect: Rect,

  /// Where the window is positioned by the redraw.
  pub target_rect: Rect,

  /// Bounds of the window's monitor.
  pub monitor_bounds: Rect,

  /// Working area of the window's monitor (its bounds minus the taskbar
  /// and app bars).
  pub monitor_work_area: Rect,

  /// Viewport of the window's workspace, which windows scrolling into or
  /// out of view are clipped to. Defaults to the monitor bounds.
  pub viewport: Option<Rect>,
}

/// How an animation starts.
#[derive(Clone, Debug, PartialEq)]
pub enum Start {
  /// The window is visible: its thumbnail covers it at its current frame
  /// before the window is hidden, and the animation starts from there.
  Cover,

  /// The window is hidden: its thumbnail starts at the given frame.
  Hidden { from: Frame },
}

/// Where an animation ends.
#[derive(Clone, Debug)]
pub struct Target {
  /// Frame that the animation ends at.
  pub to: Frame,

  /// Rect that the window's thumbnail is clipped to while animating
  /// (e.g. the monitor that it slides in on).
  pub clip: Option<Rect>,

  pub timing: Timing,

  /// Whether the window is visible once the animation ends.
  pub end_visible: bool,
}

/// An animation for a window.
#[derive(Clone, Debug)]
pub struct Plan {
  pub start: Start,
  pub target: Target,
}

/// Plans the animation for a window's transition during a redraw.
///
/// Returns `None` if the transition isn't animated.
#[must_use]
pub fn plan(input: &PlanInput, config: &AnimationsConfig) -> Option<Plan> {
  if input.is_opening && input.after.is_visible {
    return plan_open(input, config);
  }

  match input.switch_role {
    Some(SwitchRole::Entering { is_forward })
      if !input.before.is_visible && input.after.is_visible =>
    {
      return plan_switch(input, config, is_forward, true);
    }
    Some(SwitchRole::Leaving { is_forward })
      if input.before.is_visible && !input.after.is_visible =>
    {
      return plan_switch(input, config, is_forward, false);
    }
    _ => {}
  }

  plan_move(input, config)
}

/// Rect that windows sliding onto or off the monitor are clipped to.
///
/// Windows are clipped to the monitor's working area, so that they pass
/// underneath the taskbar and app bars like real windows would. A window
/// whose own rect covers more of the monitor (e.g. a fullscreen window)
/// is clipped to its rect instead.
fn monitor_clip(input: &PlanInput) -> Rect {
  let area = input.monitor_work_area.union(&input.target_rect);

  intersection(&area, &input.monitor_bounds)
    .unwrap_or_else(|| input.monitor_bounds.clone())
}

/// Whether two rects differ by more than the minimum move distance.
fn is_moved(from: &Rect, to: &Rect) -> bool {
  (from.left - to.left).abs() > MIN_MOVE_DISTANCE_PX
    || (from.top - to.top).abs() > MIN_MOVE_DISTANCE_PX
    || (from.right - to.right).abs() > MIN_MOVE_DISTANCE_PX
    || (from.bottom - to.bottom).abs() > MIN_MOVE_DISTANCE_PX
}

/// The window open animation.
fn plan_open(
  input: &PlanInput,
  config: &AnimationsConfig,
) -> Option<Plan> {
  let open_config = &config.window_open;

  if open_config.duration_ms == 0 {
    return None;
  }

  let target = &input.target_rect;

  let from = match open_config.style {
    WindowOpenAnimationStyle::None => return None,
    WindowOpenAnimationStyle::Slide => Frame::opaque(beyond_edge(
      target,
      SlideEdge::Right,
      &input.monitor_bounds,
    )),
    WindowOpenAnimationStyle::SlideVert => Frame::opaque(beyond_edge(
      target,
      SlideEdge::Bottom,
      &input.monitor_bounds,
    )),
    WindowOpenAnimationStyle::Popin => {
      Frame::transparent(scale_about_center(target, POPIN_SCALE))
    }
    WindowOpenAnimationStyle::Fade => Frame::transparent(target.clone()),
  };

  Some(Plan {
    start: Start::Hidden { from },
    target: Target {
      to: Frame::opaque(target.clone()),
      clip: Some(monitor_clip(input)),
      timing: Timing::from_config(
        open_config.duration_ms,
        &open_config.curve,
      ),
      end_visible: true,
    },
  })
}

/// The workspace switch animation, for a window that's entering or
/// leaving the displayed workspace.
fn plan_switch(
  input: &PlanInput,
  config: &AnimationsConfig,
  is_forward: bool,
  is_entering: bool,
) -> Option<Plan> {
  let switch_config = &config.workspace_switch;

  if switch_config.duration_ms == 0 {
    return None;
  }

  let edge = SlideEdge::for_switch(
    switch_config.style,
    switch_config.direction,
    is_forward,
  );

  // Frame of the window while it's off the monitor (or faded out). The
  // incoming workspace comes in from `edge`, and the outgoing workspace
  // leaves through the opposite edge.
  let offscreen = match switch_config.style {
    WorkspaceSwitchAnimationStyle::None => return None,
    WorkspaceSwitchAnimationStyle::Slide
    | WorkspaceSwitchAnimationStyle::SlideVert => {
      let edge = if is_entering { edge } else { edge.opposite() };

      Frame::opaque(beyond_edge(
        &input.target_rect,
        edge,
        &input.monitor_bounds,
      ))
    }
    WorkspaceSwitchAnimationStyle::Fade => {
      Frame::transparent(input.target_rect.clone())
    }
  };

  let timing =
    Timing::from_config(switch_config.duration_ms, &switch_config.curve);

  let clip = Some(monitor_clip(input));

  Some(if is_entering {
    Plan {
      start: Start::Hidden { from: offscreen },
      target: Target {
        to: Frame::opaque(input.target_rect.clone()),
        clip,
        timing,
        end_visible: true,
      },
    }
  } else {
    Plan {
      start: Start::Cover,
      target: Target {
        to: offscreen,
        clip,
        timing,
        end_visible: false,
      },
    }
  })
}

/// The window move animation, for a window that stays on its displayed
/// workspace but changes position or size, or is scrolled into or out
/// of the viewport of a scrolling workspace.
fn plan_move(
  input: &PlanInput,
  config: &AnimationsConfig,
) -> Option<Plan> {
  let move_config = &config.window_move;

  if !move_config.enabled || move_config.duration_ms == 0 {
    return None;
  }

  let timing =
    Timing::from_config(move_config.duration_ms, &move_config.curve);

  // Windows scrolling into or out of view disappear at the edge of the
  // viewport, like the windows that stay in it are cut off there.
  let viewport = input
    .viewport
    .clone()
    .unwrap_or_else(|| input.monitor_bounds.clone());

  // Scrolled out of view: slide to just beyond the viewport, then hide.
  if input.before.is_visible
    && !input.after.is_visible
    && input.after.is_scrolled_out
  {
    return Some(Plan {
      start: Start::Cover,
      target: Target {
        to: Frame::opaque(input.target_rect.clone()),
        clip: Some(viewport.clone()),
        timing,
        end_visible: false,
      },
    });
  }

  // Scrolled into view: slide in from just beyond the viewport (where
  // scrolled-out windows are parked while hidden).
  if !input.before.is_visible
    && input.after.is_visible
    && input.before.is_scrolled_out
  {
    return Some(Plan {
      start: Start::Hidden {
        from: Frame::opaque(input.current_rect.clone()),
      },
      target: Target {
        to: Frame::opaque(input.target_rect.clone()),
        clip: Some(viewport.clone()),
        timing,
        end_visible: true,
      },
    });
  }

  // Moved or resized while staying visible. Windows that weren't on the
  // monitor (e.g. minimized windows, which are parked far off-screen, or
  // windows coming from another monitor) snap into place instead of
  // flying across the screen.
  let is_on_monitor =
    input.current_rect.intersection_area(&input.monitor_bounds) > 0;

  if input.before.is_visible
    && input.after.is_visible
    && is_on_monitor
    && is_moved(&input.current_rect, &input.target_rect)
  {
    return Some(Plan {
      start: Start::Cover,
      target: Target {
        to: Frame::opaque(input.target_rect.clone()),
        clip: None,
        timing,
        end_visible: true,
      },
    });
  }

  None
}

#[cfg(test)]
mod tests {
  use wm_common::{
    AnimationsConfig, WindowOpenAnimationStyle,
    WorkspaceSwitchAnimationStyle,
  };

  use super::*;

  fn monitor() -> Rect {
    Rect::from_xy(0, 0, 1920, 1080)
  }

  fn config() -> AnimationsConfig {
    AnimationsConfig {
      enabled: true,
      ..AnimationsConfig::default()
    }
  }

  fn visible() -> Visibility {
    Visibility {
      is_visible: true,
      is_scrolled_out: false,
    }
  }

  fn hidden() -> Visibility {
    Visibility {
      is_visible: false,
      is_scrolled_out: false,
    }
  }

  fn scrolled_out() -> Visibility {
    Visibility {
      is_visible: false,
      is_scrolled_out: true,
    }
  }

  /// A visible window that stays put.
  fn input() -> PlanInput {
    PlanInput {
      is_opening: false,
      switch_role: None,
      before: visible(),
      after: visible(),
      current_rect: Rect::from_xy(0, 0, 960, 1080),
      target_rect: Rect::from_xy(0, 0, 960, 1080),
      monitor_bounds: monitor(),
      monitor_work_area: Rect::from_xy(0, 40, 1920, 1000),
      viewport: None,
    }
  }

  #[test]
  fn ignores_unchanged_windows() {
    assert!(plan(&input(), &config()).is_none());
  }

  #[test]
  fn opens_windows_from_off_screen() {
    let plan = plan(
      &PlanInput {
        is_opening: true,
        before: hidden(),
        ..input()
      },
      &config(),
    )
    .unwrap();

    let Start::Hidden { from } = plan.start else {
      panic!("open animations start hidden");
    };

    assert!(from.rect.left >= monitor().right);
    assert_eq!(plan.target.to, Frame::opaque(input().target_rect));
    assert!(plan.target.end_visible);
  }

  #[test]
  fn opens_windows_with_popin_and_fade() {
    let mut config = config();
    config.window_open.style = WindowOpenAnimationStyle::Popin;

    let opening = PlanInput {
      is_opening: true,
      ..input()
    };

    let Start::Hidden { from } = plan(&opening, &config).unwrap().start
    else {
      panic!("open animations start hidden");
    };

    assert!(from.rect.width() < opening.target_rect.width());
    assert!(from.opacity < f64::EPSILON);

    config.window_open.style = WindowOpenAnimationStyle::Fade;

    let Start::Hidden { from } = plan(&opening, &config).unwrap().start
    else {
      panic!("open animations start hidden");
    };

    assert_eq!(from, Frame::transparent(opening.target_rect.clone()));

    config.window_open.style = WindowOpenAnimationStyle::None;
    assert!(plan(&opening, &config).is_none());
  }

  #[test]
  fn slides_in_entering_workspace() {
    let plan = plan(
      &PlanInput {
        switch_role: Some(SwitchRole::Entering { is_forward: true }),
        before: hidden(),
        ..input()
      },
      &config(),
    )
    .unwrap();

    let Start::Hidden { from } = plan.start else {
      panic!("entering windows start hidden");
    };

    // Forward switches slide in from the right.
    assert!(from.rect.left >= monitor().right);
    assert!(plan.target.end_visible);
  }

  #[test]
  fn slides_out_leaving_workspace() {
    let plan = plan(
      &PlanInput {
        switch_role: Some(SwitchRole::Leaving { is_forward: true }),
        after: hidden(),
        ..input()
      },
      &config(),
    )
    .unwrap();

    assert_eq!(plan.start, Start::Cover);

    // The outgoing workspace leaves through the opposite (left) edge.
    assert!(plan.target.to.rect.right <= monitor().left);
    assert!(!plan.target.end_visible);
  }

  #[test]
  fn cross_fades_workspaces() {
    let mut config = config();
    config.workspace_switch.style = WorkspaceSwitchAnimationStyle::Fade;

    let leaving = plan(
      &PlanInput {
        switch_role: Some(SwitchRole::Leaving { is_forward: false }),
        after: hidden(),
        ..input()
      },
      &config,
    )
    .unwrap();

    assert_eq!(leaving.target.to, Frame::transparent(input().target_rect));

    config.workspace_switch.style = WorkspaceSwitchAnimationStyle::None;

    assert!(plan(
      &PlanInput {
        switch_role: Some(SwitchRole::Leaving { is_forward: false }),
        after: hidden(),
        ..input()
      },
      &config,
    )
    .is_none());
  }

  #[test]
  fn moves_repositioned_windows() {
    let plan = plan(
      &PlanInput {
        target_rect: Rect::from_xy(960, 0, 960, 1080),
        ..input()
      },
      &config(),
    )
    .unwrap();

    assert_eq!(plan.start, Start::Cover);
    assert_eq!(
      plan.target.to,
      Frame::opaque(Rect::from_xy(960, 0, 960, 1080))
    );
    assert!(plan.target.end_visible);
  }

  #[test]
  fn ignores_tiny_moves() {
    assert!(plan(
      &PlanInput {
        target_rect: Rect::from_xy(2, 0, 956, 1078),
        ..input()
      },
      &config(),
    )
    .is_none());
  }

  #[test]
  fn clips_slides_to_work_area() {
    let plan = plan(
      &PlanInput {
        switch_role: Some(SwitchRole::Entering { is_forward: true }),
        before: hidden(),
        target_rect: Rect::from_xy(0, 40, 960, 1000),
        ..input()
      },
      &config(),
    )
    .unwrap();

    // Tiled windows slide underneath the bar above the work area.
    assert_eq!(plan.target.clip, Some(Rect::from_xy(0, 40, 1920, 1000)));

    // Fullscreen windows cover the whole monitor while sliding.
    let plan = plan_fullscreen_switch();
    assert_eq!(plan.target.clip, Some(monitor()));
  }

  fn plan_fullscreen_switch() -> Plan {
    plan(
      &PlanInput {
        switch_role: Some(SwitchRole::Entering { is_forward: true }),
        before: hidden(),
        target_rect: monitor(),
        ..input()
      },
      &config(),
    )
    .unwrap()
  }

  #[test]
  fn snaps_windows_coming_from_off_monitor() {
    // E.g. a window that's restored from minimized.
    assert!(plan(
      &PlanInput {
        current_rect: Rect::from_xy(-32000, -32000, 160, 28),
        ..input()
      },
      &config(),
    )
    .is_none());
  }

  #[test]
  fn respects_disabled_move_animation() {
    let mut config = config();
    config.window_move.enabled = false;

    assert!(plan(
      &PlanInput {
        target_rect: Rect::from_xy(960, 0, 960, 1080),
        ..input()
      },
      &config,
    )
    .is_none());
  }

  #[test]
  fn slides_windows_out_of_viewport() {
    let plan = plan(
      &PlanInput {
        after: scrolled_out(),
        target_rect: Rect::from_xy(1920, 0, 960, 1080),
        ..input()
      },
      &config(),
    )
    .unwrap();

    assert_eq!(plan.start, Start::Cover);
    assert_eq!(plan.target.to.rect, Rect::from_xy(1920, 0, 960, 1080));
    assert!(!plan.target.end_visible);
  }

  #[test]
  fn slides_windows_into_viewport() {
    let plan = plan(
      &PlanInput {
        before: scrolled_out(),
        current_rect: Rect::from_xy(1920, 0, 960, 1080),
        target_rect: Rect::from_xy(960, 0, 960, 1080),
        ..input()
      },
      &config(),
    )
    .unwrap();

    assert_eq!(
      plan.start,
      Start::Hidden {
        from: Frame::opaque(Rect::from_xy(1920, 0, 960, 1080))
      }
    );
    assert!(plan.target.end_visible);
  }

  #[test]
  fn ignores_windows_hidden_without_switch() {
    // E.g. a window that's moved to a workspace that isn't displayed.
    assert!(plan(
      &PlanInput {
        after: hidden(),
        ..input()
      },
      &config(),
    )
    .is_none());
  }
}
