# Multi-site support: tioanime + animeflv adapters

## Problem

`SiteAdapter` exists as a trait but there is exactly one implementation and **nothing dispatches dynamically**:

```rust
const SOURCE_NAME: &str = "AnimeYT";
fn adapter() -> AnimeytxAdapter { AnimeytxAdapter }
async fn fetch_series_episodes(app: &AppHandle, mirrors: &[String], a: &AnimeytxAdapter, ...)
```

Every call site is statically bound to the concrete type. The trait is documentation, not a seam. Adding `https://tioanime.com` and `https://www4.animeflv.net` requires making the seam real first.

## Scope reality check — read before estimating

This is the largest task in the batch, and it has a consequence the user must accept:

**The library is per-site and cannot be transferred.** `series.source_id` FKs to `sources`, and a series' `url`/`slug` are site-specific. A show followed on AnimeYT has no meaningful `url` on tioanime. Switching the active site therefore shows a *different* (initially empty) library, airing grid, and pending queue — the AnimeYT data stays intact in the DB, it is simply not the active source. Cross-site matching (reusing `matching.rs` + `anilist_id` to re-link a followed series onto the newly-selected site) is **explicitly out of scope** here; it is a follow-up feature, and pretending otherwise would make this task unbounded.

Confirm this is acceptable before implementing the Settings UI. If it is not, stop and report — the alternative design (site as a per-series property, library merged across sites) is a much bigger change than "add two adapters".

## Design

### Phase 1 — make the seam real (no new sites yet)

Do this alone, verify it, and commit it before touching any new site. It is a pure refactor with zero behavior change, and it must stay that way.

- `adapter/mod.rs`: add a registry.
  ```rust
  pub struct SiteInfo { pub id: &'static str, pub name: &'static str, pub default_base_url: &'static str }
  pub fn all_sites() -> &'static [SiteInfo];
  pub fn adapter_for(site_id: &str) -> Option<Box<dyn SiteAdapter>>;
  ```
  `site_id` is a stable slug (`"animeytx"`, `"tioanime"`, `"animeflv"`) — never the base URL, which changes with every mirror.
- `sources` gains `site_id TEXT` via `ensure_column`, backfilled to `"animeytx"` for the existing row (there is exactly one; verify with `SELECT * FROM sources` before writing the migration).
- Replace `const SOURCE_NAME` and `fn adapter()` with a lookup from the active source's `site_id`. `fetch_series_episodes` and every other helper take `&dyn SiteAdapter`.
- **Mirrors become per-site.** The `settings` key `mirror_urls` currently holds one global list. Migrate to `mirror_urls:{site_id}`, moving the existing value to `mirror_urls:animeytx` and leaving the old key in place (harmless, and a rollback path). `set_mirrors`/`load_mirrors` take a `site_id`. The existing "refuse to drop the active site's base_url" guard in `set_mirrors` must keep working per-site.

Acceptance for phase 1: `cargo test` green, app behaves **identically** — same airing list, same refresh, same swipe deck. Nothing in the UI changes yet.

### Phase 2 — capture real HTML. Do not skip. Do not guess selectors.

Neither target site is DooPlay. `.bsx` / `.eplister` will not be there. Both are likely behind Cloudflare, so **capture through the WebView2 engine** (`scraper_engine::fetch_html`), not curl — and if a page happens to load without a challenge, still capture it the same way so the fixture reflects rendered DOM, not raw HTML.

For **each** of `tioanime.com` and `www4.animeflv.net`, capture and save under `src-tauri/tests/fixtures/{site_id}_{page}.html`:

| fixture | page |
|---|---|
| `{site}_airing.html` | the currently-airing / schedule listing |
| `{site}_series.html` | one series page (episode list) |
| `{site}_series_detail.html` | same page if it also carries genres/type/synopsis; otherwise the detail page |
| `{site}_search_hits.html` | search results for a query with hits |
| `{site}_search_empty.html` | search results for a nonsense query |

Capture **one page at a time**, spaced out. `SCRAPE_PERMITS` (2) still applies. This is a handful of fetches, not a crawl.

Then read the markup and write down, in the adapter's module doc comment, the confirmed selectors and where they came from — matching the precedent in `animeytx.rs` ("Selectors confirmed against real captured HTML"). A selector that is not backed by a committed fixture is a bug waiting to happen.

**Expect the trait to not fit.** Two known shapes to check for and report on before implementing:
- `genre_list_url` / `genre_page_url` / `parse_finished_page` / `parse_pagination_last_page` exist because AnimeYT has genre archives with a `.status.Completed` marker. If a target site has no equivalent "is this series finished" signal on listing pages, `parse_finished_page` cannot be implemented faithfully. Note that these methods are **only** used by `discover_swipe_card`/`decide_swipe`, which task 3 disconnected from the UI (the swipe deck now comes from the local AniList catalog). Therefore: give them a **default trait implementation returning `Ok(vec![])` / `1`**, document that the site has no genre-archive concept, and do not contort the site into one.
- `airing_url` assumes a single fixed path. If a site paginates its schedule or splits it by weekday, `parse_airing` must handle the first page only and the limitation must be stated.

### Phase 3 — implement the two adapters

One module per site (`adapter/tioanime.rs`, `adapter/animeflv.rs`), each with unit tests against its own fixtures, mirroring `animeytx.rs`'s test layout. Required methods, in priority order:

1. `airing_url` + `parse_airing` — without these the site is useless.
2. `parse_series` (episode list) — required for follow/refresh.
3. `parse_series_detail` (genres, kind, synopsis) — required for stats; `genres` may be `vec![]` if the site has none, but say so.
4. `search_url` + `parse_search_results` — required for task 4's linking to work on this site.
5. The genre-archive four: default impls per above unless the site genuinely has archives.

Where task 5's work has landed, `parse_airing` also fills `next_episode_at` / `site_episode_count`. If the site exposes no next-release timestamp, return `None` — do not synthesize one, and expect the airing sort (task 8) to fall back to title order for that site. Say so.

### Phase 4 — site selection in Ajustes

- `Settings.tsx`: a site selector (native `<select>`, design-system styled) listing `all_sites()`. Changing it: writes the chosen `site_id`, seeds `sources` + `mirror_urls:{site_id}` with the site's `default_base_url` if absent, sets it active, and triggers a `scan_airing`.
- **A confirmation step is required** before switching, because the visible library changes wholesale. Plain text, no modal library: "Cambiar de sitio mostrará la biblioteca de {sitio}. Tus series de {sitio actual} se conservan y volverán al cambiar de vuelta." Nothing is deleted — verify that by switching away and back.
- The per-site mirror list editor stays as-is, now scoped to the selected site.
- Everything downstream (`scan_airing`, `rescan_airing`, `refresh`, `link_catalog_series`, `SeriesDetail`) already routes through the active source, so it should need no change once phase 1 lands. If any of them still hardcodes AnimeYT, that is a phase-1 miss — fix it there, not with a special case.

## Acceptance criteria (verifiable)

1. **Phase 1 in isolation**: `cargo test` green; live app against AnimeYT behaves identically (airing list count, a refresh, opening a series) — verify *before* adding sites.
2. `cargo test` green with new per-adapter tests parsing each committed fixture: airing cards (count > 0, plausible titles/urls), episode rows (count > 0, numbers non-empty), search hits (> 0) and search empty (== 0, `Ok`, not an error), detail genres/kind.
3. `npx tsc --noEmit`, `npm run build` pass.
4. Live, **for each of the three sites**: select it in Ajustes → the airing grid populates with that site's shows → follow one → refresh pulls its episodes → open it → episode list matches the site. Screenshot each.
5. Switching site A → B → A leaves A's followed series and watch progress intact (`SELECT source_id, COUNT(*), SUM(followed) FROM series GROUP BY source_id` before and after).
6. Never more than 2 scraper windows at once, at any point.

## Live verification required

- One screenshot per site of its airing grid, plus one of a real episode list scraped from a non-AnimeYT site.
- The `sqlite3` group-by above, before and after a round-trip site switch.
- An explicit statement of which trait methods each new site could **not** implement faithfully, and what the default returns.
- A note on whether each site actually sat behind Cloudflare (it changes nothing in the code — WebView2 either way — but it is worth recording).

## Explicitly out of scope

- Re-linking a followed series across sites (matching AnimeYT's "Frieren" to tioanime's). Separate feature; `matching.rs` + `series.anilist_id` make it tractable later.
- Aggregating/merging multiple sites into one library or one airing grid.
- Any `reqwest`/`curl` path to any of the three sites.
- Per-site cover-fetch tuning (the one-at-a-time rule is universal).
- Removing AnimeYT-specific genre-archive methods from the trait (default impls instead — a later cleanup once the swipe-from-site path is confirmed dead).
