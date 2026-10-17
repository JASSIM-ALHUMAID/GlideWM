# Scrolling layout (Niri-style) — design & implementation plan

Status: **in progress - P1 (config/model) and P2 (core geometry) implemented** · Branch: `feat/scrolling-layout` · Fork: `JASSIM-ALHUMAID/glazewm` (upstream: `glzr-io/glazewm`)

This document organizes the work for adding a Niri/Hyprscroller-style **scrolling
layout** to GlazeWM. It is the working plan for this branch: model, config
schema, runtime semantics, platform concerns, and a file-by-file change map.

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
reinterprets existing values; clamp/normalize on switch (see §4.5).

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
  direction logic first before crossing columns (phase 2, if kept).
- After focus lands: ensure-visible adjusts `scroll_offset`.

Implementation: branching in
`packages/wm/src/commands/container/focus_in_direction.rs`
(`tiling_focus_target`) when the nearest workspace is `Layout::Scrolling`.

### 4.2 Ensure-visible (scroll on focus)

New helper module, e.g. `packages/wm/src/commands/workspace/update_scroll_position.rs`:

- `ensure_descendant_visible(workspace, container)`:
  - fully visible → no-op
  - `align: minimal` → scroll the least amount so the container is fully visible
  - `align: center` → center it
- `clamp_scroll_position(workspace)`: enforce §2.3 invariants.
- Wired into the sync pass (see `pending_sync.rs`): ensure-visible when
  `needs_focus_update()`, clamp when containers were redrawn. Called from
  `wm.rs` before `platform_sync` (keeps command handlers layout-agnostic).

### 4.3 Insert

`attach_container` + `insertion_target` (in `manage_window.rs`):

- Target parent: the scrolling workspace (flat strip) — or the focused
  column for phase-2 bundling.
- `after_focused`: insert at `sibling.index() + 1` (existing default behavior).
- `end`: append to workspace.
- **Skip sibling redistribution** (existing code resizes siblings to
  `1/(n+1)`): new child gets `window_width` from config, siblings untouched.

### 4.4 Resize

`resize_tiling_container` / `TilingSizeGetters::container_to_resize`:

- Scrolling branch: adjust **only the target's** `tiling_size` (fraction of
  viewport), clamped to `[MIN_TILING_SIZE, 1.0]`; siblings untouched.
- `handle_window_moved_or_resized_end.rs` (alt-drag resize): compute new
  fraction from dragged size ÷ viewport extent; same local semantics.

### 4.5 Move / reorder

`move_window_in_direction.rs`:

- Scrolling branch: `left`/`right` reorder within the strip (swap sibling
  positions; no size redistribution, no split wrapping).
- Do **not** invert workspace tiling direction (existing tail logic must be
  guarded).
- Moving between workspaces (`move_window_to_workspace`) keeps working; size
  adapts on arrival (default fraction or preserved fraction — start with
  preserved).

### 4.6 Layout switching & commands

- `set-layout --layout scrolling|tiling [--workspace <name>]` — runtime switch,
  emits `WmEvent`; on switch to scrolling: normalize sizes (clamp fraction)
  and reset `scroll_offset` to ensure focused window visible.
- `scroll --direction left|right [--amount 10%]` — offset-only scroll (phase 5
  nicety; not required for MVP).
- `toggle-tiling-direction` on a scrolling workspace toggles the axis.
- Registration: `wm-common/src/app_command.rs` (`InvokeCommand`), dispatch in
  `packages/wm/src/wm.rs`, following `SetTilingDirection` exactly.

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
6. `jump_cursor`: clamp the target point into the viewport (rect centers of
   off-viewport windows are off-screen).

Platform trait changes needed for v1: none beyond reusing cloak/hide/show —
`reposition_window` computes the clamped position and visibility; the clamped
position is applied in the same `SetWindowPos` call.


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
| `packages/wm/src/commands/workspace/update_workspace_config.rs`, `packages/wm/src/commands/general/reload_config.rs` | apply `layout`/`scroll` config on workspace creation & reload |
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
| `packages/wm/src/commands/container/focus_in_direction.rs` | order-based navigation branch |
| `packages/wm/src/commands/general/platform_sync.rs` | clamp + cloak off-viewport windows (§5); cursor-jump clamp |
| `packages/wm/src/events/handle_mouse_move.rs` | hit-test against viewport-clipped rect |
| tests | focus order at strip edges, focus into/out of columns |

### P4 — mutation semantics

| File | Change |
| --- | --- |
| `packages/wm/src/commands/container/attach_container.rs` | no redistribution in scrolling parent; default fraction sizing |
| `packages/wm/src/commands/window/manage_window.rs` | `insert_position` handling |
| `packages/wm/src/traits/tiling_size_getters.rs` | `container_to_resize` scrolling branch |
| `packages/wm/src/commands/container/resize_tiling_container.rs` | local-only resize |
| `packages/wm/src/commands/window/move_window_in_direction.rs` | reorder semantics; guard direction inversion |
| `packages/wm/src/commands/container/toggle_tiling_direction.rs` | axis toggle in scrolling mode |
| `packages/wm/src/events/handle_window_moved_or_resized_end.rs` | drag-resize → local fraction |
| tests | attach/resize/move/close invariants |

### P5 — commands & IPC polish

| File | Change |
| --- | --- |
| `packages/wm-common/src/app_command.rs` | `SetLayout`, `Scroll` commands |
| `packages/wm/src/wm.rs` | dispatch |
| `packages/wm-common/src/dtos/workspace_dto.rs` | expose `layout`, `scroll_offset` (Zebar/UI can render a scrollbar) |
| `packages/wm-common/src/wm_event.rs` | `WmEvent` for layout change (if needed for UI) |

### P6 — deferred (not in v1)

- Region clipping via `SetWindowRgn` — see §5; revisit only if edge spillover
  on multi-monitor setups proves annoying. LeopardWM's
  `platform_win32/src/clipping_proof.rs` is the reference if ever needed.

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
4. **`tiling_size` semantic overload** (§2.4): switching layouts mid-session
   reinterprets sizes; normalize deterministically (clamp to `[0.05, 1.0]`,
   keep fractions) — document.
5. **DPI / monitor move**: `scroll_offset` is physical px; recompute/clamp on
   `handle_display_settings_changed`.
6. **Upstream divergence**: keep changes additive & behind config so the
   branch can rebase on upstream cleanly; upstream interest in scrolling
   layout exists (accordion experiments) — design for eventual PR.
7. **Columns (stacking)**: needs `move --direction up/down` column semantics
   (create/dissolve nested splits). Deferred; base strip ships first, and the
   data model already fits (nested `SplitContainer` as column).
```

