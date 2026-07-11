# Catalog — multi-select, batch actions, and per-card info icon

**Date:** 2026-07-11
**Branch to implement on:** `feat/catalog-batch` (from `develop`, after i18n and task #2
`feat/reversibility`). Touches `Catalog.tsx`, `api.ts`; no new backend command (reuses
`decide_catalog_card` + `link_catalog_series`).
**Task:** #6.

## Problem

From the Catálogo grid the user can only click a card, which opens its AniList page in the
browser. We want to: (1) select cards (one click selects; multi-select many); (2) batch-set
all selected to **Quiero ver** or **Ya visto**; (3) a per-card info icon (ℹ) that goes to
the AniList page (taking over the old whole-card-opens-AniList behavior).

## Verified context

- `Catalog.tsx` cards currently do `onClick={() => openEpisode(a.url)}` (opens AniList URL).
  Each `CatalogAnime` has `id, title, url, cover_url, format, genres, episodes,
  average_score` — everything `decide_catalog_card` needs
  (`anilistId, title, url, posterUrl, genres, format, decision`).
- `decide_catalog_card` (`commands.rs:1577`) is **local, no scrape**: it upserts a synthetic
  `anilist-{id}` series row and sets backlog/`watched_externally`. `Want`→backlog 'want';
  `Seen`→`watched_externally=1`, followed cleared.
- Site linking is a **real scrape** and must be serialized/on-demand — see
  [[project-scraping-scope]] and `Descubrir.tsx`'s `useLinkQueue` (single promise chain,
  max concurrency 1).

## Design

### Selection model (`Catalog.tsx`)
- `selected: Set<number>` (anilist ids). A card click **toggles selection** (no longer opens
  AniList). Selected cards render with a visible selected state (ring/checkmark overlay).
- A batch action bar appears when `selected.size > 0`: shows the count, **Quiero ver**,
  **Ya visto**, and **Deseleccionar todo**. Keep it out of the way (sticky footer or top of
  grid), design-system styled.

### Per-card info icon
- Add a small ℹ button in each card's corner: `onClick` stops propagation (so it doesn't
  toggle selection) and calls `openEpisode(a.url)` (AniList page). This preserves the old
  "see more info" affordance now that the card body selects instead of opening. Title/aria
  via i18n (`catalog.infoTitle`).
  - Note: `openEpisode` currently opens the user's browser; task #7 changes *where* links
    open app-wide — this card just calls the same `openEpisode`, inheriting #7's behavior
    automatically. No special handling here.

### Batch actions (respecting scrape scope)
`api.ts` gains `decideCatalogCardBatch`-style usage — but implement batching in the
frontend to keep control over serialization; no new backend command:
- **Quiero ver:** for each selected id, call `decideCatalogCard({..., decision:"Want"})`.
  This is fully local — may run with modest concurrency (e.g. `Promise.all` in small
  chunks) since it never scrapes. After completion, clear selection and show a toast/inline
  confirmation (`catalog.batchWantDone`, with count).
- **Ya visto:** for each selected id, `decideCatalogCard({..., decision:"Seen"})` (local),
  **then** optionally fire the site link — which DOES scrape — **strictly serialized**
  through a single promise chain (reuse the `useLinkQueue` pattern from `Descubrir.tsx`:
  concurrency 1). Marking 50 as "ya visto" must enqueue at most one link scrape at a time,
  never a burst. Show progress ("Enlazando X/N…"). The `decide` calls themselves may batch;
  only the **link** step serializes.
  - Decision point stated explicitly: linking on catalog "Ya visto" follows the
    discover-site-link-search precedent (the task calls for it), but is gated behind the
    serialized queue so a large batch can't hammer the Cloudflare-fronted site. If the queue
    is long the UI stays responsive; links complete in the background.
- Decided cards are excluded from the deck (via `anilist_id`), and remain in the catalog
  grid (catalog lists all synced AniList rows regardless of decision) — so after a batch the
  grid is unchanged except selection clears. That's fine/expected.

To source card data for a selected id without re-fetching, keep a lookup from the currently
loaded `items` (all selected ids are cards currently rendered, since selection is per
visible card). Guard against a selected id that scrolled out — snapshot the needed fields
into the `selected` structure at toggle time, or key selection to the full card object.

### i18n
All new strings (`catalog.selectedCount`, `catalog.batchWant`, `catalog.batchSeen`,
`catalog.deselectAll`, `catalog.infoTitle`, `catalog.linking`, `catalog.batchWantDone`,
`catalog.batchSeenDone`) added to `es.ts` + `en.ts`.

## Acceptance criteria

1. `npx tsc --noEmit` + `npm run build` clean. `cargo test` unaffected (no backend change);
   run it anyway to confirm green.
2. Clicking a card toggles selection (does not open AniList). The ℹ icon opens the AniList
   page and does not change selection.
3. Selecting several cards + **Quiero ver** puts all of them in the Want backlog
   (verify in `sqlite3`: `backlog_status='want'`). None trigger a scrape.
4. Selecting several + **Ya visto** sets `watched_externally=1` for all; any resulting site
   links run **one at a time** (verify at most one scraper WebView2 window exists at any
   moment — never N in parallel).
5. New strings in both catalogs.

## Live verification (required)

Disable startup `refresh()`, revert before commit. Select multiple catalog cards, run each
batch action, screenshot the selection UI and confirm DB state via `sqlite3`. For "Ya visto"
on several items, observe (screenshot/log) that link scrapes serialize — no parallel
scraper windows. No synthetic OS clicks; `window.eval()` if needed.

## Out of scope

- Batch discard/follow (only Want + Seen requested).
- Server-side batch command (frontend orchestration is enough and keeps serialization
  control in the UI).
