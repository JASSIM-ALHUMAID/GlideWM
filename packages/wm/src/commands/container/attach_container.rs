use anyhow::bail;

use super::resize_tiling_container;
use crate::{
  models::Container,
  traits::{CommonGetters, TilingSizeGetters},
};

/// Inserts a child container at the specified index.
///
/// The inserted child will be resized to fit the available space, unless
/// the target parent is a scrolling workspace. Tiling children of a
/// scrolling workspace are strip items that are sized as a fraction of
/// the viewport, independent of their siblings.
pub fn attach_container(
  child: &Container,
  target_parent: &Container,
  target_index: Option<usize>,
) -> anyhow::Result<()> {
  if !child.is_detached() {
    bail!("Cannot attach an already attached container.");
  }

  if let Some(target_index) = target_index {
    // Ensure target index is within the bounds of the parent's children.
    let target_index = target_index.clamp(0, target_parent.child_count());

    // Insert the child at the specified index.
    target_parent
      .borrow_children_mut()
      .insert(target_index, child.clone());
  } else {
    target_parent.borrow_children_mut().push_back(child.clone());
  }

  target_parent
    .borrow_child_focus_order_mut()
    .push_back(child.id());

  *child.borrow_parent_mut() = Some(target_parent.clone());

  // Resize the child and its siblings if it is a tiling container.
  if let Ok(child) = child.as_tiling_container() {
    // Strip items of a scrolling workspace are sized as a fraction of
    // the viewport, independent of their siblings. Leave the size
    // untouched: new windows are sized by `create_window`, and moved
    // containers keep their size.
    if matches!(
      target_parent,
      Container::Workspace(workspace) if workspace.is_scrolling()
    ) {
      return Ok(());
    }

    let tiling_siblings = child.tiling_siblings().collect::<Vec<_>>();

    if tiling_siblings.is_empty() {
      child.set_tiling_size(1.0);
      return Ok(());
    }

    // Set initial tiling size to 0, and then size up the container
    // to the target size.
    #[allow(clippy::cast_precision_loss)]
    let target_size = 1.0 / (tiling_siblings.len() + 1) as f32;
    child.set_tiling_size(0.0);
    resize_tiling_container(&child, target_size);
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
      2,
      0.5,
      &GapsConfig::default(),
      ScrollConfig::default(),
    );

    let new_window = TilingWindow::mock().call();

    attach_container(
      &new_window.clone().into(),
      &workspace.clone().into(),
      Some(1),
    )
    .unwrap();

    // Existing strip items keep their size, and the new window keeps its
    // size. New windows are sized by `create_window` when managed.
    assert!((windows[0].tiling_size() - 0.5).abs() < f32::EPSILON);
    assert!((windows[1].tiling_size() - 0.5).abs() < f32::EPSILON);
    assert!((new_window.tiling_size() - 1.0).abs() < f32::EPSILON);

    // The window is inserted at the specified index.
    assert_eq!(workspace.children()[1].id(), new_window.id());
  }

  #[test]
  fn redistributes_sizes_in_tiling_workspace() {
    let workspace = Workspace::mock().call();

    let first = TilingWindow::mock().call();
    attach_container(
      &first.clone().into(),
      &workspace.clone().into(),
      None,
    )
    .unwrap();

    let second = TilingWindow::mock().call();
    attach_container(
      &second.clone().into(),
      &workspace.clone().into(),
      None,
    )
    .unwrap();

    // The available space is shared between the two windows.
    assert!((first.tiling_size() - 0.5).abs() < f32::EPSILON);
    assert!((second.tiling_size() - 0.5).abs() < f32::EPSILON);
  }
}
