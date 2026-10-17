//! The default tiling layout.
//!
//! Tiling children share the space of their parent; each child's
//! `tiling_size` is its share of that space alongside its siblings, and
//! resizing one child resizes the others. This module implements the
//! default behavior that other layouts override (see `super`).

use anyhow::Context;
use wm_common::{TilingDirection, WindowState};
use wm_platform::{Direction, Rect};

use crate::{
  commands::container::{
    flatten_child_split_containers, move_container_within_tree,
    wrap_in_split_container,
  },
  models::{
    Container, SplitContainer, TilingContainer, TilingWindow,
    WindowContainer, Workspace,
  },
  traits::{
    CommonGetters, PositionGetters, TilingDirectionGetters,
    TilingSizeGetters, MIN_TILING_SIZE,
  },
  user_config::UserConfig,
  wm_state::WmState,
};

/// Rect of a tiling child that shares the space of its parent with its
/// siblings.
///
/// This is the generic share-based geometry used for all tiling
/// containers; direct children of a workspace dispatch through
/// [`super::child_rect`] instead, which falls back to this for the
/// tiling layout.
pub(crate) fn shared_child_rect(
  child: &TilingContainer,
  parent_direction: &TilingDirection,
  parent_rect: &Rect,
) -> anyhow::Result<Rect> {
  let (horizontal_gap, vertical_gap) = child.inner_gaps()?;
  let inner_gap = match parent_direction {
    TilingDirection::Vertical => vertical_gap,
    TilingDirection::Horizontal => horizontal_gap,
  };

  #[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap
  )]
  let (width, height) = match parent_direction {
    TilingDirection::Vertical => {
      let available_height = parent_rect.height()
        - inner_gap * child.tiling_siblings().count() as i32;

      let height = (child.tiling_size() * available_height as f32) as i32;

      (parent_rect.width(), height)
    }
    TilingDirection::Horizontal => {
      let available_width = parent_rect.width()
        - inner_gap * child.tiling_siblings().count() as i32;

      let width =
        (available_width as f32 * child.tiling_size()).round() as i32;

      (width, parent_rect.height())
    }
  };

  let (x, y) = {
    let mut prev_siblings = child
      .prev_siblings()
      .filter_map(|sibling| sibling.as_tiling_container().ok());

    match prev_siblings.next() {
      None => (parent_rect.x(), parent_rect.y()),
      Some(sibling) => {
        let sibling_rect = sibling.to_rect()?;

        match parent_direction {
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

/// Rect of a direct child of a tiling workspace.
pub(super) fn child_rect(
  child: &TilingContainer,
  workspace: &Workspace,
  parent_rect: &Rect,
) -> anyhow::Result<Rect> {
  shared_child_rect(child, &workspace.tiling_direction(), parent_rect)
}

/// Sizes `child` to an even share of the space alongside its siblings
/// after it was attached to a tiling container.
pub(super) fn size_attached_child(child: &TilingContainer) {
  let tiling_siblings = child.tiling_siblings().collect::<Vec<_>>();

  if tiling_siblings.is_empty() {
    child.set_tiling_size(1.0);
    return;
  }

  // Set initial tiling size to 0, and then size up the container
  // to the target size.
  #[allow(clippy::cast_precision_loss)]
  let target_size = 1.0 / (tiling_siblings.len() + 1) as f32;

  child.set_tiling_size(0.0);
  resize_container(child, target_size);
}

/// Redistributes the space freed up by detaching `child_to_remove` from
/// `parent` amongst the remaining siblings.
pub(super) fn redistribute_freed_space(
  child_to_remove: &TilingContainer,
  parent: &Container,
) {
  let tiling_siblings = parent.tiling_children().collect::<Vec<_>>();

  // TODO: Share logic with `resize_container`.
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

/// Resizes a tiling container so that its tiling size becomes the given
/// target size.
///
/// The sizes that are freed up by the resize are distributed amongst the
/// container's tiling siblings.
pub(super) fn resize_container(
  container_to_resize: &TilingContainer,
  target_size: f32,
) {
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

/// Gets where to insert a new window in the container tree.
///
/// Rules:
/// - For tiling windows:
///   1. Try to insert after the focused tiling window if one exists.
///   2. If a non-tiling window is focused, try to insert after the first
///      tiling window found.
///   3. If no tiling windows exist, append to the workspace.
/// - For non-tiling windows: Always append to the workspace.
pub(super) fn insertion_target(
  window_state: &WindowState,
  focused_container: &Container,
  focused_workspace: &Workspace,
) -> anyhow::Result<(Container, usize)> {
  if *window_state == WindowState::Tiling {
    let sibling = match focused_container {
      Container::TilingWindow(_) => Some(focused_container.clone()),
      _ => focused_workspace
        .descendant_focus_order()
        .find(Container::is_tiling_window),
    };

    if let Some(sibling) = sibling {
      return Ok((
        sibling.parent().context("No parent.")?,
        sibling.index() + 1,
      ));
    }
  }

  Ok((
    focused_workspace.clone().into(),
    focused_workspace.child_count(),
  ))
}

/// Inserts a window into the target workspace.
///
/// The window is inserted into the sibling's parent at the sibling's
/// index + 1, or appended to the workspace if there is no sibling to
/// insert after.
pub(super) fn insert_window_into_workspace(
  window: &WindowContainer,
  target_workspace: &Workspace,
  insertion_sibling: Option<WindowContainer>,
  state: &mut WmState,
) -> anyhow::Result<()> {
  match (window.is_tiling_window(), insertion_sibling) {
    (true, Some(insertion_sibling)) => move_container_within_tree(
      &window.clone().into(),
      &insertion_sibling.clone().parent().context("No parent.")?,
      insertion_sibling.index() + 1,
      state,
    ),
    _ => move_container_within_tree(
      &window.clone().into(),
      &target_workspace.clone().into(),
      target_workspace.child_count(),
      state,
    ),
  }
}

/// Moves a window that has no ancestor that can contain the move, by
/// inverting the workspace's tiling direction to make room for it.
pub(super) fn invert_workspace_direction(
  window_to_move: &TilingWindow,
  direction: &Direction,
  state: &mut WmState,
  config: &UserConfig,
) -> anyhow::Result<()> {
  let workspace = window_to_move.workspace().context("No workspace.")?;

  // Get top-level tiling children of the workspace.
  let workspace_children = workspace
    .tiling_children()
    .filter(|container| container.id() != window_to_move.id())
    .collect::<Vec<_>>();

  // Create a new split container to wrap the window's siblings. For
  // example, in the layout H[1 V[2 3]] where container 3 is moved down,
  // we create a split container around 1 and 2. This results in
  // H[H[1 V[2 3]]], and V[H[1 V[2]] 3] after the tiling direction change.
  if workspace_children.len() > 1 {
    let split_container = SplitContainer::new(
      workspace.tiling_direction(),
      config.value.gaps.clone(),
    );

    wrap_in_split_container(
      &split_container,
      &workspace.clone().into(),
      &workspace_children,
    )?;
  }

  // Invert the tiling direction of the workspace.
  workspace.set_tiling_direction(workspace.tiling_direction().inverse());

  let target_index = match direction {
    Direction::Left | Direction::Up => 0,
    _ => workspace.child_count(),
  };

  // Depending on the direction, place the window either before or after
  // the split container.
  move_container_within_tree(
    &window_to_move.clone().into(),
    &workspace.clone().into(),
    target_index,
    state,
  )?;

  // Workspace might have redundant split containers after the tiling
  // direction change. For example, V[H[1 2] 3] where container 3 is moved
  // up results in H[3 H[1 2]], and needs to be flattened to H[3 1 2].
  flatten_child_split_containers(&workspace.clone().into())?;

  // Resize the window such that the split container and window are each
  // 0.5.
  super::resize_tiling_container(&window_to_move.clone().into(), 0.5);

  state
    .pending_sync
    .queue_containers_to_redraw(workspace.tiling_children());

  Ok(())
}
