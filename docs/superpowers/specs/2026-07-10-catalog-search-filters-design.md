# Catálogo: search + filters

Follow-up to `2026-07-10-full-anilist-catalog-sync-design.md` (merged — `anilist_catalog` now holds the full ~22,400-title AniList catalog locally, popularity-ordered). The Catálogo tab (`src/views/Catalog.tsx`) currently only supports linear "Cargar más" browsing. With 22k titles local, search and filtering are pure SQL against SQLite — no network, no rate limits.

## Problem

No way to find a specific title or narrow by genre/format/score/length. Popularity ordering means anything below the top few hundred is effectively unreachable.

## Technical context

- `db.rs::list_catalog(page, per_page)` — `ORDER BY popularity DESC NULLS LAST, id`, genres filled per-row via `list_catalog_genres`. `catalog_count()` for the header count.
- `commands.rs::get_anime_catalog(page)` returns `CatalogPage { items, has_next_page, total_synced }`, `per_page = 30` hardcoded.
- `anilist_catalog_genres(anilist_id, genre)` PK `(anilist_id, genre)` — no reverse index on `genre` yet.
- `Catalog.tsx` accumulates pages into `items` state; `api.ts` wraps every command.
- Design system: `src/styles.css` custom properties, `.chip`, `.btn`, `.card`, `.grid` etc. No component library.

## Design

### Search: SQL `LIKE`, not FTS5

At 22k rows a full-table `LIKE '%term%' COLLATE NOCASE` scan is single-digit milliseconds — FTS5 adds a shadow-table schema, tokenizer quirks with romaji/CJK titles, and sync-on-write complexity for zero perceptible speed gain at this scale. Decision: `WHERE title LIKE '%' || ?1 || '%' COLLATE NOCASE`. Diacritics: titles are overwhelmingly romaji/English ASCII; no normalization layer (YAGNI — revisit only if real searches miss).

### Backend

New filter struct + one filtered query path (extend, don't duplicate):

```rust
#[derive(Deserialize, Default)]
pub struct CatalogFilter {
    pub search: Option<String>,        // title substring, case-insensitive
    pub genres: Vec<String>,           // AND semantics: title must have ALL selected
    pub format: Option<String>,        // exact match (TV, MOVIE, OVA, ...)
    pub min_score: Option<i64>,        // average_score >= N (0-100)
    pub episodes: Option<String>,      // bucket: "1" | "2-12" | "13-26" | "27+" | "unknown"
}
```

- `db.rs::list_catalog_filtered(page, per_page, &CatalogFilter) -> Vec<CatalogAnime>` and `catalog_count_filtered(&CatalogFilter) -> i64` — same `ORDER BY popularity DESC NULLS LAST, id`, WHERE built from present filters. Genre AND via `id IN (SELECT anilist_id FROM anilist_catalog_genres WHERE genre IN (...) GROUP BY anilist_id HAVING COUNT(DISTINCT genre) = {n})`. Episode buckets map to explicit ranges; `"unknown"` = `episodes IS NULL`.
- Existing `list_catalog`/`catalog_count` become thin wrappers over the filtered versions with `CatalogFilter::default()` (or are replaced outright — implementer's call; no behavior change for existing callers).
- `commands.rs::get_anime_catalog(state, page, filter: Option<CatalogFilter>)` — extend the existing command (Tauri camelCase arg `filter`); `total_synced` in the response stays the **unfiltered** count (header text), add `total_matching: i64` for the filtered result count.
- New command `get_catalog_facets(state) -> CatalogFacets { genres: Vec<String>, formats: Vec<String> }` — `SELECT DISTINCT` from the two tables, alphabetical; drives the filter UI without hardcoding AniList vocabularies.
- Migration in `db.rs::init` (existing `execute_batch` pattern): `CREATE INDEX IF NOT EXISTS idx_catalog_genre ON anilist_catalog_genres(genre)`.

### Frontend (`Catalog.tsx` + `api.ts` + `types.ts`)

Filter bar between `page-head` and the grid, design-system styling only:

- **Search input**: text field, debounced 300ms, fires a page-1 reload. Styled like existing inputs (Settings has form inputs to match).
- **Genre chips**: wrapped row of `.chip`-style toggle buttons from `get_catalog_facets`; selected state uses the existing accent variable. Multi-select, AND semantics (matches backend).
- **Format select** + **min-score select** (e.g. Cualquiera/60%+/70%+/80%+) + **episodes select** (Cualquiera/1/2–12/13–26/27+/?) — native `<select>` styled to the design system.
- **"Limpiar filtros"** text button, visible only when any filter is active.
- Results header: `N resultados` (from `total_matching`) when any filter/search is active.
- Any filter change resets to page 1 and replaces `items`; "Cargar más" keeps appending within the current filter. Empty state: "Sin resultados con estos filtros."
- Facets loaded once on mount (they only change after a sync; acceptable staleness).

## Acceptance criteria (verifiable)

1. `cargo test` passes with new unit tests: `list_catalog_filtered` — title substring case-insensitive match; multi-genre AND (a title with only 1 of 2 selected genres is excluded); score floor; each episode bucket boundary (1, 2, 12, 13, 26, 27, NULL); combined filters; `catalog_count_filtered` agrees with returned row totals across pages.
2. Empty/default filter returns identical results to the pre-change `list_catalog` (ordering included).
3. `npx tsc --noEmit` + `npm run build` pass.
4. Live: searching a known title (e.g. "monster") surfaces it from deep catalog; selecting two genres shows only titles with both; score/episodes filters visibly narrow the grid; "Cargar más" pages within filters; "Limpiar filtros" restores the default popularity view. Search feels instant (<100ms perceived).

## Live verification required

- Screenshot of Catálogo with a search term + at least one genre chip + score filter active, showing coherent results and the `N resultados` count.
- Screenshot of the cleared default state (unchanged from before this task).

## Explicitly out of scope

- FTS5 / fuzzy matching / diacritic folding (revisit only on real missed-search evidence).
- Sorting options other than popularity (score/title/episodes sort could come later).
- Filters in Descubrir (task 3 territory) or persisting filter state across app restarts.
