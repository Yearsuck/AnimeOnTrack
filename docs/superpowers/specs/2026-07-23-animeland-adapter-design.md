# animeland (w7.animeland.tv) adapter

## Problem

Third of the three "domain-unstable" sites checked live instead of skipped: 123anime (parked/dead), gogoanime (real, shipped), animeland — this spec.

## Findings

`animeland.tv` redirects to `w7.animeland.tv` (a numbered-mirror-style subdomain, same idea as this app's own `mirror_urls` concept). No Cloudflare, no malicious redirect, real content. Confirmed live 2026-07-23. **Not** a DooPlay site — a different, "video post" WordPress theme (`.new_added`, `.gallery`, `.anime-col`, `.video_wrapper_container`).

- **Relative URLs**: unlike every adapter so far except `tioanime`, poster images use relative `src` (`/Thumbs/{name}.jpg?xxx`, `/ac/{name}.jpg`) while series/episode links are already absolute. Same fix as `tioanime.rs`: a fixed `BASE` constant + `abs()` helper, with the identical documented limitation (a second animeland mirror, if ever configured, would still resolve images against the hardcoded `w7` domain).
- **Airing listing is the homepage** (`{base}/`) — its `.new_added` cards (24 on a real capture) are "recently updated series," scoped safely: confirmed 0 overlap with the giant unrelated `.sidebar_right` A-Z catalog list that's appended to literally every page on this site (89 `/category/` links there alone — a real trap for a naive `a[href*="/category/"]` selector). No episode-count or release-date signal anywhere on these cards — `next_episode_at`/`site_episode_count` always `None`.
- **Series page** (`{base}/category/{slug}`): episode links are `.anime-col li.play a` (confirmed scoped away from the sidebar too), text `"Episode N"` → digit-extract. No per-episode title, no date. Synopsis is `.video_wrapper_container`'s text — note this sits inside `.entry-content`, which ALSO contains an inline `<style>` block whose raw CSS text (`.video-block{margin-right:15px!important;...}`) is siblings-away in the same ancestor; selecting `.entry-content` directly would pull that CSS text into the "synopsis" — confirmed live, worth flagging so nobody "fixes" this by widening the selector back to `.entry-content`.
- **No genre or kind/type data exists anywhere on this site** — checked the series page thoroughly (an `.Anime.Info` div exists but is empty on every real page checked). `SeriesDetail.genres`/`.kind` are always `vec![]`/`None` here — an honest site limitation, not a missed selector.
- **Search**: `{base}/?s={query}`, results in `.gallery` divs (`.title a` for link/title, `img` for poster) — **zero-hit contract differs from every other adapter**: `.gallery` is entirely ABSENT (not present-but-empty) for a genuine zero-hit search, confirmed live. The sanity/wrong-mirror check therefore can't key off `.gallery`'s presence like the others key off their own results container — it uses `.entry-content` instead (WP boilerplate present on literally every real page of this site, hit or empty), and treats zero `.gallery` matches as a legitimate empty result rather than an error.

No `episode_fetch_script` needed — plain single-fetch-single-parse.

## Acceptance criteria

1. `cargo test` green: airing fixture parses 3 series, all `next_episode_at`/`site_episode_count` `None`, poster URLs resolved absolute via `BASE`; series fixture parses 2 episodes (`title`/`released_at` both `None`), synopsis present and does NOT contain the `.video-block{` CSS text; detail fixture genres `vec![]`, kind `None`; search-hits fixture contains "Naruto Shippuuden Movie 1"; search-empty parses `Ok(vec![])` (not `Err`) despite zero `.gallery` matches; a page with no `.entry-content` at all errors.
2. `adapter_for("animeland")` resolves; registered in `all_sites()`.
3. `npx tsc --noEmit`, `cargo build` pass.

## Explicitly out of scope

- Genre-archive trait methods — no genre concept confirmed at all on this site; left at trait defaults.
- Threading the real working mirror into relative-URL resolution — same accepted `tioanime.rs` limitation, not solved here.
