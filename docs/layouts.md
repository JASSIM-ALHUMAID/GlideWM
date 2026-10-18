# Layouts — architecture

Status: **implemented** (tiling, scrolling, BSP) · Branch: `my-glaze` · Fork: `JASSIM-ALHUMAID/glazewm` (upstream: `glzr-io/glazewm`)

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

`WorkspaceLayout` currently has three variants: `Tiling` (default),
`Scrolling`, and `Bsp`. The cycle order is `WorkspaceLayout::ALL`
(`Tiling → Scrolling → Bsp → Tiling`); `WorkspaceLayout::next()` computes
the next layout.

All layout behavior is implemented in `packages/wm/src/layouts/`. Call
sites outside that module ask **layout-agnostic questions** (e.g. "what
rect does this child get?") and never match on `WorkspaceLayout`
themselves.

```
packages/wm/src/layouts/
  mod.rs        # dispatch functions + shared types; the only place that matches on WorkspaceLayout
  tiling.rs     # default tiling behavior (pub(crate))
  scrolling.rs  # scrolling (Niri-style strip) layout
  bsp.rs        # binary space partitioning (bspwm-style) layout (pub(crate))
```

## 2. The dispatch layer (`layouts/mod.rs`)

Each dispatch function answers one question that some call site has.
Layouts that share the generic tiling behavior can fall through to
`tiling::*`; layouts with their own behavior implement it in their
module.

| Function | Question it answers | `Tiling` | `Scrolling` | `Bsp` |
| --- | --- | --- | --- | --- |
| `child_rect` | Rect of a direct child within the workspace viewport | Share-based rect | Strip rect (viewport fraction − scroll offset) | Share-based rect |
| `default_child_size` | Size to give a new direct child | `None` (sized by attach) | `scroll.window_width` / `window_height` (per scroll axis) | `None` (sized by attach) |
| `child_length_reference` | Length that a direct child's `tiling_size` is a fraction of | `None` (space alongside siblings) | Viewport extent along the axis | `None` (space alongside siblings) |
| `on_child_attached` | Size child after attaching to a parent | Size child to an even share of siblings | Leave sizes untouched | Size child to an even share of siblings (splits end up 0.5/0.5) |
| `on_child_detached` | Free space after detaching from a parent | Redistribute to remaining siblings | Leave sizes untouched | Redistribute to remaining siblings |
| `resize_tiling_container` | Apply a resize to a tiling container | Redistribute freed space among siblings | Resize the item directly along the axis | Redistribute freed space among siblings |
| `direct_child_resize` | Whether a direct-child resize bypasses generic resize logic | `FallThrough` | `Resize` along the axis, `Skip` cross-axis | `FallThrough` |
| `split_child_flattening` | What counts as a redundant split child when flattening | `Fold` (flatten redundant splits) | `PreserveColumns` | `PreserveBinarySplits` (only single-child splits collapse) |
| `insertion_target` | Where a new window is inserted in the tree | Focused sibling's parent | Strip level, per `scroll.insert_position` | Wrap the focused window in a new split container |
| `insert_window_into_workspace` | Placement when moving a window into a workspace | Sibling's parent at index + 1 | Strip item after the sibling's strip item | Wrap the insertion sibling in a new split container |
| `move_child_in_direction` | Move of a window that is a direct workspace child | `None` (generic move) | Reorder in the strip, or move to the adjacent workspace | `None` (generic move) |
| `move_without_target_ancestor` | Move when no ancestor can contain it | Invert the workspace's direction | Move to the workspace in that direction | Invert the workspace's direction |
| `toggle_child_direction` | Toggle of a direct child's tiling direction | `None` (wrap in a split container) | Flip the scroll axis | `None` (wrap in a split container) |
| `rescale_factor_for_move` | Size rescale when a container moves into a new parent | `1.0` | Preserve the viewport fraction across nested parents | `1.0` |
| `initial_tiling_direction` | Direction to initialize a new workspace with | Monitor orientation | `scroll.axis` from config | Monitor orientation |
| `supports_scrolling` | Whether scroll behavior (e.g. the `scroll` command) applies | `false` | `true` | `false` |
| `clips_children_to_viewport` | Whether hit tests must be limited to the viewport | `false` | `true` | `false` |
| `window_placement` | Rect to position a window at + whether it is scrolled out | Natural rect | Clamped just beyond the viewport + `is_scrolled_out` | Natural rect |
| `on_layout_changed` | State updates after a runtime layout switch | Rescale sizes to fill the viewport | Normalize sizes/scroll position | Rebuild a balanced binary tree |
| `on_workspace_config_reloaded` | State updates after a config reload changed a workspace | Nothing | Apply the config's axis; normalize on a layout switch | Nothing |

Notes:

- `WindowPlacement { rect, is_scrolled_out }` pairs the rect to
  position a window at with whether the layout has scrolled it out of
  view (used by `platform_sync` to cloak/hide it).
- `DirectChildResize { Resize, Skip, FallThrough }` lets layouts
  override the generic direct-child resize without the call site
  matching on the layout.
- `SplitChildFlattening { Fold, PreserveColumns, PreserveBinarySplits }`
  decides what `flatten_child_split_containers` treats as a redundant
  split child: the tiling layout folds redundant nesting into the
  parent, the scrolling layout keeps multi-window columns, and the BSP
  layout collapses only splits that have a single child (so the binary
  tree survives window removals). Unlike `layout_of_direction_container`
  (which resolves anything but a workspace to the tiling semantics),
  `split_child_flattening` resolves a split through the workspace it
  lives in, so splits nested inside a BSP workspace keep following BSP
  rules.
- Helpers `layout_of_container` / `layout_of_direction_container`
  resolve "which layout applies to this parent?" (only workspaces have a
  configurable layout; everything else has tiling semantics).
- Functions that return `Option` (`move_child_in_direction`,
  `toggle_child_direction`) are hooks: `None` means "use the generic
  tiling behavior", keeping the call site unaware of which layouts hook
  in.

### The BSP layout (`bsp.rs`)

Workspaces with `layout: 'bsp'` arrange their tiling children as a
binary tree of split containers (like bspwm): every split divides its
space between exactly two children. Inserting a window wraps the focused
window in a new split container and puts the new window next to it, so
the focused window's space halves. The direction of the new split is
chosen per the workspace's `bsp.automatic_scheme` config:

- `longest_side` (default): split the longer side of the focused
  window's rect — a wide rect is cut into side-by-side children, a tall
  rect into stacked children.
- `alternate`: alternate the split direction with the depth of the tree,
  starting from the workspace's tiling direction.

Switching *to* the layout rebuilds the workspace's children as a
balanced binary tree (`bsp::rebuild_tree`), so the layout activates as a
mosaic instead of applying whatever tree the previous layout left
behind. The rebuild preserves the focused window as the workspace's
first leaf in focus order, so focus and subsequent insertions keep
targeting it. Switching *from* BSP to the scrolling layout dissolves the
tree's root split first (`bsp::collapse_root_split`), so the tree's
children become individual strip items.

## 3. Runtime switching

- `set-layout --layout <tiling|scrolling|bsp> [--workspace <name>]` sets
  the layout directly (`InvokeCommand::SetLayout` →
  `set_workspace_layout`).
- `cycle-layout [--workspace <name>]` advances to
  `WorkspaceLayout::next()` (`InvokeCommand::CycleLayout`). The default
  keybinding is `alt+shift+v`.
- Both funnel into `set_workspace_layout`, which updates the workspace
  config, records it as a session override, calls
  `layouts::on_layout_changed` (the scrolling layout clamps sizes, resets
  the scroll offset and scrolls the focused window into view; the tiling
  layout rescales sizes so that the children fill the viewport; the BSP
  layout rebuilds the children as a balanced binary tree), queues a
  redraw and emits `WorkspaceUpdated`. A BSP → scrolling switch also
  dissolves the BSP tree's root split first, so the tree's children
  become the strip's items.
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
   next to `scroll` (which plays that role for the scrolling layout;
   `bsp` does the same for the BSP layout) and read it from the layout
   module via `workspace.config()`.
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
