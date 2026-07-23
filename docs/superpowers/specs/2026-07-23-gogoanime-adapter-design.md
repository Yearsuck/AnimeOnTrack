# gogoanime.by adapter

## Problem

Continuing the multi-site batch. Live-checked the three "domain-unstable" sites the user asked for (123anime, gogoanime, animeland) instead of skipping them outright:

- **123anime.to**: parked/ad-monetization domain (redirects to `ww16.123anime.to/?sub1=<tracking-id>`, the browser's own security extension denies JS execution on it). Confirmed non-viable, not attempted further.
- **gogoanime.by**: real, working site (per gogoanime's own current-domain trend as of 2026-07, `.by` is the live one). No Cloudflare challenge, no malicious redirect. **Viable — this spec.**
- animeland: next (separate spec).

## Findings

Confirmed live (browser devtools) 2026-07-23 against `https://gogoanime.by`, real HTML captured verbatim in most spots (unlike jkanime, this site's markup doesn't trip the browser tool's content-safety filter except on the `.episode-item` nodes specifically — those carry a `style="display:block;"` attribute matching the filter's trigger pattern, so that one fixture section is hand-reassembled from individually-confirmed real values, same discipline as jkanime's fixtures).

**This is a DooPlay-theme site, same theme family as `animeytx` (the very first adapter)** — `.bsx`/`.listupd`/`.genxed`/`.typez`/`.tt` are all present and behave the same way. Differences from `animeytx.rs`, all confirmed live:

- **Airing listing lives at `/schedule/`**, not a `/anime-*-emision/`-style path — it's a 7-day weekly grid (296 `.bsx` cards covering every currently-airing show, one entry per show per release day), not a single filtered list. Cards link to `{base}/series/{slug}/` (the real series page) — this differs from the homepage's own "latest episodes" `.bsx` grid, which links straight to individual *episode* pages instead (`{base}/{slug}-episode-N-...}`); the homepage grid is NOT used as `airing_url` for exactly this reason.
- **`next_episode_at`/`site_episode_count` work identically to `animeytx`**: `.epx.cndwn[data-rlsdt]` (absolute unix timestamp, empty string when the episode already released) and `.sb` text (next episode number, same "not always numeric, parse to `None`" discipline). Confirmed real timestamps present on 295/296 cards live.
- **Episode list lives at `.episodes-container .ep-list .episode-item`**, not `.eplister` — each item carries a `data-episode-number` attribute (cleaner than `animeytx`'s own `.epl-num` text) and one `<a href>`; no per-episode title, no release date shown at all (unlike `animeytx`'s `.epl-title`/`.epl-date`) — `Episode.title`/`released_at` are always `None` here, same limitation class as tioanime's missing fields.
- **Series detail**: `.genxed a` (genres) and `.typez` (kind text) match `animeytx` exactly. Synopsis is `.ninfo > p` (first paragraph) — `animeytx`'s `.entry-content[itemprop="description"]` selector does not exist on this site at all.
- **Search**: `{base}/?s={query}`, results in `.listupd .bs .bsx` (same container/card classes as `animeytx`), kind from `.typez` text. Zero-hit contract identical: `.listupd` present with a "Not Found" message and no `.bsx` cards for a genuine miss, container absent entirely only for a wrong/incompatible mirror.

No `episode_fetch_script` needed — this is a plain single-fetch-single-parse adapter like `tioanime`/`animeflv`, the jkanime architecture addition is unused here.

## Acceptance criteria

1. `cargo test` green: schedule fixture parses to 3 series with real `next_episode_at`/`site_episode_count` (one `None` for the empty-`data-rlsdt` card); series fixture parses 6 episodes with correct `data-episode-number`-sourced numbers, `title`/`released_at` both `None`; detail fixture parses genres `["Comedy","Romance"]`, kind `Some("TV Show")`, synopsis present; search-hits fixture contains "Naruto: Shippuuden" with kind `"TV Show"`; search-empty parses to `Ok(vec![])`; unrecognizable page errors.
2. `adapter_for("gogoanime")` resolves; registered in `all_sites()`.
3. `npx tsc --noEmit`, `cargo build` pass.

## Explicitly out of scope

- animeland — separate spec, next.
- Genre-archive trait methods — no such concept confirmed distinct from `/schedule/`; left at trait defaults.
