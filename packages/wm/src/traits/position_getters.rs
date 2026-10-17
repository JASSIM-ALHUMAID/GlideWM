use ambassador::delegatable_trait;
use wm_common::TilingDirection;
use wm_platform::Rect;

use super::{CommonGetters, TilingSizeGetters};
use crate::models::{TilingContainer, Workspace};

#[delegatable_trait]
pub trait PositionGetters {
  fn to_rect(&self) -> anyhow::Result<Rect>;
}

/// Computes the rect of a top-level child of a scrolling workspace.
///
/// The child's extent along the workspace's scroll axis is its
/// `tiling_size` interpreted as a fraction of the viewport extent (see
/// [`TilingSizeGetters::scrolling_extent`]). Strip items are laid out in
/// order with an inner gap between each pair of adjacent items, and the
/// strip is shifted along the scroll axis by the workspace's current
/// scroll offset.
///
/// Expects `child` to be a direct child of `workspace` and `parent_rect`
/// to be the workspace's rect.
pub fn scrolling_child_rect(
  child: &TilingContainer,
  workspace: &Workspace,
  parent_rect: &Rect,
) -> anyhow::Result<Rect> {
  let scroll_axis = workspace.scroll_axis();

  let viewport_extent = match scroll_axis {
    TilingDirection::Horizontal => parent_rect.width(),
    TilingDirection::Vertical => parent_rect.height(),
  };

  let (horizontal_gap, vertical_gap) = child.inner_gaps()?;
  let inner_gap = match scroll_axis {
    TilingDirection::Vertical => vertical_gap,
    TilingDirection::Horizontal => horizontal_gap,
  };

  // Offset of this child from the start of the strip (i.e. the sum of the
  // extents and gaps of all preceding strip items).
  let preceding_extent = child
    .prev_siblings()
    .filter_map(|sibling| sibling.as_tiling_container().ok())
    .map(|sibling| sibling.scrolling_extent(viewport_extent) + inner_gap)
    .sum::<i32>();

  let along_extent = child.scrolling_extent(viewport_extent);
  let along_start = match scroll_axis {
    TilingDirection::Horizontal => parent_rect.x(),
    TilingDirection::Vertical => parent_rect.y(),
  } - workspace.scroll_offset()
    + preceding_extent;

  Ok(match scroll_axis {
    TilingDirection::Horizontal => Rect::from_xy(
      along_start,
      parent_rect.y(),
      along_extent,
      parent_rect.height(),
    ),
    TilingDirection::Vertical => Rect::from_xy(
      parent_rect.x(),
      along_start,
      parent_rect.width(),
      along_extent,
    ),
  })
}

/// Implements the `PositionGetters` trait for tiling containers that can
/// be resized. This is used by `SplitContainer` and `TilingWindow`.
///
/// Expects that the struct has a wrapping `RefCell` containing a struct
/// with an `id` and a `parent` field.
#[macro_export]
macro_rules! impl_position_getters_as_resizable {
  ($struct_name:ident) => {
    impl PositionGetters for $struct_name {
      fn to_rect(&self) -> anyhow::Result<Rect> {
        let parent = self
          .parent()
          .and_then(|parent| parent.as_direction_container().ok())
          .context("Parent does not have a tiling direction.")?;

        let parent_rect = parent.to_rect()?;

        // Top-level children of a scrolling workspace are laid out along
        // the scroll axis instead of sharing the workspace's space.
        if let DirectionContainer::Workspace(workspace) = &parent {
          if workspace.is_scrolling() {
            let child = self.as_tiling_container()?;

            return $crate::traits::scrolling_child_rect(
              &child,
              workspace,
              &parent_rect,
            );
          }
        }

        let (horizontal_gap, vertical_gap) = self.inner_gaps()?;
        let inner_gap = match parent.tiling_direction() {
          TilingDirection::Vertical => vertical_gap,
          TilingDirection::Horizontal => horizontal_gap,
        };

        #[allow(
          clippy::cast_precision_loss,
          clippy::cast_possible_truncation,
          clippy::cast_possible_wrap
        )]
        let (width, height) = match parent.tiling_direction() {
          TilingDirection::Vertical => {
            let available_height = parent_rect.height()
              - inner_gap * self.tiling_siblings().count() as i32;

            let height =
              (self.tiling_size() * available_height as f32) as i32;

            (parent_rect.width(), height)
          }
          TilingDirection::Horizontal => {
            let available_width = parent_rect.width()
              - inner_gap * self.tiling_siblings().count() as i32;

            let width =
              (available_width as f32 * self.tiling_size()).round() as i32;

            (width, parent_rect.height())
          }
        };

        let (x, y) = {
          let mut prev_siblings = self
            .prev_siblings()
            .filter_map(|sibling| sibling.as_tiling_container().ok());

          match prev_siblings.next() {
            None => (parent_rect.x(), parent_rect.y()),
            Some(sibling) => {
              let sibling_rect = sibling.to_rect()?;

              match parent.tiling_direction() {
                TilingDirection::Vertical => (
                  parent_rect.x(),
                  sibling_rect.y() + sibling_rect.height() + inner_gap,
                ),
                TilingDirection::Horizontal => (
                  sibling_rect.x() + sibling_rect.width() + inner_gap,
                  parent_rect.y(),
                ),
              }
            }
          }
        };

        Ok(Rect::from_xy(x, y, width, height))
      }
    }
  };
}

#[cfg(test)]
mod tests {
  use wm_common::{
    GapsConfig, ScrollAlign, ScrollConfig, WorkspaceLayout,
  };
  use wm_platform::LengthValue;

  use super::*;
  use crate::{
    commands::container::attach_container,
    models::{Monitor, TilingWindow},
  };

  /// Creates a scrolling workspace attached to a mock monitor, with the
  /// given number of equally-sized tiling windows.
  fn scrolling_workspace(
    axis: TilingDirection,
    window_count: usize,
    window_fraction: f32,
    gaps_config: &GapsConfig,
    scroll_config: ScrollConfig,
  ) -> (Workspace, Vec<TilingWindow>) {
    let workspace = Workspace::mock()
      .tiling_direction(axis)
      .gaps_config(gaps_config.clone())
      .layout(WorkspaceLayout::Scrolling)
      .scroll(scroll_config)
      .call();

    let windows = (0..window_count)
      .map(|_| {
        TilingWindow::mock().gaps_config(gaps_config.clone()).call()
      })
      .collect::<Vec<_>>();

    for window in &windows {
      attach_container(
        &window.clone().into(),
        &workspace.clone().into(),
        None,
      )
      .unwrap();
    }

    // `attach_container` resizes tiling children when they're inserted
    // with siblings. Override the sizes that it assigns here.
    for window in &windows {
      window.set_tiling_size(window_fraction);
    }

    // Attach the workspace to a mock monitor (needed for rect math).
    let _monitor =
      Monitor::mock().workspaces(vec![workspace.clone()]).call();

    (workspace, windows)
  }

  fn window_rects(windows: &[TilingWindow]) -> Vec<Rect> {
    windows
      .iter()
      .map(|window| window.to_rect().unwrap())
      .collect()
  }

  #[test]
  fn positions_horizontal_strip() {
    let (workspace, windows) = scrolling_workspace(
      TilingDirection::Horizontal,
      3,
      0.5,
      &GapsConfig::default(),
      ScrollConfig::default(),
    );

    // Viewport is the monitor's working area: (0, 0, 1680, 1000).
    let rects = window_rects(&windows);
    assert_eq!(rects[0].x(), 0);
    assert_eq!(rects[1].x(), 840);
    assert_eq!(rects[2].x(), 1680);
    assert!(rects.iter().all(|rect| rect.width() == 840));
    assert!(rects.iter().all(|rect| rect.height() == 1000));

    // Scrolling shifts the whole strip along the scroll axis.
    workspace.set_scroll_offset(840);

    let rects = window_rects(&windows);
    assert_eq!(rects[0].x(), -840);
    assert_eq!(rects[1].x(), 0);
    assert_eq!(rects[2].x(), 840);
  }

  #[test]
  fn positions_vertical_strip() {
    let (workspace, windows) = scrolling_workspace(
      TilingDirection::Vertical,
      3,
      0.5,
      &GapsConfig::default(),
      ScrollConfig::default(),
    );

    let rects = window_rects(&windows);
    assert_eq!(rects[0].y(), 0);
    assert_eq!(rects[1].y(), 500);
    assert_eq!(rects[2].y(), 1000);
    assert!(rects.iter().all(|rect| rect.width() == 1680));
    assert!(rects.iter().all(|rect| rect.height() == 500));

    workspace.set_scroll_offset(500);

    let rects = window_rects(&windows);
    assert_eq!(rects[0].y(), -500);
    assert_eq!(rects[1].y(), 0);
    assert_eq!(rects[2].y(), 500);
  }

  #[test]
  fn accounts_for_inner_gap() {
    let gaps_config = GapsConfig {
      inner_gap: LengthValue::from_px(10),
      ..GapsConfig::default()
    };

    let (workspace, windows) = scrolling_workspace(
      TilingDirection::Horizontal,
      3,
      0.5,
      &gaps_config,
      ScrollConfig::default(),
    );

    let rects = window_rects(&windows);
    assert_eq!(rects[0].x(), 0);
    assert_eq!(rects[1].x(), 850);
    assert_eq!(rects[2].x(), 1700);

    assert_eq!(workspace.strip_extent().unwrap(), 2540);
    assert_eq!(workspace.max_scroll_offset().unwrap(), 860);
  }

  #[test]
  fn clamps_scroll_offset() {
    let (workspace, _windows) = scrolling_workspace(
      TilingDirection::Horizontal,
      3,
      0.5,
      &GapsConfig::default(),
      ScrollConfig::default(),
    );

    workspace.set_scroll_offset(10_000);
    assert!(workspace.clamp_scroll_offset().unwrap());
    assert_eq!(workspace.scroll_offset(), 840);

    workspace.set_scroll_offset(-100);
    assert!(workspace.clamp_scroll_offset().unwrap());
    assert_eq!(workspace.scroll_offset(), 0);

    // Clamping to the current offset is a no-op.
    assert!(!workspace.clamp_scroll_offset().unwrap());
  }

  #[test]
  fn scrolls_focused_window_into_view() {
    let (workspace, windows) = scrolling_workspace(
      TilingDirection::Horizontal,
      3,
      0.5,
      &GapsConfig::default(),
      ScrollConfig::default(),
    );

    // The third window is out of view at offset 0.
    assert!(workspace
      .ensure_descendant_visible(&windows[2].clone().into())
      .unwrap());
    assert_eq!(workspace.scroll_offset(), 840);

    // The second window is fully visible; the offset stays the same.
    assert!(!workspace
      .ensure_descendant_visible(&windows[1].clone().into())
      .unwrap());
    assert_eq!(workspace.scroll_offset(), 840);

    // Scroll back to the first window.
    assert!(workspace
      .ensure_descendant_visible(&windows[0].clone().into())
      .unwrap());
    assert_eq!(workspace.scroll_offset(), 0);
  }

  #[test]
  fn centers_focused_window() {
    let (workspace, windows) = scrolling_workspace(
      TilingDirection::Horizontal,
      3,
      0.5,
      &GapsConfig::default(),
      ScrollConfig {
        align: ScrollAlign::Center,
        ..ScrollConfig::default()
      },
    );

    assert!(workspace
      .ensure_descendant_visible(&windows[1].clone().into())
      .unwrap());
    assert_eq!(workspace.scroll_offset(), 420);

    let rect = windows[1].to_rect().unwrap();
    assert_eq!(rect.x() + rect.width() / 2, 840);
  }

  #[test]
  fn ignores_non_scrolling_workspaces() {
    let workspace = Workspace::mock().call();

    let window = TilingWindow::mock().call();
    attach_container(
      &window.clone().into(),
      &workspace.clone().into(),
      None,
    )
    .unwrap();

    let _monitor =
      Monitor::mock().workspaces(vec![workspace.clone()]).call();

    assert!(!workspace.ensure_descendant_visible(&window.into()).unwrap());
    assert!(!workspace.clamp_scroll_offset().unwrap());
  }
}
