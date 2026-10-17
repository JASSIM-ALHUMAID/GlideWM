use anyhow::Context;
use tracing::info;
use wm_common::{WmEvent, WorkspaceConfig};

use super::sort_workspaces;
use crate::{
  commands::container::attach_container,
  layouts,
  models::{Monitor, Workspace},
  traits::{CommonGetters, PositionGetters},
  user_config::UserConfig,
  wm_state::WmState,
};

/// Activates a workspace on the target monitor.
///
/// If no workspace name is provided, the first suitable workspace defined
/// in the user's config will be used.
///
/// If no target monitor is provided, the workspace is activated on
/// whichever monitor it is bound to, or the currently focused monitor.
pub fn activate_workspace(
  workspace_name: Option<&str>,
  target_monitor: Option<Monitor>,
  state: &mut WmState,
  config: &UserConfig,
) -> anyhow::Result<()> {
  let workspace_config = workspace_config(
    workspace_name,
    target_monitor.clone(),
    state,
    config,
  )?;

  let target_monitor = target_monitor
    .or_else(|| {
      workspace_config
        .bind_to_monitor
        .and_then(|index| {
          state
            .monitors()
            .into_iter()
            .find(|monitor| monitor.index() == index as usize)
        })
        .or_else(|| {
          state
            .focused_container()
            .and_then(|focused| focused.monitor())
        })
    })
    .context("Failed to get a target monitor for the workspace.")?;

  let monitor_rect = target_monitor.to_rect()?;
  let tiling_direction =
    layouts::initial_tiling_direction(&workspace_config, &monitor_rect);

  let workspace = Workspace::new(
    workspace_config.clone(),
    config.value.gaps.clone(),
    tiling_direction,
  );

  // Attach the created workspace to the specified monitor.
  attach_container(
    &workspace.clone().into(),
    &target_monitor.clone().into(),
    None,
  )?;

  sort_workspaces(&target_monitor, config)?;

  info!("Activating workspace: {workspace}");

  state.emit_event(WmEvent::WorkspaceActivated {
    activated_workspace: workspace.to_dto()?,
  });

  Ok(())
}

/// Gets config for the workspace to activate.
///
/// Runtime config changes made during this session take precedence over
/// the config file (see [`WmState::set_workspace_config_override`]).
fn workspace_config(
  workspace_name: Option<&str>,
  target_monitor: Option<Monitor>,
  state: &mut WmState,
  config: &UserConfig,
) -> anyhow::Result<WorkspaceConfig> {
  let found_config = match workspace_name {
    Some(workspace_name) => config
      .inactive_workspace_configs(&state.workspaces())
      .into_iter()
      .find(|config| config.name == workspace_name)
      .with_context(|| {
        format!(
          "Workspace with name '{workspace_name}' doesn't exist or is already active."
        )
      }),
    None => target_monitor
      .and_then(|target_monitor| {
        config.workspace_config_for_monitor(
          &target_monitor,
          &state.workspaces(),
        )
      })
      .or_else(|| {
        config.next_inactive_workspace_config(&state.workspaces())
      })
      .context("No workspace config available to activate workspace."),
  }?;

  Ok(
    state
      .workspace_config_overrides
      .get(&found_config.name)
      .cloned()
      .unwrap_or_else(|| found_config.clone()),
  )
}

#[cfg(test)]
mod tests {
  use wm_common::{ScrollConfig, WorkspaceLayout};

  use super::*;

  /// Creates a mock user config with a single scrolling workspace named
  /// "1".
  fn mock_config() -> UserConfig {
    let mut config = UserConfig::mock();

    config.value.workspaces = vec![WorkspaceConfig {
      name: "1".to_string(),
      display_name: None,
      bind_to_monitor: None,
      keep_alive: false,
      layout: WorkspaceLayout::Scrolling,
      scroll: ScrollConfig::default(),
    }];

    config
  }

  #[test]
  fn uses_config_file_without_override() {
    let mut state = WmState::mock();
    let config = mock_config();

    let found_config =
      workspace_config(Some("1"), None, &mut state, &config).unwrap();

    assert_eq!(found_config.layout, WorkspaceLayout::Scrolling);
  }

  #[test]
  fn prefers_session_config_override() {
    let mut state = WmState::mock();

    let mut override_config = mock_config().value.workspaces[0].clone();
    override_config.layout = WorkspaceLayout::Tiling;
    state.set_workspace_config_override(override_config);

    let config = mock_config();

    let found_config =
      workspace_config(Some("1"), None, &mut state, &config).unwrap();

    assert_eq!(found_config.layout, WorkspaceLayout::Tiling);
  }
}
