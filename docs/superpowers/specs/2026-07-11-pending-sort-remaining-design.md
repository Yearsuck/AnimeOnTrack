# Pending — sort by episodes-remaining (ascending/descending)

**Date:** 2026-07-11
**Branch to implement on:** `feat/pending-sort` (from `develop`, after i18n; independent of
tasks #2–#4 — touches only `Pending.tsx` + `db.rs`/`commands.rs list_pending`).
**Task:** #5.

## Problem

The pending queue has no ordering control. Add asc/desc sort by the number of episodes each
series still has to watch, with a control in `Pending.tsx` and support in the `db.rs` query.

## Verified context

- `Pending.tsx` fetches a flat `PendingItem[]` via `listPending()`, then groups by
  `series.title` into per-series blocks. "Episodes remaining" for a series = the size of its
  group (all its unseen episodes; pending = `seen=0` rows).
- `db.rs list_pending` (`:1050`): `SELECT ... FROM episodes e JOIN series s ... WHERE
  e.seen=0 AND s.followed=1 AND s.source_id=?1 ORDER BY s.title, e.added_at DESC`. Returns
  `Vec<(Series, Episode)>`. `commands.rs list_pending` (`:885`) wraps rows into
  `PendingItem { series, episode }`.

## Design

Ordering the *series groups* by their pending count is fundamentally a grouped aggregate, so
the cleanest place for the primary ordering is the SQL query (as the task asks), with the
frontend keeping episodes contiguous per series.

### Backend
Change `db.list_pending` to accept a sort direction and order groups by per-series pending
count, keeping each series' episodes together:
```sql
SELECT ... ,
       COUNT(*) OVER (PARTITION BY s.id) AS remaining
FROM episodes e JOIN series s ON s.id = e.series_id
WHERE e.seen=0 AND s.followed=1 AND s.source_id=?1
ORDER BY remaining <ASC|DESC>, s.title, e.added_at DESC
```
(SQLite bundled here supports window functions.) Add a `sort: PendingSort` param
(`RemainingAsc` | `RemainingDesc`) to `list_pending` (db + command). Default `RemainingAsc`
(fewest-left first — quick wins) if the arg is omitted/None; pick one default and state it.
`remaining` itself need not be returned (grouping already yields the size), but ordering by
it in SQL guarantees groups come out already sorted so the frontend grouping preserves it
(insertion-ordered `Map`).

### Frontend
- `api.ts`: `listPending(sort?: "remaining_asc" | "remaining_desc")`.
- `Pending.tsx`: a small sort control in the page head (two buttons or a `<select>`:
  "Menos episodios primero" / "Más episodios primero"), state drives the `listPending`
  arg; reload on change. The existing `Map` grouping already preserves row order, so groups
  render in the SQL-provided order. All labels via i18n (`pending.sortFewest` /
  `pending.sortMost` / `pending.sortLabel`).

## Acceptance criteria

1. `cargo test` green — add/adjust a `list_pending` unit test asserting group order for
   asc vs desc given series with different pending counts. `npx tsc --noEmit` +
   `npm run build` clean.
2. Toggling the control reorders the series blocks by pending-episode count; episodes within
   a series stay contiguous and in their existing per-series order.
3. Default order is deterministic and documented.
4. New strings in `es.ts` + `en.ts`.

## Live verification (required)

Disable startup `refresh()`, revert before commit. With ≥2 followed series having different
pending counts, screenshot both sort directions and confirm block order flips. No synthetic
OS clicks.

## Out of scope

- Sorting by anything other than remaining count.
- Per-series manual ordering.
