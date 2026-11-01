use std::{
  cell::{Ref, RefCell, RefMut},
  collections::VecDeque,
  rc::Rc,
};

use anyhow::Context;
use uuid::Uuid;
use wm_common::{
  ContainerDto, GapsConfig, SplitContainerDto, TilingDirection,
  WorkspaceLayout,
};
use wm_platform::Rect;

use crate::{
  impl_common_getters, impl_container_debug,
  impl_position_getters_as_resizable, impl_tiling_direction_getters,
  impl_tiling_size_getters,
  models::{
    Container, DirectionContainer, TilingContainer, WindowContainer,
  },
  traits::{
    CommonGetters, PositionGetters, TilingDirectionGetters,
    TilingSizeGetters,
  },
};

#[derive(Clone)]
pub struct SplitContainer(Rc<RefCell<SplitContainerInner>>);

struct SplitContainerInner {
  id: Uuid,
  parent: Option<Container>,
  children: VecDeque<Container>,
  child_focus_order: VecDeque<Uuid>,
  tiling_size: f32,
  tiling_direction: TilingDirection,
  gaps_config: GapsConfig,
  layout_group: Option<WorkspaceLayout>,
}

impl SplitContainer {
  pub fn new(
    tiling_direction: TilingDirection,
    gaps_config: GapsConfig,
  ) -> Self {
    let split = SplitContainerInner {
      id: Uuid::new_v4(),
      parent: None,
      children: VecDeque::new(),
      child_focus_order: VecDeque::new(),
      tiling_size: 1.0,
      tiling_direction,
      gaps_config,
      layout_group: None,
    };

    Self(Rc::new(RefCell::new(split)))
  }

  /// The layout that arranges the contents of this split, when it is a
  /// layout group (see `crate::layouts::groups`).
  ///
  /// Layout groups are direct children of a workspace that retained its
  /// strip when its layout was switched with `layout_switch: keep`.
  pub fn layout_group(&self) -> Option<WorkspaceLayout> {
    self.0.borrow().layout_group.clone()
  }

  /// Marks the split as a layout group arranged by `layout`, or clears
  /// the mark.
  pub fn set_layout_group(&self, layout: Option<WorkspaceLayout>) {
    self.0.borrow_mut().layout_group = layout;
  }

  /// Whether the split is a layout group.
  pub fn is_layout_group(&self) -> bool {
    self.0.borrow().layout_group.is_some()
  }

  pub fn to_dto(&self) -> anyhow::Result<ContainerDto> {
    let rect = self.to_rect()?;
    let children = self
      .children()
      .iter()
      .map(CommonGetters::to_dto)
      .try_collect()?;

    Ok(ContainerDto::Split(SplitContainerDto {
      id: self.id(),
      parent_id: self.parent().map(|parent| parent.id()),
      children,
      child_focus_order: self.0.borrow().child_focus_order.clone().into(),
      has_focus: self.has_focus(None),
      tiling_size: self.tiling_size(),
      tiling_direction: self.tiling_direction(),
      width: rect.width(),
      height: rect.height(),
      x: rect.x(),
      y: rect.y(),
    }))
  }
}

impl_container_debug!(SplitContainer);
impl_common_getters!(SplitContainer);
impl_tiling_size_getters!(SplitContainer);
impl_tiling_direction_getters!(SplitContainer);
impl_position_getters_as_resizable!(SplitContainer);
