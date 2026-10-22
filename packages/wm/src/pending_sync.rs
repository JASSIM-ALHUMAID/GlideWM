use std::collections::HashMap;

use uuid::Uuid;

use crate::{
  models::{Container, Workspace},
  traits::CommonGetters,
};

#[derive(Debug, Default)]
#[allow(clippy::struct_excessive_bools)]
pub struct PendingSync {
  /// Containers (and their descendants) that have a pending redraw.
  containers_to_redraw: HashMap<Uuid, Container>,

  /// Workspaces where z-order should be updated. Windows that match the
  /// focused window's state should be brought to the front.
  workspaces_to_reorder: Vec<Workspace>,

  /// Workspaces whose strip contents changed, where the focused
  /// descendant should be scrolled back into view. Only has an effect on
  /// workspaces that use the scrolling layout.
  workspaces_to_rescroll: Vec<Workspace>,

  /// Whether native focus should be reassigned to the WM's focused
  /// container.
  needs_focus_update: bool,

  /// Whether window effect for the focused window should be updated.
  needs_focused_effect_update: bool,

  /// Whether window effects for all windows should be updated.
  needs_all_effects_update: bool,

  /// Whether to jump the cursor to the focused container (if enabled in
  /// user config).
  needs_cursor_jump: bool,
}

impl PendingSync {
  pub fn has_changes(&self) -> bool {
    !self.containers_to_redraw.is_empty()
      || !self.workspaces_to_reorder.is_empty()
      || !self.workspaces_to_rescroll.is_empty()
      || self.needs_focus_update
      || self.needs_focused_effect_update
      || self.needs_all_effects_update
      || self.needs_cursor_jump
  }

  pub fn clear(&mut self) -> &mut Self {
    self.containers_to_redraw.clear();
    self.workspaces_to_reorder.clear();
    self.workspaces_to_rescroll.clear();
    self.needs_focus_update = false;
    self.needs_focused_effect_update = false;
    self.needs_all_effects_update = false;
    self.needs_cursor_jump = false;
    self
  }

  pub fn queue_container_to_redraw<T>(&mut self, container: T) -> &mut Self
  where
    T: Into<Container>,
  {
    let container: Container = container.into();
    self.containers_to_redraw.insert(container.id(), container);
    self
  }

  pub fn queue_containers_to_redraw<I, T>(
    &mut self,
    containers: I,
  ) -> &mut Self
  where
    I: IntoIterator<Item = T>,
    T: Into<Container>,
  {
    for container in containers {
      let container: Container = container.into();
      self.containers_to_redraw.insert(container.id(), container);
    }

    self
  }

  pub fn dequeue_container_from_redraw<T>(
    &mut self,
    container: T,
  ) -> &mut Self
  where
    T: Into<Container>,
  {
    self.containers_to_redraw.remove(&container.into().id());
    self
  }

  pub fn queue_workspace_to_reorder(
    &mut self,
    workspace: Workspace,
  ) -> &mut Self {
    self.workspaces_to_reorder.push(workspace);
    self
  }

  /// Queues the focused descendant of the workspace to be scrolled back
  /// into view on the next sync.
  ///
  /// Used when the strip contents of a workspace change without a focus
  /// change (e.g. a window exiting fullscreen), since scroll offsets are
  /// otherwise only re-ensured on focus changes.
  pub fn queue_workspace_to_rescroll(
    &mut self,
    workspace: Workspace,
  ) -> &mut Self {
    if !self.needs_rescroll(&workspace) {
      self.workspaces_to_rescroll.push(workspace);
    }

    self
  }

  pub fn queue_focus_change(&mut self) -> &mut Self {
    self.needs_focus_update = true;
    self
  }

  pub fn queue_focused_effect_update(&mut self) -> &mut Self {
    self.needs_focused_effect_update = true;
    self
  }

  pub fn queue_all_effects_update(&mut self) -> &mut Self {
    self.needs_all_effects_update = true;
    self
  }

  pub fn queue_cursor_jump(&mut self) -> &mut Self {
    self.needs_cursor_jump = true;
    self
  }

  pub fn needs_focus_update(&self) -> bool {
    self.needs_focus_update
  }

  /// Whether the workspace is queued to have its focused descendant
  /// scrolled back into view.
  pub fn needs_rescroll(&self, workspace: &Workspace) -> bool {
    self
      .workspaces_to_rescroll
      .iter()
      .any(|queued| queued.id() == workspace.id())
  }

  pub fn needs_focused_effect_update(&self) -> bool {
    self.needs_focused_effect_update
  }

  pub fn needs_all_effects_update(&self) -> bool {
    self.needs_all_effects_update
  }

  pub fn needs_cursor_jump(&self) -> bool {
    self.needs_cursor_jump
  }

  pub fn containers_to_redraw(&self) -> &HashMap<Uuid, Container> {
    &self.containers_to_redraw
  }

  pub fn workspaces_to_reorder(&self) -> &Vec<Workspace> {
    &self.workspaces_to_reorder
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn queues_workspaces_to_rescroll() {
    let workspace = Workspace::mock().call();
    let other_workspace = Workspace::mock().call();
    let mut pending_sync = PendingSync::default();

    assert!(!pending_sync.needs_rescroll(&workspace));

    pending_sync
      .queue_workspace_to_rescroll(workspace.clone())
      .queue_workspace_to_rescroll(workspace.clone());

    assert!(pending_sync.has_changes());
    assert!(pending_sync.needs_rescroll(&workspace));
    assert!(!pending_sync.needs_rescroll(&other_workspace));
    assert_eq!(pending_sync.workspaces_to_rescroll.len(), 1);

    pending_sync.clear();
    assert!(!pending_sync.needs_rescroll(&workspace));
    assert!(!pending_sync.has_changes());
  }
}
