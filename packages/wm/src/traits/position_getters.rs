use ambassador::delegatable_trait;
use wm_platform::Rect;

#[delegatable_trait]
pub trait PositionGetters {
  fn to_rect(&self) -> anyhow::Result<Rect>;
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

        // Direct children of a workspace are positioned by the
        // workspace's layout.
        if let DirectionContainer::Workspace(workspace) = &parent {
          let child = self.as_tiling_container()?;

          return $crate::layouts::child_rect(
            workspace,
            &child,
            &parent_rect,
          );
        }

        // Otherwise, tiling containers share the space of their parent.
        $crate::layouts::tiling::shared_child_rect(
          &self.as_tiling_container()?,
          &parent.tiling_direction(),
          &parent_rect,
        )
      }
    }
  };
}

#[cfg(test)]
mod tests {
  use wm_common::{GapsConfig, ScrollAlign, ScrollConfig};

  use super::*;
  use crate::{
    commands::container::{attach_container, detach_container},
    layouts::scrolling,
    models::{Monitor, TilingWindow},
    test_utils::mock_scrolling_workspace,
  };

  fn window_rects(windows: &[TilingWindow]) -> Vec<Rect> {
    windows
      .iter()
      .map(|window| window.to_rect().unwrap())
      .collect()
  }

  #[test]
  fn positions_horizontal_strip() {
    let (workspace, windows) = mock_scrolling_workspace(
      wm_common::TilingDirection::Horizontal,
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
    let (workspace, windows) = mock_scrolling_workspace(
      wm_common::TilingDirection::Vertical,
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
      inner_gap: wm_platform::LengthValue::from_px(10),
      ..GapsConfig::default()
    };

    let (workspace, windows) = mock_scrolling_workspace(
      wm_common::TilingDirection::Horizontal,
      3,
      0.5,
      &gaps_config,
      ScrollConfig::default(),
    );

    let rects = window_rects(&windows);
    assert_eq!(rects[0].x(), 0);
    assert_eq!(rects[1].x(), 850);
    assert_eq!(rects[2].x(), 1700);

    assert_eq!(scrolling::strip_extent(&workspace).unwrap(), 2540);
    assert_eq!(scrolling::max_scroll_offset(&workspace).unwrap(), 860);
  }

  #[test]
  fn clamps_scroll_offset() {
    let (workspace, _windows) = mock_scrolling_workspace(
      wm_common::TilingDirection::Horizontal,
      3,
      0.5,
      &GapsConfig::default(),
      ScrollConfig::default(),
    );

    workspace.set_scroll_offset(10_000);
    assert!(scrolling::clamp_scroll_offset(&workspace).unwrap());
    assert_eq!(workspace.scroll_offset(), 840);

    workspace.set_scroll_offset(-100);
    assert!(scrolling::clamp_scroll_offset(&workspace).unwrap());
    assert_eq!(workspace.scroll_offset(), 0);

    // Clamping to the current offset is a no-op.
    assert!(!scrolling::clamp_scroll_offset(&workspace).unwrap());
  }

  #[test]
  fn scrolls_focused_window_into_view() {
    let (workspace, windows) = mock_scrolling_workspace(
      wm_common::TilingDirection::Horizontal,
      3,
      0.5,
      &GapsConfig::default(),
      ScrollConfig::default(),
    );

    // The third window is out of view at offset 0.
    assert!(scrolling::ensure_descendant_visible(
      &workspace,
      &windows[2].clone().into()
    )
    .unwrap());
    assert_eq!(workspace.scroll_offset(), 840);

    // The second window is fully visible; the offset stays the same.
    assert!(!scrolling::ensure_descendant_visible(
      &workspace,
      &windows[1].clone().into()
    )
    .unwrap());
    assert_eq!(workspace.scroll_offset(), 840);

    // Scroll back to the first window.
    assert!(scrolling::ensure_descendant_visible(
      &workspace,
      &windows[0].clone().into()
    )
    .unwrap());
    assert_eq!(workspace.scroll_offset(), 0);
  }

  #[test]
  fn centers_focused_window() {
    let (workspace, windows) = mock_scrolling_workspace(
      wm_common::TilingDirection::Horizontal,
      3,
      0.5,
      &GapsConfig::default(),
      ScrollConfig {
        align: ScrollAlign::Center,
        ..ScrollConfig::default()
      },
    );

    assert!(scrolling::ensure_descendant_visible(
      &workspace,
      &windows[1].clone().into()
    )
    .unwrap());
    assert_eq!(workspace.scroll_offset(), 420);

    let rect = windows[1].to_rect().unwrap();
    assert_eq!(rect.x() + rect.width() / 2, 840);
  }

  #[test]
  fn ignores_non_scrolling_workspaces() {
    let workspace = crate::models::Workspace::mock().call();

    let window = TilingWindow::mock().call();
    attach_container(
      &window.clone().into(),
      &workspace.clone().into(),
      None,
    )
    .unwrap();

    let _monitor =
      Monitor::mock().workspaces(vec![workspace.clone()]).call();

    assert!(!scrolling::ensure_descendant_visible(
      &workspace,
      &window.into()
    )
    .unwrap());
    assert!(!scrolling::clamp_scroll_offset(&workspace).unwrap());
  }

  #[test]
  fn expands_lone_strip_item_to_fill_viewport() {
    let (workspace, windows) = mock_scrolling_workspace(
      wm_common::TilingDirection::Horizontal,
      1,
      0.5,
      &GapsConfig::default(),
      ScrollConfig::default(),
    );

    // A lone strip item ignores its stored fraction and fills the
    // viewport, like a single column in Niri.
    let rect = windows[0].to_rect().unwrap();
    assert_eq!(rect.x(), 0);
    assert_eq!(rect.width(), 1680);
    assert_eq!(scrolling::strip_extent(&workspace).unwrap(), 1680);
    assert_eq!(scrolling::max_scroll_offset(&workspace).unwrap(), 0);
  }

  #[test]
  fn uses_stored_fraction_with_multiple_strip_items() {
    let (workspace, windows) = mock_scrolling_workspace(
      wm_common::TilingDirection::Horizontal,
      2,
      0.5,
      &GapsConfig::default(),
      ScrollConfig::default(),
    );

    let rects = window_rects(&windows);
    assert_eq!(rects[0].width(), 840);
    assert_eq!(rects[1].x(), 840);
    assert_eq!(scrolling::strip_extent(&workspace).unwrap(), 1680);
  }

  #[test]
  fn reexpands_when_returning_to_single_strip_item() {
    let (workspace, windows) = mock_scrolling_workspace(
      wm_common::TilingDirection::Horizontal,
      2,
      0.5,
      &GapsConfig::default(),
      ScrollConfig::default(),
    );

    detach_container(windows[1].clone().into()).unwrap();

    let rect = windows[0].to_rect().unwrap();
    assert_eq!(rect.width(), 1680);
    assert_eq!(scrolling::strip_extent(&workspace).unwrap(), 1680);
  }
}
