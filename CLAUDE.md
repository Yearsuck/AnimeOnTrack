# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

AnimeOnTrack: a Windows desktop app (Tauri v2 + Rust backend + React/TS frontend) that tracks currently-airing anime scraped from a pirate streaming site (`wwv.animeytx.net`, DooPlay WordPress theme). The user follows series; the app detects new episodes and shows them in a pending-to-watch list.

## Commands

Backend (run from repo root, `src-tauri` is the Cargo package):
```bash
cargo build --manifest-path src-tauri/Cargo.toml     # compile
cargo test --manifest-path src-tauri/Cargo.toml      # unit tests (models/db/diff/adapter)
```
Rust unit tests are unlikely to hit Windows Application Control / Smart App Control blocking the freshly-built test binary (`os error 4551`, "Una directiva de Control de aplicaciones bloqueó este archivo") — this is a machine-level policy issue unrelated to the code; if it happens, verify via `cargo build` + manual app testing instead of fighting it.

Frontend:
```bash
npx tsc --noEmit     # type-check only
npm run build        # tsc + vite build (production bundle to dist/)
```

Full app (dev, hot-reload):
```bash
npm run tauri dev
```
This starts Vite on :1420 and launches the Tauri window via `cargo run`. On Windows, kill any previous `aot-scaffold.exe` process before relaunching (Vite's port and the sqlite file can otherwise conflict).

Cargo requires `%USERPROFILE%\.cargo\bin` on `PATH` in a fresh shell (rustup toolchain: `stable-x86_64-pc-windows-msvc`).

The app's SQLite DB lives at `%APPDATA%\com.ernes.aot-scaffold\animeontrack.sqlite` — useful to inspect directly with `sqlite3` when debugging state (mirrors list, cover_url values, followed flags) without going through the UI.

## Architecture

### The scraping problem this whole backend is built around

The target site sits behind Cloudflare. Plain HTTP requests (curl, reqwest) get `403` even with a spoofed User-Agent/cookies — Cloudflare's JS challenge requires an actual browser engine to execute. The entire backend design flows from this constraint:

- **`src-tauri/src/scraper_engine.rs`** drives a real WebView2 window (via Tauri's `WebviewWindowBuilder`) to a target URL, polls (condition-based, not a fixed sleep) until the page is past the Cloudflare interstitial, then pulls the rendered HTML back to Rust host-side via WebView2's `ICoreWebView2::ExecuteScript` (the `eval()` helper). This is **not** Tauri's page-side IPC — Tauri doesn't inject its IPC into external/remote pages, and exposing it to an untrusted scraped site would be a security hole regardless.
- **Readiness check does not require `document.readyState==='complete'`** — pages on this site carry ad/tracker resources that can keep the `load` event from firing indefinitely, leaving `readyState` stuck at `'interactive'` forever even though the real DOM content is already there. The probe checks `readyState` is `'interactive'` **or** `'complete'`, plus a body-size floor and a title regex excluding known Cloudflare challenge titles ("just a moment", "un momento", etc).
- **`ExecuteScript` does NOT await a returned JS promise** — an unresolved promise just serializes to `null`. Any script passed to `eval()` must be synchronous (or use a poll-for-a-JS-global pattern, not `await`). This bit the cover-image fetching implementation three separate times before landing on a fix (see below).
- **Cover images are fetched one at a time, never in bulk.** Bulk-fetching ~150 poster images in rapid succession (one per series on the airing list) reads to Cloudflare as scraping abuse and gets rate-limited/blocked independent of session/cookie validity — this is a server-side behavioral limit, not something any client-side technique can route around. Covers are instead fetched only for **followed** series, one image per series per `refresh()` cycle, via `fetch_cover_image()`: navigate a small window directly to the image URL (native image viewer, same-origin, cookies apply), poll until `document.images[0]` finishes loading, then read the pixels via `<canvas>.toDataURL()` — no `fetch`/XHR/promises involved at all. Successfully-fetched covers are stored as `data:` URIs directly in `series.cover_url`, replacing the remote (Cloudflare-blocked-to-the-UI) URL.

### Site adapter (pluggable, one implementation so far)

**`src-tauri/src/adapter/`** defines `trait SiteAdapter` (`airing_url`, `parse_airing`, `parse_series`) and one implementation, `AnimeytxAdapter`, parsing DooPlay-theme HTML with the `scraper` crate. Confirmed selectors (captured from live markup, not guessed): airing schedule cards are `.bsx` (title in the anchor's `title` attribute or `.tt`, poster `img[src]`), episode-list rows are `.eplister ul li` (`.epl-num`, `.epl-title`, `.epl-date`). Fixtures for adapter unit tests live in `src-tauri/tests/fixtures/`.

### Mirror fallback

The site is periodically cloned/mirrored under different domains with the same content. `src-tauri/src/commands.rs`'s `scrape_via_mirrors()` tries each configured mirror URL in order for a given path, and — critically — falls through to the next mirror on **either** a fetch failure **or** a successful-but-empty parse (a mirror can return HTTP 200 and render fine while being a totally different, incompatible site — this must not be treated as success). `set_mirrors` refuses to let a Settings edit drop the currently-active working site (`sources.base_url`) from the list entirely, since that silently strands every future scan.

### Data flow / DB

SQLite via `rusqlite` (`src-tauri/src/db.rs`). Tables: `sources` (one row, the configured site), `series`, `episodes`, `settings` (key/value, used for the mirror list under key `mirror_urls`). `series.followed` is never touched by the scan/upsert path (`upsert_series` deliberately excludes it from its `ON CONFLICT` update) — only the dedicated `set_followed` command changes it, so re-scanning the airing catalog never silently un-follows anything.

Watching is enforced gap-free: `set_seen_cascade(series_id, number, seen)` marks every **earlier** episode seen when marking one seen, and every **later** episode unseen when un-marking one — you can't have "watched" episode 10 without 1-9, and un-marking 7 also un-marks 7-10. Both the per-series episode list (`SeriesDetail`) and the flat pending queue (`Pending`) use this cascading command, not a plain single-row toggle.

### Frontend structure

`src/api.ts` is the thin typed wrapper over every Tauri `invoke` call (command names and arg casing must match `src-tauri/src/commands.rs` exactly — Tauri maps camelCase JS args to snake_case Rust params). `src/views/*.tsx` are one file per screen (Onboarding, AiringGrid, Pending, Library, SeriesDetail, Settings, ProgressBar). `src/styles.css` is a single hand-written dark design system (CSS custom properties for spacing/color/shadow scales) — no component library, no Tailwind.

Scan/refresh progress is pushed from Rust via Tauri events (`refresh-progress` for per-series X/Y progress, `scrape-stage` for sub-steps like "verifying"/"extracting"/"covers"), consumed by `ProgressBar.tsx` — there's no polling from the frontend.

`App.tsx` guards its startup effect (deciding onboarding vs. main view, plus refresh-on-open) with a `useRef` flag against React StrictMode's dev-only double-invoke, since double-firing it means scraping the site twice on every app launch during development.
