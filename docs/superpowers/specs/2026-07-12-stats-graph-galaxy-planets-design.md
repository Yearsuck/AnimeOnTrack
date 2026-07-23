# Stats 3D graph: spacing, galaxy background, planet nodes

**Date:** 2026-07-12
**Branch to implement on:** `feat/stats-graph-galaxy`
**Type:** UI / 3D visual (frontend-only). Must preserve the frozen-layout perf design.
**Status:** approved (autonomous batch)

## Problem

The 3D relationship graph (`StatsGraph.tsx`, react-force-graph-3d + three.js) packs nodes too
tightly — connections aren't readable. The user also wants a galaxy-style background and nodes
that look like planets.

## What exists (verified)

- Force constants: `CHARGE_STRENGTH = -260`, `LINK_DISTANCE = 90`; hub radii 6–22; series drawn
  as cover-image sprites; `backgroundColor="#00000000"` (transparent) over a `.series-block`.
- **Perf design (do NOT break):** layout is bounded (`COOLDOWN_TICKS=200`), positions snapshotted
  on `onEngineStop` into `positionsRef`, and `d3ReheatSimulation` runs **only** when the node/link
  set changes structurally (`structuralKey`). First open ~1.4s, later returns ~18ms. See
  `docs/.../2026-07-10-stats-graph-cache-design.md` and memory `project-2026-07-batch`.
- Hub spheres use `MeshBasicMaterial` (unlit) deliberately — the scene has no light; a lit
  material would render black. Colors from `categoryColor` (6-digit hex) + `ROOT_COLOR`.

## Design

### 1. Spacing (readable connections)

Increase repulsion and link length so edges are visible: `CHARGE_STRENGTH` → about **-520**,
`LINK_DISTANCE` → about **150** (tune within these bounds). Because the wider layout can push
nodes out of frame, call `graphRef.current.zoomToFit(400, 60)` **once**, right after the first
`onEngineStop`, guarded by a `didFitRef` so it never fights the frozen layout on later rebuilds.
No other change to the reheat/snapshot logic — the constants are applied in the same
`structuralKey` effect that already sets `charge`/`link`.

### 2. Galaxy background (zero graph-perf cost)

Keep `backgroundColor` transparent. Add a `stats-graph-galaxy` class to the graph container div
(the `ref={containerRef}` div) and style it in `styles.css`: a dark radial nebula
(`radial-gradient` layers in deep indigo/violet/near-black) plus a starfield made of many small
`radial-gradient` dots (or a single inlined data-URI). The panel reads as deep space in **both**
themes (galaxy is inherently dark — intentional and documented; do not lighten it in light
theme). Pure CSS behind the transparent canvas — no three.js scene change, no physics impact.

### 3. Planet nodes

Give hub spheres (root/genreHub/kindHub) a lit-planet look **without adding a scene light**
(keep `MeshBasicMaterial`): build a `CanvasTexture` per node via a helper `planetTexture(hex)` —
a radial gradient with the highlight offset toward the upper-left (lighten the base ~45% at the
light side, base in the middle, darken ~45% at the rim), mapped onto the existing
`SphereGeometry`. Add a `shadeHex(hex, amt)` helper (clamp per-channel). Cache textures the same
way node objects are already cached/disposed (`nodeObjectsRef` + `disposeNodeObject` already
disposes `material.map` — verify the map is disposed). Series nodes keep their cover-image
sprites (already planet-ish disks); optionally give the poster-less fallback the same planet
gradient for consistency.

Keep sphere sizes/`hubRadius` as-is (or nudge series sprite size up slightly for balance).

## Acceptance criteria (verifiable without live UI)

1. `npx tsc --noEmit` clean; `npm run build` OK.
2. `cargo test` still green (no backend change; run anyway).
3. The reheat guard is unchanged: `d3ReheatSimulation` still only fires when
   `layoutKeyRef.current !== structuralKey` — the frozen-layout invariant is intact (diff shows
   no change to that condition). `zoomToFit` is one-shot (guarded by a ref).
4. `disposeNodeObject` still disposes each node's `material.map` (planet textures don't leak).
5. No new npm dependency; no scene light added.

## Verify live (NOT tool-reachable — state honestly)

Relaunch, Estadísticas → Grafo, confirm nodes are spread enough to read edges, the background
looks like a galaxy, and hubs look like planets. Frozen-layout perf (instant return when
switching tabs) unchanged. Cannot be screenshot-verified here.

## Out of scope

Backend/graph data. Replacing react-force-graph-3d. Real lighting/shaders. Task 8/9.
