# Open episode/info links in an app-owned window, not the user's browser

**Date:** 2026-07-11
**Branch to implement on:** `feat/open-links-app-window` (from `develop`, after i18n;
backend-focused, low overlap with other tasks — touches `player.rs`, `commands.rs`
`open_episode`, maybe `lib.rs`).
**Task:** #7.

## Problem

Links (watch episode, anime info) open in the user's already-open default browser
(`tauri-plugin-opener` → `open_url`). The user wants them to open in a **separate window on
the computer, independent of their browser** — an app-owned window. Must NOT break scraping
(which is perfect and must stay untouched).

## Verified context

- `player.rs`: `BrowserPlayer::open` → `app.opener().open_url(&episode.url, None)` (default
  browser). `open_episode` (`commands.rs:911`) constructs a stub `Episode` and calls it.
  Every user-facing link in the UI (`Pending`, `Library`, `SeriesDetail`, `Descubrir`
  poster, `Catalog` — and after task #6 the ℹ icon) goes through `openEpisode(url)` →
  `open_episode`.
- Scraping uses `scraper_engine.rs`:
  `WebviewWindowBuilder::new(app, label, WebviewUrl::External(url)).title(...).visible(false)
  .build()`, gated by `SCRAPE_PERMITS` (max 2 concurrent), then host-side `eval()` pulls
  HTML. This is the hidden, host-driven path.
- App WebView2 windows share the app's WebView2 user-data folder — **separate from the
  user's browser profile** — so an app-owned window is genuinely independent of the user's
  browser, satisfying the request. The user solves any Cloudflare challenge interactively in
  the visible window (fine — this is user viewing, not silent scraping).

## Design

Introduce a **visible, app-owned viewer window** and route `open_episode` through it instead
of the OS opener. Keep it strictly separate from the scraper path.

### New player strategy (`player.rs`)
- Add `AppWindowPlayer` implementing `EpisodePlayer`, opening the URL in a **visible**
  `WebviewWindow`:
  ```rust
  WebviewWindowBuilder::new(app, &label, WebviewUrl::External(url.parse()?))
      .title("AnimeOnTrack")
      .inner_size(1280.0, 800.0)
      .visible(true)
      .build()?;
  ```
- **Unique label per open** (e.g. `viewer-{nanos}`) so opening several episodes gives
  several independent windows the user can arrange/close. (Alternative — reuse a single
  `aot-viewer` window via `get_webview_window` + `WebviewWindow::navigate` — is out of scope
  for v1; multiple simultaneous views are desirable.)
- **Does NOT acquire `SCRAPE_PERMITS`** and does NOT run any host-side `eval()`/scraping.
  It's a plain browser window the user drives. This keeps it fully decoupled from the
  scraper's 2-window budget and behavior.
- Keep `BrowserPlayer` in the file (the swappable-trait design) but stop wiring it into
  `open_episode`. `open_episode` now uses `AppWindowPlayer`.

### Command
`open_episode` unchanged in signature; internally builds the window via `AppWindowPlayer`.
Errors (bad URL, build failure) still map to `Result<(), String>`.

### Scraper untouched
`scraper_engine.rs` is not modified. The distinction is explicit: **hidden + permit-gated +
host-eval = scrape** (unchanged); **visible + no permit + no eval = user viewing** (new).
State this in code comments so the two paths never get conflated.

### Config note
Verify `tauri.conf.json` capabilities/CSP allow creating additional external-URL webview
windows at runtime (the scraper already creates external webviews, so this should already be
permitted — confirm and note). `tauri-plugin-opener` may become unused by `open_episode`;
leave the plugin registered (harmless) unless it's trivially removable without touching
other callers.

## Acceptance criteria

1. `cargo test` green; `npx tsc --noEmit` + `npm run build` clean. Frontend unchanged (still
   calls `openEpisode`).
2. Clicking "watch" / info opens the URL in a **new app window**, not the user's browser
   (the default browser does not gain a tab).
3. Scraping still works end-to-end (airing scan + on-demand detail/follow) — verify a real
   refresh still returns data. The viewer window never consumes scrape permits (open a
   viewer while a scan runs; the scan is unaffected).
4. Opening two episodes yields two independent windows.

## Live verification (required)

Disable startup `refresh()` only if needed; revert before commit. Launch the app, click a
"watch"/info link, screenshot the new app-owned window (and confirm no new tab appeared in
the user's browser). Then run a real airing scan/refresh to confirm scraping is intact and
the viewer didn't interfere. No synthetic OS clicks.

## Out of scope

- In-app embedded player / custom chrome for the viewer window.
- Single reusable viewer window with tab management.
- Removing `tauri-plugin-opener` entirely.
