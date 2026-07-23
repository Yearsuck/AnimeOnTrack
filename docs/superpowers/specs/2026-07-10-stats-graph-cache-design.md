# Stats 3D graph: keep it mounted, freeze the layout

## Problem

`App.tsx` renders every tab conditionally (`{view === "stats" && <Stats />}`). Leaving the Estadísticas tab unmounts `Stats` → `StatsGraph` → `ForceGraph3D`, destroying the three.js renderer, the WebGL context, every `THREE.Sprite`/`CanvasTexture`, and the d3-force simulation state. Coming back rebuilds all of it and re-runs the force layout from random initial positions — slow, and the graph visibly re-scrambles into a different shape every single time.

## Technical context

- `StatsGraph.tsx` builds `{nodes, links}` with `useMemo(..., [seriesList])`, mounts `<ForceGraph3D>`, and creates one `THREE.Sprite` (cover texture) or `THREE.Mesh` sphere per node inside `nodeThreeObject`.
- A one-shot mount effect measures `containerRef` and drives `width`/`height`. Its comment already documents a real trap: the ref-bearing `<div>` must mount even when `seriesList` is empty, or the ref is null on the first pass and never re-attaches.
- `Stats.tsx` owns the `Grafo | Barras` toggle and loads `getStatsGraph()` / `getGenreStats()` / `getTypeStats()`.
- `d3ReheatSimulation()` is called whenever `nodes`/`links` change (currently: on every mount, because the memo is fresh).

## Design

Three independent changes, in increasing order of risk. Do them in order and verify each.

### 1. Don't unmount the tab (the actual fix)

In `App.tsx`, keep `<Stats />` mounted once it has been visited and hide it with CSS instead of unmounting:

```tsx
{visited.has("stats") && (
  <div hidden={view !== "stats"}>
    <Stats />
  </div>
)}
```

- Track `visited` in a `useState<Set<View>>` (or simply mount `Stats` from the first time `view === "stats"` onward). Do **not** mount it eagerly at app start — that would pay the three.js cost on launch for users who never open the tab.
- Use the `hidden` attribute (or `style={{ display: "none" }}`), **not** `visibility`/`opacity`: a `display:none` container gives `clientWidth === 0`, which matters — see the sizing trap below.
- Apply this pattern **only** to `stats`. The other tabs are cheap to remount and keeping them mounted would multiply their `useEffect` data-loading.

**The sizing trap.** While hidden with `display:none`, `ResizeObserver` fires with a zero box and `getBoundingClientRect().top` is 0. The existing effect would then push `width: 0` / a bogus height into the graph, and the canvas comes back blank on re-show. Guard it: ignore any recompute where `el.clientWidth === 0` (keep the last good size), and force one recompute when the tab becomes visible again (a `useEffect` on an `active: boolean` prop passed down from `App.tsx` → `Stats` → `StatsGraph`).

### 2. Freeze the simulation after the first layout

Pass `cooldownTicks` (e.g. `200`) and an `onEngineStop` handler to `ForceGraph3D`. Once the layout settles, write each node's `{x, y, z}` back into a `useRef` snapshot keyed by node id. On subsequent graph-data rebuilds, seed matching nodes with `fx`/`fy`/`fz` (or just `x`/`y`/`z` as initial positions) from that snapshot, so the graph re-renders in the shape the user last saw instead of re-scrambling.

Remove the unconditional `d3ReheatSimulation()` on `[nodes, links]` — reheat **only** when the node/link set actually changed, not on every mount. Compare by a cheap structural key (sorted node ids + link count), not by object identity: `useMemo` returns a new array object on every mount even when the data is identical.

### 3. Invalidate only when the data really changes

`Stats.tsx` refetches `getStatsGraph()` on mount. With the component now permanently mounted, it will not refetch on tab switches at all — which is correct, but it also means following a new series or backfilling genres leaves the graph stale.

Refetch when, and only when, one of these happens:
- the `refresh-progress` / refresh cycle completes (the app already emits Tauri events for this — reuse the existing listener pattern from `ProgressBar.tsx`, don't add a new event);
- the user presses "Rellenar géneros ahora" (already triggers a reload in `Stats.tsx`);
- `active` transitions false→true **and** a `dirty` flag was set by either of the above while hidden.

Keep it simple: a `dirty` ref plus a reload on becoming active is enough. Do not add polling.

### Disposal correctness

Keeping the graph mounted forever means textures and geometries are never garbage-collected by the unmount path they currently rely on. That is the point (reuse), but on `seriesList` change the old `nodeThreeObject` results are dropped without `.dispose()`. That leak exists today too, masked by unmounting. Since node objects will now be rebuilt in a long-lived scene, add explicit disposal: keep a `Map<nodeId, THREE.Object3D>` and `dispose()` the material/texture/geometry of any node object replaced or removed. Verify with `renderer.info.memory` (log `geometries`/`textures` counts before and after a data change; they must not grow unboundedly across several refreshes).

## Acceptance criteria (verifiable)

1. `npx tsc --noEmit`, `npm run build` pass. `cargo test` unaffected (no backend change) but must still pass.
2. Live: open Estadísticas (graph builds, note the wall time), switch to another tab, come back → the graph appears **immediately** (no rebuild, no re-layout) and in **the same orientation/shape** the user left it in. Measure both: first-open ms vs return-to-tab ms, report the numbers.
3. Live: hidden→visible does not produce a blank/zero-sized canvas (the sizing trap), including when the window was resized while the tab was hidden.
4. Live: follow a new series, run a refresh, return to Estadísticas → the new node is present (staleness handled).
5. `renderer.info.memory.textures` does not grow across 3 consecutive data reloads (log the three values).

## Live verification required

- Screenshot of the graph, then a screenshot after switching away and back, showing the same camera/layout.
- The two timing numbers (first open vs return) and the three texture-count values, from a real run.
- Confirmation that resizing the window while Estadísticas is hidden does not break the canvas on return.

## Explicitly out of scope

- Persisting node positions across app restarts (in-memory snapshot only).
- Replacing `react-force-graph-3d` or the rendering approach.
- Keeping any other tab mounted.
- The `Barras`/`Anillos` view (unchanged; it is cheap SVG).
