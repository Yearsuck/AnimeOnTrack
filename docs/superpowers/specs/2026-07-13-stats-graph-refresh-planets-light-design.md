# Stats 3D graph — refresh on new series + real planet nodes + light-mode variant

**Date:** 2026-07-13
**Branch to implement on:** `feat/stats-graph-refresh-planets-light`
**Type:** BUG (refresh) + visual redesign (planets) + theme. Frontend (three.js) + CSS.
**Status:** approved (autonomous batch)
**Depends on:** T1 (stats reload-on-activation feeds fresh `seriesList`).

## Problems

(a) The 3D graph doesn't update when you follow new series (new nodes don't show up).
(b) The node texture (`planetTexture`, a flat radial gradient) is ugly and doesn't read as a planet.
(c) The galaxy backdrop and nodes are identical in light and dark theme; they should have a legible
    light-theme variant.

## What exists (verified)

- `StatsGraph.tsx`: `seriesList` prop comes from `Stats.tsx` `getStatsGraph()` (followed series,
  `db.get_stats_graph_data`, `commands.rs:623`). `buildGraphData` → root/genreHub/kindHub/series
  nodes. Layout is **frozen**: `positionsRef` snapshots settled coords on `handleEngineStop`;
  reheat only fires when `structuralKey` (sorted node ids + link count) changes (L276-284).
- `didFitRef` gates `zoomToFit` to fire **exactly once** on the first settle (L194, L309-312) — so
  after the initial framing, newly-added nodes are never re-framed.
- Node visuals: hubs = `THREE.Mesh(SphereGeometry, MeshBasicMaterial{ map: planetTexture(color) })`
  (unlit — the scene ships no light, L330-339); series = sprites of the cover (or a flat color
  disc). `planetTexture` (L145-160) = one radial gradient (light spot upper-left, dark limb).
- `.stats-graph-galaxy` (`styles.css:1199`) is a hard-coded dark nebula+stars; `backgroundColor`
  of the canvas is transparent so this CSS shows through. No `data-theme` variant.
- Perf contract to preserve: first open ~1.4s, return ~18ms (frozen layout). Reheat only on
  structural change.

## Root cause (a) — evidence

Two compounding causes:
1. **Stale feed**: pre-T1, `Stats.load()` only re-ran on airing refresh, so following a series never
   updated `seriesList`. **T1 fixes this** (reload on tab-activation) — the graph now receives the
   new node. This spec assumes T1 is merged; verify the graph updates after a follow once T1 lands.
2. **Off-camera new nodes**: even with fresh data, `structuralKey` changes → reheat runs and lays
   the new node out, but `didFitRef` is already `true`, so the camera never re-frames. A new node
   that settles outside the current view is invisible to the user → "the graph didn't update".
   Additionally, brand-new nodes have no `positionsRef` entry and start near the origin (undefined
   coords), so they can pile at the center behind existing nodes before/if the reheat separates them.

## Design

### Fix (a) — re-frame + seed new nodes

- Track the previous node-id set (`prevIdsRef`). In the reheat effect (or `handleEngineStop`), when
  brand-new ids appeared since the last settle, **re-run `zoomToFit(600, 60)` once after this
  settle** (not every settle — only when the structure grew), so new nodes are brought into view.
  Keep the very first fit as-is. Do NOT re-fit on pure removals or on identical structure (would
  fight the user's camera).
- Seed a new node's initial position near the `root` (or its first hub's saved position) before the
  reheat, so it doesn't spawn at the world origin: in the `useMemo` that applies saved positions, for
  a node with no saved position, initialize `x/y/z` to a small random offset around the root's saved
  coords (root is always present). This keeps the reheat stable and avoids a center pile-up.
- Confirm `structuralKey` includes additions (it does: sorted ids). Add a brief note/test-by-hand:
  following a series while Stats is open (or opening Stats after following) shows the new sprite.

### Fix (b) — real planet texture

Replace `planetTexture` with a baked canvas texture that actually reads as a planet on the unlit
sphere. Build at ≥256px. Combine:
- A base sphere-shaded gradient (keep a soft light side / dark limb — the terminator).
- **Latitude bands** and/or low-frequency value noise (a few octaves of random-ish blotches or
  sinusoidal bands in a slightly shifted hue of `color`) so the surface has texture, not a flat
  wash.
- A subtle darkened rim (limb darkening) and optional faint atmospheric glow ring drawn just inside
  the disc edge.
Keep it a single `CanvasTexture` on `MeshBasicMaterial` (no scene light added — that constraint is
load-bearing). Reuse `shadeHex`. Must dispose cleanly (existing `disposeNodeObject` disposes
`.map`). **Validate visually in a three.js Chrome harness** (mount `ForceGraph3D` or a bare sphere
with the texture, serve via `python -m http.server --bind 127.0.0.1`, screenshot) and iterate the
look BEFORE finalizing.

### Fix (c) — light-theme variant

- Add a `:root[data-theme="light"] .stats-graph-galaxy { … }` block: a light sky backdrop (pale
  blue/lavender gradient) with faint darker "stars"/specks so nodes stay legible on light bg. Keep
  the dark block as the default / `[data-theme="dark"]`.
- Node contrast in light mode: the canvas `backgroundColor` stays transparent. The planet texture's
  dark limb keeps hubs readable on a light backdrop, but verify series **sprites** (cover images /
  flat color discs) and the root (`ROOT_COLOR = #e9ecef`, nearly white — invisible on light bg)
  read well. Make `ROOT_COLOR` and the flat-disc fallback theme-aware: read
  `document.documentElement.getAttribute("data-theme")` (or a small `useTheme`-style read) and pick
  a darker root/border on light. Re-generate affected node objects when the theme changes (bump a
  dependency so `nodeThreeObject` re-runs, disposing the old objects).
- Validate both themes in the harness.

## Acceptance criteria

- After T1 is merged: following a new series and opening Estadísticas → the new node appears AND the
  camera re-frames so it's visible.
- Planet nodes visibly look like planets (bands/noise/terminator/limb), validated by a harness
  screenshot attached in the agent's summary.
- Light theme shows a light backdrop with legible nodes; dark theme unchanged. Switching theme
  repaints the graph without a reload.
- Perf preserved: no reheat on identical structure; return-to-tab stays fast (frozen layout intact
  except the one extra conditional zoomToFit on growth).
- `npx tsc --noEmit`; `npm run build` clean. (No backend change; `cargo test` unaffected.)

## Live verification (user)

Relaunch, follow something, open Estadísticas (graph updates + framed); toggle theme (light
backdrop + legible planets). Tauri window not tool-reachable — the three.js harness in Chrome is the
design proof.
