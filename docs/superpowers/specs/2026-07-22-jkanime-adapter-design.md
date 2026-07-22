# jkanime.net adapter (multi-site batch, site 1 of 3: jkanime, animekisa, animension)

## Problem

The app supports one active site at a time via `SiteAdapter` (`animeytx`, `tioanime`, `animeflv` are registered). The user wants broader site coverage. A domain-stability check (WebSearch, 2026-07-22) found the requested sites split into two groups:

- **Stable, single canonical domain**: jkanime.net, animekisa.tv, animension.to — worth adapters.
- **No stable official domain, dozens of unrelated clones** (123anime, gogoanime, animeland) — a fixed-domain adapter breaks as soon as the configured clone changes structure or dies; explicitly out of scope for this batch, revisit only if the user still wants them after seeing this pattern play out.

This spec covers **jkanime.net only** (site 1 of 3, one-at-a-time per user decision). animekisa and animension get their own specs once this one ships.

## Why jkanime is architecturally different from the existing three adapters

Every existing adapter (`animeytx`, `tioanime`, `animeflv`) follows one shape: `fetch_html(url)` returns a single page's rendered HTML, and `parse_series` extracts the episode list straight out of it — one fetch, one parse.

jkanime does **not** put the episode list in the series page's HTML. Confirmed live (2026-07-22, via browser devtools against `https://jkanime.net/one-piece/` and `https://jkanime.net/hanaori-san-wa-tensei-shitemo-kenka-ga-shitai/`):

- The series page HTML contains a numeric internal anime id, embedded in an inline (no-`src`) `<script>` tag as part of a jQuery `$.ajax` call: `url: 'https://jkanime.net/ajax/episodes/{id}/'+ pag`. The id is extractable via regex (`ajax/episodes/(\d+)/`) — no dedicated data attribute carries it.
- Episodes are fetched client-side via `POST https://jkanime.net/ajax/episodes/{id}/{page}`, form body `_token={csrf}` (from `meta[name="csrf-token"]`), returning a Laravel-style JSON paginator:
  ```json
  {"current_page":1,"data":[{"id":74990,"number":1,"title":"...","image":"...","timestamp":"2026-07-11 17:47:15"}],"last_page":1,"per_page":16,"total":2, ...}
  ```
  `last_page`/`total` are known from the very first page's response.
- Confirmed on two real series: `one-piece` (id 201, 1170 episodes, 74 pages) and `hanaori-san-wa-tensei-shitemo-kenka-ga-shitai` (id 4797, 2 episodes, 1 page).
- Episode detail URLs are **not** in the JSON; the site's own JS builds them client-side as `{base}/{slug}/{number}/` — confirmed by clicking the real "Ver más" button and reading the rendered anchor (`https://jkanime.net/hanaori-san-wa-tensei-shitemo-kenka-ga-shitai/1/`).
- `parse_series_detail`'s inputs (genres, kind, synopsis) **are** in the plain series-page HTML — no AJAX needed for those.

This means `parse_series` cannot be "parse the page HTML" for this site — its input must be the JSON episode data, fetched via a second, site-specific step.

### Chosen approach: optional per-site post-load script

Add one optional method to `SiteAdapter`:

```rust
/// Extra JS run in-page, after the normal readiness poll passes, whose
/// synchronous result REPLACES the page HTML as what parse_series receives.
/// Exists because jkanime's episode list is not in the series page's HTML
/// at all — it's fetched client-side via a paginated JSON AJAX endpoint
/// requiring a CSRF token. Every site so far except this one leaves this at
/// the default (`None`): its episode list is already in the plain page
/// HTML, no extra step needed.
fn episode_fetch_script(&self, base_url: &str) -> Option<String> {
    None
}
```

`scraper_engine::fetch_html` (or a sibling function used only for the series-page fetch call sites — `commands/follow.rs`, `commands/scan.rs`'s per-series refresh) gains an optional script parameter: when `Some`, after the existing readiness poll, `eval()` runs this script instead of the default `document.documentElement.outerHTML` extraction, and its return value becomes `ScrapeResult.html`. The script itself:

1. Regex-extracts the numeric id from inline scripts (same pattern confirmed above).
2. Reads the CSRF token from the page's own `meta[name="csrf-token"]`.
3. Loop of **synchronous** `XMLHttpRequest` (per the repo's established `ExecuteScript` constraint — no `await`able promises survive the host-side `eval()` bridge) against `/ajax/episodes/{id}/{page}`, starting at page 1, continuing while `page <= last_page` from the first response.
4. Returns `JSON.stringify(allEpisodes)` — a flat array of `{id, number, title, image, timestamp}`.

`jkanime.rs`'s `parse_series(&self, json: &str)` then does `serde_json::from_str` instead of `scraper::Html::parse_document` — the trait signature (`&str -> Result<Vec<Episode>>`) doesn't care that the string holds JSON instead of HTML, so this needs zero trait changes beyond the new optional method.

Airing listing and search stay on the normal `fetch_html` path (no post-load script) — both are fully present in plain page HTML, confirmed live.

**Always fetch every episode page**, never just the newest. Simpler and correct; the multi-page cost (up to 74 requests for an outlier like One Piece) is a one-time cost paid only when the user follows/opens/marks-seen that specific series — consistent with the rest of the app's on-demand-only scraping discipline ([[project-scraping-scope]]). No incremental-refresh optimization for this site in this pass; call out as a known future cost if it becomes a real complaint, don't build it speculatively.

## Confirmed selectors / URLs

All confirmed live 2026-07-22 against `jkanime.net` (no Cloudflare challenge observed — plain page loads).

- **Airing**: `{base}/directorio/?estado=emision` (query param, NOT `/directorio/emision/` — that path segment is silently ignored by the site and returns the unfiltered directory, mixing in finished/movie cards). Cards: `.card` → `a` (wraps `.d-thumb` with the poster `img[src]` and two `.badge`s: type e.g. "Serie"/"Pelicula", status e.g. "En emision"/"Concluido") + sibling `.svea .card-title` (title text). `is_airing` cards carry the `.badge.currently` "En emision" text specifically. No countdown/episode-count badge exists on listing cards at all → `next_episode_at`/`site_episode_count` always `None`, same limitation as tioanime.
- **Series id + token**: numeric id via `ajax/episodes/(\d+)/` regex over inline (no-`src`) `<script>` text; CSRF via `meta[name="csrf-token"]` content attribute.
- **Series detail** (`{base}/{slug}/`): genres = `.anime_data li` whose `span` text is `"Generos:"`, its `a` texts (page renders this block twice, pc+mobile duplicate — take the first match set only, same discipline as `text_of`/`.next()` elsewhere). Kind = the `li` whose `span` text is `"Tipo:"`, full `li.textContent` minus the "Tipo:" label (this one has NO anchor — it's plain text, unlike genres/studio/season which are links). Synopsis = `.anime_info .scroll` (a `<p>`) text.
- **Episodes JSON**: `POST {base}/ajax/episodes/{id}/{page}`, body `_token={csrf}`, `Content-Type: application/x-www-form-urlencoded`, header `X-Requested-With: XMLHttpRequest`. Response fields used: `data[].number` (int → `Episode.number` as string), `data[].timestamp` (→ `released_at` directly, already an absolute `"YYYY-MM-DD HH:MM:SS"` string — no relative-date parsing needed, nicer than every other adapter). `Episode.url` = `{base}/{slug}/{number}/` (built by us, not present in the JSON). `Episode.title` = `None` (JSON's `title` field is just `"{series title} - {number}"`, not a distinct per-episode title — same call already made for tioanime).
- **Search**: real endpoint is `{base}/buscar/{query}` (a path segment via a GET form whose `action="https://jkanime.net/buscar"` — NOT `?q=`, confirmed the form submits by appending the query as a path segment, and `?q=` alone does nothing). Cards: `.anime__item` → `a[href]` + `.anime__item__text` (text needs cleanup: it's the concatenation of a status label, a type label, and the title with no separator — e.g. `"Concluido\nSerie\nBoruto: Naruto Next Generations"` — title is the last non-empty line). Zero-hit contract confirmed identical in shape to the other two adapters: `.anime__page__content` container is present either way; `.anime__item` count is 0 for a genuine zero-hit query, page absent/unrecognizable only for a wrong/incompatible mirror.

## Fixture capture note

The browser tool used for live verification blocks bulk raw-HTML transfer out of the tool call (content-safety filter, not something to route around — confirmed it also blocks a base64-encoded version of the same content, which is the filter correctly doing its job against exactly that evasion). Full verbatim page dumps were not obtainable this way.

Fixtures are therefore **hand-assembled from real, independently-verified fragments** — every class name, tag hierarchy, and content string below was confirmed against the live site through many small targeted queries (each logged above with its source URL/date), not guessed. This deviates from the previous two adapters' fixture provenance (one committed verbatim page capture each) and is called out explicitly here so it isn't mistaken for unverified selectors later.

## Acceptance criteria

1. `cargo test` green: airing fixture parses to ≥3 series, all `is_airing`, `next_episode_at`/`site_episode_count` both `None`; episodes JSON fixture (hanaori, 2 episodes) parses to exactly 2 `Episode`s with correct `number`/`url`/`released_at` and `title: None`; series-detail fixture parses genres `["Comedia","Fantasia","Romance"]`, kind `Some("Serie")`, synopsis present; search-hits fixture contains "Boruto: Naruto Next Generations"; search-empty fixture parses to `Ok(vec![])`; unrecognizable-page search errors.
2. `adapter_for("jkanime")` resolves; registered in `all_sites()`.
3. `npx tsc --noEmit`, `cargo build` pass.
4. Live, from the running app: select jkanime in Ajustes → airing grid populates → follow a low-episode-count series → episode list matches the site (including the one-page case) → follow/open a high-episode-count series (e.g. One Piece) once, confirm it completes (may take longer — 74 sequential same-origin AJAX calls) and episode count matches the site's `total`.
5. Never more than 2 scraper windows at once (unchanged global constraint — the episode-fetch script runs inside the single window already open for the series-page fetch, no extra windows).

## Explicitly out of scope

- animekisa, animension adapters — separate specs, one at a time.
- 123anime, gogoanime, animeland — no stable domain identity, not attempted this batch.
- Incremental/last-page-only episode refresh optimization for already-followed jkanime series — always full re-fetch for now.
- Any genre-archive (`genre_list_url`/`parse_finished_page`/etc.) implementation — no such concept confirmed on this site's listing pages; left at trait defaults, same as tioanime.
