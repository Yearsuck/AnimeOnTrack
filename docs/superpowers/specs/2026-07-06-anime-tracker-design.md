# AnimeOnTrack — Design

**Date:** 2026-07-06
**Status:** Approved (design), pending implementation plan

## Purpose

Desktop app (Windows) to track currently-airing ("estreno") anime scraped from
a pirate streaming site. User follows series; on each refresh the app detects
newly-released episodes and stacks them into a pending-to-watch list with a
total counter. Clicking a pending episode opens it in the browser and marks it
watched.

Reference site: `https://wwv.animeytx.net/` (WordPress/DooPlay theme).

## Key constraints (from research)

- Site sits behind **Cloudflare bot protection**: plain HTTP requests return
  `403` even with a spoofed User-Agent. Scraping **requires a real browser
  engine** that executes JS and solves the challenge.
- Known URL patterns:
  - Airing list: `/anime-en-emision/`
  - Series page: `/tv/<slug>/`
  - Catalog: `/tv/`, recent: `/anime/`
  - Episodes labelled "Estreno" / "Episodio N".

## Locked decisions

- **Stack:** Tauri (Rust core + web UI). On Windows the Tauri webview is
  WebView2 (Chromium/Edge) — used as the scraping engine.
- **Playback:** browser-open in v1; a player interface is defined so an in-app
  extractor can be added later without rework.
- **Scope:** single `animeytx` site adapter now, behind a pluggable
  `SiteAdapter` interface so more sites can be added later.

## Architecture

One Tauri app, three parts:

1. **Rust core** — orchestration, SQLite, episode-diff logic, site adapters.
2. **Web UI** — React + Vite + TypeScript. Screens + IPC calls to Rust.
3. **Hidden scraper webview** — Rust spawns an invisible `WebviewWindow`
   (WebView2). It loads the target page; JS runs and Cloudflare passes; cookies
   persist across scrapes. Rust injects an extractor JS, receives parsed JSON
   over Tauri IPC, then closes the window. No bundled browser, no Playwright.

### Scrape sequence

1. Rust command `scrape(url)` opens/reuses the hidden webview and navigates.
2. On load complete, Rust injects the adapter's extractor JS.
3. Extractor parses the DOM (adapter-specific selectors) and emits JSON back
   via Tauri IPC event.
4. Rust deserializes into models and updates the DB.

## Site adapter (pluggable)

Rust trait `SiteAdapter`:

- `airing_url() -> Url` → `/anime-en-emision/`
- `parse_airing(dom) -> Vec<Series>`
- `parse_series(dom) -> Vec<Episode>`
- owns the CSS selectors / extractor JS it injects.

Impl: `AnimeytxAdapter` (DooPlay layout, `/tv/<slug>/`). Adding a site later is a
new impl only — core untouched.

## Data model (SQLite via rusqlite)

- `sources(id, name, base_url)`
- `series(id, source_id, slug, title, url, cover_url, is_airing)`
- `follows(series_id, followed_at)`
- `episodes(id, series_id, number, title, url, released_at, seen, added_at)`
- `settings(key, value)`

## Core flows

1. **First run:** user enters base URL → app scans `/anime-en-emision/` →
   stores all airing series → grid UI → user clicks **Follow** on chosen series.
2. **Refresh** (triggered on app open **and** by a manual button): for each
   followed series, scrape its series page → episode list → **diff against
   stored episodes** → new ones inserted `seen=false` → pending counter grows.
   Scrapes run sequentially with a small delay, reusing one hidden webview, with
   a progress indicator.
3. **Pending list (home screen):** episodes stacked grouped by series, each
   group showing its new-episode count, plus a **total unseen badge**. Click an
   episode → `shell.open(url)` in the default browser → mark `seen=true`,
   counter decrements.
4. **Playback interface:** trait `EpisodePlayer::open(episode)`. v1 impl =
   `BrowserPlayer`. Future `EmbeddedPlayer` implements the same trait.

## Error handling

- Cloudflare not passing / navigation timeout → retry once → mark source
  unreachable and show a banner. Cached data is preserved.
- Selector mismatch (site changed its HTML) → parse returns empty/error →
  surface a "site layout changed" warning. **Never wipe existing follows or
  episodes** on a failed parse.
- Offline → keep cached data, disable the refresh action.

## Testing

- Rust unit tests: adapter parsing against **saved HTML fixtures** (real
  animeytx page snapshots checked into the repo).
- Diff logic unit tests: no duplicate episodes, correct new-episode count across
  repeated refreshes.
- DB layer tests (insert/query/mark-seen).
- Live end-to-end scrape verified as a manual smoke test.

## UI screens

- Onboarding (enter base URL, first scan).
- Airing grid (follow / unfollow toggles).
- Home / pending list (grouped, total badge, click-to-open).
- Series detail (episode history, seen state).
- Settings (source URL, manual re-scan of airing, refresh-on-open toggle).

## Out of scope (v1)

- In-app video extraction/playback (interface only, no impl).
- Multiple sites (interface only; one adapter shipped).
- Accounts, sync, notifications, mobile.
