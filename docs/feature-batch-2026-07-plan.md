# July 2026 batch #3 — Descubrir/Biblioteca/Catálogo/Stats 3D fixes

> **For agentic workers:** REQUIRED SUB-SKILL: use **superpowers:subagent-driven-development**
> for Phase 2 and Phase 3 (tasks share files, must run sequentially within each phase).
> Phase 1's three tasks may run via **superpowers:dispatching-parallel-agents** (no shared files).
> Steps use checkbox (`- [ ]`) syntax for tracking.

**Planned on:** Sonnet 5 (user explicitly waived the Opus-planner requirement in the agent
prompt for this run — see chat).

**Baseline (recorded 2026-07-18):** `cargo test --manifest-path src-tauri/Cargo.toml` → **258
passed, 0 failed**. This N is the invariant for every task below — no task may reduce it, only
grow it (new tests added, none deleted unless explicitly superseded and said so).

---

## Investigation findings that changed scope vs. the original ask

Two of the eight tasks turned out not to be what they were described as. Read this before
implementing either.

### Task 1 is NOT a followed-filter bug — it's a misleading hint string

Traced `list_airing_season` (`src-tauri/src/commands/scan.rs:221`) →
`Db::first_episode_dates` (`src-tauri/src/db/episodes.rs:76`) → `AiringGrid.tsx`'s `filtered`
memo (lines 62-75). **Neither the SQL nor the frontend filter checks `followed` for the "Esta
temporada" branch today.** The `season` filter is exactly "has a parsed `first_episode_at` within
3 months", full stop — the "followed" toggle (`onlyFollowed` state) is a fully separate,
independently-toggleable filter.

What *does* make it look followed-only: per the original design doc
(`docs/superpowers/specs/2026-07-13-airing-this-season-design.md`), episodes are only scraped
on-demand for followed/opened series (see [[project-scraping-scope]]) — so in practice only
followed/opened series ever *have* a `first_episode_at` at all, and everything else silently
falls out of the "Esta temporada" view for lack of data, not because of a followed check. The
i18n hint string reinforces the wrong mental model: `airing.seasonHint` currently reads "Series
**seguidas** que estrenaron hace menos de 3 meses" (es) / "**Followed** series that premiered...”
(en) — explicitly claiming a followed requirement that the code doesn't actually enforce.

**Real fix:** reword the hint to describe the actual, honest constraint (data coverage, not
follow status), and add one regression test asserting an unfollowed-but-episode-having series
still qualifies for "Esta temporada" (nothing currently pins this down explicitly). No SQL/filter
logic changes — there is no "seguida" requirement to remove because none exists.

### Task 6b: the "Siguiendo (0)" node — root cause found

`buildGraphData` (`src/views/StatsGraph.tsx:66`) constructs the root node as
`{ id: "root", kind: "root", label: rootLabel }` — **no `count` field is ever set on it.** The
`nodeLabel` accessor (line ~550) renders `${node.label} (${node.count ?? 0})` for every non-series
node, root included — so the root always shows `(0)` regardless of how many genre/kind hubs or
series hang off it. This is a real, narrow bug (one field never populated), not a conceptual
issue with the graph structure. Fix: set the root node's `count` to `seriesList.length` (total
followed series feeding the graph) at construction time.

---

## Hard invariants (from CLAUDE.md + prior specs) — no task below may violate these

- `upsert_series` never touches `followed`; `set_seen_cascade` stays gap-free. Not touched by
  this batch, but nothing here may route through a code path that changes that.
- New `anilist_catalog` columns (studio, duration — Tasks 2 & 5) are **nullable, additive
  migrations** via `ensure_column`, exactly like `status` (`src-tauri/src/db.rs:188`). Existing
  rows read back `NULL` until the next sync; every query touching the new column must treat NULL
  as "unknown", never as "exclude" or "zero" (see the `hide_upcoming` NULL-safety precedent in
  `docs/superpowers/specs/2026-07-18-hide-upcoming-releases-design.md`).
- `EXCLUDED_CATALOG_GENRES`'s removal (Task 3) must not change `random_catalog_anime_in_genre`'s
  other filters (format bans, popularity floor, already-decided exclusion) — only the
  Hentai/Ecchi baseline goes away.
- No task may reduce the 258-test baseline.

## Global acceptance criteria (every task)

```bash
cargo build --manifest-path src-tauri/Cargo.toml
cargo test  --manifest-path src-tauri/Cargo.toml
npx tsc --noEmit
npm run build
```
All green, test count ≥ 258 (growing as tasks add tests). Backend changes are TDD: write the
failing test first, then the minimal fix.

---

## Phase 1 — Quick, independent fixes · MODEL: **Haiku 4.5** · run in parallel (dispatching-parallel-agents)

No two tasks in this phase touch the same file, except Task 1 and Task 4 both touch
`src/i18n/catalog/{es,en}.ts` — different keys, different lines; low conflict risk, but if run
as literal parallel worktrees, merge `es.ts`/`en.ts` by hand (two one-line additions/edits, not a
real conflict).

### Task 1: Fix "Esta temporada" hint copy (not a followed-filter bug — see investigation above)

**Files:** `src/i18n/catalog/es.ts:46`, `src/i18n/catalog/en.ts:38`,
`src-tauri/src/db/episodes.rs` (new test only).

- [ ] Reword `airing.seasonHint`:
  - es: `"Solo se muestran series con fecha de estreno registrada (normalmente seguidas o
    abiertas antes) — no todo lo en emisión estrenó esta temporada tiene ese dato"`
  - en: `"Only shows a known premiere date (usually followed/previously-opened series) —
    not every airing show that started this season has that data"`
  - Exact wording is not load-bearing; keep it honest about "data coverage", never claim a
    followed requirement.
- [ ] Add test `first_episode_dates_includes_unfollowed_series_with_episodes` in
  `src-tauri/src/db/episodes.rs` (alongside the existing `first_episode_dates_*` tests at
  line ~340-378): seed an airing, **unfollowed** series with a real episode 1 (`released_at`
  set, recent), assert it appears in `first_episode_dates`'s returned map. Locks in "no followed
  requirement" as a guarantee, not an accident.
- [ ] No change to `AiringGrid.tsx` filter logic — it is already correct.

**Acceptance:** new Rust test passes; `cargo test` count = 259; i18n strings updated in both
locales; `npm run build` clean.

---

### Task 3: Hentai/Ecchi excluded only via user ban, not a hardcoded baseline

**Files:** `src-tauri/src/commands/discover.rs` (lines 293-319, 381-393).

- [ ] Delete `EXCLUDED_CATALOG_GENRES` (line 299) entirely.
- [ ] Simplify `filter_candidate_genres` (line 313-319) to a single filter step:
  ```rust
  fn filter_candidate_genres(all_genres: Vec<String>, banned_genres: &[String]) -> Vec<String> {
      all_genres
          .into_iter()
          .filter(|g| !banned_genres.iter().any(|b| b.eq_ignore_ascii_case(g)))
          .collect()
  }
  ```
- [ ] Update the doc comments on `filter_candidate_genres` and `discover_catalog_card` (lines
  307-312, 381-383, 342-344) that reference "always-on baseline" / "excluding Hentai/Ecchi" —
  they now describe user-ban-only behavior. Remove the doc-comment block above
  `EXCLUDED_CATALOG_GENRES` (lines 293-298) along with the constant.
- [ ] Existing test `filter_candidate_genres_drops_baseline_and_user_bans_case_insensitively`
  (`src-tauri/src/commands/discover.rs:513`) asserts the baseline is dropped; rewrite it to
  **only** assert user-ban filtering (rename to
  `filter_candidate_genres_drops_only_user_bans_case_insensitively`), and add a new
  case proving Hentai/Ecchi now pass through **when not banned**:
  ```rust
  #[test]
  fn filter_candidate_genres_no_longer_excludes_hentai_ecchi_by_default() {
      let all = vec!["Hentai".to_string(), "Ecchi".to_string(), "Action".to_string()];
      let result = filter_candidate_genres(all, &[]);
      assert_eq!(result, vec!["Hentai", "Ecchi", "Action"]);
  }
  ```
- [ ] Do NOT touch `random_catalog_anime_in_genre`, format bans, popularity floor, or
  already-decided exclusion — those are untouched by this filter and stay exactly as-is.
  `hide_upcoming` (added in the 2026-07-18 spec) is also untouched.

**Acceptance:** `cargo test` count ≥ 259 (net: -0 or +1 depending on whether the rewritten test
counts as replacing or adding — either way, no fewer tests than before minus the deleted
baseline-specific assertions, plus the new explicit-passthrough test). No behavior change to
banned-genre filtering, format bans, or hide-upcoming.

---

### Task 4: Rename "Biblioteca" → "Mi biblioteca"

**Files:** `src/i18n/catalog/es.ts:14`.

- [ ] `"nav.library": "Biblioteca"` → `"nav.library": "Mi biblioteca"`.
- [ ] English (`en.ts:9`, `"nav.library": "Library"`) is unchanged — the ask was Spanish-only
  ("Biblioteca" → "Mi biblioteca" doesn't map to an English rename request). If the user actually
  wants the English nav renamed too ("My Library"), that's a one-line follow-up, flagged here
  rather than assumed.
- [ ] Grep for any other literal `"Biblioteca"` string outside i18n (there should be none — the
  view already reads through `t("nav.library")`); confirm with
  `grep -rn '"Biblioteca"' src/` before closing.

**Acceptance:** `npx tsc --noEmit` clean, `npm run build` clean, nav label changed in the running
app (visual spot-check).

---

## Phase 2 — Stats 3D graph (StatsGraph.tsx) · sequential (same file) · mixed model

All four tasks below edit `src/views/StatsGraph.tsx`. **Do not parallelize this phase** — run
the sub-tasks one after another in this order (cheapest/most-diagnosed first, most creative
last), even though models differ per sub-task.

### Task 6b: Fix "Siguiendo (0)" — root node never gets a `count` (Haiku 4.5)

**Files:** `src/views/StatsGraph.tsx` (`buildGraphData`, line 66).

- [ ] Change the root node literal from:
  ```ts
  const nodes: GNode[] = [{ id: "root", kind: "root", label: rootLabel }];
  ```
  to:
  ```ts
  const nodes: GNode[] = [{ id: "root", kind: "root", label: rootLabel, count: seriesList.length }];
  ```
- [ ] No other change needed — `nodeLabel`'s `${node.label} (${node.count ?? 0})` already reads
  `count` generically for every non-series node; setting it on root is the entire fix.
- [ ] This file has no existing frontend test harness (consistent with the rest of
  `StatsGraph.tsx`/Descubrir's frontend — no test to add here). Verify manually: open Stats →
  3D graph tab, hover the root/"Siguiendo" node, confirm the count matches the number of
  followed series shown (not 0) while its edge count (to genre/kind hubs) is unchanged.

**Acceptance:** `npx tsc --noEmit`, `npm run build` clean; manual hover-check in the running app.

---

### Task 8: Theme-aware link color for the 3D graph (Haiku 4.5)

**Files:** `src/views/StatsGraph.tsx` (`ForceGraph3D` element, ~line 541; `theme` already in
scope at line 295).

- [ ] `ForceGraph3D` currently sets no `linkColor` prop at all, so it falls back to the library's
  default (near-white) — invisible against the light theme's pale background, exactly the
  reported bug. Add a `linkColor` prop keyed off the existing `theme` variable, following the
  same per-theme-constant pattern already used for `rootColor` (lines 11-12, 296) and the series
  sprite's `strokeColor` (line 485):
  ```ts
  const LINK_COLOR_DARK = "rgba(255, 255, 255, 0.25)";
  const LINK_COLOR_LIGHT = "rgba(23, 34, 46, 0.35)";
  // ...
  const linkColor = theme === "light" ? LINK_COLOR_LIGHT : LINK_COLOR_DARK;
  ```
  and pass `linkColor={() => linkColor}` (or a plain string, whichever the installed
  `react-force-graph-3d` typing accepts — check the existing `backgroundColor="#00000000"` prop
  for the accepted type shape) to the `<ForceGraph3D>` element.
- [ ] No test to add (no frontend test harness here, same as Task 6b). Manual check: switch to
  light theme, open the 3D graph, confirm link lines between root/hubs/series are visible against
  the light background; switch back to dark, confirm links still read correctly there too (not
  now-too-faint).

**Acceptance:** `npx tsc --noEmit`, `npm run build` clean; manual light+dark visual check.

---

### Task 6a: Loosen graph physics further (Haiku 4.5)

**Files:** `src/views/StatsGraph.tsx` (lines 21-22).

- [ ] Per user decision: bump `CHARGE_STRENGTH` from `-520` to `-700` and `LINK_DISTANCE` from
  `150` to `180` (one more step in the same direction as the prior `-260/90 → -520/150` change
  documented in the comment above these constants — update that comment to mention this second
  step).
- [ ] No test (physics constants, visual-only). Manual check: open Stats → 3D graph, confirm
  edges between hubs and series read more clearly than before without nodes drifting off-screen
  (the existing `zoomToFit` on first settle should still frame everything — if it doesn't,
  that's a regression to flag, not silently work around).

**Acceptance:** `npx tsc --noEmit`, `npm run build` clean; manual visual check.

---

### Task 7: Richer planet texture (Sonnet — visual/design judgment)

**Files:** `src/views/StatsGraph.tsx` (`planetTexture`, lines 196-285, and its call site at
line 503).

Read `docs/superpowers/specs/2026-07-12-stats-graph-galaxy-planets-design.md` and
`docs/superpowers/specs/2026-07-13-stats-graph-refresh-planets-light-design.md` first — this is
the third pass on the same function; don't re-litigate decisions already made there (baked
canvas texture over real lighting, no scene light, per-hub deterministic seed from color so a
rebuild doesn't re-roll the look).

Per user decision ("richer procedural"), extend `planetTexture` rather than replace its approach:

- [ ] **Per-kind texture variants** instead of one blotch style for every hub: derive a variant
  from the node's `kind` (`genreHub` vs `kindHub`) or from the seeded hash already computed
  (`seed` at line 208-209) — e.g. `seed % 3` picks among "rocky" (current blotch/band style,
  keep as-is for one variant), "gas giant" (wider/smoother bands, no blotches, higher band
  contrast), "ice" (cooler tint shift + sparser, brighter blotches simulating ice caps). Keep the
  function's signature `planetTexture(color: string): THREE.CanvasTexture` — derive the variant
  internally from the color string's own seed so no caller changes are needed.
- [ ] **Crater/ring detail** for the "rocky" variant: small circular darker-rim marks (a handful,
  positioned via the existing `hash2`/hash-based seed, not `Math.random()`) layered on top of
  the existing blotch pass (after step 3 in the current function, before the limb-darkening
  vignette at step 4) — reuse `shadeHex` for the crater rim/floor tones.
- [ ] **Sharper terminator** (per feedback "doesn't look like a planet" often means the
  light/dark transition reads flat): narrow the radial gradient's midpoint stop (currently
  `0.45` at line 226) closer to the light side (e.g. `0.32`) so the transition band is more
  pronounced, and confirm this doesn't wash out the latitude bands drawn afterward.
- [ ] Keep the existing atmospheric glow (step 5, lines 273-284) and limb darkening (step 4)
  unchanged unless the above changes visually conflict with them — if they do, note the
  adjustment inline as a one-line comment (why, not what).
- [ ] No automated test (canvas rendering, no visual-diff harness in this repo). Manual
  verification required: open Stats → 3D graph, confirm hubs now show visibly distinct
  rocky/gas/ice looks and the rocky variant shows craters, in both themes.

**Acceptance:** `npx tsc --noEmit`, `npm run build` clean; manual visual check across themes and
at least one hub of each variant kind (may need several hubs/genres to see all three, since the
variant is seeded off each hub's own color).

---

## Phase 3 — New AniList data surfaces (studio filter, real duration) · sequential (shared files) · MODEL: **Sonnet**

Both tasks add a column to `anilist_catalog` and touch the same three files
(`src-tauri/src/anilist.rs`, `src-tauri/src/db/catalog.rs`, `src-tauri/src/commands/catalog.rs`).
**Run Task 5 first, then Task 2** — smaller surface first, so Task 2's edits land on top of an
already-clean diff instead of the two colliding mid-flight.

### Task 5: Real AniList `duration` instead of format-based estimate

**Confirmed scope (per investigation):** episode *counts* are already real —
`CatalogAnime.episodes` (`src-tauri/src/anilist.rs:81`) is synced from AniList's own `episodes`
field, and site-scraped series count real rows in the `episodes` table. The only estimated
number today is **minutes per episode**, via `minutes_per_episode(format)`
(`src-tauri/src/db/stats.rs:51-59`, a hardcoded TV=24/MOVIE=100/etc. table) — its own doc
comment already says AniList has a real field for this that isn't synced yet. This task is
**duration only** — do not imply "watch time is now 100% accurate" anywhere in UI copy; it is
"accurate when AniList has the data and the series is linked, estimated otherwise", same caveat
class as every other AniList-sourced field in this codebase.

**Files:** `src-tauri/src/db.rs` (schema), `src-tauri/src/anilist.rs`,
`src-tauri/src/db/catalog.rs`, `src-tauri/src/db/stats.rs`.

- [ ] **Schema** (`src-tauri/src/db.rs`, alongside the `status` migration at line 188):
  ```rust
  ensure_column(&self.conn, "anilist_catalog", "duration", "INTEGER")?;
  ```
  Nullable, additive — existing rows are NULL until next sync.
- [ ] **GraphQL query** (`src-tauri/src/anilist.rs`, `CATALOG_QUERY` at line 34-58): add
  `duration` to the `media { ... }` selection set (after `episodes`, before `averageScore`).
- [ ] **Structs**: `MediaEntry` (line 128-143) gains `duration: Option<i64>`. `CatalogAnime`
  (line 60-92) gains `pub duration: Option<i64>` with the same `#[serde(default)]` pattern as
  `status` (doc comment: "AniList's real per-episode minutes, when synced; `None` for rows synced
  before this field existed or when AniList itself has no duration for the title"). Update the
  `From<MediaEntry> for CatalogAnime` impl (line 156-172) to carry it through.
- [ ] **Persistence** (`src-tauri/src/db/catalog.rs`, `upsert_catalog_anime` lines 46-73):
  add `duration` to the `INSERT`/`ON CONFLICT DO UPDATE` column list and the bound-params tuple,
  same pattern as `status`. `row_to_catalog_anime` (line 209+) reads it back.
- [ ] **Consumption** (`src-tauri/src/db/stats.rs`, `get_watch_insights`, both minute-estimate
  blocks):
  - `estimated_minutes_external` block (lines 288-300): query already selects `c.episodes,
    c.format` from `anilist_catalog` — add `c.duration` to the select list, and change the
    per-row calc from `minutes_per_episode(format.as_deref()) * episodes` to
    `duration.unwrap_or_else(|| minutes_per_episode(format.as_deref())) * episodes`.
  - `estimated_minutes_tracked` block (lines 269-281): currently groups seen-episode counts by
    `s.kind` only, with no catalog join (most followed/site-scraped series have `anilist_id =
    NULL` — see [[project-scraping-scope]] memory). Add a `LEFT JOIN anilist_catalog c ON c.id =
    s.anilist_id` to the existing query and select `c.duration` alongside `s.kind`; per-row calc
    becomes `duration.unwrap_or_else(|| minutes_per_episode(kind.as_deref())) * cnt`. This is a
    free correctness improvement for the rare case a followed series *is* linked — it costs
    nothing when `anilist_id` is NULL (join simply contributes NULL, same as today).
  - `minutes_per_episode` itself is untouched — it remains the fallback path, not replaced.
- [ ] **TDD**: new tests in `src-tauri/src/db/stats.rs`'s test module (near the existing
  `get_watch_insights_estimates_minutes_from_seeded_db` test, line 539+):
  - `get_watch_insights_uses_real_duration_when_synced`: a watched-externally series linked to a
    catalog row with `duration: Some(23)`, `episodes: Some(12)` → `estimated_minutes_external ==
    276` (23*12), not `288` (the format-estimated 24*12) — proves real duration wins over the
    format fallback.
  - `get_watch_insights_falls_back_to_estimate_when_duration_is_null`: same shape but
    `duration: None` → falls back to today's `minutes_per_episode(format)` behavior (reuse the
    existing acceptance numbers, 288 etc., as a regression guard).
  - `upsert_catalog_anime` round-trip test (in `db/catalog.rs`'s test module) confirming
    `duration` persists and reads back correctly, `None` when omitted — mirror whatever existing
    `status`-round-trip test does there.

**Acceptance:** `cargo test` green with 3+ new tests, ≥261 total; `cargo build`; existing
`get_watch_insights_matches_design_doc_acceptance_example` and
`_estimates_minutes_from_seeded_db` tests still pass unchanged (they all use `duration: None` /
omit the field entirely, which must default correctly via `#[serde(default)]`/`Option` — if
those tests don't compile because they construct `CatalogAnime` literals without a `duration`
field, add `duration: None` to each literal rather than changing the tests' assertions).

---

### Task 2: Studio filter (Catálogo full; Biblioteca only for AniList-linked rows)

**Confirmed scope (per user decision + investigation):**
- **Director is out of scope.** AniList's `Media.staff` is a paginated connection, not a flat
  field like `studios` — syncing it for ~22,400 catalog rows would mean a per-title paginated
  fetch (or a much heavier query shape), a materially bigger and slower sync job with more
  AniList rate-limit exposure. Not attempted in this batch.
- **Studio** is added the same way `status` was: `Media.studios` is a simple connection that
  resolves in the same page-level query already used for everything else — one extra field, no
  extra round-trips. Take the first `isMain: true` studio's name only (co-productions with
  multiple mains are rare; documented as an approximation, not chased further).
- **Biblioteca/Library has no native studio.** The scraped site doesn't expose it. A followed
  series only gets a studio value when it's been linked to a catalog row (`anilist_id` set via
  `link_catalog_series` from Descubrir) — same constraint as Task 5's tracked-minutes join.
  Library's studio filter/column silently shows nothing for unlinked rows; this is the accepted
  limit per the user's decision, not a bug to work around.

**Files:** `src-tauri/src/db.rs` (schema), `src-tauri/src/anilist.rs`,
`src-tauri/src/db/catalog.rs`, `src-tauri/src/commands/catalog.rs`, `src-tauri/src/db/series.rs`
(`list_library`), `src-tauri/src/models.rs` (`LibraryItem`), `src/types.ts`, `src/api.ts`,
`src/views/Catalog.tsx`, `src/views/Library.tsx`, `src/i18n/catalog/{es,en}.ts`.

#### Backend — schema, sync, Catálogo filter

- [ ] **Schema**: `ensure_column(&self.conn, "anilist_catalog", "studio", "TEXT")?;` (same file/
  pattern as Task 5's `duration` column — add both in the same migration block if Task 5 already
  landed, don't create two separate near-duplicate diffs).
- [ ] **GraphQL query** (`anilist.rs` `CATALOG_QUERY`): add
  `studios(isMain: true) { nodes { name } }` to the `media { ... }` selection set.
- [ ] **Structs**: `MediaEntry` gains a nested deserialize target for the connection —
  ```rust
  #[derive(Debug, Deserialize)]
  struct StudioConnection { nodes: Vec<StudioNode> }
  #[derive(Debug, Deserialize)]
  struct StudioNode { name: String }
  ```
  and a field `studios: Option<StudioConnection>` (AniList omits the key entirely for a title
  with zero studios credited, hence `Option`). `CatalogAnime` gains
  `pub studio: Option<String>` (doc comment: "the first `isMain` studio's name, when AniList has
  one; co-productions with multiple mains only keep the first — an approximation, not exhaustive
  credit data"). In `From<MediaEntry> for CatalogAnime`:
  `studio: m.studios.and_then(|s| s.nodes.into_iter().next()).map(|n| n.name)`.
- [ ] **Persistence** (`db/catalog.rs` `upsert_catalog_anime`): add `studio` to the insert/
  conflict-update column list and bound params, same pattern as `status`/`duration`.
  `row_to_catalog_anime` reads it back.
- [ ] **`CatalogFilter`** (`db/catalog.rs` lines 10-24): add `pub studio: Option<String>`
  (doc comment matching the `format` field's style: "Exact match against
  `anilist_catalog.studio`"). `build_catalog_where` (lines 87-142): add an exact-match condition
  mirroring the existing `format` block (lines 97-101):
  ```rust
  let studio = filter.studio.as_deref().map(str::trim).filter(|s| !s.is_empty());
  if let Some(studio) = studio {
      conditions.push("studio = ?".to_string());
      params.push(Value::Text(studio.to_string()));
  }
  ```
- [ ] **Facets** (`commands/catalog.rs` `CatalogFacets`/`get_catalog_facets`, lines 63-80): add
  `pub studios: Vec<String>` and a new `Db::distinct_catalog_studios()` method mirroring
  `distinct_catalog_formats` (db/catalog.rs lines 197-207) — `SELECT DISTINCT studio FROM
  anilist_catalog WHERE studio IS NOT NULL ORDER BY studio COLLATE NOCASE`.
- [ ] **TDD** (`db/catalog.rs` test module, alongside `list_catalog_filtered_format_exact_match`
  at line 933): `list_catalog_filtered_studio_exact_match`, plus a
  `distinct_catalog_studios_returns_alphabetical_non_null` test, plus a round-trip test for the
  new column in `upsert_catalog_anime`'s existing round-trip tests.

#### Backend — Library studio (linked rows only)

- [ ] **`LibraryItem`** (`models.rs`, near line 59-74): add `pub studio: Option<String>` (doc
  comment: "Only populated for series linked to an AniList catalog row (`anilist_id` set) —
  scraped-only followed series have no native studio data and this stays `None`.").
- [ ] **`list_library`** (`db/series.rs` line 360-373): the base query already
  `LEFT JOIN episodes`; add `LEFT JOIN anilist_catalog c ON c.id = s.anilist_id` and select
  `c.studio`. Thread it through the row-mapping closure (lines 374-387) into the returned
  `LibraryItem`.
- [ ] **TDD**: extend `list_library_returns_kind_and_genres_via_one_bulk_query` (line 841, or add
  a sibling test) with a case where one series has `anilist_id` set to a catalog row carrying a
  `studio`, asserting `LibraryItem.studio` reflects it, and an unlinked series in the same result
  set has `studio: None`.

#### Frontend

- [ ] `src/types.ts`: `CatalogAnime` (line 181-191) gains `studio: string | null`.
  `CatalogFilter` (line 200-206) gains `studio?: string`. `CatalogFacets` (line 208-211) gains
  `studios: string[]`. `LibraryItem` (line 48+) gains `studio: string | null`.
- [ ] `src/api.ts`: no signature changes needed if `getAnimeCatalog`/`getCatalogFacets` already
  pass the whole filter/facets object through — verify and adjust only if a field is
  hand-enumerated rather than spread.
- [ ] `src/views/Catalog.tsx`: add a studio `<select>` next to the existing format select
  (pattern at lines 43-53's `EPISODE_OPTIONS`/format select), sourced from
  `facets.studios` (empty-string = "any", matching the format select's convention).
  `decideArgs` (lines 17-27) does not need a `studio` field — it only feeds `decide_catalog_card`,
  unrelated to filtering.
- [ ] `src/views/Library.tsx`: add a studio filter alongside the existing `Tipo`
  (`NormalizedKind`, lines 6-28) filter — same `<select>` UI pattern. Since most rows have
  `studio: null`, the "any" default must not hide unlinked rows; only an explicit studio
  selection filters them out. Derive the studio option list from the loaded `LibraryItem[]`
  client-side (`Array.from(new Set(items.map(i => i.studio).filter(Boolean))).sort()`) rather
  than a new backend facets call — Library's item list is already fully loaded client-side
  (see the existing `NormalizedKind` bucketing, which works the same way).
- [ ] i18n (`src/i18n/catalog/{es,en}.ts`): `catalog.anyStudio` = "Cualquier estudio" / "Any
  studio"; `library.anyStudio` = same wording, library-scoped key if the two views don't already
  share catalog.* keys for filters (check existing `library.filterAll` at es.ts:117 for the
  naming convention already in use).

**Acceptance:** `cargo build`/`cargo test` (net new tests, ≥264 combined with Task 5's), `npx tsc
--noEmit`, `npm run build` all clean. Manual check: Catálogo studio filter narrows results
correctly; Library studio filter narrows to linked rows only, unlinked rows disappear only when
a specific studio is selected (not by default).

---

## Explicitly out of scope (do not implement without a fresh brainstorm)

- Director field anywhere (Task 2) — cost/complexity ruled out above.
- Any UI "data freshness" indicator for `duration`/`studio` (no `_data_synced`-style banner like
  the `hide_upcoming` spec's `status_data_synced` — not requested, adds surface area beyond what
  was asked).
- A user-facing slider for graph physics (Task 6a) — user chose a fixed bump instead.
- Real THREE.js scene lighting for planets (Task 7) — user chose richer procedural over the lit-
  material alternative.
