# Biblioteca — real filters (status, type, genre)

**Date:** 2026-07-13
**Branch to implement on:** `feat/library-filters`
**Type:** feature. Small backend (enrich payload) + frontend + i18n.
**Status:** approved (autonomous batch)

## Problem

`Library.tsx` only filters by free-text title and has an airing seg (En emisión/Finalizados/Todas)
scoped to the "Viendo" section. Add real filters: by **estado** (Viendo/Pendiente/Completada), by
**tipo/formato**, and by **género** — theme-aware, consistent with the design system.

## What exists (verified)

- `Library.tsx`: `listLibrary()` → `LibraryItem[]`; sections Watching/Plan/Completed derived by
  `statusOf(it)`; text filter `filtered` (L303-306); airing seg for Viendo (L365-387).
- `LibraryItem` (`models.rs`) carries `series`, counts, `watched_externally`, `next_episode`,
  `last_watched_at` — but **no genres and no kind**.
- `series.kind` column exists (`db.rs:223`; `set_kind`). Live distinct kinds among library rows are
  **messy**: `TV`(562), `MOVIE`(25), `Pelicula`(8), `OVA`(8), `ONA`(7), `SPECIAL`(4)/`Special`(2),
  plus site quality tags `4K`(11), `Blu-Ray`(3), `Resubido`(2), `Sin Censura`(5), `Yaoi`(1), and
  NULL(1).
- Genres live in `series_genres(series_id, genre)`; `list_series_genres(series_id)` exists (per-row).

## Design

### Backend — enrich `list_library` (avoid N+1)

Add to `LibraryItem`: `#[serde(default)] kind: Option<String>` and `#[serde(default)] genres:
Vec<String>`.
- `kind`: add `s.kind` to the existing `list_library` SELECT (already grouping by `s.id`).
- `genres`: one extra bulk query `SELECT series_id, genre FROM series_genres WHERE series_id IN
  (<the library ids>) ORDER BY genre`, grouped into a `HashMap<i64, Vec<String>>` in Rust and
  attached. (Do it in `list_library` after fetching the rows so it's a single extra statement, not
  one-per-series.)
- Update the `list_library` roundtrip/unit test if one asserts the struct shape.

### Frontend — filter bar + light kind normalization

- New filter bar above the sections with three controls (design-system `select`/`seg`,
  theme-aware):
  - **Estado**: Todas | Viendo | Pendiente | Completada. When a specific status is chosen, render
    only that section (others hidden). Default Todas (all sections, current behavior).
  - **Tipo**: Todos + the normalized kind set. Normalize in the frontend: `Pelicula`→`MOVIE`,
    `Special`→`SPECIAL` (case-fold), and bucket the non-type site tags
    (`4K`/`Blu-Ray`/`Resubido`/`Sin Censura`/`Yaoi`/null) under an "Otros" option. Build the option
    list from the normalized kinds actually present.
  - **Género**: Todos + the union of `item.genres` across the library, sorted. Single-select.
- Apply all three (plus the existing title query and the Viendo airing seg) to `filtered` before
  the sections split. Genre match = `item.genres` includes the selected genre; type match =
  normalized kind === selected (or "Otros" bucket).
- Keep the airing seg (it further scopes Viendo). `seriesCount` reflects the post-filter count.
- Empty state: when filters exclude everything, show `common.noResults` (already used).

### i18n (es.ts + en.ts)

- `library.filterStatus` = "Estado" / "Status"; `library.statusAll` = "Todas" / "All";
  `library.statusWatching`/`statusPlan`/`statusCompleted` (reuse existing `library.watching`/
  `planToWatch`/`completed` where wording matches, else add).
- `library.filterType` = "Tipo" / "Type"; `library.typeAll` = "Todos" / "All";
  `library.typeOther` = "Otros" / "Other".
- `library.filterGenre` = "Género" / "Genre"; `library.genreAll` = "Todos" / "All".
- Both catalogs; missing key → `tsc` fails.

## Acceptance criteria

- `list_library` returns `kind` and `genres` per item (verify via a quick DB-backed test or by
  logging shape).
- Library shows Estado + Tipo + Género filters; each narrows the visible cards correctly and
  composes with the title search and the Viendo airing seg.
- Selecting a género shows only series carrying it; a tipo shows only that normalized format; an
  estado shows only that section.
- `cargo test` green; `npx tsc --noEmit`; `npm run build` clean. No scraping.

## Live verification (user) / harness

Port the Library markup + the new filter bar to a loopback HTML harness, screenshot in Chrome in
both themes to confirm the controls fit the design system before/after porting to React. Real data
behavior confirmed on relaunch.
