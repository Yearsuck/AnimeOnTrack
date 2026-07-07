# Finished-anime scraper + genre data model

Part 1 of 3 of the "watch-history / Tinder-swipe" feature (see also: `2026-07-07-tinder-swipe-design.md`, `2026-07-07-genre-stats-design.md`). This piece has no UI of its own — it's the scraping + data layer the swipe mode (piece 2) consumes, and that genre stats (piece 3) reads from.

## Problem

The site has no single "browse all finished anime" catalog page. Finished titles only surface through per-genre archives (`/genres/{slug}/`, paginated ~10/page, tens of pages per genre, heavy overlap between genres). A full pre-scrape of the catalog is both impractical (thousands of titles) and risky — CLAUDE.md already documents that bulk/rapid scraping reads as abuse to Cloudflare and gets rate-limited independent of session validity. Everything here must fetch lazily, one page or one detail view at a time, paced by actual user actions (never a background bulk job).

## Confirmed site markup

Captured live via browser (2026-07-07), same rigor as the existing `.bsx`/`.eplister` selectors in `adapter/animeytx.rs`:

- **Genre archive**: `https://wwv.animeytx.net/genres/{slug}/` and `.../page/N/`. Cards are `.bsx` (identical structure to the airing-grid cards already parsed by `parse_airing`), each containing:
  - `.status.Completed` div, text "Finalizado" — **presence of this exact class is the finished/not-finished signal**; cards without it are not finished and must be skipped. (Ongoing cards were not observed with a `.status` div at all in sampled genre pages — absence, not a different class value, is what "not finished" looks like.)
  - `.typez` div — text is the type badge (`TV`, `OVA`, `Special`, `Movie`, `Donghua`, `Yuri`, etc. — inconsistent vocabulary on the live site, store as free text, don't validate against an enum).
  - `<a title="...">` → title + episode URL, `<img src>` → poster thumbnail (same shape as existing `.bsx` parsing).
  - Pagination: `.pagination a.page-numbers`, last page number is stable per genre (e.g. Aventura had 24 pages at capture time) — always re-check rather than caching a page count.
- **Genre discovery**: any page has `a[href*="/genres/"]` links (confirmed on the homepage). Slug is the URL segment, display name is the link text. No dedicated "all genres" index page exists — scrape this from the homepage.
- **Series detail page** (`/tv/{slug}/`): full genre list is `.genxed a` (href `/genres/{slug}/`, text = display name) — this is the *only* place a series' complete genre set is available; listing cards only imply "the genre we searched under."
- **Episode list**: unchanged, reuses the existing `parse_series` adapter method and `.eplister` selectors.

## Data model changes (`src-tauri/src/db.rs`)

1. `series` table: add nullable columns `backlog_status TEXT` (`NULL` | `'want'` | `'discarded'`) and `kind TEXT` (the type badge — "TV"/"OVA"/"Special"/"Movie"/etc, free text same as the adapter's `FinishedCard.kind`). `kind` is populated from the detail page's `Tipo:` field (`.spe` block, more authoritative than the listing card) whenever a detail fetch happens (want/seen), falling back to the listing card's `.typez` text for discard-only rows that never get a detail fetch. Added because piece 3 (genre stats) needs a per-type breakdown and this is the only place that data naturally lands.
   - `NULL` — normal (airing-catalog or already-followed) row, unchanged meaning.
   - `'want'` — swiped "quiero ver", not yet started: has a series row + genres, but no episodes, `followed=false`.
   - `'discarded'` — swiped no, exists purely to dedupe future swipe decks.
   - "Already watched" and "currently tracking" need no new state: both are just `followed=true, is_airing=false` — indistinguishable in the schema and that's fine, since both mean "this show's episodes are being tracked in the DB." Historic watches get every episode inserted with `seen=true` in one shot at swipe time instead of episode-by-episode over time.
2. New table `series_genres(series_id INTEGER, genre TEXT, PRIMARY KEY(series_id, genre))` — full genre tags per series, populated only when the detail page is fetched (i.e. on "want" or "seen" decisions, never on "discard" or plain card display).
3. Reuse the existing `settings` key/value table (same pattern as `mirror_urls`) to cache the discovered genre slug→name list under a new key `genre_list`, so it isn't re-scraped every session.

No migration framework exists yet in this codebase (`init_schema` uses `CREATE TABLE IF NOT EXISTS`); follow the same pattern — add the column/table to `init_schema`, guarded so it's a no-op on DBs that already have it (`ALTER TABLE ... ADD COLUMN` wrapped in a check, matching whatever idiom the codebase already uses for schema evolution, since `series`/`episodes` already shipped before this change).

## Adapter changes (`src-tauri/src/adapter/`)

Extend `SiteAdapter` trait with:
- `genre_list_url(&self, base_url: &str) -> String` — homepage, since that's where genre links were confirmed.
- `parse_genre_list(&self, html: &str) -> Result<Vec<(String, String)>>` — `(slug, display_name)` pairs from `a[href*="/genres/"]`.
- `genre_page_url(&self, base_url: &str, genre_slug: &str, page: u32) -> String` — `page=1` maps to the bare `/genres/{slug}/` (no `/page/1/` suffix — confirmed that's how the site's own pagination links behave).
- `parse_finished_page(&self, html: &str) -> Result<Vec<FinishedCard>>` — new struct `FinishedCard { title, url, poster_url, kind: String }`, built from `.bsx` cards **filtered to only those with `.status.Completed` present**.
- `parse_series_detail(&self, html: &str) -> Result<SeriesDetail>` — new struct `SeriesDetail { genres: Vec<String>, synopsis: Option<String> }`, from `.genxed a` (+ whatever synopsis selector — needs one more live-markup check during implementation, not yet captured).

`parse_series` (episode list) is reused unchanged.

Add fixtures under `src-tauri/tests/fixtures/` for a genre-listing page and a series-detail page, same convention as the existing `series.html` fixture.

## Commands (`src-tauri/src/commands.rs`)

- `discover_swipe_card(state) -> Result<Option<SwipeCard>, String>`: picks a random cached-or-fresh (genre, page) pair, scrapes that one page if not already buffered in memory for this session, filters out any card whose URL already has a `series` row (any `backlog_status`, or `followed`), shuffles, pops one. Returns `None` if the in-memory buffer is exhausted and the freshly-scraped replacement page is *also* fully filtered (caller re-invokes — this is a normal "everything on this page was already decided" case, not an error). The in-memory per-session buffer (keyed by genre+page) is what keeps this to ~1 HTTP fetch per 10 swipes rather than per swipe.
- `decide_swipe(state, app, series_url: String, decision: SwipeDecision) -> Result<(), String>` where `SwipeDecision` is `Seen | Want | Discard`:
  - `Discard`: upsert `series` from the card data already in hand (no fetch), `backlog_status='discarded'`.
  - `Want`: fetch detail page → upsert `series` (`backlog_status='want'`, `followed=false`) + `series_genres`.
  - `Seen`: fetch detail page (genres) + episode-list page (reusing the existing scrape path SeriesDetail's follow flow already uses) → upsert `series` (`followed=true`, `is_airing=false`) + `series_genres` + insert every episode with `seen=true`.
- `start_watching(state, app, series_id: i64) -> Result<(), String>` (used by piece 2's backlog view): for a `backlog_status='want'` row, fetch its episode list and insert all episodes with `seen=false`, set `followed=true`, clear `backlog_status`. From then on it's an ordinary followed series — `refresh()` already scans all followed rows regardless of `is_airing`, so no changes needed there.

All new scraping goes through the same `scrape_via_mirrors` + `scraper_engine::fetch_html` path already used everywhere else — no new fetch mechanism.

## Testing

- Adapter unit tests (fixtures) for `parse_finished_page` (including the "no `.status.Completed` → excluded" case) and `parse_series_detail`.
- `db.rs` unit tests for the `series_genres` upsert path and for `backlog_status` transitions (`want` → `start_watching` → `followed`, `discarded` staying excluded from a re-run of `discover_swipe_card`'s filter).
- No test for the random-batching logic's *randomness* itself (not meaningful to assert); do test that a fully-decided page causes `discover_swipe_card` to fall through to a fresh page rather than returning stale/duplicate cards.

## Explicitly out of scope here

- Swipe UI, decision buttons, backlog list screen — piece 2.
- Genre stats aggregation/charts — piece 3.
- No content filtering (e.g. excluding the "Hentai" genre) — user decision, left unfiltered.
