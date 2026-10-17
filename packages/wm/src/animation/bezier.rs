//! Cubic bezier easing.
//!
//! Mirrors the CSS `cubic-bezier()` timing function: the curve runs from
//! `(0, 0)` to `(1, 1)` with two control points `(x1, y1)` and
//! `(x2, y2)`. `x` is the progress through the animation and `y` is the
//! eased value. `y` values outside `0..=1` allow the animation to
//! overshoot its target.

use wm_common::AnimationCurve;

/// Number of iterations of the Newton-Raphson root find.
const NEWTON_ITERATIONS: u32 = 8;

/// Number of iterations of the bisection fallback root find.
const BISECTION_ITERATIONS: u32 = 32;

/// Precision of the root find.
const SOLVE_EPSILON: f64 = 1e-7;

/// A cubic bezier easing curve.
#[derive(Clone, Copy, Debug)]
pub struct CubicBezier {
  x1: f64,
  y1: f64,
  x2: f64,
  y2: f64,
}

impl CubicBezier {
  #[must_use]
  pub const fn new(x1: f64, y1: f64, x2: f64, y2: f64) -> Self {
    Self { x1, y1, x2, y2 }
  }

  /// Evaluates the curve at the given progress (`0.0` to `1.0`).
  ///
  /// Values outside the range are clamped to `0.0` / `1.0`. The eased
  /// value may overshoot beyond `1.0` for curves with `y` control points
  /// above `1.0`.
  #[must_use]
  pub fn ease(&self, progress: f64) -> f64 {
    if progress <= 0.0 {
      return 0.0;
    }

    if progress >= 1.0 {
      return 1.0;
    }

    self.sample_y(self.solve_t(progress))
  }

  /// Finds the curve parameter `t` at which `x(t) == progress`.
  ///
  /// Uses the same approach as browsers: a few Newton-Raphson iterations,
  /// with a bisection fallback for curves where the derivative is near
  /// zero.
  fn solve_t(&self, progress: f64) -> f64 {
    let mut t = progress;

    for _ in 0..NEWTON_ITERATIONS {
      let error = self.sample_x(t) - progress;

      if error.abs() < SOLVE_EPSILON {
        return t;
      }

      let derivative = self.sample_dx(t);

      if derivative.abs() < SOLVE_EPSILON {
        break;
      }

      t -= error / derivative;
    }

    let mut low = 0.0;
    let mut high = 1.0;
    let mut t = progress.clamp(0.0, 1.0);

    for _ in 0..BISECTION_ITERATIONS {
      let sampled = self.sample_x(t);

      if (sampled - progress).abs() < SOLVE_EPSILON {
        return t;
      }

      if sampled < progress {
        low = t;
      } else {
        high = t;
      }

      t = f64::midpoint(low, high);
    }

    t
  }

  /// Samples the curve's progress (`x`) at parameter `t`.
  fn sample_x(&self, t: f64) -> f64 {
    cubic_bezier(0.0, self.x1, self.x2, 1.0, t)
  }

  /// Samples the curve's eased value (`y`) at parameter `t`.
  fn sample_y(&self, t: f64) -> f64 {
    cubic_bezier(0.0, self.y1, self.y2, 1.0, t)
  }

  /// Samples the derivative of the curve's progress (`x`) at parameter
  /// `t`.
  fn sample_dx(&self, t: f64) -> f64 {
    cubic_bezier_derivative(0.0, self.x1, self.x2, 1.0, t)
  }
}

impl From<&AnimationCurve> for CubicBezier {
  fn from(curve: &AnimationCurve) -> Self {
    Self::new(curve.x1, curve.y1, curve.x2, curve.y2)
  }
}

/// Evaluates a cubic bezier with the given control points at parameter
/// `t`.
fn cubic_bezier(p0: f64, p1: f64, p2: f64, p3: f64, t: f64) -> f64 {
  let u = 1.0 - t;

  u * u * u * p0
    + 3.0 * u * u * t * p1
    + 3.0 * u * t * t * p2
    + t * t * t * p3
}

/// Evaluates the derivative of a cubic bezier at parameter `t`.
fn cubic_bezier_derivative(
  p0: f64,
  p1: f64,
  p2: f64,
  p3: f64,
  t: f64,
) -> f64 {
  let u = 1.0 - t;

  3.0 * u * u * (p1 - p0)
    + 6.0 * u * t * (p2 - p1)
    + 3.0 * t * t * (p3 - p2)
}

#[cfg(test)]
mod tests {
  use super::*;

  const EPSILON: f64 = 1e-4;

  #[test]
  fn endpoints_are_exact() {
    let curve = CubicBezier::from(&AnimationCurve::default());

    assert_eq!(curve.ease(0.0), 0.0);
    assert_eq!(curve.ease(-0.5), 0.0);
    assert_eq!(curve.ease(1.0), 1.0);
    assert_eq!(curve.ease(1.5), 1.0);
  }

  #[test]
  fn linear_curve_is_identity() {
    let curve = CubicBezier::new(0.0, 0.0, 1.0, 1.0);

    for i in 0..=10 {
      let progress = f64::from(i) / 10.0;
      assert!((curve.ease(progress) - progress).abs() < EPSILON);
    }
  }

  #[test]
  fn solver_matches_polynomial_for_collinear_curve() {
    // For this curve, `x(t) == t` exactly (the x control points are
    // evenly spaced along the diagonal), so the eased value at progress
    // `p` is the bezier polynomial evaluated at `p`.
    let curve = CubicBezier::new(1.0 / 3.0, 1.0 / 3.0, 2.0 / 3.0, 2.0 / 3.0);

    for i in 0..=20 {
      let progress = f64::from(i) / 20.0;
      let expected =
        cubic_bezier(0.0, 1.0 / 3.0, 2.0 / 3.0, 1.0, progress);

      assert!((curve.ease(progress) - expected).abs() < EPSILON);
    }
  }

  #[test]
  fn css_ease_curve_matches_reference_values() {
    // `cubic-bezier(0.25, 0.1, 0.25, 1.0)` — the CSS `ease` curve.
    let curve = CubicBezier::new(0.25, 0.1, 0.25, 1.0);

    let samples = [(0.25, 0.4085), (0.5, 0.8024), (0.75, 0.9599)];

    for (progress, expected) in samples {
      assert!(
        (curve.ease(progress) - expected).abs() < 1e-3,
        "ease({progress}) = {}, expected {expected}",
        curve.ease(progress)
      );
    }
  }

  #[test]
  fn overshooting_curve_overshoots() {
    // A back-ease style curve with a `y` control point above `1.0`.
    let curve = CubicBezier::new(0.05, 0.9, 0.1, 1.05);

    let max = (0..=100)
      .map(|i| curve.ease(f64::from(i) / 100.0))
      .fold(f64::NEG_INFINITY, f64::max);

    assert!(max > 1.0, "expected overshoot, got max {max}");
    assert!(max < 1.1, "unexpectedly large overshoot: {max}");
  }

  #[test]
  fn default_curve_stays_within_range() {
    // The default curve is used for window slides, which are
    // translations: it must not overshoot its target.
    let curve = CubicBezier::from(&AnimationCurve::default());

    for i in 0..=100 {
      let eased = curve.ease(f64::from(i) / 100.0);

      assert!(
        (0.0..=1.0).contains(&eased),
        "ease({i}%) = {eased}, expected within [0, 1]"
      );
    }
  }
}
