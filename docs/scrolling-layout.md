# Scrolling layout (Niri-style) — design & implementation plan

Status: **v1 implemented - P1–P5 complete (config/model, core geometry, navigation & off-viewport sync, mutation semantics, commands & IPC); P6 (region clipping) deferred; P7 (layout extraction into `packages/wm/src/layouts/`) complete** · Branch: `feat/scrolling-layout` · Fork: `JASSIM-ALHUMAID/glazewm` (upstream: `glzr-io/glazewm`)

This document organizes the work for adding a Niri/Hyprscroller-style **scrolling
layout** to GlazeWM. It is the working plan for this branch: model, config
schema, runtime semantics, platform concerns, and a file-by-file change map.

> The scrolling implementation was since extracted into the pluggable layout
> system under `packages/wm/src/layouts/` (see §6.7 and `docs/layouts.md`).
> File paths in the phase tables below mark where each change originally
> landed; code that §4/§5 describes as living in `commands/` or `traits/` now
> lives in `layouts::scrolling` behind a dispatch function (e.g.
> `layouts::resize_tiling_container`).

Upstream prior art that validates the approach:
`feat/accordian-v1` adds a layout mode by extending `TilingDirection`, branching
in `position_getters.rs`, and adding navigation commands. The scrolling layout
follows the same seams, plus scroll-offset state and platform clipping.

---

## 1. Goals / non-goals

**Goals (v1)**

- Workspaces can use a scrollable tiling layout: windows form an infinite strip
  along one axis. Windows keep their size; focus changes scroll the viewport.
- Per-workspace opt-in via config; existing tiling behavior is untouched by default.
- Focus/move/resize/insert/close have well-defined, Niri-like semantics.
- Runtime command to switch layout (`set-layout`), reversible.
- Works on Windows (primary) and macOS (best effort, documented differences).

**Non-goals (v1)**

- Scroll animations / smooth inertial scrolling (no animation system yet).
- Trackpad & mouse-wheel scrolling, drag-to-scroll, overview mode.
- Full column management (stacking windows vertically inside a column relies
  on existing `SplitContainer` machinery — see §2.2; not a v1 command surface).

---

## 2. Model

### 2.1 Layout vs. tiling direction

Tiling direction stays as-is (`horizontal` / `vertical`). A new per-workspace
`Layout` mode decides *how* the workspace's children are arranged:

```
Layout::Tiling     — existing behavior (children share the viewport).
Layout::Scrolling  — children form a strip along an axis; viewport scrolls.
```

`Workspace.tiling_direction` doubles as the scrolling **axis** when
`Layout::Scrolling` is active. `toggle-tiling-direction` on a scrolling
workspace therefore toggles the scroll axis (horizontal ⇄ vertical).
Split containers nested inside a scrolling workspace keep normal tiling
semantics (a vertical split inside a horizontal strip = a column).

### 2.2 Tree mapping (why this needs almost no new container types)

Niri's model — workspace → columns → stacked windows — maps directly:

| Niri concept         | GlazeWM container                                    |
| -------------------- | ---------------------------------------------------- |
| Workspace strip      | `Workspace` with `Layout::Scrolling`                 |
| Column               | child of the workspace: `TilingWindow` *or* `SplitContainer` |
| Stacked window       | child of a column `SplitContainer` (existing math)   |
| Viewport             | workspace rect (monitor working area − outer gaps)   |
| Scroll position      | new `scroll_offset: i32` field on `WorkspaceInner`   |

The only new geometry is: children of a scrolling container are positioned
along the axis with **cumulative sizes minus `scroll_offset`**, and are sized
as a **fraction of the viewport** instead of sharing remaining space.

### 2.3 Scroll position state

- Stored per workspace: `scroll_offset: i32` (physical px; `0` = strip start).
- Meaningful only while `Layout::Scrolling` and the workspace is displayed.
- Invariants:
  - `0 ≤ scroll_offset ≤ max(0, strip_extent − viewport_extent)`
  - On focus change: top-level rolling **ensure-visible** adjustment so the
    focused descendant is fully within the viewport (`minimal` = scroll just
    enough, `center` = center it, per config).
  - On any structural change (insert/close/resize/reorder/layout switch/
    monitor change): clamp only. Clamping never fights user scroll commands
    (`scroll` command adjusts the offset *without* focusing; the offset is
    re-ensured only on the next focus change).
- Derived, not stored: `strip_extent` = Σ (child size + inner gap) − trailing gap.

### 2.4 Rect math (the core)

`impl_position_getters_as_resizable!` (in
`packages/wm/src/traits/position_getters.rs`) grows a scrolling branch used
when the parent is a scrolling workspace (windows or splits as direct children):

```
child_width  = round(viewport_extent * tiling_size)   // fraction semantics
child_offset = Σ (prev sibling sizes + inner_gap)
pos_along    = viewport_start − scroll_offset + child_offset
```

Cross-axis = viewport cross-axis (minus nothing). Off-viewport children still
receive their true rect (their positions may lie outside the workspace rect);
clipping is a platform concern (§5).

Note: `tiling_size` changes semantics from "share of remaining space" to
"fraction of viewport" only under `Layout::Scrolling`. Switching layouts
normalizes existing values on the switch: sizes are clamped into the valid
range when switching to scrolling, and rescaled into shares that fill the
viewport when switching to tiling (see §4.6).

Lone item rule (Niri's "a single column fills the workspace"): when the
workspace has exactly one tiling child, its effective extent is the **full
viewport extent**, regardless of its stored `tiling_size`
(`Workspace::strip_item_extent`). The stored size is preserved and applies
again as soon as a second tiling child exists; closing back down to one item
re-expands it. `strip_extent` uses the same rule, so a lone item yields
`max_scroll_offset == 0`. Consequence: resizing a lone window only updates its
stored fraction (observable once it stops being alone).

---

## 3. Config schema

Per workspace, snake_case keys (this is what `ParsedConfig` deserializes;
camelCase is only used for serialization/IPC):

```yaml
workspaces:
  - name: '1'
    layout: scrolling        # 'tiling' (default) | 'scrolling'
    scroll:                  # only read when layout == scrolling; optional
      axis: horizontal       # 'horizontal' (default) | 'vertical'
      window_width: 0.5      # default size of new windows (fraction of viewport)
      align: minimal         # 'minimal' (default, Niri-like) | 'center'
      insert_position: after_focused   # 'after_focused' | 'end'
```

Rust side (`packages/wm-common/src/parsed_config.rs`):

```rust
pub struct WorkspaceConfig {
  // existing fields...
  #[serde(default)]
  pub layout: WorkspaceLayout,        // enum { Tiling, Scrolling } (default Tiling)
  #[serde(default)]
  pub scroll: ScrollConfig,           // axis, window_width, align, insert_position
}
```

Also: `resources/assets/sample-config.yaml` — documented commented block.

---

## 4. Runtime behavior spec

### 4.1 Focus (`focus --direction`)

Inside a scrolling workspace, navigation is **order-based**, not
geometry-based (mirrors Niri):

- `left`/`up` → previous sibling in strip order; `right`/`down` → next sibling.
- At the strip ends: fall through to existing workspace/monitor focus targets
  (wrap is a possible config follow-up).
- Focus target inside a column (nested split): descend to the column's
  last-focused descendant; `up`/`down` within a column uses existing
  direction logic first before crossing columns (verified in tests).
- After focus lands: ensure-visible adjusts `scroll_offset`.

Implementation: **no code change needed.** `tiling_focus_target` in
`packages/wm/src/commands/container/focus_in_direction.rs` compares each
ancestor's tiling direction against the pressed direction. Since a scrolling
workspace's `tiling_direction` is its scroll axis (§2.1), the existing
order-based sibling logic navigates the strip unmodified. Covered by tests in
`focus_in_direction.rs` (strip edges, cross-axis, nested columns).

### 4.2 Ensure-visible (scroll on focus)

`packages/wm/src/commands/workspace/update_scroll_position.rs` (new):

- `ensure_descendant_visible(workspace, container)`:
  - fully visible → no-op
  - `align: minimal` → scroll the least amount so the container is fully visible
  - `align: center` → center it
- `clamp_scroll_position(workspace)`: enforce §2.3 invariants.
- Wired into `platform_sync` (`packages/wm/src/commands/general/platform_sync.rs`):
  `update_scroll_position` runs after `sync_focus` and before redrawing
  containers. It iterates the displayed workspace of every monitor: the
  focused workspace is ensured-visible when `pending_sync.needs_focus_update()`
  and every displayed scrolling workspace is clamped (covers strip changes
  from closes/moves on non-focused workspaces). A changed offset queues a
  redraw of the workspace.

### 4.3 Insert

`attach_container` + `insertion_target` (in `manage_window.rs`):

- `insertion_target` returns the scrolling workspace as parent and the index
  just after the strip item that contains the focused container
  (`focused_strip_item_index`), so a new window always becomes its own strip
  item — never a child of a column:
  - `after_focused`: insert after the focused container's strip item; falls
    back to the end of the strip if the workspace has no tiling windows.
  - `end`: append to workspace.
- `attach_container` **skips sibling redistribution** for a scrolling parent
  (the usual `1/(n+1)` resize of siblings): the new child keeps its own size.
- Default size of new windows: `create_window` sets `scroll.window_width`
  (clamped to `[MIN_TILING_SIZE, 1.0]`) on tiling windows whose insertion
  parent is the scrolling workspace itself (i.e. which land as strip items).
  Windows inserted into a column get the normal attach sizing along the
  column's axis. A newly inserted strip item that is the workspace's only
  tiling child displays at the full viewport extent (lone item rule, §2.4).

`move_window_to_workspace` (moving windows between workspaces, covered in
§4.5) inserts tiling windows at the strip level after the strip item that
contains the insertion sibling (the first tiling window of the target
workspace), or appends to the end of the strip when it has none.

### 4.4 Resize

`resize_tiling_container` / `TilingSizeGetters::container_to_resize`:

- `container_to_resize` for strip items: `Some(self)` along the scroll axis;
  `None` for cross-axis resize requests (a strip item's cross-axis extent is
  always the full viewport, so a cross-axis resize is a no-op).
- `resize_tiling_container` scrolling branch: adjust **only the target's**
  `tiling_size` (fraction of viewport), clamped to
  `[MIN_TILING_SIZE, 1.0]`; siblings untouched.
- Alt-drag resize (`handle_window_moved_or_resized_end.rs` →
  `set_window_size`): a pixel length is converted to a fraction by dividing
  by the **viewport extent** when the target's parent is a scrolling
  workspace (instead of the parent's length); same local semantics and
  clamp.

### 4.5 Move / reorder

`move_window_in_direction.rs`:

- Along-axis move on a top-level strip item: reorder within the strip via
  `move_container_within_tree` to the sibling strip item's index (positions
  swap; works when either side is a window or a column; sizes preserved).
- At the strip ends and for cross-axis directions: fall through to the
  existing workspace/monitor move targets
  (`move_to_workspace_in_direction`). The existing tail logic that inverts
  the workspace tiling direction at the end of the tree **must not run** for
  a scrolling workspace — guarded; the move falls through to the adjacent
  workspace instead.
- `flatten_child_split_containers` on a scrolling workspace: single-window
  columns are flattened, multi-window columns preserved, and the scroll axis
  is never inverted.
- Moving a window **out of a column** so that it becomes a strip item of a
  scrolling workspace — including moves into a scrolling workspace from
  another workspace, where the preserved fraction is derived from the
  container's *source* chain — **rescales** it so its effective viewport
  fraction is preserved:
  - factor = product of `tiling_size`s from the container's parent up to and
    including its strip item (e.g. column 0.5 × window 0.5 → 0.25); `1.0`
    (no rescale) when the target parent is not a scrolling workspace or the
    container already is a strip item,
  - computed **before** the move but **applied after** detach/attach, clamped
    to `[MIN_TILING_SIZE, 1.0]`: `detach_container` redistributes the freed
    space to the remaining siblings using the moved container's *original*
    size, so pre-scaling would leave a gap (test: remaining column window
    must fill to 1.0 again).
- `move_window_to_workspace` into a scrolling workspace inserts at the strip
  level, directly after the strip item containing the insertion sibling
  (`strip_item_index + 1`), never into a column; size adapts on arrival via
  the same rescale rule.

### 4.6 Layout switching & commands

- `set-layout --layout scrolling|tiling [--workspace <name>]` —
  `set_workspace_layout` (new command). Targets the given workspace or the
  subject container's workspace. Switching to scrolling clamps the tiling
  sizes to `[MIN_TILING_SIZE, 1.0]` (they become viewport fractions), resets
  `scroll_offset` and scrolls the focused descendant into view; switching to
  tiling rescales the sizes proportionally into shares that fill the
  viewport (viewport fractions can sum to more than 1, e.g. two windows at
  `scroll.window_width: 0.7` sum to 1.4, which would leave the second window
  past the right edge of the viewport).
  Emits `WorkspaceUpdated` — the layout is part of the workspace config, so
  the existing event already carries it (no new `WmEvent` variant needed).
  The normalization is shared with config reloads through the layout
  dispatch layer (`layouts::on_layout_changed` /
  `layouts::on_workspace_config_reloaded`): switching a workspace to
  scrolling via its config applies the same clamp/reset/ensure-visible
  steps, and while a workspace is scrolling, a changed `scroll.axis` is
  applied as its runtime axis. The updated config is also kept as a
  session override (`WmState::workspace_config_overrides`): an empty
  workspace that is destroyed and later activated again comes back with
  its overridden layout, and `wm-reload-config` clears the overrides.
- `cycle-layout [--workspace <name>]` — `set_workspace_layout` with the
  next layout in `WorkspaceLayout::ALL` (`tiling → scrolling → tiling`).
  The default keybinding is `alt+shift+v`.
- `scroll --direction <left|right|up|down> [--amount <length>]` —
  `scroll_workspace` (new command). Offsets the subject workspace's
  `scroll_offset` without moving focus; default amount is 10% of the
  viewport extent, `%` amounts are relative to the viewport and px amounts
  are physical pixels. No-op for non-scrolling workspaces and for directions
  perpendicular to the scroll axis. The offset is clamped to the valid
  range; the next focus change re-ensures the focused strip item is visible,
  which may undo scrolling away from it.
- `toggle-tiling-direction` on a window whose direction container is a
  scrolling workspace flips the workspace's scroll axis in place — the
  window is never wrapped in a new split container to hold the inverted
  direction.
- IPC: `WorkspaceLayout` implements `FromStr` so `--layout` parses in clap;
  `InvokeCommand::SetLayout` and `InvokeCommand::Scroll` are dispatched in
  `packages/wm/src/wm.rs` following `SetTilingDirection`. `WorkspaceDto`
  exposes `layout` and `scrollOffset` for UI/scrollbar consumers.

### 4.7 Redraw triggers

Any scroll_offset change implies **all** strip children change position →
queue every top-level child of the workspace for redraw
(`pending_sync.queue_container_to_redraw`). Structural mutations already queue
parents. No changes to the redraw *pipeline*, only to what gets queued.

---

## 5. Platform: off-viewport windows (v1 decision)

Reference implementations studied:

- **LeopardWM** (scroll-first WM, Rust/Windows): layout engine is pure
  (`compute_placements(viewport) -> Vec<WindowPlacement { rect, visibility }>`).
  Production behavior: fully off-viewport windows are **DWM-cloaked and their
  positions clamped just beyond the viewport edge** (defense-in-depth against
  peek-through while cloaking lags); partially visible windows keep their
  natural positions. Region clipping (`SetWindowRgn`) exists only as an
  experimental proof module — explicitly *not* used in production (region
  ownership races with framed windows are a real hazard).
- **Komorebi**: quantized columns model (N visible columns, index-based
  scroll, remainder absorbed by last column); off-viewport windows keep
  natural positions (spillover accepted).

**Chosen for v1** — same as LeopardWM's production approach:

1. Windows keep their true strip rects in the layout engine.
2. A window whose rect does not intersect the viewport is **cloaked** (uses
   the existing `DisplayState` / `set_cloaked` machinery in `platform_sync`),
   and its position is **clamped to just beyond the viewport edge**:
   - off-screen right → `x = viewport.right` (left edge flush with viewport edge)
   - off-screen left → `x = viewport.left − width`
   This means no region APIs are needed, and nothing can peek through gaps.
3. Partially visible windows are positioned naturally; on multi-monitor
   setups their overhang may visually reach an adjacent monitor — accepted in
   v1 (identical to LeopardWM/Komorebi behavior). Region clipping is recorded
   as a possible future enhancement (§7, risk 1).
4. `hide_method: place_in_corner` interaction: an off-viewport window on a
   displayed scrolling workspace parks at the hide corner instead of being
   clamped (it is invisible either way); it is repositioned naturally when
   scrolled into view.
5. `focus_follows_cursor` (`handle_mouse_move.rs`): hit-test must use the
   viewport-clipped rect so cursors over a clamped/invisible overhang do not
   focus hidden windows.
6. `jump_cursor`: no code change needed. Ensure-visible (§4.2) runs earlier in
   `platform_sync` and scrolls the focused window fully into the viewport
   before the cursor jump is processed, so the focused rect's center is
   on-screen whenever the jump happens.
7. Hiding a scrolled-out window must go through a `Hiding` display-state
   transition (`transition_display_state` in `platform_sync.rs`), exactly like
   windows of a non-displayed workspace. Both hide methods emit an OS event
   (`EVENT_OBJECT_HIDE` / `EVENT_OBJECT_CLOAKED` → `WindowEvent::Hidden`); if
   the window's state were still `Shown`, `handle_window_hidden` would treat
   the hide as app-initiated and **unmanage** the window — collapsing the
   strip and making scrolling appear broken. Regression tests:
   `platform_sync::tests::transitions_to_hiding_when_scrolled_out` and
   `platform_sync::tests::keeps_hidden_windows_hidden_while_scrolled_out`.

Platform trait changes needed for v1: none beyond reusing cloak/hide/show —
`layouts::window_placement` computes the clamped rect and visibility
(`layouts::WindowPlacement`); `reposition_window` applies both (the clamped
rect via the same `SetWindowPos`/`set_frame` call a normal sync would use).


---

## 6. File-by-file change map

Ordered by dependency. P0 = scaffolding/docs, each Pn is independently
reviewable.

### P1 — config & model plumbing

| File | Change |
| --- | --- |
| `packages/wm-common/src/parsed_config.rs` | `WorkspaceLayout` enum, `ScrollConfig`, fields on `WorkspaceConfig` (+ defaults, serde) |
| `resources/assets/sample-config.yaml` | commented example block |
| `packages/wm/src/models/workspace.rs` | `layout` + `scroll_offset` fields, getters/setters, `is_scrolling()`, `viewport_rect()`, `strip_extent()` |
| `packages/wm/src/commands/workspace/update_workspace_config.rs`, `packages/wm/src/commands/general/reload_config.rs` | apply `layout`/`scroll` config on workspace creation & reload; reload normalizes layout switches via `normalize_for_scrolling` (§4.6) |
| tests | config parse + workspace construction |

### P2 — core geometry

| File | Change |
| --- | --- |
| `packages/wm/src/traits/position_getters.rs` | scrolling branch in `impl_position_getters_as_resizable!` (§2.4) |
| `packages/wm/src/commands/workspace/update_scroll_position.rs` (new) | ensure-visible + clamp helpers |
| `packages/wm/src/commands/general/platform_sync.rs` | invoke ensure-visible/clamp in the sync pipeline |
| tests | rect math vs. offset, clamp, ensure-visible align modes |

### P3 — navigation & sync

| File | Change |
| --- | --- |
| `packages/wm/src/commands/container/focus_in_direction.rs` | **no code change** — existing axis/sibling logic navigates strips (§4.1); tests added |
| `packages/wm/src/commands/workspace/update_scroll_position.rs` | clamp all displayed scrolling workspaces; ensure-visible the focused one |
| `packages/wm/src/commands/general/platform_sync.rs` | `ScrollingPlacement` clamp + cloak of off-viewport windows (§5) |
| `packages/wm/src/events/handle_mouse_move.rs` | hit-test against viewport-clipped rect |
| tests | focus at strip edges, cross-axis, focus into/out of columns; placement clamp math |

### P4 — mutation semantics

| File | Change |
| --- | --- |
| `packages/wm/src/commands/container/attach_container.rs` | skip sibling redistribution when the parent is a scrolling workspace |
| `packages/wm/src/commands/container/detach_container.rs` | skip sibling redistribution / size normalization when the parent is a scrolling workspace |
| `packages/wm/src/commands/window/manage_window.rs` | `insert_position` handling (`focused_strip_item_index`); default `scroll.window_width` sizing in `create_window` |
| `packages/wm/src/traits/tiling_size_getters.rs` | `container_to_resize`: `Some(self)` along the scroll axis, `None` cross-axis, for strip items |
| `packages/wm/src/commands/container/resize_tiling_container.rs` | local-only resize of strip items, clamped to `[MIN_TILING_SIZE, 1.0]` |
| `packages/wm/src/commands/window/set_window_size.rs` | drag-resize converts px → viewport fraction for strip items |
| `packages/wm/src/commands/window/move_window_in_direction.rs` | strip reorder branch; cross-axis/strip-end fall-through; guard workspace direction inversion |
| `packages/wm/src/commands/container/move_container_within_tree.rs` | rescale on strip-boundary crossings (compute before, apply after) |
| `packages/wm/src/commands/container/toggle_tiling_direction.rs` | axis toggle in scrolling mode (no split wrapping) |
| `packages/wm/src/commands/container/flatten_child_split_containers.rs` | preserve columns, never invert the scroll axis |
| `packages/wm/src/commands/window/move_window_to_workspace.rs` | strip-level insert into scrolling targets |
| `packages/wm/src/models/workspace.rs` | `strip_item_index` helper |
| tests | attach/detach/resize/reorder/cross-axis/workspace-move rescale invariants (21 new tests) |

### P5 — commands & IPC polish

| File | Change |
| --- | --- |
| `packages/wm-common/src/app_command.rs` | `SetLayout`, `Scroll` variants; parsing tests for both |
| `packages/wm-common/src/parsed_config.rs` | `FromStr` for `WorkspaceLayout` (clap parsing of `--layout`) |
| `packages/wm-common/src/dtos/workspace_dto.rs` | expose `layout`, `scroll_offset` (Zebar/UI can render a scrollbar) |
| `packages/wm/src/commands/workspace/set_workspace_layout.rs` (new) | runtime layout switch (clamp sizes, reset/ensure offset, `WorkspaceUpdated`); `normalize_for_scrolling` is shared with config reloads |
| `packages/wm/src/commands/workspace/scroll_workspace.rs` (new) | offset-only scroll command (axis check, clamp, redraw) |
| `packages/wm/src/models/workspace.rs` | DTO fields in `to_dto` |
| `packages/wm/src/wm.rs` | dispatch for `SetLayout`/`Scroll` |
| `resources/assets/sample-config.yaml` | runtime command examples in the scrolling block |
| tests | layout switch (clamp / offset reset + ensure visible / keep sizes / no-op), scroll command (amounts, clamping, no-ops) |

### P6 — deferred (not in v1)

- Region clipping via `SetWindowRgn` — see §5; revisit only if edge spillover
  on multi-monitor setups proves annoying. LeopardWM's
  `platform_win32/src/clipping_proof.rs` is the reference if ever needed.

### P7 — layout extraction (pluggable layouts)

Moved layout behavior out of the command/model call sites so that layouts
are pluggable (see `docs/layouts.md` for the architecture).

| File | Change |
| --- | --- |
| `packages/wm/src/layouts/mod.rs` (new) | Dispatch functions for every layout-specific question; `WindowPlacement`, `DirectChildResize`; moved resize tests |
| `packages/wm/src/layouts/scrolling.rs` (new) | All strip math/semantics (rects, insert, move, resize, scroll, placement, normalize) |
| `packages/wm/src/layouts/tiling.rs` (new) | Default tiling behaviors, extracted from command implementations |
| `packages/wm/src/traits/position_getters.rs` | Macro delegates rect math to `layouts::child_rect` |
| `packages/wm/src/traits/tiling_size_getters.rs` | Direct-child resize via `layouts::direct_child_resize` |
| `packages/wm/src/models/workspace.rs` | Strip math moved out; keeps `scroll_offset` state and `layout()`/`is_scrolling()` accessors |
| `packages/wm/src/commands/container/resize_tiling_container.rs` | Deleted; dispatched via `layouts::resize_tiling_container` |
| `packages/wm/src/commands/{container,window,workspace,general}/*`, `packages/wm/src/events/handle_mouse_move.rs` | Call sites rewired to `layouts::*`; no match on `WorkspaceLayout` outside `layouts` |
| `packages/wm-common/src/parsed_config.rs` | `WorkspaceLayout::ALL` + `next()` (cycle order, kept in sync by test) |
| `packages/wm-common/src/app_command.rs` | `CycleLayout` variant + parsing test |
| `packages/wm/src/wm.rs` | Dispatch for `CycleLayout` |
| `packages/wm/src/commands/workspace/set_workspace_layout.rs` | Uses `layouts::on_layout_changed`; `normalize_for_scrolling` moved to `layouts::scrolling::normalize` |
| `packages/wm/src/commands/general/reload_config.rs` | Uses `layouts::on_workspace_config_reloaded` |
| `resources/assets/sample-config.yaml` | `cycle-layout` keybinding (`alt+shift+v`) |
| tests | Moved into the layout modules (strip resize/clamp, placement clamping); dispatch-level test for non-scrolling placement |

---

## 7. Risks / open questions

1. **Spillover on adjacent monitors**: partially visible windows at the
   viewport edge may overhang onto an adjacent monitor (same as LeopardWM /
   Komorebi). Mitigation if it annoys: future `SetWindowRgn` clip region
   (LeopardWM proved viability but hit region-ownership races; treat as a
   separate project), or per-monitor outer gaps on adjacent edges.
2. **Scroll-step performance**: each scroll offset change repositions all
   strip children. Fine for typical counts (<20/workspace); measure.
3. **`hide_method: place_in_corner`**: parked windows + scrolling recalculate
   frames — must not park partially visible windows.
4. **`tiling_size` semantic overload** (§2.4): **resolved** — switching
   to scrolling clamps sizes into `[MIN_TILING_SIZE, 1.0]`; switching to
   tiling rescales them into shares that fill the viewport. Runtime
   switches are kept as session overrides, so an empty workspace that is
   destroyed and re-activated comes back with the switched layout.
5. **DPI / monitor move**: `scroll_offset` is physical px; recompute/clamp on
   `handle_display_settings_changed`.
6. **Upstream divergence**: keep changes additive & behind config so the
   branch can rebase on upstream cleanly; upstream interest in scrolling
   layout exists (accordion experiments) — design for eventual PR.
7. **Columns (stacking)**: needs `move --direction up/down` column semantics
   (create/dissolve nested splits). Deferred; base strip ships first, and the
   data model already fits (nested `SplitContainer` as column).

