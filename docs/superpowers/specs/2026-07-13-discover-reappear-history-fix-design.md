# Discover — reappearing cards + stuck history rows + rename "Deshacer"

**Date:** 2026-07-13
**Branch to implement on:** `feat/discover-reappear-history-fix`
**Type:** BUG (reappearance + stuck history) + clarity (rename). Backend + frontend + i18n.
**Status:** approved (autonomous batch)

## Problem

1. **Cards already classified keep reappearing** in the swipe deck.
2. **Some cards get stuck in the "Últimas clasificadas" history strip** (appear duplicated /
   won't clear).
3. **"Deshacer"** is the wrong word for what the undo does (it returns the card to the deck /
   deletes the classification, it isn't a generic undo).

## Root causes (evidence, not guesses)

### Reappearance — async-persistence race + client dedup gap

- The deck excludes a decided card only once it is **persisted** to `series`
  (`discover_catalog_card` → `random_catalog_anime_in_genre` filters by `anilist_id NOT IN
  series` and by normalized `engaged_series_titles`; `commands.rs:1847-1871`).
- But `decide()` in `Descubrir.tsx` (L231-276) fires `decideCatalogCard(...)` as `decidePromise`
  and does **not** await it before advancing: after a 160ms timeout it calls `popNext()` (which,
  when the queue is at/below `REFILL_THRESHOLD`, calls `fillQueue()` — L202-204). `fillQueue`
  (L168-188) issues **concurrent** `discoverCatalogCard()` calls whose dedup `seen` set is built
  from **only the current queue + the on-screen card** (L175-177). The just-swiped card is
  neither, and its `series` row may not be written yet (the upsert is still in-flight) → a
  concurrent picker re-serves the exact card the user just swiped. It comes back within seconds.
- Ruled out: a url that fails to parse an AniList id would also never persist, but **all 22420
  `anilist_catalog.url` rows match `anilist.co/anime/N`** (verified) and `decideCatalogCard` is
  only skipped when `anilistIdFromUrl` is null, which never happens for catalog cards. So the
  race is the sole cause.
- Within one `fillQueue` round, two concurrent pickers can also return the **same** card and both
  pass the filter (the `seen` set doesn't include same-batch siblings) — a secondary duplicate
  source.

### Stuck history — `push_history` doesn't dedup sids

- `swipe_history` is an in-memory `VecDeque<i64>` (cap 5). `push_history` (`commands.rs:70-73`)
  does `push_front(sid); truncate(CAP)` with **no dedup**. `decide_catalog_card` upserts by the
  `anilist-{id}` slug (`ON CONFLICT` → the **same** sid) and pushes it every time. So a card
  re-served by the race above and swiped again puts the **same sid twice** in the deque →
  `list_swipe_history` renders it twice → looks stuck/duplicated. (`list_swipe_history` already
  self-heals *deleted* rows by skipping them, so the only persistent artifact is the duplicate.)

### "Deshacer" naming

- Top button + hint use `discover.undoTitle` = "Deshacer (Ctrl+Z)" and `discover.hint` "… Ctrl+Z
  Deshacer". The action pops the most-recent decision and **hard-deletes** the row so the card
  returns to the deck (`undo_last_swipe`, `commands.rs:1226`). The per-row strip action is already
  correctly named `discover.returnToDeck` = "Devolver al mazo". Only the top/undo wording is off.

## Design

### Fix 1 — client-side decided-set (frontend, no scraping)

Add a session `decidedUrlsRef = useRef<Set<string>>(new Set())`. In `decide()`, **synchronously**
`decidedUrlsRef.current.add(activeCard.url)` before anything async. In `fillQueue`, seed the
`seen` set with `decidedUrlsRef.current` as well as the queue + on-screen card, and dedup
same-batch results against each other (add each accepted card's url to `seen` as it's accepted).
This guarantees a swiped card can't be re-served regardless of when its DB row lands.

Reverse it where the card legitimately returns to the deck:
- `undo` (undoLastSwipe) — remove the just-undone card's url from `decidedUrlsRef`. Track the last
  decided url (e.g. `lastDecidedUrlRef`) so undo knows which to clear.
- `returnToDeck` (undoSwipeEntry) — remove `item`'s url; the strip item carries `series_id`, not a
  url, so either (a) add `url` to `SwipeHistoryItem`/`SwipeHistoryRow` and clear by url, or (b)
  clear by matching — simplest is to extend `SwipeHistoryRow`/`get_series_for_history` to return
  the row's `url` and include it in `SwipeHistoryItem`. Prefer (b): add `url` to the history item
  so returnToDeck can `decidedUrlsRef.current.delete(item.url)`.

### Fix 2 — dedup `push_history` (backend)

Change `push_history` to move-to-front-with-dedup: if `sid` already present, remove it first, then
`push_front`, then `truncate(CAP)`. Add a unit test asserting pushing the same sid twice yields a
single front entry, and that a distinct sid still evicts past the cap. This kills the duplicate
strip rows even independent of Fix 1.

### Fix 3 — rename

- `discover.undoTitle`: "Deshacer (Ctrl+Z)" → **"Volver a decidir (Ctrl+Z)"** /
  "Reconsider (Ctrl+Z)".
- `discover.hint`: replace the trailing "Ctrl+Z Deshacer" with "Ctrl+Z Volver a decidir" /
  "Ctrl+Z Reconsider".
- Keep `discover.returnToDeck` as is ("Devolver al mazo" / "Return to deck").
- Both `es.ts` and `en.ts`.

## Acceptance criteria

- Rapidly swiping (arrow keys / drag) a run of cards never re-shows a card just decided, even
  under fast repeated input (the previous failure mode). Verify by swiping ~15 cards quickly and
  confirming no repeat title within the run.
- The "Últimas clasificadas" strip never shows the same title twice.
- `push_history` unit test passes; `cargo test` green; `npx tsc --noEmit` and `npm run build`
  clean.
- The undo control reads "Volver a decidir" (not "Deshacer") in both languages; the per-row action
  still reads "Devolver al mazo".

## Live verification (user)

Relaunch: swipe a batch fast, confirm no reappearance and no duplicate strip rows; confirm the
renamed control. Tauri window not tool-reachable; a Chrome harness can preview the strip/labels
markup but not the swipe/persistence race.
