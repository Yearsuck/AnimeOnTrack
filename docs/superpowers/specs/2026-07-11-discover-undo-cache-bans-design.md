# Discover — multi-level undo cache + configurable genre/type bans

**Date:** 2026-07-11
**Branch to implement on:** `feat/discover-undo-bans` (from `develop`, AFTER `feat/i18n-es-en`
and `feat/reversibility` merge — this builds on the i18n catalog AND on
`reclassify_series` from task #2).
**Task:** #3. Two sections, one spec, **two atomic commits** in the branch:
`feat: multi-level swipe undo cache` and `feat: configurable deck genre/type bans`.

## Context (verified against code)

- Descubrir's live deck is the **catalog** deck: `Descubrir.tsx` uses `discoverCatalogCard`
  + `decideCatalogCard` (not the older site `decide_swipe`). Cards are AniList catalog rows.
- **Undo today is single-level:** `AppState.last_swiped_series_id: Mutex<Option<i64>>`;
  `decide_catalog_card` (`commands.rs:1615`) and `decide_swipe` (`:1102`) set it;
  `undo_last_swipe` (`:1110`) takes it and hard-deletes that series row.
- **Why undo deletes the row:** the deck excludes decided titles via
  `db.rs random_catalog_anime_in_genre` →
  `c.id NOT IN (SELECT anilist_id FROM series WHERE anilist_id IS NOT NULL)`. A decided
  card is excluded *because a `series` row with its `anilist_id` exists*. So returning a
  card to the deck requires deleting that row; merely clearing its backlog flags
  (`reclassify none`) is NOT enough — it stays excluded.
- **A card's decision is derivable from its live series row** (no separate storage needed):
  `watched_externally=1` → Seen; `backlog_status='want'` → Want; `'discarded'` → Discard.
- **Deck genre exclusion** is a hardcoded baseline: `commands.rs:1496`
  `EXCLUDED_CATALOG_GENRES = ["Hentai","Ecchi"]`, applied in `discover_catalog_card`
  (`:1527`). **Format** ("tipo") is hardcoded in the query:
  `c.format IN ('TV','MOVIE','OVA','ONA','SPECIAL')`.
- Settings storage is the `settings` key/value table. Per-site keys go through
  `save_mirrors`/`save_genre_list` (site-id-scoped). Bans are a **user preference**, not
  site-specific → store under **global** (un-prefixed) keys.

## Section A — multi-level undo cache (~5)

### Backend

- Replace `AppState.last_swiped_series_id: Mutex<Option<i64>>` with
  `swipe_history: Mutex<VecDeque<i64>>` (series_ids, most-recent front, capped at 5).
  `decide_catalog_card` and `decide_swipe` `push_front` their new `sid` and truncate to 5
  (instead of setting `last_swiped_series_id`).
- `undo_last_swipe` (Ctrl+Z / most-recent undo): `pop_front`, delete that series row. No-op
  when empty (unchanged contract).
- New command `list_swipe_history() -> Vec<SwipeHistoryItem>`: for each id still present in
  the deque, look up the live series row; return `{ series_id, title, poster_url,
  decision }` where `decision` is derived live (`watched_externally`→"seen",
  backlog `want`→"want", `discarded`→"discard", else "none"). Skip ids whose row was
  deleted (keeps the deque self-healing). Ordered most-recent first.
- New command `undo_swipe_entry(series_id)`: delete that specific series row (return it to
  the deck) and remove its id from the deque — multi-level "return card N to the deck",
  not just the most recent.
- **Reclassifying** a past card to a *different* list (Want↔Discard↔Seen without returning
  it to the deck) reuses `reclassify_series` from task #2 (targets `want`/`discarded`/
  `watched`). The history strip then re-reads `list_swipe_history` and the derived decision
  reflects the change. No new command needed for that path.

Note: `swipe_history` is in-memory (session-scoped), same lifetime as the old single-slot
field — undo/history does not persist across app restarts, which matches the existing
"fix my last few misclicks" intent (not an audit log).

### Frontend (`Descubrir.tsx` SwipeView)

Replace the single `canUndo` state + lone ↺ button with a **history strip** below the
swipe actions: up to 5 recent cards as small rows/thumbs (poster + title + a decision
badge ✕/★/✓). Each entry offers: re-classify to Discard/Want/Seen (`reclassifySeries`) and
**return to deck** (`undoSwipeEntry`). Ctrl+Z still calls `undoLastSwipe` (most recent).
After any history action, refetch `list_swipe_history`. All strings via i18n catalog.

New `api.ts` wrappers: `listSwipeHistory()`, `undoSwipeEntry(seriesId)` (plus
`reclassifySeries` already added in task #2).

## Section B — configurable genre + type bans

### Backend

- Global settings helpers in `db.rs` (un-prefixed keys `banned_genres`, `banned_formats`,
  newline-joined like the mirror list): `get_banned_genres`/`set_banned_genres`,
  `get_banned_formats`/`set_banned_formats`. Commands:
  `get_deck_bans() -> { genres: Vec<String>, formats: Vec<String> }`,
  `set_deck_bans(genres, formats)`.
- `discover_catalog_card`: candidate-genre filter becomes
  `EXCLUDED_CATALOG_GENRES ∪ banned_genres` (Hentai/Ecchi stay an always-on baseline; user
  bans are additive). Load banned lists once at the top of the command.
- `random_catalog_anime_in_genre(genre, banned_formats: &[String])`: the allowed format set
  becomes the default whitelist `['TV','MOVIE','OVA','ONA','SPECIAL']` minus any banned
  format, injected into the `IN (...)` clause (build the placeholder list dynamically). If
  every format is banned, return `Ok(None)` (empty deck) rather than an invalid empty
  `IN ()`.
- Also feed banned genres/formats into `discover_swipe_card` if trivial; if the site deck
  is dead code in the live UI, note it and leave it — do not expand scope.

### Frontend (`Descubrir.tsx`)

Add a **"Filtros" sub-view** (third tab alongside Swipe / Listas): two chip groups —
genres (from `distinctCatalogGenres`, needs a small read command or reuse
`getCatalogFacets` if it already returns genres) and formats (fixed
TV/MOVIE/OVA/ONA/SPECIAL). Clicking a chip toggles its banned state; Save persists via
`setDeckBans`. Banned chips render visibly "off". After save, the next `fillQueue` reflects
the new bans (deck refills already happen on decide). All strings via i18n.

New `api.ts` wrappers: `getDeckBans()`, `setDeckBans(genres, formats)`, and a genre-list
source for the chips (`getCatalogFacets` already exists — confirm it returns the genre
list; if not, add `distinctCatalogGenres()`).

## Acceptance criteria (verifiable)

1. `cargo test` green — add unit tests: history push/cap-at-5/pop; `undo_swipe_entry`
   removes the right id; `random_catalog_anime_in_genre` excludes a banned format and
   returns `None` when all formats banned; `discover_catalog_card` never picks a banned
   genre. `npx tsc --noEmit` + `npm run build` clean.
2. Swiping 5+ cards then opening the history strip shows the last 5; Ctrl+Z returns the
   most recent to the deck; `undo_swipe_entry` on an older one returns exactly that one;
   re-classifying a history card moves it between lists (verify in `sqlite3`).
3. Banning a genre → that genre's titles stop appearing in the deck. Banning a format →
   that format stops appearing. Bans persist across app restart (settings table).
4. Hentai/Ecchi remain excluded regardless of user ban list (baseline intact).
5. No scraping triggered by any undo/reclassify/ban action (all local — honors
   [[project-scraping-scope]]).

## Live verification (required, real screenshots)

Disable App.tsx startup `refresh()`; revert before commit. Swipe several catalog cards,
screenshot the history strip, exercise Ctrl+Z + a mid-history return-to-deck + a
re-classify, confirming state via `sqlite3`. Open Filtros, ban a genre and a format, save,
restart the app, confirm bans persisted and the deck respects them. No synthetic OS clicks;
`window.eval()` if needed; passive screenshots.

## Out of scope

- Persisting undo history across restarts.
- Reworking the (apparently unused) site `decide_swipe` deck UI.
