# Descubrir → automatic site search + link

Depends on `2026-07-10-discover-from-catalog-only-design.md` (task 3): the swipe deck is fed exclusively from the local AniList catalog, and `decide_catalog_card` persists a **synthetic** `series` row (slug `anilist-{id}`, AniList URL, no episodes, no real cover).

## Problem

A synthetic row is a dead end. "Quiero ver" and "Ya la vi" produce a title the app can never track: no site URL, so no episode list, no new-episode detection, no playable link, and the poster is AniList's CDN image rather than the site's. The user's backlog fills with rows that Biblioteca/Pendientes cannot act on.

## Goal

When the user decides `Want` or `Seen` on a catalog card, the app searches the configured site (`sources.base_url`, with mirror fallback) for that title, and — on a confident match — rewrites the row into a real, fully-tracked series: real site URL, real slug, scraped episode list, site genres/kind, and (for followed series) the site cover fetched through the existing one-at-a-time path.

## Technical context

- **Cloudflare**: all site HTML must come through `scraper_engine::fetch_html` (WebView2). Never `reqwest` against the site. Global `SCRAPE_PERMITS` semaphore caps 2 concurrent scraper windows — a search must take a permit like any other fetch.
- `commands.rs::scrape_via_mirrors(app, mirrors, path, parse)` already handles "mirror failed **or** parsed empty → try next mirror" and returns `(ScrapeResult, Vec<T>, base_url_that_worked)`. Reuse it verbatim.
- `SiteAdapter` (`adapter/mod.rs`) has `airing_url`/`parse_airing`/`parse_series`/`genre_list_url`/`parse_genre_list`/`genre_page_url`/`parse_finished_page`/`parse_pagination_last_page`/`parse_series_detail`. No search method yet.
- `AnimeytxAdapter` parses DooPlay markup; listing cards are `.bsx` (title in anchor `title` attr or `.tt`, poster `img[data-src|src]`), handled by the shared `card_basics` helper.
- `decide_swipe` (site path) already knows how to persist a scraped card + its episodes; `refresh()` owns cover fetching and genre backfill. Reuse, don't reimplement.
- `models::FinishedCard { title, url, poster_url, kind, matched_genre }`.

## Design

### 1. `SiteAdapter::search_url` + `parse_search_results`

```rust
/// Search-results URL for a free-text query (URL-encoded by the impl).
fn search_url(&self, base_url: &str, query: &str) -> String;
/// Cards from a search-results page. Same card shape as a genre listing.
fn parse_search_results(&self, html: &str) -> Result<Vec<FinishedCard>>;
```

DooPlay serves search at `{base}/?s={urlencoded}`. **The result-card markup must be confirmed against real captured HTML, not assumed** — it is probably `.result-item` (DooPlay's search template), *not* the `.bsx` used on archive pages. The implementer must:

1. Use the WebView2 engine (a throwaway `fetch_html` call from a scratch binary/test, or the running app's dev console) to capture one real search page for a query with hits and one with zero hits.
2. Save both as fixtures under `src-tauri/tests/fixtures/` (e.g. `animeytx_search_hits.html`, `animeytx_search_empty.html`).
3. Write `parse_search_results` against those fixtures. If cards turn out to be `.bsx`, reuse `card_basics`; if `.result-item`, add a sibling helper — don't contort one function to serve both.

Zero results must parse to `Ok(vec![])`, which `scrape_via_mirrors` treats as "this mirror didn't work, try the next" — a real risk of masking a genuine no-hits answer. Therefore **`search_site` must not use `scrape_via_mirrors`' empty-means-failure semantics**: call it with a parse closure that returns a one-element wrapper (`Vec<SearchOutcome>` where `SearchOutcome { cards: Vec<FinishedCard> }`), so "page parsed, zero cards" is a successful non-empty result and only a genuinely unparseable/wrong-site page falls through. Document this at the call site.

### 2. Title matching (`src-tauri/src/matching.rs`, pure + unit-testable)

AniList gives romaji and English titles; the site uses Spanish/romaji with noise (`"Sub Español"`, season suffixes, punctuation). Matching is the risky part, so it lives in its own module with no I/O.

```rust
pub struct TitleCandidate<'a> { pub title: &'a str, pub url: &'a str }
pub struct MatchResult { pub index: usize, pub score: f64 }
/// Best candidate for any of `queries` (romaji + english), or None if
/// nothing clears MATCH_THRESHOLD.
pub fn best_match(queries: &[&str], candidates: &[TitleCandidate]) -> Option<MatchResult>;
```

- **Normalize** both sides: lowercase, strip accents (NFD + drop combining marks), collapse punctuation/whitespace to single spaces, strip a trailing noise set (`sub español`, `latino`, `castellano`, `hd`, `online`) and leading/trailing articles-free — keep this list short and explicit.
- **Score**: token-set Jaccard × 0.6 + normalized Levenshtein ratio × 0.4, computed against **each** query (romaji, english) and take the max. Implement Levenshtein in-module (a ~20-line DP; the `strsim` crate is also acceptable — implementer's call, but justify a new dep).
- `MATCH_THRESHOLD = 0.72`. Below it → no match. This number is a guess; the acceptance criteria below pin it against real titles, and the implementer **must adjust it based on the fixture-driven test outcomes** rather than treating it as sacred.
- Exact normalized equality short-circuits to score `1.0`.

Also add `CatalogAnime.title_romaji` / `title_english` — currently `From<MediaEntry>` collapses to one `title` (english → romaji fallback), throwing away the other. Persist both (`ensure_column` on `anilist_catalog`: `title_romaji TEXT`, `title_english TEXT`; keep `title` as the display value). Matching against both roughly doubles hit rate on titles the site lists under romaji.

**Re-sync note**: existing 22k rows have no romaji/english. The columns backfill on the next sync; until then `best_match` falls back to the single `title`. Do **not** force a full re-sync — the incremental path (task 1) plus a `NULL`-tolerant query is enough. Add a note in the Catálogo UI? No — out of scope, keep it silent.

### 3. `link_catalog_series` command (the actual work)

```rust
#[tauri::command]
pub async fn link_catalog_series(app: AppHandle, state: State<'_, AppState>, series_id: i64)
    -> Result<LinkOutcome, String>
```

`LinkOutcome` = `Linked { url, episodes: i64 }` | `NoMatch` | `AlreadyLinked` (serde-tagged enum, so the UI can distinguish).

Steps:
1. Load the `series` row; bail `AlreadyLinked` if its slug isn't `anilist-%`.
2. Read `title_romaji`/`title_english`/`title` from `anilist_catalog` for that id.
3. `search_site(app, mirrors, romaji_or_title)` → candidates. If `best_match` fails and an english title exists **and differs**, do a **second** search with it (one extra scrape, only on failure — not two searches every time).
4. On match: `fetch_html` the matched series URL, `parse_series` → episodes, `parse_series_detail` → genres + kind.
5. Update the row **in place** (keep `id`, `followed`, `backlog_status`, `watched_externally` — don't delete/recreate, that would break `last_swiped_series_id` and any FK): set `slug` = site slug, `url` = site URL, `kind`, `cover_url` = site poster URL (remote; the existing cover-fetch path replaces it with a `data:` URI on the next `refresh()` if followed), replace `series_genres` with the site's, insert episodes.
   - **Slug collision**: the site series may already exist as a row (e.g. it's on the airing list). Then *merge*: keep the existing row as canonical, move `followed`/`backlog_status`/`watched_externally` onto it (logical OR / most-specific-wins), delete the synthetic row, and return its id. A `UNIQUE(source_id, slug)` violation must never surface as an error.
6. `Seen` semantics: if the row was decided `Seen` (task 3's `watched_externally=1`), mark **all** scraped episodes seen via the existing `set_seen_cascade` on the highest episode number — that's exactly what the gap-free watching invariant expects, and it's the whole point of linking a "ya la vi" title.

**Concurrency/politeness**: `link_catalog_series` is one-at-a-time per call and takes `SCRAPE_PERMITS` implicitly through `fetch_html`. It performs at most 3 scrapes (search, [search #2], series page + detail page — reuse one `fetch_html` of the series page for both parses; DooPlay serves episodes and detail on the same page). No batching, no background sweep of the existing backlog.

### 4. Frontend (`Descubrir.tsx`)

`decide()` currently fires `decideCatalogCard` and immediately pops the next card. Linking takes seconds (a real scrape), so it must **not** block the swipe:

- After `decideCatalogCard` resolves for `Want`/`Seen` (not `Discard`), fire `linkCatalogSeries(seriesId)` **without awaiting** it before popping the next card.
- `decide_catalog_card` must therefore return the new `series_id` (today it returns `()`); `api.ts`/`types.ts` updated accordingly.
- Show a small, non-blocking status line under the swipe actions: "Buscando *Título* en la web…" → "✓ Enlazado (12 episodios)" / "No encontrado en el sitio". Keep a tiny queue of in-flight links (`useRef<Map<id, status>>`), render the most recent 1–2. Rapid swiping must not spawn unbounded parallel scrapes: serialize the links through a simple in-flight promise chain on the frontend (one at a time), since the backend semaphore would otherwise queue them behind the deck's own needs.
- `Discard` never triggers a search.

A `NoMatch` leaves the synthetic row exactly as it is today — the title is still in the backlog, just unlinked. Show it in Descubrir's "Listas" with a subtle "sin enlazar" marker and a manual "Buscar en la web" retry button (calls the same command). That is the escape hatch for the matcher's inevitable misses.

## Acceptance criteria (verifiable)

1. `cargo test` passes with new tests:
   - `matching.rs`: normalization (accents, `Sub Español` suffix, punctuation); `best_match` picks the right candidate among realistic decoys (e.g. query `"Shingeki no Kyojin"` vs candidates `["Shingeki no Kyojin Sub Español", "Shingeki no Kyojin: The Final Season", "Shingeki no Bahamut"]` → picks the first); returns `None` for `"Monster"` vs `["Monster Musume", "Kaiju No. 8"]` at the chosen threshold; romaji-vs-english cross matching.
   - `animeytx.rs`: `parse_search_results` against the two **real captured** fixtures (hits + empty).
   - `db.rs`: linking a synthetic row onto an existing site slug merges rather than violating uniqueness; `watched_externally` + link marks all episodes seen.
2. `npx tsc --noEmit`, `npm run build` pass.
3. Live: swipe `Want` on a catalog title known to exist on the site (e.g. a currently-airing show) → status line reports "Enlazado" with a plausible episode count; `SELECT slug, url, (SELECT COUNT(*) FROM episodes e WHERE e.series_id=s.id) FROM series s WHERE id=…` shows a site slug/URL and >0 episodes. Swipe `Want` on something the site certainly lacks (obscure 1970s OVA) → "No encontrado", row stays synthetic, no crash.
4. Swiping fast through 5 cards does not open more than 2 scraper windows at once (observe: at most 2 WebView2 windows visible).

## Live verification required

- Screenshot of Descubrir showing the "Enlazado ✓" status after a real `Want`.
- `sqlite3` output of the linked row + its episode count, and of a `NoMatch` row still on `anilist-%`.
- Confirmation (visual) that no more than 2 scraper windows ever appear during a fast swipe run.

## Explicitly out of scope

- Backfilling/linking the **existing** backlog of synthetic rows (a "Enlazar todo" sweep) — politeness and blast radius; the manual per-row retry button covers it.
- Multi-site search (task 6 generalizes `search_url` to other adapters; this spec only implements `AnimeytxAdapter`).
- Fuzzy matching UI (showing the user 3 candidates to pick from). Automatic-or-nothing for v1.
- Replacing the AniList cover with the site's for non-followed series (cover fetching stays exactly as `refresh()` does it today).
