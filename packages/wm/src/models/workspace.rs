use std::{
  cell::{Ref, RefCell, RefMut},
  collections::VecDeque,
  rc::Rc,
};

use anyhow::Context;
use uuid::Uuid;
use wm_common::{
  ContainerDto, GapsConfig, ScrollAlign, TilingDirection, WorkspaceConfig,
  WorkspaceDto, WorkspaceLayout,
};
use wm_platform::{Rect, RectDelta};

use crate::{
  impl_common_getters, impl_container_debug,
  impl_tiling_direction_getters,
  models::{
    Container, DirectionContainer, TilingContainer, WindowContainer,
  },
  traits::{
    CommonGetters, PositionGetters, TilingDirectionGetters,
    TilingSizeGetters,
  },
};

#[derive(Clone)]
pub struct Workspace(Rc<RefCell<WorkspaceInner>>);

#[derive(Debug)]
struct WorkspaceInner {
  id: Uuid,
  parent: Option<Container>,
  children: VecDeque<Container>,
  child_focus_order: VecDeque<Uuid>,
  config: WorkspaceConfig,
  gaps_config: GapsConfig,
  tiling_direction: TilingDirection,
  scroll_offset: i32,
}

impl Workspace {
  pub fn new(
    config: WorkspaceConfig,
    gaps_config: GapsConfig,
    tiling_direction: TilingDirection,
  ) -> Self {
    let workspace = WorkspaceInner {
      id: Uuid::new_v4(),
      parent: None,
      children: VecDeque::new(),
      child_focus_order: VecDeque::new(),
      config,
      gaps_config,
      tiling_direction,
      scroll_offset: 0,
    };

    Self(Rc::new(RefCell::new(workspace)))
  }

  /// Underlying config for the workspace.
  pub fn config(&self) -> WorkspaceConfig {
    self.0.borrow().config.clone()
  }

  /// Update the underlying config for the workspace.
  pub fn set_config(&self, config: WorkspaceConfig) {
    self.0.borrow_mut().config = config;
  }

  /// Whether the workspace is currently displayed by the parent monitor.
  pub fn is_displayed(&self) -> bool {
    self
      .monitor()
      .and_then(|monitor| monitor.displayed_workspace())
      .is_some_and(|workspace| workspace.id() == self.id())
  }

  pub fn set_gaps_config(&self, gaps_config: GapsConfig) {
    self.0.borrow_mut().gaps_config = gaps_config;
  }

  /// Whether the workspace uses the scrolling layout.
  pub fn is_scrolling(&self) -> bool {
    self.0.borrow().config.layout == WorkspaceLayout::Scrolling
  }

  /// Axis that the strip of windows scrolls along.
  ///
  /// The axis is stored as the workspace's tiling direction so that it can
  /// be toggled at runtime (e.g. by the `toggle-tiling-direction`
  /// command).
  pub fn scroll_axis(&self) -> TilingDirection {
    self.tiling_direction()
  }

  /// Current scroll offset in physical pixels along the scroll axis.
  ///
  /// A value of `0` corresponds to the start of the strip being aligned
  /// with the start of the viewport.
  pub fn scroll_offset(&self) -> i32 {
    self.0.borrow().scroll_offset
  }

  /// Sets the scroll offset. Returns whether the offset changed.
  pub fn set_scroll_offset(&self, offset: i32) -> bool {
    let mut inner = self.0.borrow_mut();

    if inner.scroll_offset == offset {
      return false;
    }

    inner.scroll_offset = offset;
    true
  }

  /// Extent (width or height) of the viewport along the scroll axis.
  ///
  /// The viewport is the workspace rect, i.e. the monitor's working area
  /// with outer gaps applied.
  pub fn viewport_extent(&self) -> anyhow::Result<i32> {
    let rect = self.to_rect()?;

    Ok(match self.scroll_axis() {
      TilingDirection::Horizontal => rect.width(),
      TilingDirection::Vertical => rect.height(),
    })
  }

  /// Total extent of the strip of tiling children in pixels along the
  /// scroll axis.
  ///
  /// This includes the inner gap between each pair of adjacent strip
  /// items. Returns 0 if the workspace has no tiling children.
  pub fn strip_extent(&self) -> anyhow::Result<i32> {
    let viewport_extent = self.viewport_extent()?;
    let children = self.tiling_children().collect::<Vec<_>>();

    let Some(first_child) = children.first() else {
      return Ok(0);
    };

    let (horizontal_gap, vertical_gap) = first_child.inner_gaps()?;
    let inner_gap = match self.scroll_axis() {
      TilingDirection::Vertical => vertical_gap,
      TilingDirection::Horizontal => horizontal_gap,
    };

    let mut extent = 0;

    for (index, child) in children.iter().enumerate() {
      if index > 0 {
        extent += inner_gap;
      }

      extent += child.scrolling_extent(viewport_extent);
    }

    Ok(extent)
  }

  /// Maximum scroll offset for the current strip contents.
  ///
  /// Returns 0 when the strip is narrower than the viewport.
  pub fn max_scroll_offset(&self) -> anyhow::Result<i32> {
    Ok((self.strip_extent()? - self.viewport_extent()?).max(0))
  }

  /// Clamps the scroll offset into bounds that are valid for the current
  /// strip contents. Returns whether the offset changed.
  pub fn clamp_scroll_offset(&self) -> anyhow::Result<bool> {
    let clamped = self.scroll_offset().clamp(0, self.max_scroll_offset()?);

    Ok(self.set_scroll_offset(clamped))
  }

  /// Scrolls the viewport so that the given descendant is fully visible
  /// when the scrolling layout is active.
  ///
  /// The descendant is resolved to the top-level strip item that contains
  /// it. Returns whether the offset changed. Has no effect for windows
  /// that are not tiling children of this workspace.
  pub fn ensure_descendant_visible(
    &self,
    container: &Container,
  ) -> anyhow::Result<bool> {
    if !self.is_scrolling() {
      return Ok(false);
    }

    // Get the top-level strip item containing the descendant.
    let strip_item = container
      .self_and_ancestors()
      .take_while(|ancestor| ancestor.id() != self.id())
      .last();

    let Some(strip_item) = strip_item else {
      return Ok(false);
    };

    let Ok(strip_item) = strip_item.as_tiling_container() else {
      return Ok(false);
    };

    let viewport_rect = self.to_rect()?;
    let item_rect = strip_item.to_rect()?;

    let (viewport_start, viewport_extent, item_start, item_extent) =
      match self.scroll_axis() {
        TilingDirection::Horizontal => (
          viewport_rect.x(),
          viewport_rect.width(),
          item_rect.x(),
          item_rect.width(),
        ),
        TilingDirection::Vertical => (
          viewport_rect.y(),
          viewport_rect.height(),
          item_rect.y(),
          item_rect.height(),
        ),
      };

    let delta = match self.config().scroll.align {
      ScrollAlign::Minimal => {
        let item_end = item_start + item_extent;
        let viewport_end = viewport_start + viewport_extent;

        if item_start < viewport_start {
          item_start - viewport_start
        } else if item_end > viewport_end {
          item_end - viewport_end
        } else {
          0
        }
      }
      ScrollAlign::Center => {
        let item_center = item_start + item_extent / 2;
        let viewport_center = viewport_start + viewport_extent / 2;

        item_center - viewport_center
      }
    };

    let new_offset =
      (self.scroll_offset() + delta).clamp(0, self.max_scroll_offset()?);

    Ok(self.set_scroll_offset(new_offset))
  }

  /// Gets the index of the top-level strip item that contains the given
  /// descendant.
  ///
  /// Returns `None` if the container is not a descendant of this
  /// workspace.
  pub fn strip_item_index(&self, descendant: &Container) -> Option<usize> {
    descendant
      .self_and_ancestors()
      .find(|ancestor| {
        ancestor
          .parent()
          .is_some_and(|parent| parent.id() == self.id())
      })
      .map(|strip_item| strip_item.index())
  }

  /// Effective outer gaps for this workspace.
  ///
  /// Uses `single_window_outer_gap` when the workspace has a single tiling
  /// window, otherwise falls back to `outer_gap`.
  pub fn outer_gaps(&self) -> RectDelta {
    let is_single_window = self.tiling_children().nth(1).is_none();

    let gaps_config = &self.0.borrow().gaps_config;
    let gaps = if is_single_window {
      gaps_config
        .single_window_outer_gap
        .as_ref()
        .unwrap_or(&gaps_config.outer_gap)
    } else {
      &gaps_config.outer_gap
    };

    // TODO: Should this be scaled by the monitor's DPI?
    gaps.clone()
  }

  /// Gets the bounds of a workspace with the given outer gap config.
  fn workspace_rect_with_gap_config(
    &self,
    outer_gaps: &RectDelta,
  ) -> anyhow::Result<Rect> {
    let monitor =
      self.monitor().context("Workspace has no parent monitor.")?;

    let gaps_config = &self.0.borrow().gaps_config;
    let scale_factor = if gaps_config.scale_with_dpi {
      monitor.native_properties().scale_factor
    } else {
      1.
    };

    // Get the delta between the monitor's bounds and its working area.
    let monitor_bounds = monitor.native_properties().bounds;
    let working_area_delta = monitor
      .native_properties()
      .working_area
      .delta(&monitor_bounds);

    Ok(
      monitor_bounds
        // Scale the gaps if `scale_with_dpi` is enabled. Outer gap config
        // values can be a percentage (relative to the monitor bounds), so
        // the outer gap delta needs to be applied prior to the working
        // area delta.
        .apply_delta(&outer_gaps.inverse(), Some(scale_factor))
        .apply_delta(&working_area_delta, None),
    )
  }

  /// Gets the maximum bounds of a workspace considering both `outer_gap`
  /// and `single_window_outer_gap` config values.
  pub fn max_workspace_rect(&self) -> anyhow::Result<Rect> {
    let gaps_config = &self.0.borrow().gaps_config;

    // Get the workspace rect using `outer_gap`.
    let multi_window_rect =
      self.workspace_rect_with_gap_config(&gaps_config.outer_gap)?;

    let Some(single_gap) = &gaps_config.single_window_outer_gap else {
      return Ok(multi_window_rect);
    };

    // Get the workspace rect using `single_window_outer_gap`.
    let single_window_rect =
      self.workspace_rect_with_gap_config(single_gap)?;

    Ok(multi_window_rect.union(&single_window_rect))
  }

  pub fn to_dto(&self) -> anyhow::Result<ContainerDto> {
    let rect = self.to_rect()?;
    let config = self.config();

    let children = self
      .children()
      .iter()
      .map(CommonGetters::to_dto)
      .try_collect()?;

    Ok(ContainerDto::Workspace(WorkspaceDto {
      id: self.id(),
      name: config.name,
      display_name: config.display_name,
      parent_id: self.parent().map(|parent| parent.id()),
      children,
      child_focus_order: self.0.borrow().child_focus_order.clone().into(),
      has_focus: self.has_focus(None),
      is_displayed: self.is_displayed(),
      width: rect.width(),
      height: rect.height(),
      x: rect.x(),
      y: rect.y(),
      tiling_direction: self.tiling_direction(),
      layout: config.layout,
      scroll_offset: self.scroll_offset(),
    }))
  }
}

impl_container_debug!(Workspace);
impl_common_getters!(Workspace);
impl_tiling_direction_getters!(Workspace);

impl PositionGetters for Workspace {
  fn to_rect(&self) -> anyhow::Result<Rect> {
    self.workspace_rect_with_gap_config(&self.outer_gaps())
  }
}

impl std::fmt::Display for Workspace {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    write!(
      f,
      "Workspace(name={}, tiling_direction={:?})",
      self.config().name,
      self.tiling_direction(),
    )
  }
}
