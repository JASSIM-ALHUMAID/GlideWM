use super::flatten_split_container;
use crate::{
  layouts::{self, SplitChildFlattening},
  models::{Container, DirectionContainer},
  traits::{CommonGetters, TilingDirectionGetters},
};

/// Flattens any redundant split containers at the top-level of the given
/// parent container.
///
/// What counts as redundant depends on the parent's layout (see
/// [`layouts::split_child_flattening`]):
///
/// - The tiling layout folds redundant nesting into the parent:
///   ```ignore,compile_fail H[1 H[V[2, 3]]] -> H[1, 2, 3] H[1 H[2, 3]] ->
///   H[1, 2, 3] H[V[1]] -> V[1] ```
/// - The scrolling layout keeps the workspace's multi-window columns; only
///   a column that contains a single window is dissolved.
/// - The BSP layout keeps multi-child splits (they are the layout's binary
///   tree) and collapses any split that has a single child.
pub fn flatten_child_split_containers(
  parent: &Container,
) -> anyhow::Result<()> {
  let Ok(parent) = parent.as_direction_container() else {
    return Ok(());
  };

  match layouts::split_child_flattening(&parent) {
    SplitChildFlattening::Fold => fold_split_children(&parent),
    SplitChildFlattening::PreserveColumns => {
      preserve_column_split_children(&parent)
    }
    SplitChildFlattening::PreserveBinarySplits => {
      collapse_single_child_splits(&parent)
    }
  }
}

/// Folds redundant split children into the parent (tiling behavior).
fn fold_split_children(parent: &DirectionContainer) -> anyhow::Result<()> {
  let tiling_children = tiling_children_of(parent);

  if tiling_children.len() == 1 {
    // Handle case where the parent is a split container and has a
    // single split container child.
    if let Some(split_child) = tiling_children[0].as_split() {
      flatten_split_container(split_child.clone())?;
      parent.set_tiling_direction(parent.tiling_direction().inverse());
    }
  } else {
    flatten_same_direction_split_children(parent)?;
  }

  Ok(())
}

/// Keeps the workspace's multi-window columns; only a column that
/// contains a single window is dissolved (scrolling behavior).
///
/// A multi-window column that is the parent's only tiling child stays as
/// it is, and the parent's tiling direction (the scroll axis) is never
/// inverted.
fn preserve_column_split_children(
  parent: &DirectionContainer,
) -> anyhow::Result<()> {
  let tiling_children = tiling_children_of(parent);

  if tiling_children.len() == 1 {
    // A column is only redundant if it contains a single window.
    if let Some(split_child) = tiling_children[0].as_split() {
      if split_child.child_count() == 1 {
        flatten_split_container(split_child.clone())?;
      }
    }
  } else {
    flatten_same_direction_split_children(parent)?;
  }

  Ok(())
}

/// Collapses split children that have a single child, until none are
/// left (BSP behavior).
///
/// A binary split that lost one of its two children is redundant, while
/// multi-child splits are the layout's binary tree.
fn collapse_single_child_splits(
  parent: &DirectionContainer,
) -> anyhow::Result<()> {
  while let Some(redundant_split) = tiling_children_of(parent)
    .into_iter()
    .filter_map(|child| child.as_split().cloned())
    .find(|split| split.child_count() == 1)
  {
    flatten_split_container(redundant_split)?;
  }

  Ok(())
}

/// Flattens the same-direction split children of `parent`, e.g. the
/// nested `H[2, 3]` in `H[1 H[2, 3]]` becomes a direct child.
fn flatten_same_direction_split_children(
  parent: &DirectionContainer,
) -> anyhow::Result<()> {
  let split_children = tiling_children_of(parent)
    .into_iter()
    .filter_map(|child| child.as_split().cloned())
    .collect::<Vec<_>>();

  for split_child in split_children.iter().filter(|split_child| {
    split_child.tiling_direction() == parent.tiling_direction()
  }) {
    // Additionally flatten redundant top-level split containers in the
    // child.
    if split_child.child_count() == 1 {
      if let Some(split_grandchild) = split_child.children()[0].as_split()
      {
        flatten_split_container(split_grandchild.clone())?;
      }
    }

    flatten_split_container(split_child.clone())?;
  }

  Ok(())
}

/// Tiling children of a direction container: children that are either
/// tiling windows or split containers.
fn tiling_children_of(parent: &DirectionContainer) -> Vec<Container> {
  parent
    .children()
    .into_iter()
    .filter(|child| child.is_tiling_window() || child.is_split())
    .collect()
}

#[cfg(test)]
mod tests {
  use wm_common::{ScrollConfig, TilingDirection, WorkspaceLayout};

  use super::*;
  use crate::{
    commands::container::attach_container,
    models::{SplitContainer, TilingWindow, Workspace},
    traits::TilingSizeGetters,
  };

  #[test]
  fn preserves_columns_in_scrolling_workspace() {
    let workspace = Workspace::mock()
      .tiling_direction(TilingDirection::Horizontal)
      .layout(WorkspaceLayout::Scrolling)
      .scroll(ScrollConfig::default())
      .call();

    let top = TilingWindow::mock().call();
    let bottom = TilingWindow::mock().call();

    let column = SplitContainer::mock()
      .tiling_direction(TilingDirection::Vertical)
      .tiling_containers(vec![top.into(), bottom.into()])
      .call();

    attach_container(
      &column.clone().into(),
      &workspace.clone().into(),
      None,
    )
    .unwrap();

    flatten_child_split_containers(&workspace.clone().into()).unwrap();

    // The multi-window column is preserved, and the scroll axis is not
    // inverted.
    assert_eq!(workspace.child_count(), 1);
    assert_eq!(workspace.children()[0].id(), column.id());
    assert_eq!(workspace.tiling_direction(), TilingDirection::Horizontal);
  }

  #[test]
  fn flattens_single_window_column_without_inverting_axis() {
    let workspace = Workspace::mock()
      .tiling_direction(TilingDirection::Horizontal)
      .layout(WorkspaceLayout::Scrolling)
      .scroll(ScrollConfig::default())
      .call();

    let window = TilingWindow::mock().call();

    let column = SplitContainer::mock()
      .tiling_direction(TilingDirection::Vertical)
      .tiling_containers(vec![window.clone().into()])
      .call();

    attach_container(
      &column.clone().into(),
      &workspace.clone().into(),
      None,
    )
    .unwrap();
    column.set_tiling_size(0.5);

    flatten_child_split_containers(&workspace.clone().into()).unwrap();

    // The redundant column is flattened, but the scroll axis is not
    // inverted.
    assert_eq!(workspace.child_count(), 1);
    assert_eq!(workspace.children()[0].id(), window.id());
    assert_eq!(workspace.tiling_direction(), TilingDirection::Horizontal);

    // The window keeps its effective size: half of the viewport.
    assert!((window.tiling_size() - 0.5).abs() < f32::EPSILON);
  }

  #[test]
  fn collapses_single_child_split_in_bsp_workspace() {
    let workspace = Workspace::mock().layout(WorkspaceLayout::Bsp).call();

    let window = TilingWindow::mock().call();

    let split = SplitContainer::mock()
      .tiling_direction(TilingDirection::Horizontal)
      .tiling_containers(vec![window.clone().into()])
      .call();

    attach_container(
      &split.clone().into(),
      &workspace.clone().into(),
      None,
    )
    .unwrap();

    flatten_child_split_containers(&workspace.clone().into()).unwrap();

    // The redundant root split is dissolved: a binary split that lost
    // one of its children is not part of the layout's tree.
    assert_eq!(workspace.child_count(), 1);
    assert_eq!(workspace.children()[0].id(), window.id());
  }

  #[test]
  fn preserves_multi_child_splits_in_bsp_workspace() {
    let workspace = Workspace::mock().layout(WorkspaceLayout::Bsp).call();

    let first = TilingWindow::mock().call();
    let second = TilingWindow::mock().call();
    let third = TilingWindow::mock().call();

    // The nested split has the same direction as the workspace, like
    // `H[1 H[2, 3]]` for the tiling layout (which would be folded).
    let nested = SplitContainer::mock()
      .tiling_direction(TilingDirection::Horizontal)
      .tiling_containers(vec![second.into(), third.into()])
      .call();

    attach_container(
      &first.clone().into(),
      &workspace.clone().into(),
      None,
    )
    .unwrap();
    attach_container(
      &nested.clone().into(),
      &workspace.clone().into(),
      None,
    )
    .unwrap();

    flatten_child_split_containers(&workspace.clone().into()).unwrap();

    // Multi-child splits are the binary tree of the layout: they are
    // kept as-is.
    assert_eq!(workspace.child_count(), 2);
    assert_eq!(workspace.children()[1].id(), nested.id());
    assert_eq!(nested.child_count(), 2);
  }
}
