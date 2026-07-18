# Reversibility of classifications — inverse actions across the app

**Date:** 2026-07-11
**Branch to implement on:** `feat/reversibility` (from `develop`, AFTER `feat/i18n-es-en`
has merged — all new UI strings MUST go through the i18n catalog from task #1).
**Task:** #2 of the 2026-07-11 batch. Cross-cutting foundation — Task #3's multi-level
undo cache builds on the inverse primitive defined here.

## Problem

Once the user classifies a series there is often no way back from the UI. "Empezar a ver"
(start_watching), following, "quiero ver", "descartar", "ya visto" — several of these are
one-way in practice. We must guarantee an inverse (de-classify / move between lists) is
reachable from the UI for every state a series can be put into.

## The classification state model (verified against code)

A `series` row carries three user-classification signals (`src-tauri/src/db.rs`, schema
lines 142/166/185):
- `followed` (INTEGER) — actively watching/tracking.
- `backlog_status` (TEXT NULL) — `'want'` | `'discarded'` | NULL.
- `watched_externally` (INTEGER) — "I watched this outside the app" (set by a catalog
  "Ya lo vi" swipe).

These combine into user-visible states:

| State | followed | backlog_status | watched_externally | Where shown today |
|-------|----------|----------------|--------------------|-------------------|
| Watching | 1 | NULL | 0 | Library "Viendo", Pending |
| Want ("Quiero ver") | 0 | 'want' | 0 | Descubrir → Listas |
| Discarded | 0 | 'discarded' | 0 | Descubrir → Listas |
| Watched externally | 0 | NULL | 1 | **nowhere** |
| Unclassified | 0 | NULL | 0 | Airing grid, Catalog |

## State-writing commands and current reverse coverage (verified in commands.rs)

- `set_followed(id, bool)` (`:604`) — AiringGrid toggles follow/unfollow (`AiringGrid.tsx:44`).
  **Gap:** no unfollow from Library or SeriesDetail once you're deep in the app.
- `start_watching(id)` (`:1194`) — want→watching (scrapes to link/fetch episodes; an
  allowed scrape trigger, see [[project-scraping-scope]]). **Gap:** no inverse ("dejar de
  ver" → back to Want or to Unclassified).
- `set_backlog_status(id, want|discarded|null)` (`:1258`) — Descubrir Listas covers
  want→discarded (`Descubrir.tsx:448`), discarded→want via `promote_discarded` (`:470`),
  discarded→delete (`:479`). **Gap:** no want→Unclassified ("quitar de la lista" without
  discarding).
- `decide_catalog_card` / `decide_swipe` — reversible only one level via
  `undo_last_swipe` (`:1110`, single `last_swiped_series_id`). Task #3 extends this.
- `set_seen_cascade` (`:936`) — already reversible (toggle seen/unseen in SeriesDetail
  `:171` and Pending `:89`). No work needed.
- `set_watched_externally(id, bool)` — db method exists (`db.rs:560`) but is **not exposed
  as a command** and there is **no UI** to reach a watched-externally row. **Gap:** the
  catalog "Ya lo vi" swipe is completely irreversible.

## Design

### Backend — one atomic local reclassify primitive

Add a single command that moves a series between the **non-scraping** states. Composing
two separate `invoke`s frontend-side risks partial state on failure; one atomic command
is safer and gives Task #3 a clean primitive to replay.

```rust
#[derive(serde::Deserialize)]
pub enum Classification { None, Want, Discarded, WatchedExternally }

#[tauri::command]
pub fn reclassify_series(state, series_id: i64, to: Classification) -> Result<(), String>
```

Semantics (one DB transaction; reuse existing `db` methods):
1. Clear all three signals: `set_followed(id,false)`, `set_backlog_status(id,None)`,
   `set_watched_externally(id,false)`.
2. Apply target: `Want`→`set_backlog_status(id,Some("want"))`; `Discarded`→`…Some("discarded")`;
   `WatchedExternally`→`set_watched_externally(id,true)`; `None`→leave cleared.
3. **Never scrapes. Never touches episodes/seen rows.** Re-following later goes through the
   existing `start_watching`/`set_followed` paths, which still find the episode rows intact.

This is the universal "de-classify / move between lists" inverse. Note the one target it
does **not** handle: making a series *actively watched* (`followed=1`) from Want when the
row is a catalog stub with no episodes — that legitimately needs a scrape, so it stays on
`start_watching` (unchanged). Reversing *out of* watching is pure-local and IS handled here.

Add `list_watched_externally(source_id) -> Vec<Series>` in `db.rs` (mirror `list_backlog`)
and a `list_watched_externally()` command, so those rows become reachable/reversible.

### Frontend — expose the inverse everywhere (all strings via i18n catalog)

New thin api wrappers in `src/api.ts`:
```ts
reclassifySeries(seriesId, to: "none"|"want"|"discarded"|"watched")
listWatchedExternally()
```

UI affordances:
1. **Library card** (`Library.tsx`) — add a small overflow action (`⋯`) on Watching cards
   opening a lightweight menu: **Dejar de seguir** (`reclassify none`) and **Mover a
   "Quiero ver"** (`reclassify want`). Both local/instant, no scrape. Card leaves the
   Viendo section on success (parent refetch).
2. **SeriesDetail** (`SeriesDetail.tsx`) — add a **Dejar de seguir** button near the header,
   shown when the series is followed → `reclassify none`, then `onBack()`/refresh.
3. **Descubrir → Listas** (`Descubrir.tsx`) — on Want rows add **Quitar de la lista**
   (`reclassify none`, distinct from the existing Descartar which is want→discarded). Add a
   third sub-list tab **"Ya vistas"** populated by `listWatchedExternally()`, each row with
   **Ya no la he visto** (`reclassify none`) and **Mover a "Quiero ver"** (`reclassify want`).
   Reuse the existing Listas row layout/component.
4. Keep `undo_last_swipe` and existing Listas moves working (Task #3 extends undo).

### Scope safety

Every action added here is a pure-local state move — **none scrape** (verify: no
`fetch_*`/adapter call in `reclassify_series`). This honors [[project-scraping-scope]]:
browsing/moving lists never hits the pirate site. The only scraping inverse-adjacent path
(re-`start_watching` a catalog stub) is unchanged and already an allowed trigger.

## Acceptance criteria (verifiable)

1. `cargo test` green (add a unit test for `reclassify_series` transitions and
   `list_watched_externally` against an in-memory DB, following existing db test style).
2. `npx tsc --noEmit` + `npm run build` clean. All new visible strings resolve through the
   i18n catalog (no hardcoded Spanish/English literals in JSX).
3. From the UI, each of these round-trips works and is observable:
   - Watching → Dejar de seguir → series gone from Viendo; re-followable from Airing.
   - Watching → Mover a "Quiero ver" → appears in Listas/Quiero ver; `start_watching`
     brings it back to Watching with episodes intact.
   - Want → Quitar de la lista → gone from Listas, no longer excluded from the deck.
   - Catalog "Ya lo vi" swipe → the title appears under Listas/"Ya vistas" → "Ya no la he
     visto" returns it to Unclassified (reappears in future decks).
4. `reclassify_series` never triggers a scrape (no WebView2 window opens on any reclassify).
5. No regression: `set_seen_cascade`, follow toggle in Airing, existing Listas moves still
   work.

## Live verification (required, real screenshots)

Disable App.tsx startup `refresh()` to avoid scraping during verification; revert before
commit (clean `git diff`). No synthetic OS clicks — drive via `window.eval()` if needed;
passive screenshots only.

Walk each round-trip in criterion #3 and screenshot before/after state in the relevant
view (Library, Listas). Confirm via `sqlite3 %APPDATA%\com.ernes.aot-scaffold\
animeontrack.sqlite` that `followed`/`backlog_status`/`watched_externally` flip as
specified and episode/seen rows are untouched by a reclassify.

## Out of scope

- Multi-level undo of swipes (Task #3).
- Deleting episode/seen history on unfollow (we intentionally preserve it).
- Cross-site moves.
