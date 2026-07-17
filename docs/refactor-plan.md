# AnimeOnTrack Architecture Reorganization — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Break the two Rust god-files (`db.rs` 4495 lines, `commands.rs` 2861 lines) and the frontend god-file (`Descubrir.tsx` 1196 lines) into cohesive, per-domain modules — **moving/splitting only, never rewriting logic or changing behavior**.

**Architecture:** Rust uses the `foo.rs` + `foo/` submodule pattern: `db.rs` stays as the public facade (`struct Db`, `open`, schema, re-exports) and gains a `db/` folder of per-domain files, each holding an `impl Db { … }` block plus its own `#[cfg(test)]` tests. Methods travel with their domain; `db::Db`, `db::CatalogFilter`, etc. keep their exact public paths, so no call site changes. `commands.rs` becomes `commands/` the same way, keeping every `#[tauri::command]` fn re-exported so `lib.rs`'s `generate_handler!` list is untouched. `Descubrir.tsx` becomes a `Descubrir/` folder of components + hooks + pure helpers.

**Tech Stack:** Rust (rusqlite, Tauri v2), TypeScript/React, Vite.

**Strangler strategy:** Each task extracts exactly one domain and leaves the codebase compiling + tests green. Tasks are independently mergeable. A phase = a coherent group of tasks against one god-file.

---

## Hard Invariants (from CLAUDE.md) — a refactor task that touches any of these has a bug

These behaviors MUST be byte-for-byte preserved. The tests that guard them already exist and MUST stay green:

- `upsert_series` excludes `followed` from its `ON CONFLICT` update (scan never un-follows). Guard: `upsert_series_writes_and_updates_scan_owned_airing_metadata`, `carry_follow_only_touches_unfollowed_rows`.
- `set_seen_cascade(series_id, number, seen)` marks every earlier episode seen and every later episode unseen (gap-free). Guards: `set_seen_cascade_stamps_and_clears_seen_at`, `seen_cascade_handles_non_integer_episode_numbers`, `mark_all_episodes_seen_marks_every_episode_via_cascade`.
- `scrape_via_mirrors` falls through to the next mirror on **either** fetch failure **or** empty parse. Guard: `listing_scan_failure_falls_back_to_fetching_everything`, `load_mirrors_*` tests.
- `set_mirrors` refuses to drop the active `sources.base_url` from the list. Guard: `switch_site_core_does_not_clobber_mirrors_the_user_already_configured`.
- `ExecuteScript` scripts are synchronous only (never `await` a promise in `eval()`). This lives in `scraper_engine.rs` — **out of scope for this plan; do not touch that file.**
- Covers fetched one image at a time, only for followed series (`fetch_cover_image` in `scraper_engine.rs`) — **out of scope.**
- `reclassify_series_core` never touches episodes. Guard: `reclassify_series_core_never_touches_episodes`.

**Rule for every task:** `refactor = move / split / rename`. If you find yourself editing an expression, a SQL string, a condition, or control flow, STOP — that is out of scope. The only edits allowed are: cutting a symbol from file A, pasting it verbatim into file B, adding `mod`/`use`/`pub(crate)` lines to satisfy the compiler, and adding re-exports.

---

## Global Acceptance Criteria (apply to EVERY task)

A task is complete only when ALL of these pass from the repo root, with **zero** behavior change:

```bash
cargo build --manifest-path src-tauri/Cargo.toml
cargo test  --manifest-path src-tauri/Cargo.toml
npx tsc --noEmit
npm run build
```

Plus:
- `git diff --stat` shows only moves + `mod`/`use`/re-export lines (no logic hunks inside moved bodies).
- The test **count** reported by `cargo test` is unchanged from the pre-task baseline (nothing silently dropped).

> **Note on the Windows test-binary block:** if `cargo test` fails with `os error 4551` / "Una directiva de Control de aplicaciones bloqueó este archivo", that is the machine-level Application Control policy, not the code. Fall back to `cargo build` green + confirm the test file still compiles, and note it in the task report. Do not fight it.

---

## Phase 0: Baseline

### Task 0: Capture the green baseline

**Files:** none (measurement only).

- [ ] **Step 1: Build + test + typecheck, record counts**

```bash
cargo build --manifest-path src-tauri/Cargo.toml
cargo test  --manifest-path src-tauri/Cargo.toml 2>&1 | tail -5
npx tsc --noEmit
npm run build
```

Expected: all green. Record the `test result: ok. N passed` number — this N is the invariant for every later task.

- [ ] **Step 2: Confirm clean working tree**

Run: `git status --porcelain`
Expected: only the untracked `docs/refactor-plan.md` / `docs/refactor-reorg-agent-prompt.md`. Commit the plan first so later diffs are clean.

```bash
git add docs/refactor-plan.md && git commit -m "docs: architecture reorganization plan"
```

---

## Phase 1: Split `db.rs` → `db/` submodules  ·  MODEL: **Haiku 4.5**

**Why Haiku:** every task is a mechanical cut-and-paste of an `impl Db` block + its tests into a new file. Zero design decisions — the symbol→file assignment is fully specified below.

**Mechanics every Phase-1 task follows (read once, apply to all):**

1. Create the new file `src-tauri/src/db/<domain>.rs`.
2. Top of the new file:
   ```rust
   use super::*; // brings in Db, shared free helpers, and db.rs's own `use` set
   ```
   Then add any extra `use` lines the moved code needs (rusqlite items, `crate::models::*`, `crate::dates::*`, etc.). `cargo build` will name every unresolved symbol — add imports until green. This is error-resolution, not design.
3. In the new file, wrap the moved methods in a single `impl Db { … }` block. Structs/enums that belong to this domain move too (listed per task).
4. Move the domain's `#[cfg(test)] mod tests { … }` test functions (listed per task) into a `#[cfg(test)] mod tests { use super::*; … }` block at the bottom of the new file. Shared seed helpers used by tests across domains go to `test_support` (see Task 1).
5. In `db.rs`: delete the moved bodies; add `mod <domain>;` near the top; if the domain defines a public type, add `pub use <domain>::<Type>;` so the old path (`db::<Type>`) still resolves.
6. Free (non-method) helper functions that are now referenced from another module: mark them `pub(crate)` in their home module and add the `use` line at the reference site.
7. Run the full Global Acceptance Criteria. Commit.

**Assignment map (authoritative — every db.rs symbol has exactly one home):**

| Domain file | Methods / functions | Types moved |
|---|---|---|
| `db.rs` (facade, stays) | `open`, `init_schema`, `ensure_column`, `snapshot_to` | `struct Db`, field `conn` |
| `db/sources.rs` | `upsert_source`, `get_source_id_for_site`, `get_source_base_url` | — |
| `db/series.rs` | `upsert_series`, `delete_series`, `relink_series`, `merge_series_into`, `find_series_id_by_slug`, `get_series_url`, `update_series_cover`, `update_series_url`, `set_followed`, `set_kind`, `set_backlog_status`, `get_backlog_status`, `set_anilist_id`, `set_watched_externally`, `row_to_series`, `list_followed`, `list_library`, `list_backlog`, `list_watched_externally`, `get_series_for_link`, `get_series_for_history`, `carry_follow`, `take_carried_seen_number`, `already_linked_to_site`, `engaged_series_titles`, `insert_series_genres`, `replace_series_genres`, `list_series_genres`, `series_needs_genre_backfill`, `franchise_key` (free fn) | `SeriesForLink`, `SwipeHistoryRow` |
| `db/episodes.rs` | `insert_episode`, `list_series_episodes`, `set_seen`, `set_seen_cascade`, `mark_all_episodes_seen`, `next_unseen_episode`, `max_episode_number`, `episode_count`, `existing_episode_urls`, `known_series_urls`, `insert_eps_seen_up_to` (free fn), `first_episode_dates`, `parse_ep_number` (free fn) | — |
| `db/airing.rs` | `list_airing`, `list_pending`, `pending_count`, `mk_airing` (free fn) | `enum PendingSort` |
| `db/catalog.rs` | `upsert_catalog_anime`, `catalog_anime`, `catalog_anime_full`, `catalog_anime_with_popularity`, `list_catalog`, `list_catalog_filtered`, `catalog_count`, `catalog_count_filtered`, `list_catalog_genres`, `distinct_catalog_genres`, `distinct_catalog_formats`, `build_catalog_where`, `random_catalog_anime_in_genre`, `get_catalog_titles`, `row_to_catalog_anime` | `struct CatalogFilter` |
| `db/settings.rs` | `get_setting`, `set_setting`, `delete_setting`, `get_banned_genres`, `set_banned_genres`, `get_banned_formats`, `set_banned_formats`, `set_last_checked_at`, `last_checked_age_secs` | — |
| `db/stats.rs` | `get_stats_graph_data`, `get_genre_stats`, `get_type_stats`, `get_watch_insights`, `get_watch_summary`, `get_genre_affinity`, `signature_counts`, `minutes_per_episode` (free fn) | — |

**Test routing** (each test fn goes to the domain file that owns the method it exercises — the name says which):
- `*catalog*`, `random_catalog_anime_in_genre*`, `list_catalog_filtered*`, `build_catalog_where`, `seed_filter_catalog` → `db/catalog.rs`
- `*seen_cascade*`, `insert_episode*`, `mark_all_episodes_seen*`, `max_episode_number*`, `library_next_episode*`, `existing_episode_urls*`, `known_series_urls*`, `first_episode_dates*`, `episode_count*` → `db/episodes.rs`
- `upsert_series*`, `set_*` (followed/kind/backlog/anilist/watched_externally), `merge_series_into*`, `relink_series*`, `carry_follow*`, `carried_watermark*`, `find_series_id_by_slug*`, `get_series_for_*`, `engaged_series_titles*`, `series_genres*`, `series_needs_genre_backfill*`, `delete_series_cascades*`, `franchise_key*`, `list_library*`, `list_backlog*`, `list_watched_externally*` → `db/series.rs`
- `list_airing*`, `list_pending*`, `pending_count*` → `db/airing.rs`
- `*setting*`, `banned_genres_and_formats*`, `last_checked_age*` → `db/settings.rs`
- `get_watch_*`, `get_stats_graph*`, `get_genre_affinity*`, `minutes_per_episode*`, `signature_changes*`, `get_type_stats*`, `get_genre_stats*` → `db/stats.rs`
- `upsert_source*`, `get_source_id_for_site*`, `sources_site_id_backfilled*` → `db/sources.rs`
- `open_creates_schema`, `migration_is_idempotent*`, `series_table_has_backlog_status_and_kind_columns`, `catalog_genre_index_exists`, `catalog_popularity_index_exists`, `anilist_id_is_backfilled_from_synthetic_slug_on_migration` → keep in `db.rs` (schema/migration).

If a test's target domain is ambiguous from its name, leave it in `db.rs`'s test module — it will still compile and run; correctness is unaffected. Do not agonize over placement.

---

### Task 1: Scaffold `db/` + shared test support

**Files:**
- Create: `src-tauri/src/db/test_support.rs`
- Modify: `src-tauri/src/db.rs` (add `mod db;`-style child declarations incrementally; add `#[cfg(test)] mod test_support;`)

- [ ] **Step 1: Identify the cross-domain test seed helpers**

In `db.rs`, find the test-only helper functions that multiple test modules call (e.g. `insert_test_series`, `seed_filter_catalog`, and any `fn seed_*` / `fn mk_*` used only under `#[cfg(test)]`). List them.

- [ ] **Step 2: Create `db/test_support.rs`**

```rust
// Shared #[cfg(test)] seed helpers used by the per-domain test modules.
#![cfg(test)]
use super::Db;
// + whatever the moved helpers reference (rusqlite, models, dates…)

// <paste each shared test helper fn here VERBATIM, changed only to `pub(crate)`>
```

- [ ] **Step 3: Wire it into `db.rs`**

Add near the top of `db.rs`:
```rust
#[cfg(test)]
mod test_support;
```
Delete the moved helpers from `db.rs`. In `db.rs`'s remaining `#[cfg(test)] mod tests`, add `use super::test_support::*;`.

- [ ] **Step 4: Global Acceptance Criteria + commit**

```bash
git add -A && git commit -m "refactor(db): scaffold db/ folder + shared test_support"
```

---

### Task 2: Extract `db/sources.rs`

**Files:**
- Create: `src-tauri/src/db/sources.rs`
- Modify: `src-tauri/src/db.rs`

- [ ] **Step 1:** Follow the Phase-1 Mechanics for the `sources` row of the assignment map. Move `upsert_source`, `get_source_id_for_site`, `get_source_base_url` and their tests (`upsert_source*`, `get_source_id_for_site*`, `sources_site_id_backfilled*`).
- [ ] **Step 2:** Add `mod sources;` to `db.rs`.
- [ ] **Step 3:** Global Acceptance Criteria (note: `open` in `db.rs` still calls these methods on `self` — they resolve because they're `impl Db`; no re-export needed for methods).
- [ ] **Step 4: Commit**

```bash
git add -A && git commit -m "refactor(db): extract sources into db/sources.rs"
```

---

### Task 3: Extract `db/settings.rs`

**Files:** Create `src-tauri/src/db/settings.rs`; Modify `src-tauri/src/db.rs`.

- [ ] **Step 1:** Move the `settings` row of the map (`get_setting`, `set_setting`, `delete_setting`, `get_banned_genres`, `set_banned_genres`, `get_banned_formats`, `set_banned_formats`, `set_last_checked_at`, `last_checked_age_secs`) + tests (`*setting*`, `banned_genres_and_formats*`, `last_checked_age*`). Add `mod settings;`.
- [ ] **Step 2:** Global Acceptance Criteria.
- [ ] **Step 3: Commit** `refactor(db): extract settings into db/settings.rs`

---

### Task 4: Extract `db/episodes.rs`

**Files:** Create `src-tauri/src/db/episodes.rs`; Modify `src-tauri/src/db.rs`.

- [ ] **Step 1:** Move the `episodes` row. **Invariant guard:** `set_seen_cascade` and `mark_all_episodes_seen` bodies are copied byte-for-byte. Move free fns `insert_eps_seen_up_to`, `parse_ep_number` (mark `pub(crate)` if referenced elsewhere) + the episode tests. Add `mod episodes;`.
- [ ] **Step 2:** Global Acceptance Criteria — confirm `set_seen_cascade_*` and `mark_all_episodes_seen_*` tests still pass.
- [ ] **Step 3: Commit** `refactor(db): extract episodes into db/episodes.rs`

---

### Task 5: Extract `db/catalog.rs`

**Files:** Create `src-tauri/src/db/catalog.rs`; Modify `src-tauri/src/db.rs`.

- [ ] **Step 1:** Move the `catalog` row including `struct CatalogFilter`; add `pub use catalog::CatalogFilter;` to `db.rs`. Move the catalog tests + the `seed_filter_catalog` helper (into `test_support.rs` if used by more than one module, else keep local). Add `mod catalog;`.
- [ ] **Step 2:** Global Acceptance Criteria — `list_catalog_filtered_*` and `random_catalog_anime_in_genre_*` tests green.
- [ ] **Step 3: Commit** `refactor(db): extract catalog into db/catalog.rs`

---

### Task 6: Extract `db/stats.rs`

**Files:** Create `src-tauri/src/db/stats.rs`; Modify `src-tauri/src/db.rs`.

- [ ] **Step 1:** Move the `stats` row (`get_watch_insights`, `get_watch_summary`, `get_stats_graph_data`, `get_genre_stats`, `get_type_stats`, `get_genre_affinity`, `signature_counts`, `minutes_per_episode`) + tests. Add `mod stats;`.
- [ ] **Step 2:** Global Acceptance Criteria — `get_watch_insights_*` acceptance-example tests green.
- [ ] **Step 3: Commit** `refactor(db): extract stats into db/stats.rs`

---

### Task 7: Extract `db/airing.rs`

**Files:** Create `src-tauri/src/db/airing.rs`; Modify `src-tauri/src/db.rs`.

- [ ] **Step 1:** Move `list_airing`, `list_pending`, `pending_count`, `mk_airing`, `enum PendingSort`; add `pub use airing::PendingSort;`. Move `list_airing*` / `list_pending*` / `pending_count*` tests. Add `mod airing;`.
- [ ] **Step 2:** Global Acceptance Criteria.
- [ ] **Step 3: Commit** `refactor(db): extract airing/pending into db/airing.rs`

---

### Task 8: Extract `db/series.rs` (largest; do last so db.rs is already thin)

**Files:** Create `src-tauri/src/db/series.rs`; Modify `src-tauri/src/db.rs`.

- [ ] **Step 1:** Move the `series` row (all series CRUD + link/merge/classify + series_genres + `carry_follow`/`take_carried_seen_number` + `franchise_key`) and `SeriesForLink`, `SwipeHistoryRow`; add `pub use series::{SeriesForLink, SwipeHistoryRow};`. **Invariant guard:** `upsert_series` keeps `followed` out of its `ON CONFLICT DO UPDATE` set — copy verbatim. Move the series tests. Add `mod series;`.
- [ ] **Step 2:** Global Acceptance Criteria — `upsert_series_writes_and_updates_scan_owned_airing_metadata`, `carry_follow_only_touches_unfollowed_rows`, `merge_series_into_*`, `reclassify`-adjacent series tests green.
- [ ] **Step 3:** Confirm `db.rs` is now just: `use`s, `struct Db`, `open`, `init_schema`, `ensure_column`, `snapshot_to`, the `mod` declarations, the `pub use` re-exports, and schema/migration tests. Should be well under ~400 lines.
- [ ] **Step 4: Commit** `refactor(db): extract series into db/series.rs; db.rs is now a thin facade`

---

## Phase 2: Split `commands.rs` → `commands/` submodules  ·  MODEL: **Haiku 4.5**

**Why Haiku:** same mechanical cut-and-paste. The pure `_core`/helper fns already exist as clean, test-covered seams; they travel with their feature. Assignment is fully specified.

**Mechanics every Phase-2 task follows:**

1. Create `src-tauri/src/commands/<feature>.rs`.
2. Top of file:
   ```rust
   use super::*; // re-exports commands.rs's shared use-set + helpers
   ```
   Add extra `use` lines as the compiler demands (`tauri::State`, `crate::db::Db`, `crate::adapter::*`, event structs, etc.).
3. `#[tauri::command]` attributes and full fn signatures are copied **verbatim** — the Tauri arg-name → snake_case contract must not change.
4. `commands.rs` becomes `commands/mod.rs`-style facade **without renaming**: keep `commands.rs`, add `mod <feature>;` for each, and **`pub use <feature>::*;`** so every `#[tauri::command]` fn stays reachable at `crate::commands::<fn>` exactly as `lib.rs`'s `generate_handler!` expects. **Do not edit `lib.rs`.**
5. Move each feature's inline `#[cfg(test)]` tests with it (they mostly test the `_core` fns).
6. Shared helpers used by multiple features (`get_source_id`, `emit_refresh_progress`, `with_mirror`, `scrape_via_mirrors`, `normalize`, `slug_from_url`, `path_of`, `push_history`) stay in `commands.rs` as `pub(crate)` and submodules call them via `use super::*` — see Task 9.
7. Global Acceptance Criteria. Commit.

**Assignment map (authoritative):**

| Feature file | `#[tauri::command]` fns + their `_core`/helpers/tests |
|---|---|
| `commands.rs` (facade + shared) | `get_source_id`, `get_active_site`, `get_active_site_id`, `get_active_adapter`, `emit_refresh_progress`, `with_mirror`, `scrape_via_mirrors`, `slug_from_url`, `normalize`, `path_of`, `push_history`, `push_history_dedups*`/`push_history_keeps*` tests — the cross-feature primitives |
| `commands/scan.rs` | `refresh`, `scan_airing`, `rescan_airing`, `scan_airing_via_mirrors`, `scanned`, `should_fetch_series`, `fetch_episode_list_for`, `fetch_series_detail`, `fetch_series_episodes`, `list_airing`, `list_airing_season`, `list_episodes`, `backfill_genres`, `backfill_series_genre_if_missing`, `ensure_genre_list`, `load_genre_list`, `save_genre_list` + all `fetch_when_*`/`skip_when_*`/`off_listing_*`/`force_always_fetches`/`unknown_badge_*`/`listing_scan_failure_*`/`regression_recap_row_*` tests |
| `commands/mirrors.rs` | `get_mirrors`, `set_mirrors`, `load_mirrors`, `save_mirrors`, `parse_mirrors`, `get_active_site`→(if only used by sites, keep here), `set_active_site`, `switch_site_core`, `list_sites`, `search_site` + `load_mirrors_*`, `mirrors_are_isolated_per_site`, `parse_mirrors`, `switch_site_core_*`, `list_sites_matches_the_adapter_registry` tests |
| `commands/follow.rs` | `set_followed`, `reclassify_series`, `reclassify_series_core`, `start_watching`, `set_backlog_status`, `link_series_core`, `link_catalog_series`, `promote_discarded`, `plan_carryover` + `reclassify_series_core_*`, `plan_carryover_*` tests |
| `commands/seen.rs` | `set_seen`, `set_seen_cascade`, `open_episode`, `delete_series` |
| `commands/discover.rs` | `decide_swipe`, `decide_catalog_card`, `discover_swipe_card`, `discover_catalog_card`, `to_candidates`, `filter_candidate_genres`, `get_deck_bans`, `set_deck_bans`, `undo_last_swipe`, `undo_swipe_entry`, `list_swipe_history`, `push_swipe_history`, `decision_for_history_row` + `filter_candidate_genres_*`, `decision_for_history_row_*` tests |
| `commands/catalog.rs` | `get_anime_catalog`, `get_catalog_facets`, `sync_anime_catalog`, `get_series_genres`, `get_top_genres` |
| `commands/stats.rs` | `get_stats_graph`, `get_genre_stats`, `get_type_stats`, `get_watch_insights`, `get_watch_summary` |
| `commands/backup.rs` | `backup_now`, `backup_now_iso`, `backup_now_unix`, `backup_status`, `auto_backup_if_due`, `connect_drive`, `disconnect_drive`, `backup_dir` |
| `commands/library.rs` | `list_library`, `list_backlog`, `list_pending`, `pending_count`, `list_watched_externally` |

> Placement heuristic for any fn not listed: put it with the feature whose `#[tauri::command]` it supports; if it is called by ≥2 features, leave it in `commands.rs` as `pub(crate)`. When genuinely unsure, leaving a fn in `commands.rs` is always safe (it stays reachable via `use super::*`).

---

### Task 9: Scaffold `commands/` + hoist shared primitives

**Files:** Modify `src-tauri/src/commands.rs`.

- [ ] **Step 1:** In `commands.rs`, mark the cross-feature helpers from the facade row (`get_source_id`, `emit_refresh_progress`, `with_mirror`, `scrape_via_mirrors`, `slug_from_url`, `normalize`, `path_of`, `push_history`, `get_active_adapter`, `get_active_site_id`) as `pub(crate)` so submodules can call them via `use super::*`. Do not move them yet.
- [ ] **Step 2:** Global Acceptance Criteria (should be a no-op behavior-wise; only visibility changed).
- [ ] **Step 3: Commit** `refactor(commands): make shared command helpers pub(crate) for submodules`

---

### Task 10: Extract `commands/backup.rs`

**Files:** Create `src-tauri/src/commands/backup.rs`; Modify `src-tauri/src/commands.rs`.

- [ ] **Step 1:** Follow Phase-2 Mechanics for the `backup` row. Add `mod backup;` + `pub use backup::*;` to `commands.rs`.
- [ ] **Step 2:** Global Acceptance Criteria.
- [ ] **Step 3: Commit** `refactor(commands): extract backup commands`

---

### Task 11: Extract `commands/stats.rs`

**Files:** Create `src-tauri/src/commands/stats.rs`; Modify `src-tauri/src/commands.rs`.

- [ ] **Step 1:** Move the `stats` row + `pub use stats::*;`.
- [ ] **Step 2:** Global Acceptance Criteria. **Note:** these command fns share names with `db/stats.rs` methods (`get_genre_stats` etc.) — that is fine, different modules; do not merge them.
- [ ] **Step 3: Commit** `refactor(commands): extract stats commands`

---

### Task 12: Extract `commands/library.rs`

**Files:** Create `src-tauri/src/commands/library.rs`; Modify `src-tauri/src/commands.rs`.

- [ ] **Step 1:** Move the `library` row + `pub use library::*;`.
- [ ] **Step 2:** Global Acceptance Criteria.
- [ ] **Step 3: Commit** `refactor(commands): extract library/pending commands`

---

### Task 13: Extract `commands/seen.rs`

**Files:** Create `src-tauri/src/commands/seen.rs`; Modify `src-tauri/src/commands.rs`.

- [ ] **Step 1:** Move the `seen` row + `pub use seen::*;`.
- [ ] **Step 2:** Global Acceptance Criteria.
- [ ] **Step 3: Commit** `refactor(commands): extract seen/episode commands`

---

### Task 14: Extract `commands/catalog.rs`

**Files:** Create `src-tauri/src/commands/catalog.rs`; Modify `src-tauri/src/commands.rs`.

- [ ] **Step 1:** Move the `catalog` row + `pub use catalog::*;`.
- [ ] **Step 2:** Global Acceptance Criteria.
- [ ] **Step 3: Commit** `refactor(commands): extract catalog commands`

---

### Task 15: Extract `commands/discover.rs`

**Files:** Create `src-tauri/src/commands/discover.rs`; Modify `src-tauri/src/commands.rs`.

- [ ] **Step 1:** Move the `discover` row (swipe/catalog card decisions, deck bans, undo, history) + tests. `pub use discover::*;`.
- [ ] **Step 2:** Global Acceptance Criteria — `filter_candidate_genres_*`, `decision_for_history_row_*` green.
- [ ] **Step 3: Commit** `refactor(commands): extract discover/swipe commands`

---

### Task 16: Extract `commands/follow.rs`

**Files:** Create `src-tauri/src/commands/follow.rs`; Modify `src-tauri/src/commands.rs`.

- [ ] **Step 1:** Move the `follow` row + tests. **Invariant guard:** `reclassify_series_core` still never touches episodes — copy verbatim. `pub use follow::*;`.
- [ ] **Step 2:** Global Acceptance Criteria — `reclassify_series_core_*`, `plan_carryover_*` green.
- [ ] **Step 3: Commit** `refactor(commands): extract follow/reclassify/carryover commands`

---

### Task 17: Extract `commands/mirrors.rs`

**Files:** Create `src-tauri/src/commands/mirrors.rs`; Modify `src-tauri/src/commands.rs`.

- [ ] **Step 1:** Move the `mirrors` row + tests. **Invariant guard:** `set_mirrors` still refuses to drop the active `base_url`; `switch_site_core` still preserves user mirrors — copy verbatim. `pub use mirrors::*;`.
- [ ] **Step 2:** Global Acceptance Criteria — `load_mirrors_*`, `mirrors_are_isolated_per_site`, `switch_site_core_*` green.
- [ ] **Step 3: Commit** `refactor(commands): extract mirrors/sites commands`

---

### Task 18: Extract `commands/scan.rs` (largest; do last)

**Files:** Create `src-tauri/src/commands/scan.rs`; Modify `src-tauri/src/commands.rs`.

- [ ] **Step 1:** Move the `scan` row (refresh/scan/rescan, fetch decision helpers, genre backfill) + all `fetch_when_*`/`skip_when_*`/`off_listing_*`/`listing_scan_failure_*`/`regression_recap_row_*` tests. **Invariant guard:** the mirror-fallback path in `refresh`/`scan_airing_via_mirrors` still calls the shared `scrape_via_mirrors` (which lives in `commands.rs`) — do not inline or alter it. `pub use scan::*;`.
- [ ] **Step 2:** Global Acceptance Criteria — `listing_scan_failure_falls_back_to_fetching_everything` green.
- [ ] **Step 3:** Confirm `commands.rs` now holds only the shared primitives + `mod`/`pub use` lines (well under ~400 lines).
- [ ] **Step 4: Commit** `refactor(commands): extract scan/refresh commands; commands.rs is now a thin facade`

---

## Phase 3: Decompose `Descubrir.tsx` → `Descubrir/` folder  ·  MODEL: **Sonnet**

**Why Sonnet:** component/hook boundaries need bounded judgment (which state stays local to `SwipeView`, what the shared prop interfaces are). The target structure is fixed below; Sonnet decides only the fine-grained internal cut lines, and must not change rendered behavior.

**Target structure (fixed):**

```
src/views/Descubrir/
  index.tsx              // `Descubrir` router (sub-view tabs → SwipeView | ListasView). Default export.
  SwipeView.tsx          // the swipe deck (fillQueue, decide, undo, keyboard, prefetch)
  ListasView.tsx         // Want/Discarded/Watched tabs + search
  DeckPanel.tsx          // genre/format bans panel
  DiscoverModeToggle.tsx
  TasteChips.tsx
  HistoryRow.tsx
  rows.tsx               // WantRow, DiscardedRow, WatchedRow (they share row layout + PosterThumb)
  components.tsx         // PosterThumb, OverflowMenu (shared presentational, reused by rows + panels)
  useLinkQueue.ts        // the link-queue hook
  helpers.ts             // anilistIdFromUrl, listasInitials, norm, getInitialDeckPanelOpen, persistDeckPanelOpen, handleStartWatching-if-extractable
  constants.ts           // DECISION_BADGE, DECK_FORMATS, DECK_PANEL_STORAGE_KEY, MAX_FILL_ROUNDS, PREFETCH_TARGET, RECLASSIFY_TARGET, REFILL_THRESHOLD, TOP_GENRES_LIMIT
  types.ts               // LinkStatus, ListTab, SubView, SwipeOutDirection, SwipeHistoryItem (if locally defined)
```

**Rules for the whole phase:**
- Move JSX/logic **verbatim**. No markup, className, style, effect-dependency, or handler-logic changes. This phase is a pure code-move; the app must render and behave identically.
- The existing import path is `src/views/Descubrir.tsx`. Callers (`App.tsx`) import `Descubrir` from `"./views/Descubrir"` — creating `Descubrir/index.tsx` keeps that specifier resolving. Verify `App.tsx` (and any other importer) needs **zero** edits.
- Cross-file sharing: constants/types/helpers become named exports; components import what they use. Keep exports minimal — only what another file needs becomes `export`.
- Acceptance for every task here: `npx tsc --noEmit` **and** `npm run build` green, plus the Rust Global Acceptance commands (unchanged, should stay green trivially).

**Verification note:** there are no unit tests for these components. After the phase, the human reviewer (or `/run`) must launch the app (`npm run tauri dev`) and confirm the Descubrir screen — swipe deck, undo, keyboard shortcuts, bans panel, Listas tabs + search — behaves identically. Flag this in the final report; the agent cannot self-verify rendered behavior.

---

### Task 19: Extract leaf modules (constants, types, helpers, shared components)

**Files:**
- Create: `src/views/Descubrir/constants.ts`, `src/views/Descubrir/types.ts`, `src/views/Descubrir/helpers.ts`, `src/views/Descubrir/components.tsx`
- Modify: `src/views/Descubrir.tsx` (import from the new files instead of defining inline) — **do not move the folder yet.**

- [ ] **Step 1:** Move the constants (`DECISION_BADGE`, `DECK_FORMATS`, `DECK_PANEL_STORAGE_KEY`, `MAX_FILL_ROUNDS`, `PREFETCH_TARGET`, `RECLASSIFY_TARGET`, `REFILL_THRESHOLD`, `TOP_GENRES_LIMIT`) into `constants.ts` as named exports.
- [ ] **Step 2:** Move the types (`LinkStatus`, `ListTab`, `SubView`, `SwipeOutDirection`, and any local `SwipeHistoryItem`) into `types.ts`.
- [ ] **Step 3:** Move pure helpers (`anilistIdFromUrl`, `listasInitials`, `norm`, `getInitialDeckPanelOpen`, `persistDeckPanelOpen`) into `helpers.ts`.
- [ ] **Step 4:** Move `PosterThumb` and `OverflowMenu` into `components.tsx` (they carry their own local state — copy verbatim).
- [ ] **Step 5:** In `Descubrir.tsx`, replace the moved definitions with imports from `./Descubrir/constants` etc. (Note: while the file is still `Descubrir.tsx`, the folder `Descubrir/` can coexist as long as there is no `Descubrir/index.tsx` yet — Vite/TS resolve `./Descubrir/constants` to the file.)
- [ ] **Step 6:** Run: `npx tsc --noEmit && npm run build`. Expected: green.
- [ ] **Step 7: Commit** `refactor(descubrir): extract constants/types/helpers/shared components`

---

### Task 20: Extract the panels and rows

**Files:**
- Create: `src/views/Descubrir/DeckPanel.tsx`, `src/views/Descubrir/DiscoverModeToggle.tsx`, `src/views/Descubrir/TasteChips.tsx`, `src/views/Descubrir/HistoryRow.tsx`, `src/views/Descubrir/rows.tsx`
- Modify: `src/views/Descubrir.tsx`

- [ ] **Step 1:** Move `DeckPanel`, `DiscoverModeToggle`, `TasteChips`, `HistoryRow` each to its own file (verbatim; import constants/types/helpers/components as needed). Their internal helper closures (`toggle`, `save`, `onClick`, `onDown`, etc.) move with them.
- [ ] **Step 2:** Move `WantRow`, `DiscardedRow`, `WatchedRow` (and their closures `retryLink`, `handleStartWatching`) into `rows.tsx`.
- [ ] **Step 3:** In `Descubrir.tsx`, import these instead of defining them.
- [ ] **Step 4:** Run: `npx tsc --noEmit && npm run build`. Expected: green.
- [ ] **Step 5: Commit** `refactor(descubrir): extract deck panels and list rows`

---

### Task 21: Extract `useLinkQueue`, `SwipeView`, `ListasView`; convert to folder module

**Files:**
- Create: `src/views/Descubrir/useLinkQueue.ts`, `src/views/Descubrir/SwipeView.tsx`, `src/views/Descubrir/ListasView.tsx`, `src/views/Descubrir/index.tsx`
- Delete: `src/views/Descubrir.tsx`

- [ ] **Step 1:** Move the `useLinkQueue` hook into `useLinkQueue.ts` (verbatim; export it).
- [ ] **Step 2:** Move `SwipeView` (with its `enqueue`/`fillQueue`/`decide`/`undo`/`onKeyDown` closures) into `SwipeView.tsx`, importing `useLinkQueue`, constants, `HistoryRow`, `DeckPanel`, `TasteChips`, `DiscoverModeToggle`, `components`. **Do not alter effect dependency arrays or the prefetch/refill logic.**
- [ ] **Step 3:** Move `ListasView` into `ListasView.tsx`, importing `rows`, `helpers` (`norm`), `components`.
- [ ] **Step 4:** Move the top-level `Descubrir` router into `index.tsx` as the default export, importing `SwipeView` + `ListasView`. Delete the old `src/views/Descubrir.tsx`.
- [ ] **Step 5:** Confirm importers are unchanged: `git grep -n "views/Descubrir" src` should show only the resolving `./views/Descubrir` specifier(s) in `App.tsx`, now resolving to `Descubrir/index.tsx`.
- [ ] **Step 6:** Run: `npx tsc --noEmit && npm run build`. Expected: green.
- [ ] **Step 7:** Human/`/run` verification of the Descubrir screen (see phase note).
- [ ] **Step 8: Commit** `refactor(descubrir): split SwipeView/ListasView/router into Descubrir/ module`

---

## Phase 4 (OPTIONAL): Secondary files  ·  MODEL: **Sonnet** (Haiku for the pure-mechanical ones)

Lower priority; each is independently mergeable and only worth doing if the file is actively being worked on. Not required for the core goal.

- **`src/views/StatsGraph.tsx` (568)** — extract the 3D/galaxy render helpers + `categoryColor` usage into a `statsGraph/` folder (geometry/layout pure fns → `layout.ts`, presentational → components). MODEL: Sonnet.
- **`src/views/Library.tsx` (560)** — extract filter controls + row components into `Library/` folder. MODEL: Sonnet.
- **`src/views/Catalog.tsx` (425)** — extract filter panel + card grid. MODEL: Sonnet.
- **`src-tauri/src/anilist.rs` (569)** — split the GraphQL query builders / response DTOs from the sync driver into `anilist/` (`query.rs`, `model.rs`, `sync.rs`). Pure-mechanical → MODEL: Haiku.

Each follows the same pattern: move verbatim, keep public paths via re-export, `npx tsc --noEmit` + `npm run build` (or `cargo build`/`cargo test`) green, zero behavior change.

---

## Execution Order & Dependencies

```
Phase 0  →  Phase 1 (Tasks 1→8, sequential; each leaves db.rs green)
            Phase 2 (Tasks 9→18, sequential; independent of Phase 1 — can run in a parallel worktree)
            Phase 3 (Tasks 19→21, sequential; independent of Phases 1–2 — parallel worktree)
Phase 4  →  optional, any time after its file is otherwise idle
```

- **Within a phase, tasks are sequential** (each depends on the previous task's `mod` scaffolding + shrinking facade).
- **Across Phases 1/2/3 there is no code overlap** (db vs commands vs frontend) — they may be executed in parallel worktrees per superpowers:dispatching-parallel-agents, then merged. If run in one tree, do them in order to keep diffs clean.
- Every task ends on a green commit, so any phase can be merged to `develop` on its own.

## Self-Review Notes

- Spec coverage: db.rs (Phase 1), commands.rs (Phase 2), Descubrir.tsx (Phase 3), secondary files (Phase 4) — all god-files from the goal are covered. ✅
- Every hard invariant from CLAUDE.md is named in the task that could threaten it, with the guarding test to re-run. ✅
- No task rewrites logic; acceptance is `build + test + tsc + npm build` green with unchanged test count. ✅
- Symbol→file assignment is exhaustive for db.rs (from the graph) and commands.rs; the only judgment left ("ambiguous placement") has a documented safe default (leave it in the facade). ✅
