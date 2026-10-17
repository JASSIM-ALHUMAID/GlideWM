use anyhow::Context;
use tracing::{info, warn};
#[cfg(target_os = "windows")]
use wm_common::{HideMethod, ParsedConfig};
use wm_common::{WindowRuleEvent, WmEvent, WorkspaceLayout};
#[cfg(target_os = "windows")]
use wm_platform::NativeWindowWindowsExt;

use crate::{
  commands::{
    window::run_window_rules,
    workspace::{normalize_for_scrolling, sort_workspaces},
  },
  traits::{
    CommonGetters, TilingDirectionGetters, TilingSizeGetters,
    WindowGetters,
  },
  user_config::UserConfig,
  wm::WindowManager,
  wm_state::WmState,
};

pub fn reload_config(
  state: &mut WmState,
  config: &mut UserConfig,
) -> anyhow::Result<()> {
  info!("Config reloaded.");

  // Keep reference to old config for comparison.
  #[cfg(target_os = "windows")]
  let old_config = config.value.clone();

  // Re-evaluate user config file and set its values in state.
  config.reload()?;

  // Re-run window rules on all active windows.
  for window in state.windows() {
    window.set_done_window_rules(Vec::new());
    run_window_rules(window, &WindowRuleEvent::Manage, state, config)?;
  }

  update_workspace_configs(state, config)?;

  update_container_gaps(state, config);

  #[cfg(target_os = "windows")]
  update_window_effects(&old_config, state, config)?;

  // Ensure all windows are shown when hide method is changed.
  #[cfg(target_os = "windows")]
  if old_config.general.hide_method != config.value.general.hide_method
    && config.value.general.hide_method == HideMethod::Cloak
  {
    for window in state.windows() {
      let _ = window.native().show();
    }
  }

  // Ensure all windows are shown in taskbar when `show_all_in_taskbar` is
  // changed.
  #[cfg(target_os = "windows")]
  if old_config.general.show_all_in_taskbar
    != config.value.general.show_all_in_taskbar
    && config.value.general.show_all_in_taskbar
  {
    for window in state.windows() {
      let _ = window.native().set_taskbar_visibility(true);
    }
  }

  // Clear active binding modes.
  state.binding_modes = Vec::new();

  // Redraw full container tree.
  state
    .pending_sync
    .queue_container_to_redraw(state.root_container.clone());

  // Emit the updated config.
  state.emit_event(WmEvent::UserConfigChanged {
    config_path: config
      .path
      .to_str()
      .context("Invalid config path.")?
      .to_string(),
    config_string: config.value_str.clone(),
    parsed_config: config.value.clone(),
  });

  // Run config reload commands.
  WindowManager::run_commands(
    &config.value.general.config_reload_commands.clone(),
    state.focused_container().context("No focused container.")?,
    state,
    config,
  )?;

  Ok(())
}

/// Updates configs of active workspaces.
///
/// When a workspace's config switches it to the scrolling layout, its
/// state is normalized the same way as in the `set-layout` command (see
/// `normalize_for_scrolling`), and the scroll axis from the config is
/// applied as the workspace's tiling direction.
fn update_workspace_configs(
  state: &mut WmState,
  config: &UserConfig,
) -> anyhow::Result<()> {
  let workspaces = state.workspaces();

  for workspace in &workspaces {
    let monitor = workspace.monitor().context("No monitor.")?;

    let workspace_config = config
      .value
      .workspaces
      .iter()
      .find(|config| config.name == workspace.config().name)
      .or_else(|| {
        // When the workspace config is not found, the current name of the
        // workspace has been removed. So, we reassign the first suitable
        // workspace config to the workspace.
        config
          .workspace_config_for_monitor(&monitor, &workspaces)
          .or_else(|| config.next_inactive_workspace_config(&workspaces))
      });

    match workspace_config {
      None => {
        warn!(
          "Unable to update workspace config. No available workspace configs."
        );
      }
      Some(workspace_config) => {
        if *workspace_config != workspace.config() {
          let prev_config = workspace.config();

          workspace.set_config(workspace_config.clone());

          if workspace_config.layout == WorkspaceLayout::Scrolling {
            // The scroll axis doubles as the workspace's tiling
            // direction while the scrolling layout is active. Keep it
            // in sync with the config.
            if prev_config.layout != WorkspaceLayout::Scrolling
              || prev_config.scroll.axis != workspace_config.scroll.axis
            {
              workspace.set_tiling_direction(
                workspace_config.scroll.axis.clone(),
              );
            }

            // Normalize state when the layout was just switched to
            // scrolling, so that the switch behaves like the
            // `set-layout` command.
            if prev_config.layout != WorkspaceLayout::Scrolling {
              normalize_for_scrolling(workspace, state)?;
            }
          }

          sort_workspaces(&monitor, config)?;

          state.emit_event(WmEvent::WorkspaceUpdated {
            updated_workspace: workspace.to_dto()?,
          });
        }
      }
    }
  }

  Ok(())
}

/// Updates outer gap of workspaces and inner gaps of tiling containers.
fn update_container_gaps(state: &mut WmState, config: &UserConfig) {
  let tiling_containers = state
    .root_container
    .self_and_descendants()
    .filter_map(|container| container.as_tiling_container().ok());

  for container in tiling_containers {
    container.set_gaps_config(config.value.gaps.clone());
  }

  for workspace in state.workspaces() {
    workspace.set_gaps_config(config.value.gaps.clone());
  }
}

#[cfg(target_os = "windows")]
fn update_window_effects(
  old_config: &ParsedConfig,
  state: &mut WmState,
  config: &UserConfig,
) -> anyhow::Result<()> {
  let focused_container =
    state.focused_container().context("No focused container.")?;

  let window_effects = &config.value.window_effects;
  let old_window_effects = &old_config.window_effects;

  // Window border effects are left at system defaults if disabled in the
  // config. However, when transitioning from colored borders to having
  // them disabled, it's best to reset to the system defaults.
  if !window_effects.focused_window.border.enabled
    && old_window_effects.focused_window.border.enabled
  {
    if let Ok(window) = focused_container.as_window_container() {
      _ = window.native().set_border_color(None);
    }
  }

  if !window_effects.other_windows.border.enabled
    && old_window_effects.other_windows.border.enabled
  {
    let unfocused_windows = state
      .windows()
      .into_iter()
      .filter(|window| window.id() != focused_container.id());

    for window in unfocused_windows {
      _ = window.native().set_border_color(None);
    }
  }

  state.pending_sync.queue_all_effects_update();

  Ok(())
}

#[cfg(test)]
mod tests {
  use wm_common::{
    ScrollConfig, TilingDirection, WorkspaceConfig, WorkspaceLayout,
  };

  use super::*;
  use crate::{
    commands::container::{attach_container, set_focused_descendant},
    models::{Monitor, TilingWindow, Workspace},
    traits::{
      PositionGetters, TilingDirectionGetters, TilingSizeGetters,
      MIN_TILING_SIZE,
    },
  };

  /// Creates a `UserConfig` that switches the given workspace to the
  /// scrolling layout with the given scroll axis.
  fn config_for_workspace_layout(
    workspace: &Workspace,
    axis: TilingDirection,
  ) -> UserConfig {
    let mut config = UserConfig::mock();

    config.value.workspaces = vec![WorkspaceConfig {
      name: workspace.config().name.clone(),
      display_name: None,
      bind_to_monitor: None,
      keep_alive: false,
      layout: WorkspaceLayout::Scrolling,
      scroll: ScrollConfig {
        axis,
        ..ScrollConfig::default()
      },
    }];

    config
  }

  #[test]
  fn clamps_sizes_and_applies_axis_on_layout_change() {
    let workspace = Workspace::mock().call();
    let first = TilingWindow::mock().call();
    let second = TilingWindow::mock().call();

    attach_container(
      &first.clone().into(),
      &workspace.clone().into(),
      None,
    )
    .unwrap();
    attach_container(
      &second.clone().into(),
      &workspace.clone().into(),
      None,
    )
    .unwrap();

    // A size that is out of range when interpreted as a viewport
    // fraction.
    first.set_tiling_size(0.005);
    second.set_tiling_size(0.75);

    let monitor =
      Monitor::mock().workspaces(vec![workspace.clone()]).call();

    let mut state = WmState::mock();
    attach_container(
      &monitor.clone().into(),
      &state.root_container.clone().into(),
      None,
    )
    .unwrap();

    let config =
      config_for_workspace_layout(&workspace, TilingDirection::Vertical);

    update_workspace_configs(&mut state, &config).unwrap();

    assert!(workspace.is_scrolling());

    // The config's scroll axis is applied as the tiling direction.
    assert_eq!(workspace.tiling_direction(), TilingDirection::Vertical);

    // Out-of-range sizes are clamped to valid viewport fractions.
    assert!((first.tiling_size() - MIN_TILING_SIZE).abs() < f32::EPSILON);
    assert!((second.tiling_size() - 0.75).abs() < f32::EPSILON);

    // The stale scroll offset is reset.
    assert_eq!(workspace.scroll_offset(), 0);
  }

  #[test]
  fn scrolls_focused_item_into_view_on_layout_change() {
    let workspace = Workspace::mock().call();
    let first = TilingWindow::mock().call();
    let second = TilingWindow::mock().call();

    attach_container(
      &first.clone().into(),
      &workspace.clone().into(),
      None,
    )
    .unwrap();
    attach_container(
      &second.clone().into(),
      &workspace.clone().into(),
      None,
    )
    .unwrap();

    // The strip is wider than the viewport, so the second window needs
    // scrolling into view.
    first.set_tiling_size(0.75);
    second.set_tiling_size(0.75);

    let monitor =
      Monitor::mock().workspaces(vec![workspace.clone()]).call();

    let mut state = WmState::mock();
    attach_container(
      &monitor.clone().into(),
      &state.root_container.clone().into(),
      None,
    )
    .unwrap();

    set_focused_descendant(&second.clone().into(), None);

    let config =
      config_for_workspace_layout(&workspace, TilingDirection::Horizontal);

    update_workspace_configs(&mut state, &config).unwrap();

    assert_eq!(
      workspace.scroll_offset(),
      workspace.max_scroll_offset().unwrap()
    );

    let viewport_rect = workspace.to_rect().unwrap();
    let item_rect = second.to_rect().unwrap();

    assert!(item_rect.x() >= viewport_rect.x());
    assert!(item_rect.right <= viewport_rect.right);
  }
}
