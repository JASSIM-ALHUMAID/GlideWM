//! Tweening of animation frames over time.

use std::time::{Duration, Instant};

use wm_common::AnimationCurve;
use wm_platform::Rect;

use super::bezier::CubicBezier;

/// What an animated window looks like at a point in time.
#[derive(Clone, Debug, PartialEq)]
pub struct Frame {
  /// Where the window is drawn, in screen coordinates.
  pub rect: Rect,

  /// Opacity of the window, from transparent (`0.0`) to opaque (`1.0`).
  pub opacity: f64,
}

impl Frame {
  /// A fully opaque frame at the given rect.
  #[must_use]
  pub fn opaque(rect: Rect) -> Self {
    Self { rect, opacity: 1.0 }
  }

  /// A fully transparent frame at the given rect.
  #[must_use]
  pub fn transparent(rect: Rect) -> Self {
    Self { rect, opacity: 0.0 }
  }

  /// Interpolates between this frame and `to`, where `progress` is
  /// `0.0` at this frame and `1.0` at `to`.
  #[must_use]
  pub fn lerp(&self, to: &Frame, progress: f64) -> Frame {
    let lerp = |start: i32, end: i32| {
      let value =
        f64::from(start) + (f64::from(end) - f64::from(start)) * progress;

      // The interpolated values are between two screen coordinates, so
      // the cast can't truncate.
      #[allow(clippy::cast_possible_truncation)]
      (value.round() as i32)
    };

    Frame {
      rect: Rect::from_ltrb(
        lerp(self.rect.left, to.rect.left),
        lerp(self.rect.top, to.rect.top),
        lerp(self.rect.right, to.rect.right),
        lerp(self.rect.bottom, to.rect.bottom),
      ),
      opacity: (self.opacity + (to.opacity - self.opacity) * progress)
        .clamp(0.0, 1.0),
    }
  }
}

/// Duration and easing of an animation.
#[derive(Clone, Copy, Debug)]
pub struct Timing {
  pub duration: Duration,
  pub curve: CubicBezier,
}

impl Timing {
  /// Timing from the duration and curve of an animation config.
  #[must_use]
  pub fn from_config(duration_ms: u32, curve: &AnimationCurve) -> Self {
    Self {
      duration: Duration::from_millis(u64::from(duration_ms)),
      curve: CubicBezier::from(curve),
    }
  }
}

/// Interpolation from one frame to another over time.
#[derive(Clone, Debug)]
pub struct Tween {
  from: Frame,
  to: Frame,
  started_at: Instant,
  timing: Timing,
}

impl Tween {
  #[must_use]
  pub fn new(
    from: Frame,
    to: Frame,
    started_at: Instant,
    timing: Timing,
  ) -> Self {
    Self {
      from,
      to,
      started_at,
      timing,
    }
  }

  /// Eased progress at the given time, from `0.0` to `1.0`.
  ///
  /// The eased value is clamped to the tween's endpoints: an overshoot
  /// would read as the window bouncing off its target.
  fn progress(&self, now: Instant) -> f64 {
    let duration = self.timing.duration.as_secs_f64();

    if duration <= 0.0 {
      return 1.0;
    }

    let elapsed = now.saturating_duration_since(self.started_at);
    let linear = elapsed.as_secs_f64() / duration;

    self.timing.curve.ease(linear).clamp(0.0, 1.0)
  }

  /// The frame at the given time.
  #[must_use]
  pub fn sample(&self, now: Instant) -> Frame {
    if self.is_finished(now) {
      return self.to.clone();
    }

    self.from.lerp(&self.to, self.progress(now))
  }

  /// Whether the tween reached its target frame at the given time.
  #[must_use]
  pub fn is_finished(&self, now: Instant) -> bool {
    now.saturating_duration_since(self.started_at) >= self.timing.duration
  }

  /// Redirects the tween to a new target frame, continuing from the
  /// frame at the given time so that the change is seamless.
  pub fn retarget(&mut self, now: Instant, to: Frame, timing: Timing) {
    self.from = self.sample(now);
    self.to = to;
    self.started_at = now;
    self.timing = timing;
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  fn linear_timing(duration_ms: u64) -> Timing {
    Timing {
      duration: Duration::from_millis(duration_ms),
      curve: CubicBezier::new(0.0, 0.0, 1.0, 1.0),
    }
  }

  fn frame(x: i32, opacity: f64) -> Frame {
    Frame {
      rect: Rect::from_xy(x, 0, 100, 100),
      opacity,
    }
  }

  #[test]
  fn samples_endpoints_and_midpoint() {
    let start = Instant::now();
    let tween = Tween::new(
      frame(0, 0.0),
      frame(200, 1.0),
      start,
      linear_timing(100),
    );

    assert_eq!(tween.sample(start), frame(0, 0.0));
    assert_eq!(
      tween.sample(start + Duration::from_millis(50)),
      frame(100, 0.5)
    );
    assert_eq!(
      tween.sample(start + Duration::from_millis(100)),
      frame(200, 1.0)
    );
    assert!(!tween.is_finished(start + Duration::from_millis(99)));
    assert!(tween.is_finished(start + Duration::from_millis(100)));
  }

  #[test]
  fn clamps_overshooting_curves() {
    let start = Instant::now();
    let timing = Timing {
      duration: Duration::from_millis(100),
      curve: CubicBezier::new(0.3, 1.8, 0.6, 1.4),
    };
    let tween = Tween::new(frame(0, 1.0), frame(100, 1.0), start, timing);

    for ms in 0..=100 {
      let x = tween.sample(start + Duration::from_millis(ms)).rect.x();
      assert!((0..=100).contains(&x), "x={x} at {ms}ms");
    }
  }

  #[test]
  fn retargets_without_jumping() {
    let start = Instant::now();
    let mut tween = Tween::new(
      frame(0, 1.0),
      frame(200, 1.0),
      start,
      linear_timing(100),
    );

    let retarget_at = start + Duration::from_millis(50);
    let before = tween.sample(retarget_at);

    tween.retarget(retarget_at, frame(400, 1.0), linear_timing(100));

    // The retargeted tween continues from where the window was.
    assert_eq!(tween.sample(retarget_at), before);
    assert_eq!(
      tween.sample(retarget_at + Duration::from_millis(100)),
      frame(400, 1.0)
    );
  }

  #[test]
  fn zero_duration_finishes_immediately() {
    let start = Instant::now();
    let tween =
      Tween::new(frame(0, 0.0), frame(200, 1.0), start, linear_timing(0));

    assert!(tween.is_finished(start));
    assert_eq!(tween.sample(start), frame(200, 1.0));
  }
}
