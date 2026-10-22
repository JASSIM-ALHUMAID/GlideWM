<div align="center">

  <br>
  <img src="./resources/assets/logo.svg" width="230" alt="GlideWM logo" />
  <br>

# GlideWM

**A tiling window manager for Windows inspired by i3wm.**

[![Discord invite][discord-badge]][discord-link]
[![Downloads][downloads-badge]][downloads-link]

GlideWM lets you easily organize windows and adjust their layout on the fly by using keyboard-driven commands.

[Installation](#installation) •
[What's new](#whats-new) •
[Migrating from GlazeWM](#migrating-from-glazewm) •
[Default keybindings](#default-keybindings) •
[Config documentation](#config-documentation) •
[FAQ](#faq) •
[Contributing ↗](CONTRIBUTING.md)

![Demo video][demo-video]

</div>

> The demo video is inherited from GlazeWM, so it shows the original's tiling layout.

> [!NOTE]
> **GlideWM is a fork of [GlazeWM](https://github.com/glzr-io/glazewm)**, the original tiling
> window manager for Windows by Glzr Software.
>
> It began as a personal fork and grew into its own thing: a pluggable layout system, a
> niri-style scrolling layout, and animations driven by live compositor thumbnails. GlideWM
> is not affiliated with or endorsed by the GlazeWM authors.
>
> GlideWM builds on top of GlazeWM, so everything that works upstream works here too. If you
> hit a bug, check whether it reproduces on upstream GlazeWM first.
>
> **GlideWM is Windows only.** The upstream project also supports macOS, but this fork
> doesn't build, package, or support it. The macOS code is still in the tree, inherited from
> upstream, and is left untouched so that upstream changes stay easy to merge.

## What's new

### Layouts

GlazeWM had exactly one layout: a set of splitting containers that share the viewport.
GlideWM replaces that with a pluggable layout system, so a workspace can be arranged in
several different ways.

- **Scrolling layout** (inspired by [niri](https://github.com/YaLTeR/niri) and
  [PaperWM](https://github.com/paperwm/PaperWM)). Windows form a strip along an axis, and the
  viewport scrolls to keep the focused window in view. New windows are inserted next to the
  focused one, and can be cycled through a set of preset sizes with `alt+r` then `+` / `-`.
- **BSP layout** (like [bspwm](https://github.com/baskerville/bspwm)). Windows form a binary
  tree of splits; inserting a window splits the focused window's space in half.
- **Per-workspace layouts.** Each workspace picks its own layout, and can override
  individual layout options without repeating the rest.
- **One global `layout` block.** Layout options are configured once and inherited by every
  workspace, so a config that sets up nine workspaces doesn't set up the same scrolling
  options nine times.
- **Runtime layout switching.** `set-layout` sets a workspace's layout, `cycle-layout` cycles
  through the available ones. Switches apply to the session; the config decides what a
  workspace uses on startup.
- **Layout switch behavior.** Whether existing windows are rearranged into the new layout
  (`reflow`) or keep their arrangement (`keep`) is now configurable.

New commands: `set-layout`, `cycle-layout`, `scroll`, `cycle-preset-size`, `center-focused`,
`toggle-tiling-direction`.

### Animations

GlazeWM moves real windows around, which is why window animations were either absent or
glitchy. GlideWM animates **live compositor thumbnails** instead, and keeps the real window
cloaked for the duration of the animation. The animation stays smooth no matter how busy the
application is, because the compositor is only ever asked to draw a thumbnail.

- **Workspace switch animations**: `slide`, `slide_vert`, or `fade`, with a direction, a
  duration, and a cubic-bezier easing curve.
- **Window open animations**: `slide`, `slide_vert`, `popin`, or `fade`.
- **Window move animations**: windows that change position or size animate into place
  instead of snapping. Windows that are already animating redirect to their new position
  rather than restarting.

Animations require `hide_method: 'cloak'`. Windows that an external tool animates (e.g. the
Windhawk "Hyprland Animations" mod) are left to that tool.

### Everything else

- **Parked offscreen windows.** With `offscreen_windows: 'park'`, windows that scroll out of
  view are moved beyond the edges of all monitors instead of being hidden, so they stay in
  Alt+Tab and the taskbar. Picking one in Alt+Tab scrolls it into view.
- **A single layout config.** `general.layout_switch`, `workspaces[].scroll`, and
  `workspaces[].bsp` are consolidated into the global `layout` block, with per-workspace
  overrides in `layout_overrides`. The old keys still work, but are deprecated.
- **Sensible defaults.** The scrolling layout, a vertical axis, centered scrolling, and
  `keep` as the layout switch behavior are the out-of-the-box defaults.
- **A local install script.** `resources/scripts/install-local.ps1` builds a signed
  UIAccess build, installs it, registers it for login, and launches it.

## Installation

GlideWM is **Windows only**, and is built and installed from source. It needs a Rust
toolchain. Install it with [rustup](https://rustup.rs), since the build requires nightly.

```sh
git clone https://github.com/JASSIM-ALHUMAID/glazewm.git
cd glazewm
./resources/scripts/install-local.ps1
```

That's the whole thing. The script builds the release binaries, creates a self-signed
code-signing certificate, signs the binaries with it, copies them to
`C:\Program Files\GlideWM`, registers GlideWM to start on login, and launches it. It needs
administrator rights, so it will show one UAC prompt.

The build is incremental, so re-running the script after a `git pull` only rebuilds what
changed. Pass `-SkipBuild` to install the binaries that are already in `target\release`.

The `ui_access` feature is what lets GlideWM manage elevated windows (such as Task
Manager) without running as administrator itself. It needs a *trusted* signature, which is
what the self-signed certificate is for. That's why the script installs the binaries into
Program Files and trusts the certificate machine-wide. The certificate's private key is
kept in the machine store, where only administrators can use it, so nothing running as
your user can sign code with it. Signing happens in the elevated step.

The script is currently the only supported way to install GlideWM. The WiX installer
sources in `resources/wix` are kept, but no installer is built or published, since an
installer without a trusted (paid) code-signing certificate can't grant UIAccess. Zebar is
no longer bundled; install it separately if you want it.

### The CLI

The CLI is installed to `C:\Program Files\GlideWM\cli\glidewm.exe`. Add that directory to
your `PATH` to use it from a terminal:

```sh
glidewm query workspaces
glidewm command set-layout --layout scrolling --workspace 1
glidewm command scroll --direction down --amount 10%
```

## Migrating from GlazeWM

GlideWM keeps GlazeWM's config format, with a few renames. If you already run GlazeWM, this
is what changed:

| GlazeWM                          | GlideWM                                     |
| -------------------------------- | ------------------------------------------- |
| `%userprofile%\.glzr\glazewm\`   | `%userprofile%\.glidewm\`                   |
| `GLAZEWM_CONFIG_PATH`            | `GLIDEWM_CONFIG_PATH`                       |
| `glazewm.exe`                    | `glidewm.exe`                               |
| `general.layout_switch`          | `layout.switch_behavior`                    |
| `workspaces[].scroll`            | `layout.scrolling` (+ `layout_overrides`)    |
| `workspaces[].bsp`               | `layout.bsp` (+ `layout_overrides`)         |

The old keys are still accepted: they are logged as deprecated and the new keys win, but
they should be moved over, since they will be removed eventually.

Workspaces default to the `scrolling` layout. To keep GlazeWM's tiling behavior, set
`layout.default: 'tiling'`.

Unknown keys inside the `layout` block (and the per-workspace `scroll`, `bsp`, and
`layout_overrides` blocks) are rejected rather than ignored, so a typo fails the config
load instead of being silently dropped.

## Default keybindings

On the first launch of GlideWM, a default configuration can optionally be generated.

Below is a cheat sheet of all available commands and their default keybindings.

![Infographic](/resources/assets/cheatsheet.png)

> The cheat sheet is inherited from GlazeWM, so it doesn't list the layout and animation
> commands added here. The full list lives in
> [`resources/assets/sample-config.yaml`](resources/assets/sample-config.yaml).

## Config documentation

The [default config](resources/assets/sample-config.yaml) file is generated at
`%userprofile%\.glidewm\config.yaml`.

To use a different config file location, you can launch the GlideWM executable with the CLI
argument `--config="..."`, like so:

```sh
./glidewm.exe start --config="C:\<PATH_TO_CONFIG>\config.yaml"
```

Or pass a value for the `GLIDEWM_CONFIG_PATH` environment variable:

```sh
setx GLIDEWM_CONFIG_PATH "C:\<PATH_TO_CONFIG>\config.yaml"
```

With the benefit of using a custom path being that you can choose a different name for the
config file, such as `glidewm.yaml`.

### Config: General

```yaml
general:
  # Commands to run when the WM has started (e.g. to run a script or launch
  # another application).
  startup_commands: []

  # Commands to run just before the WM is shutdown.
  shutdown_commands: []

  # Commands to run after the WM config has reloaded.
  config_reload_commands: []

  # Whether to automatically focus windows underneath the cursor.
  focus_follows_cursor: false

  # Whether to switch back and forth between the previously focused
  # workspace when focusing the current workspace.
  toggle_workspace_on_refocus: false

  cursor_jump:
    # Whether to automatically move the cursor on the specified trigger.
    enabled: true

    # Trigger for cursor jump:
    # - 'monitor_focus': Jump when focus changes between monitors.
    # - 'window_focus': Jump when focus changes between windows.
    trigger: "monitor_focus"
```

### Config: Keybindings

The available keyboard shortcuts can be customized via the `keybindings` option. A keybinding consists of one or more key combinations and one or more commands to run when pressed.

It's recommended to use the alt key for keybindings. The Windows key is unfortunately a pain to remap, since the OS reserves certain keybindings (e.g. `lwin+l`).

```yaml
keybindings:
  # Command(s) to run.
  - commands: ["focus --workspace 1"]

    # Key combination(s) to trigger the keybinding.
    bindings: ["alt+1"]

  # Multiple commands can be run in a sequence (e.g. to move a window to a
  # workspace + focus workspace).
  - commands: ["move --workspace 1", "focus --workspace 1"]
    bindings: ["alt+shift+1"]
```

**Full list of keys that can be used for keybindings:**

<details>
<summary>Keys list</summary>

| Key                   | Description                                                               |
| --------------------- | ------------------------------------------------------------------------- |
| `a` - `z`             | Alphabetical letter keys                                                  |
| `0` - `9`             | Number keys                                                               |
| `numpad0` - `numpad9` | Numerical keypad keys                                                     |
| `f1` - `f24`          | Function keys                                                             |
| `shift`               | Either left or right SHIFT key                                            |
| `lshift`              | The left SHIFT key                                                        |
| `rshift`              | The right SHIFT key                                                       |
| `control`             | Either left or right CTRL key                                             |
| `lctrl`               | The left CTRL key                                                         |
| `rctrl`               | The right CTRL key                                                        |
| `alt`                 | Either left or right ALT key                                              |
| `lalt`                | The left ALT key                                                          |
| `ralt`                | The right ALT key                                                         |
| `lwin`                | The left ⊞ Windows logo key                                               |
| `rwin`                | The right ⊞ Windows logo key                                              |
| `space`               | The spacebar key                                                          |
| `escape`              | The ESCAPE key                                                            |
| `back`                | The BACKSPACE key                                                         |
| `tab`                 | The TAB key                                                               |
| `enter`               | The ENTER key                                                             |
| `left`                | The ← arrow key                                                           |
| `right`               | The → arrow key                                                           |
| `up`                  | The ↑ arrow key                                                           |
| `down`                | The ↓ arrow key                                                           |
| `num_lock`            | The NUM LOCK key                                                          |
| `scroll_lock`         | The SCROLL LOCK key                                                       |
| `caps_lock`           | The CAPS LOCK key                                                         |
| `page_up`             | The PAGE UP key                                                           |
| `page_down`           | The PAGE DOWN key                                                         |
| `insert`              | The INSERT key                                                            |
| `delete`              | The DELETE key                                                            |
| `end`                 | The END key                                                               |
| `home`                | The HOME key                                                              |
| `print_screen`        | The PRINT SCREEN key                                                      |
| `multiply`            | The `*` key (only on numpad)                                              |
| `add`                 | The `+` key (only on numpad)                                              |
| `subtract`            | The `-` key (only on numpad)                                              |
| `decimal`             | The DEL key (only on numpad)                                              |
| `divide`              | The `/` key (only on numpad)                                              |
| `volume_up`           | The volume up key                                                         |
| `volume_down`         | The volume down key                                                       |
| `volume_mute`         | The volume mute key                                                       |
| `media_next_track`    | The media next track key                                                  |
| `media_prev_track`    | The media prev track key                                                  |
| `media_stop`          | The media stop key                                                        |
| `media_play_pause`    | The media play/pause key                                                  |
| `oem_semicolon`       | The `;`/`:` key on a US standard keyboard (varies by keyboard)            |
| `oem_question`        | The `/`/`?` key on a US standard keyboard (varies by keyboard)            |
| `oem_tilde`           | The `` ` ``/`~` key on a US standard keyboard (varies by keyboard)        |
| `oem_open_brackets`   | The `[`/`{` key on a US standard keyboard (varies by keyboard)            |
| `oem_pipe`            | The `\`/`\|` key on a US standard keyboard (varies by keyboard)           |
| `oem_close_brackets`  | The `]`/`}` key on a US standard keyboard (varies by keyboard)            |
| `oem_quotes`          | The `'`/`"` key on a US standard keyboard (varies by keyboard)            |
| `oem_8`               | The `` ` ``/`¬` key on a UK keyboard (varies by keyboard)                 |
| `oem_102`             | The `\`/`\|` key next to left Shift on ISO keyboards (varies by keyboard) |
| `oem_plus`            | The `=`/`+` key on a US standard keyboard (varies by keyboard)            |
| `oem_comma`           | The `,`/`<` key on a US standard keyboard (varies by keyboard)            |
| `oem_minus`           | The `-`/`_` key on a US standard keyboard (varies by keyboard)            |
| `oem_period`          | The `.`/`>` key on a US standard keyboard (varies by keyboard)            |
| `muhenkan`            | The 無変換 (non-convert) key for Japanese keyboard layouts                |
| `henkan`              | The 変換 (convert) key for Japanese keyboard layouts                      |

</details>

If a key is not in the list above, it is likely still supported if you use its character in a keybinding (e.g. `alt+å` for the Norwegian Å character).

> German and US international keyboards treat the right-side alt key differently. For these keyboard layouts, use `ralt+ctrl` instead of `ralt` to bind the right-side alt key.

### Config: Gaps

The gaps between windows can be changed via the `gaps` property in the config file. Inner and outer gaps are set separately.

```yaml
gaps:
  # Gap between adjacent windows.
  inner_gap: "20px"

  # Gap between windows and the screen edge.
  outer_gap:
    top: "20px"
    right: "20px"
    bottom: "20px"
    left: "20px"
```

### Config: Layouts

Layout options are configured once in the `layout` property in the config file, and shared by all workspaces. The values below are the built-in defaults.

```yaml
layout:
  # Layout used by workspaces that don't set their own `layout`.
  # - 'tiling': Windows share the viewport.
  # - 'scrolling': Windows form a strip along the scroll axis, and the
  #   viewport scrolls to keep the focused window in view (inspired by
  #   niri/PaperWM).
  # - 'bsp': Windows form a binary tree of splits; inserting a window
  #   splits the focused window's space in half (like bspwm).
  default: "scrolling"

  # What happens to the existing windows of a workspace when its layout
  # is switched (via `set-layout`, `cycle-layout`, or a config reload).
  # - 'reflow': Existing windows are rearranged into the new layout.
  # - 'keep': Existing windows keep their arrangement; only windows that
  #   are added afterwards follow the new layout.
  switch_behavior: "keep"

  # Options for the tiling layout. The tiling layout has no options yet.
  tiling: {}

  # Options for the scrolling layout.
  scrolling:
    # Axis that the strip of windows scrolls along. Also the initial
    # tiling direction of a scrolling workspace: `toggle-tiling-direction`
    # flips it at runtime, and that change is not written back here.
    axis: "vertical"

    # Default width of new windows as a fraction of the viewport width.
    # Used while the scroll axis is horizontal.
    window_width: 0.7

    # Default height of new windows as a fraction of the viewport height.
    # Used while the scroll axis is vertical.
    window_height: 0.7

    # Sizes that `cycle-preset-size` cycles the focused window through
    # (fractions of the viewport), for a horizontal and a vertical scroll
    # axis respectively.
    preset_widths: [0.3, 0.5, 0.7, 1.0]
    preset_heights: [0.5, 1.0]

    # How the focused window is scrolled into view:
    # - 'minimal': Scroll the least amount required (niri-like).
    # - 'center': Center the focused window in the viewport.
    align: "center"

    # Where new windows are inserted in the strip:
    # - 'start': Insert at the start of the strip (top/left).
    # - 'before_focused': Insert directly before the focused window.
    # - 'after_focused': Insert directly after the focused window.
    # - 'end': Append to the end of the strip (bottom/right).
    insert_position: "after_focused"

    # How windows scrolled out of view are kept out of view:
    # - 'hide': Hidden with the hide method (not listed in Alt+Tab).
    # - 'park': Moved beyond the edges of all monitors while staying shown,
    #   so they remain in Alt+Tab and the taskbar.
    offscreen_windows: "park"

  # Options for the BSP layout.
  bsp:
    # How the direction of a new split is chosen:
    # - 'longest_side': Split the longer side of the focused window
    #   (like bspwm).
    # - 'alternate': Alternate the split direction with the depth of the
    #   tree.
    automatic_scheme: "longest_side"
```

A workspace can use a different layout, and can override individual layout options. Overrides are merged field by field, so only the options that are listed are changed.

```yaml
workspaces:
  # Uses the global `layout.default`.
  - name: "1"

  # Uses the scrolling layout, with the global scrolling options.
  - name: "2"
    layout: "scrolling"

  # Uses the scrolling layout, with a smaller default window size.
  - name: "3"
    layout: "scrolling"
    layout_overrides:
      scrolling:
        window_width: 0.5
```

### Config: Animations

Animations are configured in the `general.animations` property. They require
`general.hide_method: 'cloak'`, since the real window is cloaked while its thumbnail is
animated.

```yaml
general:
  hide_method: "cloak"

  animations:
    # Whether window animations are enabled.
    enabled: true

    # Animation to play when the displayed workspace on a monitor changes.
    workspace_switch:
      # - 'none': No animation.
      # - 'slide': Windows slide horizontally.
      # - 'slide_vert': Windows slide vertically.
      # - 'fade': The workspaces cross-fade in place.
      style: "slide"

      # Where the incoming workspace slides in from. 'auto' derives the
      # direction from the workspace switch.
      # - 'auto' | 'left' | 'right' | 'up' | 'down'.
      direction: "auto"

      # How long the animation lasts, in milliseconds.
      duration_ms: 250

      # Easing curve as cubic-bezier control points [x1, y1, x2, y2].
      curve: [0.22, 1.0, 0.36, 1.0]

    # Animation to play when a window is first shown.
    window_open:
      # - 'none': No animation.
      # - 'slide': Window slides in from the right edge of the monitor.
      # - 'slide_vert': Window slides in from the bottom edge.
      # - 'popin': Window grows into place while fading in.
      # - 'fade': Window fades in.
      style: "slide"

      # How long the animation lasts, in milliseconds.
      duration_ms: 200

      # Easing curve as cubic-bezier control points [x1, y1, x2, y2].
      curve: [0.22, 1.0, 0.36, 1.0]

    # Animation to play when a visible window changes position or size.
    window_move:
      # Whether the animation is enabled.
      enabled: true

      # How long the animation lasts, in milliseconds.
      duration_ms: 150

      # Easing curve as cubic-bezier control points [x1, y1, x2, y2].
      curve: [0.22, 1.0, 0.36, 1.0]
```

### Config: Workspaces

Workspaces need to be predefined via the `workspaces` property in the config file. A workspace is automatically assigned to each monitor on startup.

```yaml
workspaces:
  # This is the unique ID for the workspace. It's used in keybinding
  # commands, and is also the label shown in 3rd-party apps (e.g. Zebar) if
  # `display_name` is not provided.
  - name: "1"

    # Optional override for the workspace label used in 3rd-party apps.
    # Does not need to be unique.
    display_name: "Work"

    # Optionally force the workspace on a specific monitor if it exists.
    # 0 is your leftmost screen, 1 is the next one to the right, and so on.
    bind_to_monitor: 0

    # Optionally prevent workspace from being deactivated when empty.
    keep_alive: false

    # Optionally use a different layout than `layout.default`.
    # - 'tiling': Windows share the viewport.
    # - 'scrolling': Windows form a strip along the scroll axis.
    # - 'bsp': Windows form a binary tree of splits.
    layout: "scrolling"

    # Optionally override individual layout options. Only the listed
    # options are overridden; the rest are inherited from `layout`.
    layout_overrides:
      scrolling:
        window_width: 0.5

    # Optionally override `layout.switch_behavior` for this workspace.
    # - 'reflow': Existing windows are rearranged into the new layout.
    # - 'keep': Existing windows keep their arrangement.
    layout_switch: "keep"
```

### Config: Window rules

Commands can be run when a window is first launched. This is useful for adding window-specific behaviors like always starting a window as fullscreen or assigning to a specific workspace.

Windows can be targeted by their process, class, and title. Multiple matching criteria can be used together to target a window more precisely.

```yaml
window_rules:
  - commands: ["move --workspace 1"]
    match:
      # Move browsers to workspace 1.
      - window_process: { regex: "msedge|brave|chrome" }

  - commands: ["ignore"]
    match:
      # Ignores any Zebar windows.
      - window_process: { equals: "zebar" }

      # Ignores picture-in-picture windows for browsers.
      # Note that *both* the title and class must match for the rule to run.
      - window_title: { regex: "[Pp]icture.in.[Pp]icture" }
        window_class: { regex: "Chrome_WidgetWin_1|MozillaDialogClass" }
```

### Config: Window effects

Visual effects can be applied to windows via the `window_effects` option. Currently, colored borders are the only effect available with more to come in the future.

> Note: Window effects are exclusive to Windows 11.

```yaml
window_effects:
  # Visual effects to apply to the focused window.
  focused_window:
    # Highlight the window with a colored border.
    border:
      enabled: true
      color: "#0000ff"

  # Visual effects to apply to non-focused windows.
  other_windows:
    border:
      enabled: false
      color: "#d3d3d3"
```

### Config: Window behavior

The `window_behavior` config option exists to customize the states that a window can be in (`tiling`, `floating`, `minimized`, and `fullscreen`).

```yaml
window_behavior:
  # New windows are created in this state whenever possible.
  # Allowed values: 'tiling', 'floating'.
  initial_state: "tiling"

  # Sets the default options for when a new window is created. This also
  # changes the defaults for when the state change commands, like
  # `set-floating`, are used without any flags.
  state_defaults:
    floating:
      # Whether to center floating windows by default.
      centered: true

      # Whether to show floating windows as always on top.
      shown_on_top: false

    fullscreen:
      # Maximize the window if possible. If the window doesn't have a
      # maximize button, then it'll be made fullscreen normally instead.
      maximized: false
```

### Config: Binding modes

Binding modes are used to modify keybindings while GlideWM is running.

A binding mode can be enabled with `wm-enable-binding-mode --name <NAME>` and disabled with `wm-disable-binding-mode --name <NAME>`.

```yaml
binding_modes:
  # When enabled, the focused window can be resized via arrow keys or HJKL.
  - name: "resize"
    keybindings:
      - commands: ["resize --width -2%"]
        bindings: ["h", "left"]
      - commands: ["resize --width +2%"]
        bindings: ["l", "right"]
      - commands: ["resize --height +2%"]
        bindings: ["k", "up"]
      - commands: ["resize --height -2%"]
        bindings: ["j", "down"]
      # Cycle the focused window through the preset sizes of the scrolling
      # layout. Only scrolling workspaces have preset sizes.
      - commands: ["cycle-preset-size"]
        bindings: ["shift+oem_plus"]
      - commands: ["cycle-preset-size --reverse"]
        bindings: ["oem_minus"]
      # Press enter/escape to return to default keybindings.
      - commands: ["wm-disable-binding-mode --name resize"]
        bindings: ["escape", "enter"]
```

> The `+` key needs shift on most keyboard layouts, so it's spelled out as `shift` plus the
> `=` key (`oem_plus`). A literal `+` can't be used in a binding: binding strings are split
> on `+`, and a key that requires shift can't be resolved as a literal character.

## FAQ

**Q: How do I run GlideWM on startup?**

Right-click the GlideWM icon in the system tray and select "Run on system startup". The
local install script also registers it for you.

**Q: How do I switch a workspace between layouts?**

`set-layout --layout <tiling|scrolling|bsp>` sets the layout of the focused workspace (or
of the workspace given by `--workspace`), and `cycle-layout` cycles through the available
ones. The switch applies for the session; set the workspace's `layout` in the config to
make it permanent.

By default, existing windows keep their arrangement when the layout changes
(`switch_behavior: 'keep'`). Set it to `'reflow'` to rearrange them into the new layout.

**Q: How do I scroll a scrolling workspace?**

`scroll --direction <left|right|up|down> --amount <value>` scrolls the strip along its axis
without moving focus. `center-focused` centers the focused window in the viewport, and
`cycle-preset-size` resizes the focused window through the configured preset sizes. It's
bound to `+` and `-` inside the `resize` binding mode (`alt+r`), so `alt+r` then `+` jumps to
the next preset and `alt+r` then `-` to the previous one.

**Q: How do I create `<insert layout>`?**

Layouts are first-class in GlideWM. Pick one of `tiling`, `scrolling`, or `bsp` in the
`layout` property, or switch a workspace at runtime with `set-layout`. Within the tiling
layout, `alt+v` still changes the tiling direction, which determines where the next window
is placed _in relation to the current window_.

Community-made scripts like [Dutch-Raptor/GAT-GWM](https://github.com/Dutch-Raptor/GAT-GWM)
and [burgr033/GlazeWM-autotiling-python](https://github.com/burgr033/GlazeWM-autotiling-python)
can be used to automatically change the tiling direction. Native support for automatic
layouts isn't _currently_ supported.

**Q: How do I create a rule for `<insert application>`?**

To match a specific application, you need a command to execute and either the window's process name, title, or class name. For example, if you use Flow-Launcher and want to make the settings window float, you can do the following:

```yaml
window_rules:
  - commands: ["set-floating"]
    match:
      - window_process: { equals: "Flow.Launcher" }
        window_title: { equals: "Settings" }
```

Programs like Winlister or AutoHotkey's Window Spy can be useful for getting info about a window.

**Q: How can I ignore GlideWM's keybindings when `<insert application>` is focused?**

This isn't currently supported, however, the keybinding `alt+shift+p` in the default config is used to disable all other keybindings until `alt+shift+p` is pressed again.

**Q: Why do my animations not play?**

Animations need `general.hide_method: 'cloak'` and `general.animations.enabled: true`. They
are also Windows-only. If a window is animated by an external tool (e.g. Windhawk), GlideWM
leaves it alone.

## Credits

GlideWM stands on the shoulders of [GlazeWM](https://github.com/glzr-io/glazewm) by Glzr
Software, and of the wider tiling window manager community: niri, bspwm, PaperWM, i3, and
Polybar. Thank you.

[discord-badge]: https://img.shields.io/discord/1041662798196908052.svg?logo=discord&colorB=7289DA
[discord-link]: https://discord.gg/ud6z3qjRvM
[downloads-badge]: https://img.shields.io/github/downloads/JASSIM-ALHUMAID/glazewm/total?logo=github&logoColor=white
[downloads-link]: https://github.com/JASSIM-ALHUMAID/glazewm/releases
[demo-video]: resources/assets/demo.webp
