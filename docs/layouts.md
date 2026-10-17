# Layouts — architecture

Status: **implemented** · Branch: `feat/scrolling-layout` · Fork: `JASSIM-ALHUMAID/glazewm` (upstream: `glzr-io/glazewm`)

This document describes how workspace layouts are organized so that new
layouts can be added without touching layout-specific `if`s around the
codebase. It is the entry point for working on layouts; the scrolling
layout's own design lives in `docs/scrolling-layout.md`.

---

## 1. Overview

A workspace arranges its **top-level tiling children** according to its
layout. The layout is:

- set per workspace in the config (`workspaces[].layout`),
- switchable at runtime with `set-layout` and `cycle-layout`,
- readable from `Workspace::layout() -> WorkspaceLayout` (a cheap clone;
  defined in `packages/wm-common/src/parsed_config.rs`, so it round-trips
  through the config and the IPC DTOs).

`WorkspaceLayout` currently has two variants: `Tiling` (default) and
`Scrolling`. The cycle order is `WorkspaceLayout::ALL`
(`Tiling → Scrolling → Tiling`); `WorkspaceLayout::next()` computes the
next layout.

All layout behavior is implemented in `packages/wm/src/layouts/`. Call
sites outside that module ask **layout-agnostic questions** (e.g. "what
rect does this child get?") and never match on `WorkspaceLayout`
themselves.

```
packages/wm/src/layouts/
  mod.rs        # dispatch functions + shared types; the only place that matches on WorkspaceLayout
  tiling.rs     # default tiling behavior (pub(crate))
  scrolling.rs  # scrolling (Niri-style strip) layout
```

## 2. The dispatch layer (`layouts/mod.rs`)

Each dispatch function answers one question that some call site has.
Layouts that share the generic tiling behavior can fall through to
`tiling::*`; layouts with their own behavior implement it in their
module.

| Function | Question it answers | `Tiling` | `Scrolling` |
| --- | --- | --- | --- |
| `child_rect` | Rect of a direct child within the workspace viewport | Share-based rect | Strip rect (viewport fraction − scroll offset) |
| `default_child_size` | Size to give a new direct child | `None` (sized by attach) | `scroll.window_width` |
| `child_length_reference` | Length that a direct child's `tiling_size` is a fraction of | `None` (space alongside siblings) | Viewport extent along the axis |
| `on_child_attached` | Size child after attaching to a parent | Size child to an even share of siblings | Leave sizes untouched |
| `on_child_detached` | Free space after detaching from a parent | Redistribute to remaining siblings | Leave sizes untouched |
| `resize_tiling_container` | Apply a resize to a tiling container | Redistribute freed space among siblings | Resize the item directly along the axis |
| `direct_child_resize` | Whether a direct-child resize bypasses generic resize logic | `FallThrough` | `Resize` along the axis, `Skip` cross-axis |
| `preserves_split_children` | Whether split children are kept as first-class "columns" | `false` (flatten redundant splits) | `true` |
| `insertion_target` | Where a new window is inserted in the tree | Focused sibling's parent | Strip level, per `scroll.insert_position` |
| `insert_window_into_workspace` | Placement when moving a window into a workspace | Sibling's parent at index + 1 | Strip item after the sibling's strip item |
| `move_child_in_direction` | Move of a window that is a direct workspace child | `None` (generic move) | Reorder in the strip, or move to the adjacent workspace |
| `move_without_target_ancestor` | Move when no ancestor can contain it | Invert the workspace's direction | Move to the workspace in that direction |
| `toggle_child_direction` | Toggle of a direct child's tiling direction | `None` (wrap in a split container) | Flip the scroll axis |
| `rescale_factor_for_move` | Size rescale when a container moves into a new parent | `1.0` | Preserve the viewport fraction across nested parents |
| `initial_tiling_direction` | Direction to initialize a new workspace with | Monitor orientation | `scroll.axis` from config |
| `supports_scrolling` | Whether scroll behavior (e.g. the `scroll` command) applies | `false` | `true` |
| `clips_children_to_viewport` | Whether hit tests must be limited to the viewport | `false` | `true` |
| `window_placement` | Rect to position a window at + whether it is scrolled out | Natural rect | Clamped just beyond the viewport + `is_scrolled_out` |
| `on_layout_changed` | State updates after a runtime layout switch | Rescale sizes to fill the viewport | Normalize sizes/scroll position |
| `on_workspace_config_reloaded` | State updates after a config reload changed a workspace | Nothing | Apply the config's axis; normalize on a layout switch |

Notes:

- `WindowPlacement { rect, is_scrolled_out }` pairs the rect to
  position a window at with whether the layout has scrolled it out of
  view (used by `platform_sync` to cloak/hide it).
- `DirectChildResize { Resize, Skip, FallThrough }` lets layouts
  override the generic direct-child resize without the call site
  matching on the layout.
- Helpers `layout_of_container` / `layout_of_direction_container`
  resolve "which layout applies to this parent?" (only workspaces have a
  configurable layout; everything else has tiling semantics).
- Functions that return `Option` (`move_child_in_direction`,
  `toggle_child_direction`) are hooks: `None` means "use the generic
  tiling behavior", keeping the call site unaware of which layouts hook
  in.

## 3. Runtime switching

- `set-layout --layout <tiling|scrolling> [--workspace <name>]` sets the
  layout directly (`InvokeCommand::SetLayout` →
  `set_workspace_layout`).
- `cycle-layout [--workspace <name>]` advances to
  `WorkspaceLayout::next()` (`InvokeCommand::CycleLayout`). The default
  keybinding is `alt+shift+v`.
- Both funnel into `set_workspace_layout`, which updates the workspace
  config, records it as a session override, calls
  `layouts::on_layout_changed` (the scrolling layout clamps sizes, resets
  the scroll offset and scrolls the focused window into view; the tiling
  layout rescales sizes so that the children fill the viewport), queues a
  redraw and emits `WorkspaceUpdated`.
- Config reloads go through `layouts::on_workspace_config_reloaded` so
  that a config that switches a workspace's layout behaves the same as
  `set-layout`.
- Runtime switches last for the rest of the WM session
  (`WmState::workspace_config_overrides`): an empty workspace that is
  destroyed and later activated again comes back with its overridden
  config. Config reload clears the overrides, so permanence comes from
  the workspace's `layout` in the config file.

Because the layout is part of the workspace config, no new event or DTO
is needed for switching: `WorkspaceUpdated` / `WorkspaceDto` already
carry `layout` (and `scrollOffset`).

## 4. Adding a new layout

1. Add a variant to `WorkspaceLayout`
   (`packages/wm-common/src/parsed_config.rs`), including its
   `from_str` arm, its place in `WorkspaceLayout::ALL`, and its
   `next()` arm. The `parsed_config` test
   `cycle_order_matches_all_layouts` keeps `ALL` and `next()` in sync.
2. Add a `packages/wm/src/layouts/<name>.rs` submodule implementing the
   layout's behavior. Behavior that matches the default tiling semantics
   can be omitted and dispatched to `tiling::*`.
3. Add match arms in every dispatch function of `layouts/mod.rs`. The
   matches on `WorkspaceLayout` are exhaustive, so the compiler lists
   exactly what needs an arm.
4. If the layout needs per-workspace config, add it to `WorkspaceConfig`
   next to `scroll` (which plays that role for the scrolling layout) and
   read it from the layout module via `workspace.config()`.
5. Put the layout's unit tests in its module and dispatch-level tests in
   `layouts/mod.rs`.

`WorkspaceLayout::Tiling` is the reference implementation: whenever a
new layout doesn't specify a behavior, dispatching to `tiling::*` keeps
the generic behavior working.

## 5. What the seam deliberately does *not* cover

- **Focus navigation** is shared: `focus --direction` is order-based
  (siblings are ordered by the workspace's `tiling_direction`, which
  doubles as the scroll axis), so no layout-specific code is needed.
- **Container tree, attach/detach, move plumbing** are shared; layouts
  only hook into them via the dispatch functions above.
- **Scroll-specific commands** (`scroll`) are gated by
  `supports_scrolling` and then call into
  `layouts::scrolling::*` for strip math (see `docs/scrolling-layout.md`
  §4.6). A future scrolling-capable layout would extend these dispatch
  points rather than forking the commands.
