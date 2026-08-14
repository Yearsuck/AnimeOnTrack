# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

AnimeOnTrack: a Windows desktop app (Tauri v2 + Rust backend + React/TS frontend) that tracks currently-airing anime scraped from pirate streaming sites. The user follows series; the app detects new episodes and shows them in a pending-to-watch list. Alongside the scraped-site tracking it keeps a full local mirror of the **AniList catalog (~22,000 titles)** for offline browsing, discovery (a swipe deck), and stats, plus optional **Google Drive backup**.

Public, **GPL-3.0-or-later** (`LICENSE`). First release is tagged `v0.1.0`.

Note on names: `productName`/window title/installer are "AnimeOnTrack", but the Cargo package (and the dev binary) is still `aot-scaffold`. The Tauri identifier was `com.ernes.aot-scaffold` until 0.5.5, when it became **`com.animeontrack.app`** (dropping the author's personal name ahead of publicising the repo). Because `app_data_dir()` is derived from the identifier, that move strands the old database — `lib.rs`'s `migrate_legacy_app_data` copies it across on first launch (a copy, never a move; see its doc comment). Windows treats the new identifier as a separate app, so the new build installs *alongside* the old one rather than upgrading it. The dev build and the installed release share the same database.

## Commands

Backend (run from repo root, `src-tauri` is the Cargo package):
```bash
cargo build  --manifest-path src-tauri/Cargo.toml                     # compile
cargo test   --manifest-path src-tauri/Cargo.toml                     # unit tests (models/db/diff/adapter/stats/backup/matching)
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings   # CI gates on this — keep it clean
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

The app's SQLite DB lives at `%APPDATA%\com.animeontrack.app\animeontrack.sqlite` (pre-0.5.5 installs: `%APPDATA%\com.ernes.aot-scaffold\`) — useful to inspect directly with `sqlite3` when debugging state (mirrors list, cover_url values, followed flags, catalog rows) without going through the UI.

### CI/CD (GitHub Actions)

- **`.github/workflows/ci.yml`** — on every push/PR to `main`/`develop`, on `windows-latest`: clippy `-D warnings`, `cargo test`, `tsc`, `npm run build`. Windows because the app is Windows-only and `rusqlite` bundles SQLite from C with MSVC. Concurrency `cancel-in-progress` cancels superseded runs.
- **`.github/workflows/release.yml`** — on pushing a `v*` tag, `tauri-apps/tauri-action@v1` builds the NSIS `.exe` + MSI on `windows-latest` and publishes them to a GitHub Release. No secrets needed (Google Drive creds are entered at runtime, not baked in). To cut a release: bump `version` in `tauri.conf.json` / `package.json` / `Cargo.toml`, then `git tag vX.Y.Z && git push origin vX.Y.Z`.

## Architecture

### The scraping problem this whole backend is built around

The target sites sit behind Cloudflare. Plain HTTP requests (curl, reqwest) get `403` even with a spoofed User-Agent/cookies — Cloudflare's JS challenge requires an actual browser engine to execute. The entire scraping design flows from this constraint:

- **`src-tauri/src/scraper_engine.rs`** drives a real WebView2 window (via Tauri's `WebviewWindowBuilder`) to a target URL, polls (condition-based, not a fixed sleep) until the page is past the Cloudflare interstitial, then pulls the rendered HTML back to Rust host-side via WebView2's `ICoreWebView2::ExecuteScript` (the `eval()` helper). This is **not** Tauri's page-side IPC — Tauri doesn't inject its IPC into external/remote pages, and exposing it to an untrusted scraped site would be a security hole regardless.
- **`eval()` is `#[cfg(windows)]`-gated** — the WebView2 COM interop is Windows-only. There is a `#[cfg(not(windows))]` stub that returns "scraping is only implemented on Windows", and the WebView2 deps are behind `[target.'cfg(windows)'.dependencies]`. The app *compiles* on mac/linux but the scraper wouldn't work there without porting `eval()` to WKWebView / WebKitGTK; only the local (AniList/catalog/stats) side would function.
- **Readiness check does not require `document.readyState==='complete'`** — pages carry ad/tracker resources that can keep the `load` event from firing indefinitely, leaving `readyState` stuck at `'interactive'` forever even though the real DOM content is already there. The probe checks `readyState` is `'interactive'` **or** `'complete'`, plus a body-size floor and a title regex excluding known Cloudflare challenge titles ("just a moment", "un momento", etc).
- **`ExecuteScript` does NOT await a returned JS promise** — an unresolved promise just serializes to `null`. Any script passed to `eval()` must be synchronous (or use a poll-for-a-JS-global pattern, not `await`).
- **Cover images are fetched one at a time, never in bulk.** Bulk-fetching ~150 poster images in rapid succession reads to Cloudflare as scraping abuse and gets rate-limited/blocked independent of session/cookie validity. Covers are fetched only for **followed** series, one image per series per `refresh()` cycle, via `fetch_cover_image()`: navigate a small window directly to the image URL (native image viewer, same-origin, cookies apply), poll until `document.images[0]` finishes loading, then read the pixels via `<canvas>.toDataURL()` — no `fetch`/XHR/promises involved at all. Fetched covers are stored as `data:` URIs directly in `series.cover_url`.

### Site adapters (pluggable, 6 implementations)

**`src-tauri/src/adapter/`** defines `trait SiteAdapter` (`airing_url`, `parse_airing`, `parse_series`, search/genre helpers) and a registry in `adapter/mod.rs`: `SITES` (id/name/default_base_url) + `adapter_for(site_id)`. Six sites ship: **animeytx, tioanime, animeflv, jkanime, gogoanime, animeland**. Each site has its **own library** — switching the active site in Settings shows that site's followed series/progress without touching the others (`sources.site_id`). animeytx parses a DooPlay WordPress theme (confirmed selectors, not guessed: airing cards `.bsx`, episode rows `.eplister ul li` → `.epl-num`/`.epl-title`/`.epl-date`); the others have their own parsers. Adapter unit-test fixtures live in `src-tauri/tests/fixtures/`.

### Mirror fallback

Sites are periodically cloned/mirrored under different domains with the same content. `src-tauri/src/commands.rs`'s `scrape_via_mirrors()` tries each configured mirror URL in order for a given path, and — critically — falls through to the next mirror on **either** a fetch failure **or** a successful-but-empty parse (a mirror can return HTTP 200 and render fine while being a totally different, incompatible site — this must not be treated as success). `set_mirrors` refuses to let a Settings edit drop the currently-active working site (`sources.base_url`) from the list entirely, since that silently strands every future scan.

### Data flow / DB

SQLite via `rusqlite`, split across **`src-tauri/src/db/`** (`series.rs`, `episodes.rs`, `catalog.rs`, `stats.rs`, `airing.rs`, `settings.rs`, `sources.rs`) with schema/migrations in `db.rs`. Tables: `sources`, `series`, `episodes`, `settings` (key/value — mirror list, Google creds/tokens, backup state, sync bookkeeping), `anilist_catalog` (+`anilist_catalog_genres`), `series_genres`. `series.followed` is never touched by the scan/upsert path (`upsert_series` excludes it from its `ON CONFLICT` update) — only the dedicated `set_followed` command changes it, so re-scanning never silently un-follows anything.

Watching is enforced gap-free: `set_seen_cascade(series_id, number, seen)` marks every **earlier** episode seen when marking one seen, and every **later** episode unseen when un-marking one. Both the per-series episode list and the flat pending queue use this cascading command, not a plain single-row toggle.

The Tauri command surface is split across **`src-tauri/src/commands/`** (`scan.rs`, `follow.rs`, `seen.rs`, `library.rs`, `catalog.rs`, `discover.rs`, `stats.rs`, `mirrors.rs`, `backup.rs`) plus `commands.rs`. **Every registered command in `lib.rs`'s `generate_handler!` must have a matching `api.ts` wrapper, and vice versa** — a call to an unregistered command crashes at runtime. (`auto_backup_if_due` is the one command invoked from Rust, not JS.)

### AniList catalog

`src-tauri/src/anilist.rs` fetches AniList's public GraphQL catalog into `anilist_catalog`. The full ~22k-title catalog is crawled in **date/status partitions** (AniList hard-caps offset pagination at 5000 rows and its `total`/`lastPage` are fake — only `hasNextPage` is trustworthy), paced ~28.6 req/min. `commands::backfill_catalog_metadata` re-fetches rows stored before newer fields existed (`fetch_by_ids`); `commands::link_series_to_catalog` resolves engaged series to catalog rows locally via `matching::CatalogIndex` (exact-match only, no network). Both run fire-and-forget on startup.

### Stats (franchise roll-up)

`db/stats.rs` computes all stats locally (no network). The key subtlety: the scraped site splits long-runners into one `series` row per arc (One Piece, Boruto…), so stats roll up per **franchise** (`franchise_rollups` / `franchise_key`), shared by `get_watch_summary` and `get_watch_insights` so they can't disagree. Real-seen vs "Ya lo vi" catalog-estimate signals are combined by `max()`, never summed (they overlap). A colon in a title only collapses into its base franchise when that base is independently present in the user's own data (so "Re:Zero"/"Code:Breaker" don't collapse to "re"/"code"); day buckets are local-time and zero-filled to a contiguous 30-day spine.

### Google Drive backup

`src-tauri/src/backup/` (+ `commands/backup.rs`). Backs the SQLite DB up to a hidden `appDataFolder` in the user's own Drive (OAuth **Desktop + PKCE**, loopback redirect). The OAuth **client id/secret are entered at runtime** in Settings and stored in the `settings` table (`backup::configured_client`), with compile-time env (`.cargo/config.toml`, gitignored) as a fallback — a Desktop client's secret is non-confidential by design, which is why PKCE is used. Restore stages validated bytes and swaps them in on the next startup **before `Db::open`** (`apply_pending_restore`). Both backup and restore fall back to `drive::find_backup_file` (lookup by name) when no `gdrive_file_id` is stored locally — that's what makes restore-onto-a-new-machine work. Setup steps: `docs/google-drive-setup.md`.

### Frontend structure

`src/api.ts` is the thin typed wrapper over every Tauri `invoke` call (command names and arg casing must match the Rust commands exactly — Tauri maps camelCase JS args to snake_case Rust params). `src/views/*.tsx` are one file per screen (Onboarding, AiringGrid, Pending, Library, SeriesDetail, Settings, Catalog, Stats + StatsGraph/StatsInsights/StatsRings, ProgressBar) plus `src/views/Descubrir/` (the swipe-deck discover feature). 

`src/styles.css` is a single hand-written design system — a full token set (spacing/color/radius/shadow **and** type scale `--fs-*`, weights, motion durations, focus ring), light + dark themes via `data-theme`, on a bundled **Inter variable** font (`@fontsource-variable/inter`, imported in `main.tsx`, not a CDN). The brand accent is the logo's **red** (`--accent`/`--accent-solid`) in both themes; `--danger` stays a distinct rose. No component library, no Tailwind. Views carry **no static inline styles** — every fixed value is a class/token; only data-driven values (computed widths/colours) are inline.

`src/i18n/` is a dependency-free catalog layer: `catalog/es.ts` is the source of truth for the key set, and `catalog/en.ts` + `catalog/ca.ts` (English, Catalan) are typed against `Messages` so a missing key is a `tsc` error. Adding a language = one `catalog/xx.ts` + one entry each in `LANGS`/`CATALOGS`/`isLang` + a locale in `lib/formatNumber.ts`.

The logo is `src-tauri/app-icon.png` (AOT monogram, red on white, rounded); the full icon set is regenerated from it via `npx tauri icon`. The Windows exe embeds the icon at **compile time** via `build.rs` (`tauri_build`) — changing `icons/icon.ico` may need a forced relink (`touch src-tauri/build.rs`) and a Windows icon-cache refresh for the new icon to show on the taskbar/desktop.

Scan/refresh progress is pushed from Rust via Tauri events (`refresh-progress` for per-series X/Y progress, `scrape-stage` for sub-steps; `catalog-sync-progress`, `catalog-backfill-progress`), consumed by the relevant views — no polling from the frontend. `App.tsx` guards its startup effect with a `useRef` flag against React StrictMode's dev-only double-invoke, and keeps Stats mounted (hidden via CSS) once visited to preserve the 3D graph's three.js/d3-force state.
