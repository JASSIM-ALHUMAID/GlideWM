//! Geometry of animation frames.

use wm_common::{AnimationDirection, WorkspaceSwitchAnimationStyle};
use wm_platform::{Direction, Rect};

/// Distance in pixels that a window slides beyond the monitor's edge, so
/// that the window (including its shadow) is fully off-screen at the
/// start/end of a slide.
const SLIDE_MARGIN_PX: i32 = 64;

/// Scale that windows grow from with the `popin` open animation.
pub const POPIN_SCALE: f64 = 0.85;

/// Edge of a monitor that windows slide in from or out through.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SlideEdge {
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
  #[must_use]
  pub fn for_switch(
    style: WorkspaceSwitchAnimationStyle,
    direction: AnimationDirection,
    is_forward: bool,
  ) -> Self {
    // The vertical style (`slide_vert`) slides along the y-axis and the
    // other styles along the x-axis. Directions that don't match the
    // style's axis are treated as `auto`.
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
      | WorkspaceSwitchAnimationStyle::Fade
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
  #[must_use]
  pub fn opposite(self) -> Self {
    match self {
      Self::Left => Self::Right,
      Self::Right => Self::Left,
      Self::Top => Self::Bottom,
      Self::Bottom => Self::Top,
    }
  }

  fn direction(self) -> Direction {
    match self {
      Self::Left => Direction::Left,
      Self::Right => Direction::Right,
      Self::Top => Direction::Up,
      Self::Bottom => Direction::Down,
    }
  }
}

/// Translates `rect` beyond the given edge of the monitor, far enough
/// that a window at `rect` is fully off the monitor.
#[must_use]
pub fn beyond_edge(rect: &Rect, edge: SlideEdge, monitor: &Rect) -> Rect {
  let distance = match edge {
    SlideEdge::Left | SlideEdge::Right => monitor.width(),
    SlideEdge::Top | SlideEdge::Bottom => monitor.height(),
  } + SLIDE_MARGIN_PX;

  rect.translate_in_direction(&edge.direction(), distance)
}

/// Scales `rect` about its center.
#[must_use]
pub fn scale_about_center(rect: &Rect, scale: f64) -> Rect {
  let scale_length = |length: i32| {
    // Scaled window lengths are within the range of screen coordinates,
    // so the cast can't truncate.
    #[allow(clippy::cast_possible_truncation)]
    let scaled = (f64::from(length) * scale).round() as i32;
    scaled
  };

  let width = scale_length(rect.width());
  let height = scale_length(rect.height());

  Rect::from_xy(
    rect.left + (rect.width() - width) / 2,
    rect.top + (rect.height() - height) / 2,
    width,
    height,
  )
}

/// Intersection of two rects, or `None` if they don't overlap.
#[must_use]
pub fn intersection(a: &Rect, b: &Rect) -> Option<Rect> {
  let rect = Rect::from_ltrb(
    a.left.max(b.left),
    a.top.max(b.top),
    a.right.min(b.right),
    a.bottom.min(b.bottom),
  );

  (rect.width() > 0 && rect.height() > 0).then_some(rect)
}

/// Placement of a thumbnail after clipping it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClippedThumbnail {
  /// Where the visible part of the thumbnail is drawn, in screen
  /// coordinates.
  pub destination: Rect,

  /// Part of the source window that's drawn, relative to the window's
  /// top-left corner.
  pub source: Rect,
}

/// Clips a thumbnail that's drawn at `destination` to `clip`.
///
/// The source window (of size `source_size`) is stretched to fill
/// `destination`, so the part of the source that's cropped off is
/// proportional to the part of `destination` that's outside `clip`.
///
/// Returns `None` if nothing of the thumbnail is visible.
#[must_use]
pub fn clip_thumbnail(
  destination: &Rect,
  source_size: (i32, i32),
  clip: Option<&Rect>,
) -> Option<ClippedThumbnail> {
  let (source_width, source_height) = source_size;

  if destination.width() <= 0
    || destination.height() <= 0
    || source_width <= 0
    || source_height <= 0
  {
    return None;
  }

  let full_source = Rect::from_xy(0, 0, source_width, source_height);

  let Some(clip) = clip else {
    return Some(ClippedThumbnail {
      destination: destination.clone(),
      source: full_source,
    });
  };

  let visible = intersection(destination, clip)?;

  if visible == *destination {
    return Some(ClippedThumbnail {
      destination: visible,
      source: full_source,
    });
  }

  let scale_x = f64::from(source_width) / f64::from(destination.width());
  let scale_y = f64::from(source_height) / f64::from(destination.height());

  let to_source = |offset: i32, scale: f64| {
    // Offsets are bounded by the source window's size, so the cast can't
    // truncate.
    #[allow(clippy::cast_possible_truncation)]
    let offset = (f64::from(offset) * scale).round() as i32;
    offset
  };

  let source = Rect::from_ltrb(
    to_source(visible.left - destination.left, scale_x),
    to_source(visible.top - destination.top, scale_y),
    to_source(visible.right - destination.left, scale_x),
    to_source(visible.bottom - destination.top, scale_y),
  );

  (source.width() > 0 && source.height() > 0).then_some(ClippedThumbnail {
    destination: visible,
    source,
  })
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn resolves_switch_edges() {
    use AnimationDirection as D;
    use WorkspaceSwitchAnimationStyle as S;

    assert_eq!(
      SlideEdge::for_switch(S::Slide, D::Auto, true),
      SlideEdge::Right
    );
    assert_eq!(
      SlideEdge::for_switch(S::Slide, D::Auto, false),
      SlideEdge::Left
    );
    assert_eq!(
      SlideEdge::for_switch(S::Slide, D::Left, true),
      SlideEdge::Left
    );
    // Directions that don't match the style's axis fall back to `auto`.
    assert_eq!(
      SlideEdge::for_switch(S::Slide, D::Up, false),
      SlideEdge::Left
    );
    assert_eq!(
      SlideEdge::for_switch(S::SlideVert, D::Auto, true),
      SlideEdge::Bottom
    );
    assert_eq!(
      SlideEdge::for_switch(S::SlideVert, D::Down, true),
      SlideEdge::Top
    );
    assert_eq!(
      SlideEdge::for_switch(S::SlideVert, D::Right, false),
      SlideEdge::Top
    );
    assert_eq!(SlideEdge::Right.opposite(), SlideEdge::Left);
    assert_eq!(SlideEdge::Top.opposite(), SlideEdge::Bottom);
  }

  #[test]
  fn translates_beyond_monitor_edge() {
    let monitor = Rect::from_xy(0, 0, 1920, 1080);
    let rect = Rect::from_xy(100, 50, 800, 600);

    let right = beyond_edge(&rect, SlideEdge::Right, &monitor);
    assert_eq!(right, Rect::from_xy(100 + 1920 + 64, 50, 800, 600));
    assert!(right.left >= monitor.right);

    let top = beyond_edge(&rect, SlideEdge::Top, &monitor);
    assert_eq!(top, Rect::from_xy(100, 50 - 1080 - 64, 800, 600));
    assert!(top.bottom <= monitor.top);
  }

  #[test]
  fn scales_about_center() {
    let rect = Rect::from_xy(100, 100, 200, 100);

    assert_eq!(
      scale_about_center(&rect, 0.5),
      Rect::from_xy(150, 125, 100, 50)
    );
    assert_eq!(scale_about_center(&rect, 1.0), rect);
  }

  #[test]
  fn keeps_unclipped_thumbnails_whole() {
    let destination = Rect::from_xy(10, 10, 100, 50);
    let clip = Rect::from_xy(0, 0, 1000, 1000);

    let clipped =
      clip_thumbnail(&destination, (200, 100), Some(&clip)).unwrap();

    assert_eq!(clipped.destination, destination);
    assert_eq!(clipped.source, Rect::from_xy(0, 0, 200, 100));
    assert_eq!(
      clip_thumbnail(&destination, (200, 100), None)
        .unwrap()
        .destination,
      destination
    );
  }

  #[test]
  fn crops_partially_clipped_thumbnails() {
    // The thumbnail is drawn at half the source's size, and its right
    // half is outside the clip rect.
    let destination = Rect::from_xy(950, 0, 100, 50);
    let clip = Rect::from_xy(0, 0, 1000, 1000);

    let clipped =
      clip_thumbnail(&destination, (200, 100), Some(&clip)).unwrap();

    assert_eq!(clipped.destination, Rect::from_ltrb(950, 0, 1000, 50));
    assert_eq!(clipped.source, Rect::from_ltrb(0, 0, 100, 100));
  }

  #[test]
  fn hides_fully_clipped_thumbnails() {
    let destination = Rect::from_xy(2000, 0, 100, 50);
    let clip = Rect::from_xy(0, 0, 1000, 1000);

    assert!(
      clip_thumbnail(&destination, (200, 100), Some(&clip)).is_none()
    );
    assert!(clip_thumbnail(&destination, (0, 0), None).is_none());
  }
}
