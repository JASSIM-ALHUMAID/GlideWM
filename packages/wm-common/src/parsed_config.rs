use std::str::FromStr;

use anyhow::bail;
use serde::{Deserialize, Serialize};
use wm_platform::{
  Color, CornerStyle, Key, Keybinding, LengthValue, OpacityValue,
  RectDelta,
};

use crate::{app_command::InvokeCommand, TilingDirection};

/// Parsed user config.
///
/// The config is deserialized through [`RawParsedConfig`], which resolves
/// the global layout config and the per-workspace overrides into fully
/// populated workspace configs.
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all(serialize = "camelCase"))]
pub struct ParsedConfig {
  pub binding_modes: Vec<BindingModeConfig>,
  pub gaps: GapsConfig,
  pub general: GeneralConfig,
  pub keybindings: Vec<KeybindingConfig>,

  /// Layout config shared by all workspaces.
  pub layout: LayoutsConfig,

  pub window_behavior: WindowBehaviorConfig,
  pub window_effects: WindowEffectsConfig,
  pub window_rules: Vec<WindowRuleConfig>,
  pub workspaces: Vec<WorkspaceConfig>,
}

impl<'de> Deserialize<'de> for ParsedConfig {
  fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
  where
    D: serde::Deserializer<'de>,
  {
    RawParsedConfig::deserialize(deserializer)
      .map(RawParsedConfig::resolve)
  }
}

/// Raw (unresolved) user config.
///
/// The raw config mirrors the user-facing config file: layout options
/// can be set globally and overridden per workspace. Resolving it
/// produces a [`ParsedConfig`] whose workspace configs are fully
/// populated.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
struct RawParsedConfig {
  binding_modes: Vec<BindingModeConfig>,
  gaps: GapsConfig,
  general: GeneralConfig,
  keybindings: Vec<KeybindingConfig>,
  layout: LayoutsConfig,
  window_behavior: WindowBehaviorConfig,
  window_effects: WindowEffectsConfig,
  window_rules: Vec<WindowRuleConfig>,
  workspaces: Vec<RawWorkspaceConfig>,
}

impl RawParsedConfig {
  /// Resolves the raw config into a fully populated config.
  ///
  /// The global layout config is merged with the per-workspace
  /// overrides, field by field. The workspace-level `scroll` and `bsp`
  /// blocks are deprecated aliases of `layout_overrides`, and are merged
  /// before the new keys.
  fn resolve(self) -> ParsedConfig {
    let RawParsedConfig {
      binding_modes,
      gaps,
      general,
      keybindings,
      layout,
      window_behavior,
      window_effects,
      window_rules,
      workspaces: raw_workspaces,
    } = self;

    if general.layout_switch.is_some() {
      tracing::warn!(
        "The `general.layout_switch` config option is deprecated. Use \
         `layout.switch_behavior` instead."
      );
    }

    let switch_behavior = layout.switch_behavior.or(general.layout_switch);

    let workspaces = raw_workspaces
      .into_iter()
      .map(|workspace| {
        if !workspace.scroll.is_empty() {
          tracing::warn!(
            "The `scroll` workspace config option is deprecated. Use \
             `layout_overrides.scrolling` instead."
          );
        }

        if !workspace.bsp.is_empty() {
          tracing::warn!(
            "The `bsp` workspace config option is deprecated. Use \
             `layout_overrides.bsp` instead."
          );
        }

        let scroll = workspace
          .layout_overrides
          .scrolling
          .apply_to(&workspace.scroll.apply_to(&layout.scrolling));

        let bsp = workspace
          .layout_overrides
          .bsp
          .apply_to(&workspace.bsp.apply_to(&layout.bsp));

        WorkspaceConfig {
          name: workspace.name,
          display_name: workspace.display_name,
          bind_to_monitor: workspace.bind_to_monitor,
          keep_alive: workspace.keep_alive,
          layout: workspace.layout.unwrap_or(layout.default.clone()),
          scroll,
          bsp,
          layout_switch: workspace
            .layout_switch
            .or(switch_behavior)
            .unwrap_or_default(),
        }
      })
      .collect();

    ParsedConfig {
      binding_modes,
      gaps,
      general,
      keybindings,
      layout,
      window_behavior,
      window_effects,
      window_rules,
      workspaces,
    }
  }
}

/// Raw (unresolved) workspace config.
#[derive(Clone, Debug, Deserialize)]
struct RawWorkspaceConfig {
  /// Name of the workspace.
  name: String,

  /// Display name of the workspace.
  #[serde(default)]
  display_name: Option<String>,

  /// Index of the monitor that the workspace is bound to.
  #[serde(default)]
  bind_to_monitor: Option<u32>,

  /// Whether the workspace is kept alive when it becomes empty.
  #[serde(default = "default_bool::<false>")]
  keep_alive: bool,

  /// Layout that the workspace uses. When unset, `layout.default` is
  /// used.
  #[serde(default)]
  layout: Option<WorkspaceLayout>,

  /// Deprecated. Partial overrides for the options of the scrolling
  /// layout. Merged before `layout_overrides`.
  #[serde(default)]
  scroll: ScrollConfigOverrides,

  /// Deprecated. Partial overrides for the options of the BSP layout.
  /// Merged before `layout_overrides`.
  #[serde(default)]
  bsp: BspConfigOverrides,

  /// Partial overrides for the layout options of this workspace.
  #[serde(default)]
  layout_overrides: LayoutOverrides,

  /// What happens to the existing windows of the workspace when its
  /// layout is switched. When unset, `layout.switch_behavior` is used.
  #[serde(default)]
  layout_switch: Option<LayoutSwitchBehavior>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all(serialize = "camelCase"))]
pub struct BindingModeConfig {
  /// Name of the binding mode.
  pub name: String,

  /// Display name of the binding mode.
  #[serde(default)]
  pub display_name: Option<String>,

  /// Keybindings that will be active when the binding mode is active.
  #[serde(default)]
  pub keybindings: Vec<KeybindingConfig>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, rename_all(serialize = "camelCase"))]
pub struct GapsConfig {
  /// Whether to scale the gaps with the DPI of the monitor.
  pub scale_with_dpi: bool,

  /// Gap between adjacent windows.
  pub inner_gap: LengthValue,

  /// Gap between windows and the screen edge.
  pub outer_gap: RectDelta,

  /// Gap between window and the screen edge if there is only one window
  /// in the workspace
  pub single_window_outer_gap: Option<RectDelta>,
}

impl Default for GapsConfig {
  fn default() -> Self {
    GapsConfig {
      scale_with_dpi: true,
      inner_gap: LengthValue::from_px(0),
      outer_gap: RectDelta::new(
        LengthValue::from_px(0),
        LengthValue::from_px(0),
        LengthValue::from_px(0),
        LengthValue::from_px(0),
      ),
      single_window_outer_gap: None,
    }
  }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, rename_all(serialize = "camelCase"))]
pub struct GeneralConfig {
  /// Config for automatically moving the cursor.
  pub cursor_jump: CursorJumpConfig,

  /// Whether to automatically focus windows underneath the cursor.
  pub focus_follows_cursor: bool,

  /// Whether to switch back and forth between the previously focused
  /// workspace when focusing the current workspace.
  pub toggle_workspace_on_refocus: bool,

  /// Commands to run when the WM has started (e.g. to run a script or
  /// launch another application).
  pub startup_commands: Vec<InvokeCommand>,

  /// Commands to run just before the WM is shutdown.
  pub shutdown_commands: Vec<InvokeCommand>,

  /// Commands to run after the WM config has reloaded.
  pub config_reload_commands: Vec<InvokeCommand>,

  /// How windows should be hidden when switching workspaces.
  #[serde(deserialize_with = "deserialize_hide_method")]
  pub hide_method: HideMethod,

  /// Affects which windows get shown in the native Windows taskbar.
  pub show_all_in_taskbar: bool,

  /// What happens to the existing windows of a workspace when its layout
  /// is switched.
  ///
  /// Deprecated: use `layout.switch_behavior` instead. When unset, the
  /// value of `layout.switch_behavior` is used.
  #[serde(skip_serializing_if = "Option::is_none")]
  pub layout_switch: Option<LayoutSwitchBehavior>,

  /// Config for animating window positions.
  pub animations: AnimationsConfig,
}

impl Default for GeneralConfig {
  fn default() -> Self {
    GeneralConfig {
      cursor_jump: CursorJumpConfig::default(),
      focus_follows_cursor: false,
      toggle_workspace_on_refocus: true,
      startup_commands: vec![],
      shutdown_commands: vec![],
      config_reload_commands: vec![],
      hide_method: {
        #[cfg(target_os = "macos")]
        {
          HideMethod::PlaceInCorner
        }
        #[cfg(not(target_os = "macos"))]
        {
          HideMethod::Cloak
        }
      },
      show_all_in_taskbar: false,
      layout_switch: None,
      animations: AnimationsConfig::default(),
    }
  }
}

/// Config for animating windows.
///
/// Animations are disabled by default, and require the `cloak` hide
/// method: windows are animated via live compositor thumbnails while the
/// real windows are cloaked. Window minimize/restore/close animations
/// aren't handled by `GlideWM` — use a tool such as Windhawk for those.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, rename_all(serialize = "camelCase"))]
pub struct AnimationsConfig {
  /// Whether window animations are enabled.
  pub enabled: bool,

  /// Animation when the displayed workspace on a monitor changes.
  pub workspace_switch: WorkspaceSwitchAnimationConfig,

  /// Animation when a window is first shown (e.g. an application that's
  /// launched).
  pub window_open: WindowOpenAnimationConfig,

  /// Animation when a visible window changes position or size (e.g.
  /// tiling windows reflowing when a window opens or closes, windows
  /// being moved or resized, or the strip of a scrolling workspace
  /// scrolling).
  pub window_move: WindowMoveAnimationConfig,
}

/// Config for the workspace switch animation.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, rename_all(serialize = "camelCase"))]
pub struct WorkspaceSwitchAnimationConfig {
  /// Style of the animation.
  pub style: WorkspaceSwitchAnimationStyle,

  /// Where the incoming workspace slides in from. See
  /// [`AnimationDirection`] for how each value maps onto an edge or a
  /// travel direction.
  pub direction: AnimationDirection,

  /// How long the animation lasts, in milliseconds.
  pub duration_ms: u32,

  /// Easing curve as `[x1, y1, x2, y2]` cubic-bezier control points.
  pub curve: AnimationCurve,
}

impl Default for WorkspaceSwitchAnimationConfig {
  fn default() -> Self {
    Self {
      style: WorkspaceSwitchAnimationStyle::Slide,
      direction: AnimationDirection::Auto,
      duration_ms: 250,
      curve: AnimationCurve::default(),
    }
  }
}

/// Style of the workspace switch animation.
#[derive(
  Clone, Copy, Debug, Default, Deserialize, PartialEq, Serialize,
)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceSwitchAnimationStyle {
  /// No animation.
  None,

  /// Windows slide horizontally: the outgoing workspace slides out of
  /// one monitor edge while the incoming workspace slides in from the
  /// opposite edge.
  #[default]
  Slide,

  /// Same as `slide`, but vertically.
  ///
  /// Hyprland spells this style `slidevert`; that spelling is accepted
  /// as an alias.
  #[serde(alias = "slidevert")]
  SlideVert,

  /// The outgoing workspace fades out while the incoming workspace fades
  /// in, in place.
  Fade,
}

/// Config for the window open animation.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, rename_all(serialize = "camelCase"))]
pub struct WindowOpenAnimationConfig {
  /// Style of the animation.
  pub style: WindowOpenAnimationStyle,

  /// How long the animation lasts, in milliseconds.
  pub duration_ms: u32,

  /// Easing curve as `[x1, y1, x2, y2]` cubic-bezier control points.
  pub curve: AnimationCurve,
}

impl Default for WindowOpenAnimationConfig {
  fn default() -> Self {
    Self {
      style: WindowOpenAnimationStyle::Slide,
      duration_ms: 200,
      curve: AnimationCurve::default(),
    }
  }
}

/// Style of the window open animation.
#[derive(
  Clone, Copy, Debug, Default, Deserialize, PartialEq, Serialize,
)]
#[serde(rename_all = "snake_case")]
pub enum WindowOpenAnimationStyle {
  /// No animation.
  None,

  /// Window slides in from the right edge of its monitor.
  #[default]
  Slide,

  /// Window slides in from the bottom edge of its monitor.
  ///
  /// Hyprland spells this style `slidevert`; that spelling is accepted
  /// as an alias.
  #[serde(alias = "slidevert")]
  SlideVert,

  /// Window grows into place from a slightly smaller size while fading
  /// in.
  Popin,

  /// Window fades in, in place.
  Fade,
}

/// Config for the window move animation.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, rename_all(serialize = "camelCase"))]
pub struct WindowMoveAnimationConfig {
  /// Whether the animation is enabled.
  pub enabled: bool,

  /// How long the animation lasts, in milliseconds.
  pub duration_ms: u32,

  /// Easing curve as `[x1, y1, x2, y2]` cubic-bezier control points.
  pub curve: AnimationCurve,
}

impl Default for WindowMoveAnimationConfig {
  fn default() -> Self {
    Self {
      enabled: true,
      duration_ms: 150,
      curve: AnimationCurve::default(),
    }
  }
}

/// Where a workspace slides in from. `left`/`right` name the side the
/// incoming workspace comes in from; `up`/`down` name the direction the
/// windows travel (`up` rises from the bottom edge, `down` descends
/// from the top edge).
#[derive(
  Clone, Copy, Debug, Default, Deserialize, PartialEq, Serialize,
)]
#[serde(rename_all = "snake_case")]
pub enum AnimationDirection {
  /// Direction is derived from the workspace switch (e.g. focusing the
  /// next workspace slides in from the right).
  #[default]
  Auto,

  Left,

  Right,

  Up,

  Down,
}

/// Cubic bezier easing curve with control points `(x1, y1)` and
/// `(x2, y2)`.
///
/// Serialized as an array of four numbers: `[x1, y1, x2, y2]`. The start
/// `(0, 0)` and end `(1, 1)` points are implicit. `y` values outside
/// `0..=1` are accepted, but window animations clamp the eased value to
/// the animation's endpoints (an overshoot would read as a bounce).
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
#[serde(try_from = "[f64; 4]", into = "[f64; 4]")]
pub struct AnimationCurve {
  pub x1: f64,
  pub y1: f64,
  pub x2: f64,
  pub y2: f64,
}

impl Default for AnimationCurve {
  /// An ease-out curve (`easeOutQuint`): the value decelerates into its
  /// target, instead of arriving in the first frames.
  fn default() -> Self {
    Self {
      x1: 0.22,
      y1: 1.0,
      x2: 0.36,
      y2: 1.0,
    }
  }
}

impl TryFrom<[f64; 4]> for AnimationCurve {
  type Error = anyhow::Error;

  fn try_from([x1, y1, x2, y2]: [f64; 4]) -> Result<Self, Self::Error> {
    if !(0.0..=1.0).contains(&x1) || !(0.0..=1.0).contains(&x2) {
      anyhow::bail!(
        "Cubic bezier x coordinates must be within [0, 1], got \
         '[{x1}, {y1}, {x2}, {y2}]'."
      );
    }

    Ok(Self { x1, y1, x2, y2 })
  }
}

impl From<AnimationCurve> for [f64; 4] {
  fn from(curve: AnimationCurve) -> Self {
    [curve.x1, curve.y1, curve.x2, curve.y2]
  }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, rename_all(serialize = "camelCase"))]
pub struct CursorJumpConfig {
  /// Whether to automatically move the cursor on the specified trigger.
  pub enabled: bool,

  /// Trigger for cursor jump.
  pub trigger: CursorJumpTrigger,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CursorJumpTrigger {
  #[default]
  MonitorFocus,
  WindowFocus,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HideMethod {
  Hide,
  #[default]
  Cloak,
  PlaceInCorner,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, rename_all(serialize = "camelCase"))]
pub struct KeybindingConfig {
  /// Keyboard shortcut to trigger the keybinding.
  #[serde(
    deserialize_with = "deserialize_bindings",
    serialize_with = "serialize_bindings"
  )]
  pub bindings: Vec<Keybinding>,

  /// WM commands to run when the keybinding is triggered.
  pub commands: Vec<InvokeCommand>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, rename_all(serialize = "camelCase"))]
pub struct WindowBehaviorConfig {
  /// New windows are created in this state whenever possible.
  pub initial_state: InitialWindowState,

  /// Sets the default options for when a new window is created. This also
  /// changes the defaults for when the state change commands, like
  /// `set_floating`, are used without any flags.
  pub state_defaults: WindowStateDefaultsConfig,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InitialWindowState {
  #[default]
  Tiling,
  Floating,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, rename_all(serialize = "camelCase"))]
pub struct WindowStateDefaultsConfig {
  pub floating: FloatingStateConfig,
  pub fullscreen: FullscreenStateConfig,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(default, rename_all(serialize = "camelCase"))]
pub struct FloatingStateConfig {
  /// Whether to center new floating windows.
  pub centered: bool,

  /// Whether to show floating windows as always on top.
  pub shown_on_top: bool,
}

impl Default for FloatingStateConfig {
  fn default() -> Self {
    FloatingStateConfig {
      centered: true,
      shown_on_top: false,
    }
  }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(default, rename_all(serialize = "camelCase"))]
pub struct FullscreenStateConfig {
  /// Whether to prefer fullscreen windows to be maximized.
  pub maximized: bool,

  /// Whether to show fullscreen windows as always on top.
  pub shown_on_top: bool,
}

impl Default for FullscreenStateConfig {
  fn default() -> Self {
    FullscreenStateConfig {
      maximized: true,
      shown_on_top: false,
    }
  }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, rename_all(serialize = "camelCase"))]
pub struct WindowEffectsConfig {
  /// Visual effects to apply to the focused window.
  pub focused_window: WindowEffectConfig,

  /// Visual effects to apply to non-focused windows.
  pub other_windows: WindowEffectConfig,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, rename_all(serialize = "camelCase"))]
pub struct WindowEffectConfig {
  /// Config for optionally applying a colored border.
  pub border: BorderEffectConfig,

  /// Config for optionally hiding the title bar.
  pub hide_title_bar: HideTitleBarEffectConfig,

  /// Config for optionally changing the corner style.
  pub corner_style: CornerEffectConfig,

  /// Config for optionally applying transparency.
  pub transparency: TransparencyEffectConfig,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, rename_all(serialize = "camelCase"))]
pub struct BorderEffectConfig {
  /// Whether to enable the effect.
  pub enabled: bool,

  /// Color of the window border.
  pub color: Color,
}

impl Default for BorderEffectConfig {
  fn default() -> Self {
    BorderEffectConfig {
      enabled: false,
      color: Color {
        r: 140,
        g: 190,
        b: 255,
        a: 255,
      },
    }
  }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, rename_all(serialize = "camelCase"))]
pub struct HideTitleBarEffectConfig {
  /// Whether to enable the effect.
  pub enabled: bool,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, rename_all(serialize = "camelCase"))]
pub struct CornerEffectConfig {
  /// Whether to enable the effect.
  pub enabled: bool,

  /// Style of the window corners.
  pub style: CornerStyle,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, rename_all(serialize = "camelCase"))]
pub struct TransparencyEffectConfig {
  /// Whether to enable the effect.
  pub enabled: bool,

  /// The opacity to apply.
  pub opacity: OpacityValue,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all(serialize = "camelCase"))]
pub struct WindowRuleConfig {
  pub commands: Vec<InvokeCommand>,

  #[serde(rename = "match")]
  pub match_window: Vec<WindowMatchConfig>,

  #[serde(default = "default_window_rule_on")]
  pub on: Vec<WindowRuleEvent>,

  #[serde(default = "default_bool::<true>")]
  pub run_once: bool,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(default, rename_all(serialize = "camelCase"))]
pub struct WindowMatchConfig {
  pub window_process: Option<MatchType>,
  pub window_class: Option<MatchType>,
  pub window_title: Option<MatchType>,
}

/// Due to limitations in `serde_yaml`, we need to use an untagged enum
/// instead of a regular enum for serialization. Using a regular enum
/// causes issues with flow-style objects in YAML.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(untagged)]
pub enum MatchType {
  Equals { equals: String },
  Includes { includes: String },
  Regex { regex: String },
  NotEquals { not_equals: String },
  NotRegex { not_regex: String },
}

impl MatchType {
  /// Whether the given value is a match for the match type.
  #[must_use]
  pub fn is_match(&self, value: &str) -> bool {
    match self {
      MatchType::Equals { equals } => value == equals,
      MatchType::Includes { includes } => value.contains(includes),
      MatchType::Regex { regex } => {
        regex::Regex::new(regex).is_ok_and(|re| re.is_match(value))
      }
      MatchType::NotEquals { not_equals } => value != not_equals,
      MatchType::NotRegex { not_regex } => {
        regex::Regex::new(not_regex).is_ok_and(|re| !re.is_match(value))
      }
    }
  }
}

#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WindowRuleEvent {
  /// When a window receives native focus.
  Focus,

  /// When a window is initially managed.
  Manage,

  /// When the title of a window changes.
  TitleChange,
}

/// Resolved workspace config.
///
/// Built from a [`RawWorkspaceConfig`] by [`RawParsedConfig::resolve`],
/// with the global layout config already applied. Not deserialized
/// directly, so that the global layout config can't be bypassed.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all(serialize = "camelCase"))]
pub struct WorkspaceConfig {
  pub name: String,

  pub display_name: Option<String>,

  pub bind_to_monitor: Option<u32>,

  pub keep_alive: bool,

  /// Layout that the workspace uses: `'tiling'`, `'scrolling'`, or
  /// `'bsp'`. Inherited from `layout.default` when unset in the config
  /// file.
  pub layout: WorkspaceLayout,

  /// Options for the scrolling layout. Only read when `layout` is
  /// `'scrolling'`.
  pub scroll: ScrollConfig,

  /// Options for the BSP layout. Only read when `layout` is `'bsp'`.
  pub bsp: BspConfig,

  /// What happens to the existing windows of the workspace when its
  /// layout is switched. Inherited from `layout.switch_behavior` (or the
  /// deprecated `general.layout_switch`) when unset in the config file.
  pub layout_switch: LayoutSwitchBehavior,
}

/// Global layout config, shared by all workspaces.
///
/// Workspaces inherit the values of this config and can override them
/// individually (see [`LayoutOverrides`]).
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields, rename_all(serialize = "camelCase"))]
pub struct LayoutsConfig {
  /// Layout used by workspaces that don't set their own `layout`.
  pub default: WorkspaceLayout,

  /// What happens to the existing windows of a workspace when its layout
  /// is switched.
  ///
  /// When unset, the deprecated `general.layout_switch` is used as a
  /// fallback.
  #[serde(skip_serializing_if = "Option::is_none")]
  pub switch_behavior: Option<LayoutSwitchBehavior>,

  /// Options for the tiling layout.
  #[serde(default, deserialize_with = "deserialize_tiling_config")]
  pub tiling: TilingConfig,

  /// Options for the scrolling layout.
  pub scrolling: ScrollConfig,

  /// Options for the BSP layout.
  pub bsp: BspConfig,
}

/// Options for the tiling layout.
///
/// The tiling layout has no configurable options yet. The block exists
/// so that future options have a home.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields, rename_all(serialize = "camelCase"))]
pub struct TilingConfig {}

/// Partial overrides for the layout options of a workspace.
///
/// The overrides are keyed by layout name, so that a workspace can keep
/// its preferred options for every layout — even for layouts that it
/// doesn't currently use.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields, rename_all(serialize = "camelCase"))]
pub struct LayoutOverrides {
  /// Overrides for the options of the tiling layout.
  #[serde(default, deserialize_with = "deserialize_tiling_config")]
  pub tiling: TilingConfig,

  /// Overrides for the options of the scrolling layout.
  pub scrolling: ScrollConfigOverrides,

  /// Overrides for the options of the BSP layout.
  pub bsp: BspConfigOverrides,
}

/// Partial overrides for the options of the scrolling layout.
///
/// Fields that aren't set inherit the value of the global layout config.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields, rename_all(serialize = "camelCase"))]
pub struct ScrollConfigOverrides {
  /// Axis that the strip of windows scrolls along.
  pub axis: Option<TilingDirection>,

  /// Default width of new strip items as a fraction of the viewport
  /// width. Used while the scroll axis is horizontal.
  pub window_width: Option<f32>,

  /// Default height of new strip items as a fraction of the viewport
  /// height. Used while the scroll axis is vertical.
  pub window_height: Option<f32>,

  /// Widths that the `cycle-preset-size` command cycles the focused
  /// strip item through, for a horizontal scroll axis.
  #[serde(
    default,
    deserialize_with = "deserialize_optional_preset_sizes"
  )]
  pub preset_widths: Option<Vec<f32>>,

  /// Heights that the `cycle-preset-size` command cycles the focused
  /// strip item through, for a vertical scroll axis.
  #[serde(
    default,
    deserialize_with = "deserialize_optional_preset_sizes"
  )]
  pub preset_heights: Option<Vec<f32>>,

  /// How the focused strip item is scrolled into view.
  pub align: Option<ScrollAlign>,

  /// Where new strip items are inserted.
  pub insert_position: Option<ScrollInsertPosition>,

  /// How windows that are scrolled out of the viewport are kept out of
  /// view.
  pub offscreen_windows: Option<OffscreenWindows>,
}

impl ScrollConfigOverrides {
  /// Applies the overrides that are set on top of the given config.
  #[must_use]
  pub fn apply_to(&self, base: &ScrollConfig) -> ScrollConfig {
    ScrollConfig {
      axis: self.axis.unwrap_or(base.axis),
      window_width: self.window_width.unwrap_or(base.window_width),
      window_height: self.window_height.unwrap_or(base.window_height),
      preset_widths: self
        .preset_widths
        .clone()
        .unwrap_or_else(|| base.preset_widths.clone()),
      preset_heights: self
        .preset_heights
        .clone()
        .unwrap_or_else(|| base.preset_heights.clone()),
      align: self.align.unwrap_or(base.align),
      insert_position: self
        .insert_position
        .unwrap_or(base.insert_position),
      offscreen_windows: self
        .offscreen_windows
        .unwrap_or(base.offscreen_windows),
    }
  }

  /// Whether no overrides are set.
  #[must_use]
  pub fn is_empty(&self) -> bool {
    *self == Self::default()
  }
}

/// Partial overrides for the options of the BSP layout.
///
/// Fields that aren't set inherit the value of the global layout config.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields, rename_all(serialize = "camelCase"))]
pub struct BspConfigOverrides {
  /// How the direction of a new split is chosen when a window splits the
  /// focused window's space.
  pub automatic_scheme: Option<BspAutomaticScheme>,
}

impl BspConfigOverrides {
  /// Applies the overrides that are set on top of the given config.
  #[must_use]
  pub fn apply_to(&self, base: &BspConfig) -> BspConfig {
    BspConfig {
      automatic_scheme: self
        .automatic_scheme
        .unwrap_or(base.automatic_scheme),
    }
  }

  /// Whether no overrides are set.
  #[must_use]
  pub fn is_empty(&self) -> bool {
    *self == Self::default()
  }
}

/// What happens to the existing windows of a workspace when its layout
/// is switched (via `set-layout`, `cycle-layout`, or a config reload).
///
/// Configured globally via `layout.switch_behavior`, and overridable per
/// workspace.
#[derive(
  Clone, Copy, Debug, Default, Deserialize, PartialEq, Serialize,
)]
#[serde(rename_all = "snake_case")]
pub enum LayoutSwitchBehavior {
  /// Existing windows are rearranged into the new layout (e.g. switching
  /// to `bsp` rebuilds them as a binary tree, and switching from `bsp`
  /// to `scrolling` turns its windows into separate strip items).
  Reflow,

  /// Existing windows keep their arrangement; only windows that are
  /// added afterwards are placed per the new layout. Sizes are still
  /// adjusted where the new layout requires it (e.g. strip items that
  /// add up to more than the viewport are scaled down to fit when
  /// switching to `tiling`).
  #[default]
  Keep,
}

/// Layout that a workspace uses to arrange its tiling children.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceLayout {
  /// Children share the viewport (existing behavior).
  Tiling,

  /// Children form a strip along the scroll axis. The viewport scrolls
  /// to keep the focused child in view.
  #[default]
  Scrolling,

  /// Children form a binary tree of split containers ("binary space
  /// partitioning", like bspwm). Inserting a window splits the focused
  /// window's space in half.
  Bsp,
}

impl FromStr for WorkspaceLayout {
  type Err = anyhow::Error;

  /// Parses a string into a workspace layout.
  ///
  /// Example:
  /// ```
  /// # use wm_common::WorkspaceLayout;
  /// # use std::str::FromStr;
  /// let layout = WorkspaceLayout::from_str("tiling");
  /// assert_eq!(layout.unwrap(), WorkspaceLayout::Tiling);
  ///
  /// let layout = WorkspaceLayout::from_str("scrolling");
  /// assert_eq!(layout.unwrap(), WorkspaceLayout::Scrolling);
  ///
  /// let layout = WorkspaceLayout::from_str("bsp");
  /// assert_eq!(layout.unwrap(), WorkspaceLayout::Bsp);
  /// ```
  fn from_str(unparsed: &str) -> anyhow::Result<Self> {
    match unparsed {
      "tiling" => Ok(Self::Tiling),
      "scrolling" => Ok(Self::Scrolling),
      "bsp" => Ok(Self::Bsp),
      _ => bail!("Not a valid workspace layout: {}", unparsed),
    }
  }
}

impl WorkspaceLayout {
  /// All workspace layouts, in the order that the `cycle-layout`
  /// command cycles through them.
  ///
  /// Keep in sync with [`WorkspaceLayout::next`].
  pub const ALL: [Self; 3] = [Self::Tiling, Self::Scrolling, Self::Bsp];

  /// The next layout in the cycle, wrapping around at the end.
  ///
  /// The exhaustive match flags newly added layouts; new layouts must
  /// also be added to [`WorkspaceLayout::ALL`].
  #[must_use]
  pub fn next(&self) -> Self {
    match self {
      Self::Tiling => Self::Scrolling,
      Self::Scrolling => Self::Bsp,
      Self::Bsp => Self::Tiling,
    }
  }
}

/// Options for the scrolling layout.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields, rename_all(serialize = "camelCase"))]
pub struct ScrollConfig {
  /// Axis that the strip of windows scrolls along.
  pub axis: TilingDirection,

  /// Default width of new strip items as a fraction of the viewport
  /// width. Used while the scroll axis is horizontal. Clamped to
  /// `ScrollConfig::MIN_ITEM_SIZE..=1.0`.
  pub window_width: f32,

  /// Default height of new strip items as a fraction of the viewport
  /// height. Used while the scroll axis is vertical. Clamped to
  /// `ScrollConfig::MIN_ITEM_SIZE..=1.0`.
  pub window_height: f32,

  /// Widths (as fractions of the viewport width) that the
  /// `cycle-preset-size` command cycles the focused strip item through
  /// while the scroll axis is horizontal.
  ///
  /// Sorted, deduplicated, and clamped on deserialization. Must not be
  /// empty.
  #[serde(deserialize_with = "deserialize_preset_sizes")]
  pub preset_widths: Vec<f32>,

  /// Heights (as fractions of the viewport height) that the
  /// `cycle-preset-size` command cycles the focused strip item through
  /// while the scroll axis is vertical.
  ///
  /// Sorted, deduplicated, and clamped on deserialization. Must not be
  /// empty.
  #[serde(deserialize_with = "deserialize_preset_sizes")]
  pub preset_heights: Vec<f32>,

  /// How the focused strip item is scrolled into view.
  pub align: ScrollAlign,

  /// Where new strip items are inserted.
  pub insert_position: ScrollInsertPosition,

  /// How windows that are scrolled out of the viewport are kept out of
  /// view.
  pub offscreen_windows: OffscreenWindows,
}

/// How windows of a scrolling workspace that are scrolled out of the
/// viewport are kept out of view.
#[derive(
  Clone, Copy, Debug, Default, Deserialize, PartialEq, Serialize,
)]
#[serde(rename_all = "snake_case")]
pub enum OffscreenWindows {
  /// Hidden with the configured hide method. Hidden windows aren't listed
  /// in Alt+Tab.
  #[default]
  Hide,

  /// Moved beyond the edges of all monitors while staying shown, so that
  /// they remain listed in Alt+Tab and the taskbar. Focusing one (e.g.
  /// via Alt+Tab) scrolls it into view.
  Park,
}

impl ScrollConfig {
  /// Minimum size of a strip item as a fraction of the viewport.
  pub const MIN_ITEM_SIZE: f32 = 0.05;

  /// Clamps a strip item size into the range of valid viewport
  /// fractions (`MIN_ITEM_SIZE..=1.0`).
  #[must_use]
  pub fn clamp_item_size(size: f32) -> f32 {
    size.clamp(Self::MIN_ITEM_SIZE, 1.0)
  }

  /// Default size of new strip items for the given scroll axis
  /// (`window_width` for a horizontal axis, `window_height` for a
  /// vertical axis), clamped to the range of valid viewport fractions.
  #[must_use]
  pub fn default_item_size(&self, axis: &TilingDirection) -> f32 {
    Self::clamp_item_size(match axis {
      TilingDirection::Horizontal => self.window_width,
      TilingDirection::Vertical => self.window_height,
    })
  }

  /// Preset sizes for the given scroll axis (`preset_widths` for a
  /// horizontal axis, `preset_heights` for a vertical axis).
  #[must_use]
  pub fn preset_sizes(&self, axis: &TilingDirection) -> &[f32] {
    match axis {
      TilingDirection::Horizontal => &self.preset_widths,
      TilingDirection::Vertical => &self.preset_heights,
    }
  }
}

impl Default for ScrollConfig {
  fn default() -> Self {
    ScrollConfig {
      axis: TilingDirection::Vertical,
      window_width: 0.7,
      window_height: 0.7,
      preset_widths: vec![0.3, 0.5, 0.7, 1.0],
      preset_heights: vec![0.5, 1.0],
      align: ScrollAlign::Center,
      insert_position: ScrollInsertPosition::AfterFocused,
      offscreen_windows: OffscreenWindows::Park,
    }
  }
}

/// Helper function for deserializing the preset sizes of the scrolling
/// layout.
///
/// Sizes are clamped to the range of valid viewport fractions, then
/// sorted and deduplicated. Returns an error for an empty list or for
/// non-finite values.
fn deserialize_preset_sizes<'de, D>(
  deserializer: D,
) -> Result<Vec<f32>, D::Error>
where
  D: serde::de::Deserializer<'de>,
{
  let sizes = Vec::<f32>::deserialize(deserializer)?;
  normalize_preset_sizes(sizes).map_err(serde::de::Error::custom)
}

/// Helper function for deserializing optional preset sizes of the
/// scrolling layout.
///
/// Sizes are clamped to the range of valid viewport fractions, then
/// sorted and deduplicated. Returns an error for an empty list or for
/// non-finite values.
fn deserialize_optional_preset_sizes<'de, D>(
  deserializer: D,
) -> Result<Option<Vec<f32>>, D::Error>
where
  D: serde::de::Deserializer<'de>,
{
  let sizes = Option::<Vec<f32>>::deserialize(deserializer)?;

  match sizes {
    None => Ok(None),
    Some(sizes) => normalize_preset_sizes(sizes)
      .map(Some)
      .map_err(serde::de::Error::custom),
  }
}

/// Helper function for deserializing the tiling config.
///
/// The tiling layout has no options yet, so both an empty block
/// (`tiling: {}`) and an empty value (`tiling:`) are accepted.
fn deserialize_tiling_config<'de, D>(
  deserializer: D,
) -> Result<TilingConfig, D::Error>
where
  D: serde::de::Deserializer<'de>,
{
  Ok(
    Option::<TilingConfig>::deserialize(deserializer)?.unwrap_or_default(),
  )
}

/// Clamps, sorts, and deduplicates preset sizes of the scrolling layout.
fn normalize_preset_sizes(sizes: Vec<f32>) -> anyhow::Result<Vec<f32>> {
  if sizes.is_empty() {
    bail!("Preset sizes must contain at least one size.");
  }

  if let Some(size) = sizes.iter().find(|size| !size.is_finite()) {
    bail!("Preset sizes must be finite numbers, got '{size}'.");
  }

  let mut sizes = sizes
    .into_iter()
    .map(ScrollConfig::clamp_item_size)
    .collect::<Vec<_>>();

  sizes.sort_by(f32::total_cmp);
  sizes.dedup_by(|a, b| (*a - *b).abs() < f32::EPSILON);

  Ok(sizes)
}

/// How the viewport scrolls to keep the focused strip item in view.
#[derive(
  Clone, Copy, Debug, Default, Deserialize, PartialEq, Serialize,
)]
#[serde(rename_all = "snake_case")]
pub enum ScrollAlign {
  /// Scroll the least amount required to make the item fully visible.
  #[default]
  Minimal,

  /// Center the focused item in the viewport.
  Center,
}

/// Where new strip items are inserted in a scrolling workspace.
#[derive(
  Clone, Copy, Debug, Default, Deserialize, PartialEq, Serialize,
)]
#[serde(rename_all = "snake_case")]
pub enum ScrollInsertPosition {
  /// Insert at the start of the strip (the top for a vertical scroll
  /// axis, the left for a horizontal one).
  Start,

  /// Insert directly before the focused strip item.
  BeforeFocused,

  /// Insert directly after the focused strip item.
  #[default]
  AfterFocused,

  /// Append to the end of the strip (the bottom for a vertical scroll
  /// axis, the right for a horizontal one).
  End,
}

/// Options for the BSP layout.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields, rename_all(serialize = "camelCase"))]
pub struct BspConfig {
  /// How the direction of a new split is chosen when a window splits
  /// the focused window's space.
  pub automatic_scheme: BspAutomaticScheme,
}

/// How new split directions are chosen in the BSP layout.
///
/// Mirrors the `automatic_scheme` setting of bspwm.
#[derive(
  Clone, Copy, Debug, Default, Deserialize, PartialEq, Serialize,
)]
#[serde(rename_all = "snake_case")]
pub enum BspAutomaticScheme {
  /// Split the longer side of the focused container's rect: a wide rect
  /// is cut into side-by-side children, a tall rect into stacked
  /// children.
  #[default]
  LongestSide,

  /// Alternate the split direction with the depth of the tree: children
  /// of a horizontal split are split vertically and vice versa. Splits
  /// at the workspace's root level use the workspace's tiling direction.
  Alternate,
}

/// Helper function for setting a default value for a boolean field.
const fn default_bool<const V: bool>() -> bool {
  V
}

/// Helper function for setting a default value for window rule events.
fn default_window_rule_on() -> Vec<WindowRuleEvent> {
  vec![WindowRuleEvent::Manage, WindowRuleEvent::TitleChange]
}

/// Helper function for serializing a vector of keybindings.
///
/// Returns a vector of strings (e.g. `["cmd+shift+a", "ctrl+shift+b"]`).
fn serialize_bindings<S>(
  bindings: &[Keybinding],
  serializer: S,
) -> Result<S::Ok, S::Error>
where
  S: serde::Serializer,
{
  let binding_strings: Vec<String> = bindings
    .iter()
    .map(|binding| {
      binding
        .keys()
        .iter()
        .map(|key| key.to_string().to_lowercase())
        .collect::<Vec<_>>()
        .join("+")
    })
    .collect();

  binding_strings.serialize(serializer)
}

/// Helper function for deserializing a vector of strings into keybindings.
///
/// Returns a vector of [`Keybinding`].
fn deserialize_bindings<'de, D>(
  deserializer: D,
) -> Result<Vec<Keybinding>, D::Error>
where
  D: serde::de::Deserializer<'de>,
{
  let s: Vec<&str> = serde::de::Deserialize::deserialize(deserializer)?;
  s.iter()
    .map(|keybinding_str| {
      let keys: Vec<Key> = keybinding_str
        .split('+')
        .map(|key| {
          key.trim().parse().or_else(|_| Key::try_from_literal(key))
        })
        .collect::<Result<Vec<Key>, _>>()
        .map_err(serde::de::Error::custom)?;

      Keybinding::new(keys).map_err(serde::de::Error::custom)
    })
    .collect()
}

/// Helper function for deserializing [`HideMethod`].
///
/// On macOS, [`HideMethod::Hide`] and [`HideMethod::Cloak`] are not valid
/// and are automatically converted to [`HideMethod::PlaceInCorner`].
fn deserialize_hide_method<'de, D>(
  deserializer: D,
) -> Result<HideMethod, D::Error>
where
  D: serde::de::Deserializer<'de>,
{
  // LINT: The deserialized value is ignored on macOS, but we still want
  // to produce an error for invalid values.
  #[allow(unused_variables)]
  let method = HideMethod::deserialize(deserializer)?;

  #[cfg(target_os = "macos")]
  {
    Ok(HideMethod::PlaceInCorner)
  }

  #[cfg(not(target_os = "macos"))]
  {
    Ok(method)
  }
}

#[cfg(test)]
mod tests {
  use indoc::indoc;

  use super::*;

  /// Parses a config from the given YAML string.
  fn parse_config(yaml: &str) -> Result<ParsedConfig, serde_yaml::Error> {
    serde_yaml::from_str(yaml)
  }

  /// The `+` and `-` keys of the preset size cycle are bound as
  /// `shift+oem_plus` and `oem_minus`. A literal `+` can't be used: the
  /// binding string is split on `+`, and a key that needs shift can't be
  /// resolved as a literal.
  #[test]
  fn parses_preset_size_cycle_bindings() {
    let config = parse_config(indoc! {r"
      keybindings:
        - commands: ['cycle-preset-size']
          bindings: ['shift+oem_plus']
        - commands: ['cycle-preset-size --reverse']
          bindings: ['oem_minus']
      "})
    .unwrap();

    let bindings = &config.keybindings[0].bindings[0];
    assert_eq!(
      bindings.keys(),
      &[Key::Shift, Key::OemPlus],
      "The next preset is bound to shift + the `=` key."
    );

    let bindings = &config.keybindings[1].bindings[0];
    assert_eq!(
      bindings.keys(),
      &[Key::OemMinus],
      "The previous preset is bound to the `-` key."
    );
  }

  /// The cycle order in [`WorkspaceLayout::ALL`] must match what
  /// repeatedly calling [`WorkspaceLayout::next`] produces.
  #[test]
  fn cycle_order_matches_all_layouts() {
    let mut cycle = Vec::new();
    let mut layout = WorkspaceLayout::ALL[0].clone();

    for _ in 0..WorkspaceLayout::ALL.len() {
      cycle.push(layout.clone());
      layout = layout.next();
    }

    // The cycle wraps around back to the first layout.
    assert_eq!(layout, WorkspaceLayout::ALL[0]);
    assert_eq!(cycle, WorkspaceLayout::ALL);
  }

  #[test]
  fn picks_default_item_size_by_axis() {
    let config = ScrollConfig {
      window_width: 0.4,
      window_height: 0.8,
      ..ScrollConfig::default()
    };

    assert!(
      (config.default_item_size(&TilingDirection::Horizontal) - 0.4).abs()
        < f32::EPSILON
    );
    assert!(
      (config.default_item_size(&TilingDirection::Vertical) - 0.8).abs()
        < f32::EPSILON
    );

    // Out-of-range sizes are clamped into valid viewport fractions.
    let config = ScrollConfig {
      window_width: 2.0,
      window_height: 0.0,
      ..ScrollConfig::default()
    };

    assert!(
      (config.default_item_size(&TilingDirection::Horizontal) - 1.0).abs()
        < f32::EPSILON
    );
    assert!(
      (config.default_item_size(&TilingDirection::Vertical)
        - ScrollConfig::MIN_ITEM_SIZE)
        .abs()
        < f32::EPSILON
    );
  }

  #[test]
  fn normalizes_preset_sizes() {
    let sizes = normalize_preset_sizes(vec![1.5, 0.5, 0.01, 0.5]).unwrap();

    assert_eq!(sizes, vec![ScrollConfig::MIN_ITEM_SIZE, 0.5, 1.0]);
  }

  #[test]
  fn rejects_invalid_preset_sizes() {
    assert!(normalize_preset_sizes(vec![]).is_err());
    assert!(normalize_preset_sizes(vec![0.5, f32::NAN]).is_err());
  }

  #[test]
  fn deserializes_scroll_insert_positions() {
    for (value, expected) in [
      ("start", ScrollInsertPosition::Start),
      ("before_focused", ScrollInsertPosition::BeforeFocused),
      ("after_focused", ScrollInsertPosition::AfterFocused),
      ("end", ScrollInsertPosition::End),
    ] {
      let config: ScrollConfig =
        serde_yaml::from_str(&format!("insert_position: {value}"))
          .unwrap();

      assert_eq!(config.insert_position, expected);
    }
  }

  #[test]
  fn deserializes_scroll_config() {
    let config: ScrollConfig = serde_yaml::from_str(
      "window_height: 0.7\npreset_widths: [1.0, 0.25]",
    )
    .unwrap();

    assert!((config.window_height - 0.7).abs() < f32::EPSILON);
    assert_eq!(config.preset_widths, vec![0.25, 1.0]);
    assert_eq!(
      config.preset_heights,
      ScrollConfig::default().preset_heights
    );

    assert!(
      serde_yaml::from_str::<ScrollConfig>("preset_heights: []").is_err()
    );
  }

  #[test]
  fn defaults_without_layout_block() {
    let config = parse_config(indoc! {r"
      workspaces:
        - name: '1'
      "})
    .unwrap();

    assert_eq!(config.layout, LayoutsConfig::default());

    let workspace = &config.workspaces[0];

    assert_eq!(workspace.layout, WorkspaceLayout::Scrolling);
    assert_eq!(workspace.scroll, ScrollConfig::default());
    assert_eq!(workspace.bsp, BspConfig::default());
    assert_eq!(workspace.layout_switch, LayoutSwitchBehavior::Keep);
  }

  #[test]
  fn workspace_inherits_default_layout() {
    let config = parse_config(indoc! {r"
      layout:
        default: 'scrolling'
      workspaces:
        - name: '1'
      "})
    .unwrap();

    assert_eq!(config.workspaces[0].layout, WorkspaceLayout::Scrolling);
  }

  #[test]
  fn explicit_workspace_layout_wins() {
    let config = parse_config(indoc! {r"
      layout:
        default: 'scrolling'
      workspaces:
        - name: '1'
          layout: 'tiling'
      "})
    .unwrap();

    assert_eq!(config.workspaces[0].layout, WorkspaceLayout::Tiling);
  }

  #[test]
  fn workspace_overrides_are_partial() {
    let config = parse_config(indoc! {r"
      layout:
        scrolling:
          window_width: 0.5
          align: 'center'
      workspaces:
        - name: '1'
          layout_overrides:
            scrolling:
              window_width: 0.8
      "})
    .unwrap();

    let scroll = &config.workspaces[0].scroll;

    assert!((scroll.window_width - 0.8).abs() < f32::EPSILON);

    // The other options are inherited from the global config.
    assert_eq!(scroll.align, ScrollAlign::Center);
    assert_eq!(scroll.axis, ScrollConfig::default().axis);
  }

  #[test]
  fn legacy_scroll_block_merges_with_global() {
    let config = parse_config(indoc! {r"
      layout:
        scrolling:
          window_width: 0.5
          align: 'center'
      workspaces:
        - name: '1'
          scroll:
            window_width: 0.8
      "})
    .unwrap();

    let scroll = &config.workspaces[0].scroll;

    assert!((scroll.window_width - 0.8).abs() < f32::EPSILON);
    assert_eq!(scroll.align, ScrollAlign::Center);
  }

  #[test]
  fn new_overrides_beat_legacy_overrides() {
    let config = parse_config(indoc! {r"
      workspaces:
        - name: '1'
          scroll:
            window_width: 0.3
          layout_overrides:
            scrolling:
              window_width: 0.8
      "})
    .unwrap();

    let scroll = &config.workspaces[0].scroll;

    assert!((scroll.window_width - 0.8).abs() < f32::EPSILON);
  }

  #[test]
  fn bsp_overrides_are_partial() {
    let config = parse_config(indoc! {r"
      layout:
        bsp:
          automatic_scheme: 'alternate'
      workspaces:
        - name: '1'
          layout_overrides:
            bsp:
              automatic_scheme: 'longest_side'
      "})
    .unwrap();

    assert_eq!(
      config.workspaces[0].bsp.automatic_scheme,
      BspAutomaticScheme::LongestSide
    );
  }

  #[test]
  fn legacy_and_new_config_resolve_identically() {
    let legacy = parse_config(indoc! {r"
      general:
        layout_switch: 'keep'
      workspaces:
        - name: '1'
          layout: 'scrolling'
          scroll:
            window_width: 0.8
            align: 'center'
          bsp:
            automatic_scheme: 'alternate'
          layout_switch: 'reflow'
      "})
    .unwrap();

    let new = parse_config(indoc! {r"
      layout:
        switch_behavior: 'keep'
        scrolling:
          window_width: 0.8
          align: 'center'
        bsp:
          automatic_scheme: 'alternate'
      workspaces:
        - name: '1'
          layout: 'scrolling'
          layout_switch: 'reflow'
      "})
    .unwrap();

    assert_eq!(legacy.workspaces, new.workspaces);
  }

  #[test]
  fn preset_sizes_are_normalized_in_overrides() {
    let config = parse_config(indoc! {r"
      workspaces:
        - name: '1'
          layout_overrides:
            scrolling:
              preset_widths: [1.0, 0.25, 0.25]
      "})
    .unwrap();

    assert_eq!(config.workspaces[0].scroll.preset_widths, vec![0.25, 1.0]);

    // An empty list is invalid.
    assert!(parse_config(indoc! {r"
      workspaces:
        - name: '1'
          layout_overrides:
            scrolling:
              preset_widths: []
      "})
    .is_err());
  }

  #[test]
  fn switch_behavior_precedence() {
    // The workspace config takes precedence over the global config.
    let config = parse_config(indoc! {r"
      layout:
        switch_behavior: 'keep'
      workspaces:
        - name: '1'
          layout_switch: 'reflow'
      "})
    .unwrap();

    assert_eq!(
      config.workspaces[0].layout_switch,
      LayoutSwitchBehavior::Reflow
    );

    // The global config takes precedence over the deprecated config.
    let config = parse_config(indoc! {r"
      general:
        layout_switch: 'reflow'
      layout:
        switch_behavior: 'keep'
      workspaces:
        - name: '1'
      "})
    .unwrap();

    assert_eq!(
      config.workspaces[0].layout_switch,
      LayoutSwitchBehavior::Keep
    );

    // The deprecated config is used when the global config is unset.
    let config = parse_config(indoc! {r"
      general:
        layout_switch: 'keep'
      workspaces:
        - name: '1'
      "})
    .unwrap();

    assert_eq!(
      config.workspaces[0].layout_switch,
      LayoutSwitchBehavior::Keep
    );

    // The default is used when neither config is set.
    let config = parse_config(indoc! {r"
      workspaces:
        - name: '1'
      "})
    .unwrap();

    assert_eq!(
      config.workspaces[0].layout_switch,
      LayoutSwitchBehavior::Keep
    );
  }

  #[test]
  fn tiling_block_is_accepted() {
    let config = parse_config(indoc! {r"
      layout:
        tiling: {}
      "})
    .unwrap();

    assert_eq!(config.layout.tiling, TilingConfig::default());

    // An empty value is accepted as well.
    let config = parse_config(indoc! {r"
      layout:
        tiling:
      "})
    .unwrap();

    assert_eq!(config.layout.tiling, TilingConfig::default());

    // Unknown keys are rejected.
    assert!(parse_config(indoc! {r"
      layout:
        tiling:
          unknown: 1
      "})
    .is_err());

    assert!(parse_config(indoc! {r"
        layout:
          unknown: 1
        "})
    .is_err());
  }

  #[test]
  fn rejects_unknown_layout_options() {
    assert!(parse_config(indoc! {r"
      layout:
        scrolling:
          window_widht: 0.5
      "})
    .is_err());

    assert!(parse_config(indoc! {r"
      workspaces:
        - name: '1'
          layout_overrides:
            scrolling:
              window_widht: 0.5
      "})
    .is_err());
  }
}
