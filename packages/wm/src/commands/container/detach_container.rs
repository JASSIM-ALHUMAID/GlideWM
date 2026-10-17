use anyhow::Context;

use super::flatten_split_container;
use crate::{
  models::Container,
  traits::{CommonGetters, TilingSizeGetters, MIN_TILING_SIZE},
};

/// Removes a container from the tree.
///
/// If the container is a tiling container, the siblings will be resized to
/// fill the freed up space. Will flatten empty parent split containers.
///
/// Siblings of a scrolling workspace are not resized, since strip items
/// are sized as fractions of the viewport, independent of each other.
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

  // Resize the siblings if it is a tiling container. Siblings are left
  // untouched when the parent is a scrolling workspace.
  let is_scrolling_workspace = matches!(
    &parent,
    Container::Workspace(workspace) if workspace.is_scrolling()
  );

  if !is_scrolling_workspace {
    if let Ok(child_to_remove) = child_to_remove.as_tiling_container() {
      let tiling_siblings = parent.tiling_children().collect::<Vec<_>>();

      // TODO: Share logic with `resize_tiling_container`.
      let available_size =
        tiling_siblings.iter().fold(0.0, |sum, container| {
          sum + container.tiling_size() - MIN_TILING_SIZE
        });

      // Adjust size of the siblings based on the freed up space.
      for sibling in &tiling_siblings {
        let resize_factor =
          (sibling.tiling_size() - MIN_TILING_SIZE) / available_size;

        let size_delta = resize_factor * child_to_remove.tiling_size();
        sibling.set_tiling_size(sibling.tiling_size() + size_delta);
      }
    }
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
