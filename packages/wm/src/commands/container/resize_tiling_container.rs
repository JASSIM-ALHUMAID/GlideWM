use crate::{
  models::{Container, TilingContainer},
  traits::{CommonGetters, TilingSizeGetters, MIN_TILING_SIZE},
};

/// Resizes a tiling container so that its tiling size becomes the given
/// target size.
///
/// The sizes that are freed up by the resize are distributed amongst the
/// container's tiling siblings, unless the container is a strip item of a
/// scrolling workspace. Strip items are sized as a fraction of the
/// viewport, independent of their siblings.
pub fn resize_tiling_container(
  container_to_resize: &TilingContainer,
  target_size: f32,
) {
  let is_scrolling_strip_item =
    container_to_resize.parent().is_some_and(|parent| {
      matches!(
        parent,
        Container::Workspace(workspace) if workspace.is_scrolling()
      )
    });

  if is_scrolling_strip_item {
    container_to_resize
      .set_tiling_size(target_size.clamp(MIN_TILING_SIZE, 1.0));
    return;
  }

  let tiling_siblings =
    container_to_resize.tiling_siblings().collect::<Vec<_>>();

  // Ignore cases where the container is the only child.
  if tiling_siblings.is_empty() {
    container_to_resize.set_tiling_size(1.);
    return;
  }

  // Prevent the container from being smaller than the minimum size, and
  // larger than the space available from sibling containers.
  #[allow(clippy::cast_precision_loss)]
  let clamped_target_size = target_size.clamp(
    MIN_TILING_SIZE,
    1. - (tiling_siblings.len() as f32 * MIN_TILING_SIZE),
  );

  let size_delta = clamped_target_size - container_to_resize.tiling_size();
  container_to_resize.set_tiling_size(clamped_target_size);

  // Get available tiling size amongst siblings.
  let available_size =
    tiling_siblings.iter().fold(0.0, |sum, container| {
      sum + container.tiling_size() - MIN_TILING_SIZE
    });

  // Distribute the available tiling size amongst its siblings.
  for sibling in &tiling_siblings {
    // Get percentage of resize that affects this container. Siblings are
    // resized in proportion to their current size (i.e. larger containers
    // are shrunk more).
    let resize_factor =
      (sibling.tiling_size() - MIN_TILING_SIZE) / available_size;

    let size_delta = resize_factor * size_delta;

    sibling.set_tiling_size(sibling.tiling_size() - size_delta);
  }
}

#[cfg(test)]
mod tests {
  use wm_common::{GapsConfig, ScrollConfig, TilingDirection};

  use super::*;
  use crate::{
    models::{SplitContainer, TilingWindow},
    test_utils::mock_scrolling_workspace,
  };

  #[test]
  fn resizes_strip_item_without_affecting_siblings() {
    let (_workspace, windows) = mock_scrolling_workspace(
      TilingDirection::Horizontal,
      2,
      0.5,
      &GapsConfig::default(),
      ScrollConfig::default(),
    );

    resize_tiling_container(&windows[0].clone().into(), 0.25);

    assert!((windows[0].tiling_size() - 0.25).abs() < f32::EPSILON);
    assert!((windows[1].tiling_size() - 0.5).abs() < f32::EPSILON);
  }

  #[test]
  fn clamps_strip_item_size() {
    let (_workspace, windows) = mock_scrolling_workspace(
      TilingDirection::Horizontal,
      1,
      0.5,
      &GapsConfig::default(),
      ScrollConfig::default(),
    );

    let strip_item = windows[0].clone();

    // A strip item can be at most the full viewport.
    resize_tiling_container(&strip_item.clone().into(), 2.0);
    assert!((strip_item.tiling_size() - 1.0).abs() < f32::EPSILON);

    // A strip item cannot be resized below the minimum tiling size.
    resize_tiling_container(&strip_item.clone().into(), 0.0);
    assert!(
      (strip_item.tiling_size() - MIN_TILING_SIZE).abs() < f32::EPSILON
    );
  }

  #[test]
  fn redistributes_among_siblings_in_tiling_split() {
    let first = TilingWindow::mock().call();
    let second = TilingWindow::mock().call();

    let _split = SplitContainer::mock()
      .tiling_direction(TilingDirection::Horizontal)
      .tiling_containers(vec![first.clone().into(), second.clone().into()])
      .call();

    resize_tiling_container(&first.clone().into(), 0.25);

    assert!((first.tiling_size() - 0.25).abs() < f32::EPSILON);
    assert!((second.tiling_size() - 0.75).abs() < f32::EPSILON);
  }
}
