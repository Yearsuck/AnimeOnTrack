# Stats: circles ↔ bars toggle with a fluid morph

**Date:** 2026-07-12
**Branch to implement on:** `feat/stats-rings-bars-toggle`
**Type:** UI / dataviz + animation (frontend-only). Theme-aware.
**Status:** approved (autonomous batch)

## Problem

The "Barras" tab of Estadísticas actually renders **rings** (`StatsRings.tsx`). The user
wants a toggle to switch between circles and bars, with a very fluid, dynamic morph between the
two forms.

## What exists

- `StatsRings.tsx`: per-category SVG rings (arc via `strokeDasharray`) in a wrapped flex grid,
  for genres and types. Color per category via `categoryColor(name)` (existing hash palette —
  keep it). Value shown in the ring center.
- Data: `genres: {genre,count}[]`, `types: {kind,count}[]` (backend-sorted desc).

## Design

### Unified row layout (enables the morph)

Morphing across two different layouts (wrapped ring grid ↔ bar list) is intractable. Use ONE
layout for both modes: a **vertical list of full-width rows**, one category per row. Each row:
`[ mark SVG ][ label + value ]`. Rings mode draws a small ring as the mark; bars mode draws a
horizontal bar. Rows stay put across the toggle, so only the mark morphs — clean and fluid.
(Tradeoff: rings move from a grid to a list. Documented; the fluid morph is the priority the
user set.)

### The morph — "unroll" a stroked arc into a bar

Per category, fraction `f = count / max`. Draw the mark as **two stroked SVG paths** (a
recessive full-length track + the colored value path), rounded line caps (→ 4px rounded
data-ends, per dataviz marks spec). A single animation value `m ∈ [0,1]` (0 = ring, 1 = bar),
eased, drives an **unroll**:
- Sample `K` points along the value path by `s ∈ [0, f]` (track uses `s ∈ [0,1]`).
- Ring position of a sample: on a circle radius `R`, angle `-90° + s·360°`, centered in the
  mark box.
- Bar position of a sample: on a horizontal line, `x = x0 + s·W`, `y = yc`.
- Point = `lerp(ringPoint, barPoint, ease(m))`. The arc length is preserved (`W = 2πR·`… use a
  fixed `R` and `W = 2πR` so track arc-length == bar width), so the arc visibly straightens into
  the bar and back. Colored path length scales with `f` in both forms (partial arc ↔ partial
  bar length).
- Drive `m` with `requestAnimationFrame` over ~450–550ms, `ease = easeInOutCubic`. Respect
  `prefers-reduced-motion`: skip the tween, snap `m` to the target.

Value label sits centered in the ring at `m=0` and to the right of the bar at `m=1` — lerp its
position/opacity by `m` too, or simply crossfade (keep it readable throughout). Text uses text
tokens (`--text`/`--muted`), never the category color (dataviz non-negotiable). The mark carries
the color.

### Toggle control

A segmented control (reuse `.seg`/`.seg-btn`) above the genre/type blocks: "Círculos | Barras".
Persist the choice in `localStorage` (`aot.statsShape`, default `"rings"`) so it survives
relaunch, mirroring the i18n/theme pattern. New i18n keys `stats.shapeRings` / `stats.shapeBars`
in both catalogs.

### Correctness / dataviz

- Bars anchored to a common baseline; length encodes magnitude; sorted desc (data already is).
- 2px surface gap between adjacent bars (row gap).
- Recessive track (`--surface-3`), value in category color, count label in text ink.
- One shared `max` per block (genres vs types) so lengths compare within a block.

## Acceptance criteria (verifiable without live UI)

1. `npx tsc --noEmit` clean; `npm run build` OK.
2. `cargo test` still green (no backend change; run anyway).
3. New i18n keys in BOTH catalogs.
4. `prefers-reduced-motion` path snaps without a tween (guard present in code).
5. No hardcoded hex/rgba added (tokens + `categoryColor` only).

## Verify live (NOT tool-reachable — state honestly)

Relaunch, Estadísticas → Barras tab, toggle Círculos/Barras, confirm the morph is smooth and the
choice persists. Cannot be screenshot-verified here. (Optional: the implementer MAY build a tiny
standalone HTML harness of the morph math and open it in Chrome to eyeball the tween, but the
React component itself renders only in the Tauri window.)

## Out of scope

Task 10 (3D graph). Replacing `categoryColor`. Changing the genre/type data or backend.
