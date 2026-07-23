# Stats: 3D relationship graph + polished ring gauges

Follow-up to `2026-07-07-genre-stats-design.md` (which shipped the current Estadísticas tab: summary tiles + flat Recharts horizontal bars for genre/type). The user found the bars visually flat and wants something more striking — specifically referencing 3D force-directed network graphs (nodes/edges, like dependency or social-graph visualizations), not a generic 3D bar chart.

## What this replaces

The existing Recharts genre/type bar `RankingChart` component in `src/views/Stats.tsx` is replaced by two views behind a toggle. `recharts` becomes unused and is removed from `package.json`. The three summary tiles (episodes watched/total, followed series, backlog) are unchanged and always visible above the toggle.

## Views

**Toggle**: a segmented control `Grafo | Barras` at the top of the stats section (below the summary tiles). Defaults to **Grafo**. Switching views does not refetch data — both views consume the same loaded dataset, transformed differently.

### Grafo (3D relationship graph)

A 3D force-directed graph via **`react-force-graph-3d`** (wraps `3d-force-graph` → `three.js` + `d3-force-3d` physics; peer dep is just `react`, everything else — including `three` itself — comes transitively). We additionally add `three` as a direct dependency (plus `@types/three`) because building custom node visuals (below) requires importing `THREE.Sprite`/`SpriteMaterial`/`CanvasTexture` ourselves rather than relying on an undeclared transitive package.

**Graph shape** (built client-side from one raw payload, see Backend below):
- One root node `"Seguidas"` (center).
- One hub node per genre that has at least one followed series, plus a synthetic **`"Sin género"`** hub (fixed gray color) collecting followed series with zero `series_genres` rows — link `root → each genre hub` (and `root → "Sin género"` if non-empty).
- One hub node per `kind` value present among followed series (e.g. "TV", "OVA") — link `root → each kind hub`.
- One node per followed series — linked to **every** genre hub it belongs to, plus its kind hub. A series with 3 genres gets 4 edges total (3 genre + 1 kind).

**Node rendering**:
- Series nodes: a `THREE.Sprite` textured with the series' `cover_url` (already a `data:` URI for any series that's had a successful cover fetch — see CLAUDE.md's cover-fetch mechanism). Series without a fetched cover yet (remote/blocked URL, or `null`) fall back to a plain colored circle sprite (drawn to an offscreen `<canvas>`, same code path as the image case — just canvas-drawn instead of image-drawn — so there's one `nodeThreeObject` function, not two rendering paths).
- Hub nodes (root, genre, kind, "Sin género"): plain colored sphere, no image. Size scales with `count` (number of series in that hub) via a small linear scale, clamped to a sane min/max radius.
- Hover: native `nodeLabel` tooltip showing the series title (or hub name + count).
- Click: `graphRef.current.centerAt(...)` / `cameraPosition(...)` to fly the camera to the clicked node (the library's built-in helper, not custom camera math).
- Drag/scroll: library defaults (orbit controls) — no custom interaction code needed.

**Color**: genre/kind hub colors (and, by extension, their series' sprite fallback / edge tint) come from the shared deterministic palette described under "Shared genre color palette" below — same palette instance used by the ring view, so a genre reads as the same hue in both views.

**Empty/loading state**: if there are zero followed series, show the existing `.empty` text instead of mounting the graph component (an empty 3D scene is not useful, and `react-force-graph-3d` doesn't render meaningfully with zero nodes).

### Barras → Anillos (ring gauges)

Replaces the flat horizontal bars with a wrapped row of radial progress rings (SVG `<circle>` with `stroke-dasharray`, matching the approved mockup) — one ring per genre, a second row of rings per `kind`. Each ring: colored stroke (shared palette), center label = count, small caption below = genre/kind name. Pure SVG, no new dependency. Reuses the existing `get_genre_stats` / `get_type_stats` commands and `GenreStat`/`TypeStat` types unchanged — this view's data need didn't change, only its rendering.

### Shared genre/type color palette

A small frontend utility (`src/lib/categoryColor.ts` or inlined in `Stats.tsx` if small enough — implementer's call) that deterministically maps a genre or kind **name string** to a color from a fixed palette array, via a stable string hash (e.g. sum of char codes mod palette length — doesn't need to be cryptographically distributed, just stable and visually spread out). Genre names are scraped, open-ended, and not known ahead of time, so this can't be a static enum lookup. `"Sin género"` is special-cased to a fixed neutral gray regardless of hash, both because it's not a real scraped genre and to visually mark it as "incomplete data" rather than a normal category. Both `Stats.tsx`'s graph-building code and its ring-rendering code call the same function so a genre never has two different colors depending on which view you're looking at.

## Backend

### New: `get_stats_graph` command

```rust
#[derive(Serialize)]
pub struct SeriesGraphNode {
    pub id: i64,
    pub title: String,
    pub cover_url: Option<String>,
    pub genres: Vec<String>,
    pub kind: Option<String>,
}

#[tauri::command]
pub fn get_stats_graph(state: State<'_, AppState>) -> Result<Vec<SeriesGraphNode>, String>
```

One query in `db.rs` (`get_stats_graph_data(source_id) -> Vec<SeriesGraphNode>`): all `followed=1` series joined to `series_genres` (aggregated per series — `GROUP_CONCAT` or a follow-up per-series query into a `Vec<String>`, implementer's call on whichever is less awkward in `rusqlite`), plus `kind` and `cover_url`. The frontend builds the root/hub/link graph structure from this flat list — no hub-aggregation logic duplicated on the Rust side, since `get_genre_stats`/`get_type_stats` already do that aggregation for the ring view independently.

### New: `backfill_genres` command + shared helper

The per-series "does this followed series have `series_genres` rows? If not, fetch its detail page and insert genres+kind" block currently lives inline inside `refresh()`'s loop (`commands.rs`, added when the genre-stats feature shipped). Extract it to:

```rust
/// Returns true if a fetch was attempted (regardless of success) — used by
/// the caller to decide whether to apply the polite inter-series delay.
async fn backfill_series_genre_if_missing(
    app: &AppHandle,
    db: &Db,
    mirrors: &[String],
    series: &Series,
) -> bool
```

`refresh()`'s loop calls this exactly as it does today (one attempt per followed series per refresh cycle, silent failure, never blocks episode scanning). The new `#[tauri::command] pub async fn backfill_genres(app, state) -> Result<i64, String>` loops **all** followed series with a Db lock to filter to those still missing `series_genres`, then calls the same helper for each with the same `tokio::time::sleep(800ms)` politeness delay between series (same constant `refresh()` uses — no bulk-fetch shortcut, same Cloudflare-abuse concern applies regardless of trigger), returning the count that got genres populated. Progress reported via the existing `refresh-progress` event (reuse `emit_refresh_progress`, same shape, so no new frontend event-listener code is needed in `ProgressBar.tsx`).

The scraper window itself is unchanged — still `visible(true)` in `scraper_engine.rs` by design (Cloudflare manual-solve escape hatch, documented in CLAUDE.md and load-bearing for every scrape path, not just this one). Out of scope here.

## Frontend

- `src/types.ts` / `src/api.ts`: add `SeriesGraphNode` type + `getStatsGraph()` and `backfillGenres()` wrappers, alongside the existing genre/type/summary ones.
- `src/views/Stats.tsx`: add the `Grafo | Barras` toggle (local `useState`), a "Rellenar géneros ahora" button (calls `backfillGenres()`, disabled + shows a spinner/label while running via the existing `refreshing`-style pattern already used in `App.tsx`'s top bar, then reloads `getStatsGraph()`/`getGenreStats()`/`getTypeStats()` on completion).
- New file `src/views/StatsGraph.tsx`: owns the `ForceGraph3D` mount, graph-data construction from `SeriesGraphNode[]`, the `nodeThreeObject` sprite function, and click/hover handlers. Kept separate from `Stats.tsx` to keep the three.js-specific code isolated and the file sizes manageable.
- New file `src/views/StatsRings.tsx` (or inline if small): the ring-gauge rendering, replacing the old `RankingChart`.
- `package.json`: add `react-force-graph-3d`, `three`, `@types/three`; remove `recharts`.

## Testing

- `db.rs`: unit test for `get_stats_graph_data` — seed a couple of series with overlapping/missing genres and kinds, assert the returned rows' `genres`/`kind`/`cover_url` match.
- `commands.rs`/`db.rs`: test that `backfill_series_genre_if_missing` is a no-op (no fetch attempted) for a series that already has `series_genres` rows, mirroring the existing `refresh()`-backfill test from the prior spec.
- Frontend: `npx tsc --noEmit`, `npm run build`.
- End-to-end (`npm run tauri dev`): open Estadísticas, confirm it defaults to Grafo with the root/hub/series structure, hover shows titles, click flies the camera, drag orbits. Toggle to Barras, confirm rings render with matching colors per genre. Click "Rellenar géneros ahora" with at least one followed, non-swipe-added series present and confirm it gains genre nodes in the graph afterward (verify via `sqlite3 ... "SELECT * FROM series_genres"` as before).

## Explicitly out of scope

- Hiding/backgrounding the scraper window (separate, higher-risk task touching core scraping reliability, not just stats).
- Watch-trend-over-time (already out of scope per the prior spec, unchanged).
- Per-node drill-down beyond click-to-focus-camera (e.g. no side panel with full series detail on click — camera focus + hover tooltip is enough for v1).
- Editing/removing genres from the graph UI (read-only visualization).
