use anyhow::Context;
use wm_common::{InvokeUpdateWorkspaceConfig, WmEvent, WorkspaceConfig};

use super::sort_workspaces;
use crate::{
  models::Workspace, traits::CommonGetters, user_config::UserConfig,
  wm_state::WmState,
};

/// Updates the config of a workspace at runtime (name, display name,
/// monitor binding, or keep-alive).
///
/// The updated config is recorded as a session override (see
/// [`WmState::set_workspace_config_override`]), so that the update
/// survives the workspace being destroyed while empty. A renamed
/// workspace drops the override under its previous name.
pub fn update_workspace_config(
  workspace: &Workspace,
  state: &mut WmState,
  config: &UserConfig,
  new_config: &InvokeUpdateWorkspaceConfig,
) -> anyhow::Result<()> {
  let current_config = workspace.config();

  // Validate the workspace name change.
  if let Some(new_name) = &new_config.name {
    if new_name != &current_config.name {
      if let Some(_other_workspace) = state.workspace_by_name(new_name) {
        anyhow::bail!("The workspace \"{}\" already exists", new_name);
      }
    }
  }

  // Update the config with the incoming values.
  let updated_config = WorkspaceConfig {
    name: new_config
      .name
      .clone()
      .unwrap_or(current_config.name.clone()),
    display_name: new_config
      .display_name
      .clone()
      .or(current_config.display_name.clone()),
    bind_to_monitor: new_config
      .bind_to_monitor
      .or(current_config.bind_to_monitor),
    keep_alive: new_config.keep_alive.unwrap_or(current_config.keep_alive),
    layout: current_config.layout.clone(),
    scroll: current_config.scroll.clone(),
    bsp: current_config.bsp.clone(),
    layout_switch: current_config.layout_switch,
  };

  workspace.set_config(updated_config.clone());

  // Keep the update as a session override, so that it survives the
  // workspace being destroyed while empty. The previous name's override
  // is dropped, since the freed-up name might later belong to a
  // different workspace.
  if updated_config.name != current_config.name {
    state.remove_workspace_config_override(&current_config.name);
  }
  state.set_workspace_config_override(updated_config);

  sort_workspaces(
    &workspace.monitor().context("No displayed workspace.")?,
    config,
  )?;

  // TODO: Re-assign bound workspaces to their respective monitors.

  state.emit_event(WmEvent::WorkspaceUpdated {
    updated_workspace: workspace.to_dto()?,
  });

  Ok(())
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::models::{Monitor, Workspace};

  /// Runs `update_workspace_config` with the given new config on a mock
  /// workspace attached to a mock monitor.
  fn run_update(
    workspace: &Workspace,
    state: &mut WmState,
    new_config: &InvokeUpdateWorkspaceConfig,
  ) {
    update_workspace_config(
      workspace,
      state,
      &UserConfig::mock(),
      new_config,
    )
    .unwrap();
  }

  #[test]
  fn records_config_as_session_override() {
    let workspace = Workspace::mock().call();
    let _monitor =
      Monitor::mock().workspaces(vec![workspace.clone()]).call();

    let mut state = WmState::mock();

    run_update(
      &workspace,
      &mut state,
      &InvokeUpdateWorkspaceConfig {
        name: None,
        display_name: Some("renamed".to_string()),
        bind_to_monitor: None,
        keep_alive: None,
      },
    );

    // The updated config is kept as a session override, so that the
    // update survives the workspace being destroyed while empty.
    let override_config =
      state.workspace_config_overrides.get("1").unwrap();

    assert_eq!(override_config.display_name, Some("renamed".to_string()));
  }

  #[test]
  fn moves_session_override_on_rename() {
    let workspace = Workspace::mock().call();
    let _monitor =
      Monitor::mock().workspaces(vec![workspace.clone()]).call();

    let mut state = WmState::mock();
    state.set_workspace_config_override(workspace.config());

    run_update(
      &workspace,
      &mut state,
      &InvokeUpdateWorkspaceConfig {
        name: Some("2".to_string()),
        display_name: None,
        bind_to_monitor: None,
        keep_alive: None,
      },
    );

    // The override follows the workspace to its new name, and the
    // freed-up name no longer has one.
    assert!(!state.workspace_config_overrides.contains_key("1"));
    assert!(state.workspace_config_overrides.contains_key("2"));
  }
}
