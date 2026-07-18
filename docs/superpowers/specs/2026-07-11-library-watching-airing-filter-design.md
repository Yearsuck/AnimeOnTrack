# Library — "Viendo" includes caught-up airing series + airing filter

**Date:** 2026-07-11
**Branch to implement on:** `feat/library-watching-filter` (from `develop`, AFTER task #2
`feat/reversibility` merges — both edit `Library.tsx`; sequencing avoids a conflict).
**Task:** #4. Frontend-only (`src/views/Library.tsx`); no backend change needed.

## Problem

An airing series whose every currently-available episode is watched ("al día") shows up
under **Completadas**, which is wrong — it isn't finished, more episodes are coming. It
should stay in **Viendo**. Completadas must mean *the series is finished (not airing) AND
everything is watched*. Also: add a filter to Viendo — **En emisión / Finalizados / Todos**
(default Todos).

## Verified context

`Library.tsx` derives status client-side (no stored field), currently:
```ts
function statusOf(it): Status {
  if (it.total_episodes > 0 && it.seen_episodes === it.total_episodes) return "completed";
  if (it.seen_episodes > 0) return "watching";
  return "plan";
}
```
The `completed` branch ignores airing status → the bug. `LibraryItem.series` already
carries `is_airing` (verified: `db.rs list_library` selects `s.is_airing`,
`row_to_series` maps it, TS `Series.is_airing` exists). So the fix is data we already have.

## Design

### Fixed status derivation
```ts
function statusOf(it): Status {
  const allSeen = it.total_episodes > 0 && it.seen_episodes === it.total_episodes;
  // Completed = FINISHED series with everything watched.
  if (allSeen && !it.series.is_airing) return "completed";
  // Watching = anything you've started (>0 seen) that isn't completed —
  // this now includes an airing series you're caught up on (allSeen && is_airing).
  if (it.seen_episodes > 0) return "watching";
  return "plan";
}
```
`plan` (0 seen) unchanged. A finished series partway watched stays `watching` (correct).

### Airing filter on the Viendo section
Add a 3-way control (segmented buttons or a `<select>`, matching the existing design
system) scoped to the **watching** list only: **Todos** (default) / **En emisión**
(`it.series.is_airing`) / **Finalizados** (`!it.series.is_airing`). State lives in
`Library`, applied when computing the `watching` array (before the existing
`byRecentWatched` sort). Plan/Completed sections are unaffected. All labels via the i18n
catalog (new keys `library.filterAll` / `library.filterAiring` / `library.filterFinished`).

The count in the Viendo section header reflects the filtered subset. When the filter yields
zero but there are watching items, show a small inline "nothing matches this filter" note
rather than hiding the whole section (so the control stays reachable to reset it).

## Acceptance criteria

1. `npx tsc --noEmit` + `npm run build` clean. No backend/`cargo` change.
2. A followed airing series with `seen == total > 0` appears under **Viendo**, not
   Completadas. A finished (`is_airing=false`) series with `seen == total` appears under
   **Completadas**.
3. The Viendo filter switches among En emisión / Finalizados / Todos; default is Todos.
4. Plan-to-watch and Completadas sections behave exactly as before.
5. All new strings are in `es.ts` + `en.ts`.

## Live verification (required)

Disable App.tsx startup `refresh()`, revert before commit. Needs DB state with (a) a
followed airing series fully caught up and (b) a followed finished series fully watched —
create/adjust via `sqlite3 %APPDATA%\com.ernes.aot-scaffold\animeontrack.sqlite`
(`UPDATE series SET is_airing=... ; UPDATE episodes SET seen=1 ...`) if the live DB lacks
them. Screenshot Library showing the airing-caught-up series under Viendo and the finished
one under Completadas; screenshot the three filter states. No synthetic OS clicks;
`window.eval()` if needed.

## Out of scope

- Backend/query changes (status stays client-derived).
- Using `next_episode_at`/`site_episode_count` as extra finished/airing signals — `is_airing`
  is sufficient and authoritative (scan-owned). Note them only if `is_airing` proves
  unreliable in live data.
