use anyhow::Context;

use super::flatten_split_container;
use crate::{layouts, models::Container, traits::CommonGetters};

/// Removes a container from the tree.
///
/// If the container is a tiling container, the space freed up by the
/// removal is redistributed according to the parent's layout (see
/// [`layouts::on_child_detached`]). Will flatten empty parent split
/// containers.
#[allow(clippy::needless_pass_by_value)]
pub fn detach_container(child_to_remove: Container) -> anyhow::Result<()> {
  // Flatten the parent split container if it'll be empty after removing
  // the child.
  if let Some(split_parent) = child_to_remove
    .parent()
    .and_then(|parent| parent.as_split().cloned())
  {
    if split_parent.child_count() == 1 {
      flatten_split_container(split_parent)?;
    }
  }

  let parent = child_to_remove.parent().context("No parent.")?;

  parent
    .borrow_children_mut()
    .retain(|c| c.id() != child_to_remove.id());

  parent
    .borrow_child_focus_order_mut()
    .retain(|id| *id != child_to_remove.id());

  *child_to_remove.borrow_parent_mut() = None;

  // Redistribute the freed up space amongst the siblings if the removed
  // container is a tiling container. Layouts whose children are sized
  // independently (e.g. strip items) leave the sizes untouched.
  if let Ok(child_to_remove) = child_to_remove.as_tiling_container() {
    layouts::on_child_detached(&child_to_remove, &parent);
  }

  Ok(())
}

#[cfg(test)]
mod tests {
  use wm_common::{GapsConfig, ScrollConfig, TilingDirection};

  use super::*;
  use crate::{
    models::{TilingWindow, Workspace},
    test_utils::mock_scrolling_workspace,
    traits::TilingSizeGetters,
  };

  #[test]
  fn leaves_strip_item_sizes_untouched() {
    let (workspace, windows) = mock_scrolling_workspace(
      TilingDirection::Horizontal,
      3,
      0.5,
      &GapsConfig::default(),
      ScrollConfig::default(),
    );

    detach_container(windows[1].clone().into()).unwrap();

    // The remaining strip items keep their size.
    assert_eq!(workspace.child_count(), 2);
    assert!((windows[0].tiling_size() - 0.5).abs() < f32::EPSILON);
    assert!((windows[2].tiling_size() - 0.5).abs() < f32::EPSILON);
  }

  #[test]
  fn redistributes_sizes_in_tiling_workspace() {
    let first = TilingWindow::mock().call();
    let second = TilingWindow::mock().call();

    let _workspace = Workspace::mock()
      .tiling_containers(vec![first.clone().into(), second.clone().into()])
      .call();

    detach_container(second.clone().into()).unwrap();

    // The remaining window fills the freed up space.
    assert!((first.tiling_size() - 1.0).abs() < f32::EPSILON);
  }
}
