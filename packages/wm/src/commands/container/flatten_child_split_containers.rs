use super::flatten_split_container;
use crate::{
  layouts,
  models::Container,
  traits::{CommonGetters, TilingDirectionGetters},
};

/// Flattens any redundant split containers at the top-level of the given
/// parent container.
///
/// For example:
/// ```ignore,compile_fail
/// H[1 H[V[2, 3]]] -> H[1, 2, 3]
/// H[1 H[2, 3]] -> H[1, 2, 3]
/// H[V[1]] -> V[1]
/// ```
///
/// For layouts that preserve top-level split children as explicit
/// "columns" (see [`layouts::preserves_split_children`]), a column is
/// only flattened when it contains a single window, and the parent's
/// tiling direction is never inverted.
pub fn flatten_child_split_containers(
  parent: &Container,
) -> anyhow::Result<()> {
  if let Ok(parent) = parent.as_direction_container() {
    let preserves_split_children =
      layouts::preserves_split_children(&parent);

    // Get children that are either tiling windows or split containers.
    let tiling_children = parent
      .children()
      .into_iter()
      .filter(|child| child.is_tiling_window() || child.is_split())
      .collect::<Vec<_>>();

    if tiling_children.len() == 1 {
      // Handle case where the parent is a split container and has a
      // single split container child.
      if let Some(split_child) = tiling_children[0].as_split() {
        // A column is only redundant if it contains a single window.
        // Multi-window columns are preserved.
        if preserves_split_children {
          if split_child.child_count() == 1 {
            flatten_split_container(split_child.clone())?;
          }

          return Ok(());
        }

        flatten_split_container(split_child.clone())?;
        parent.set_tiling_direction(parent.tiling_direction().inverse());
      }
    } else {
      let split_children = tiling_children
        .into_iter()
        .filter_map(|child| child.as_split().cloned())
        .collect::<Vec<_>>();

      for split_child in split_children.iter().filter(|split_child| {
        split_child.tiling_direction() == parent.tiling_direction()
      }) {
        // Additionally flatten redundant top-level split containers in
        // the child.
        if split_child.child_count() == 1 {
          if let Some(split_grandchild) =
            split_child.children()[0].as_split()
          {
            flatten_split_container(split_grandchild.clone())?;
          }
        }

        flatten_split_container(split_child.clone())?;
      }
    }
  }

  Ok(())
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
}
