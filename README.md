<div align="center">

<img src="https://capsule-render.vercel.app/api?type=waving&color=0:0b1521,100:4aa8ff&height=210&section=header&text=AnimeOnTrack&fontSize=56&fontColor=e9eff5&fontAlignY=38&animation=fadeIn&desc=Stop%20refreshing%20the%20site.%20Let%20the%20app%20refresh%20it.&descAlignY=58&descSize=16&descColor=9fd4ff" width="100%" alt="AnimeOnTrack" />

<img src="https://img.shields.io/badge/Tauri-2-4aa8ff?style=for-the-badge&logo=tauri&logoColor=e9eff5&labelColor=0b1521" alt="Tauri 2" />
<img src="https://img.shields.io/badge/Rust-stable-4aa8ff?style=for-the-badge&logo=rust&logoColor=e9eff5&labelColor=0b1521" alt="Rust" />
<img src="https://img.shields.io/badge/React-19-4aa8ff?style=for-the-badge&logo=react&logoColor=e9eff5&labelColor=0b1521" alt="React 19" />
<img src="https://img.shields.io/badge/SQLite-embedded-46d19e?style=for-the-badge&logo=sqlite&logoColor=0b1521&labelColor=0b1521" alt="SQLite" />
<img src="https://img.shields.io/badge/i18n-EN%20%C2%B7%20ES-46d19e?style=for-the-badge&labelColor=0b1521" alt="i18n EN/ES" />
<img src="https://img.shields.io/badge/platform-Windows-e9eff5?style=for-the-badge&logo=windows11&logoColor=0b1521&labelColor=0b1521" alt="Windows" />

<a href="https://github.com/Yearsuck/AnimeOnTrack/releases/latest"><img src="https://img.shields.io/github/v/release/Yearsuck/AnimeOnTrack?style=flat-square&color=4aa8ff&labelColor=0b1521&label=latest" alt="Latest release" /></a>
<a href="https://github.com/Yearsuck/AnimeOnTrack/releases"><img src="https://img.shields.io/github/downloads/Yearsuck/AnimeOnTrack/total?style=flat-square&color=46d19e&labelColor=0b1521&label=downloads" alt="Downloads" /></a>
<a href="LICENSE"><img src="https://img.shields.io/badge/license-GPL--3.0--or--later-9fd4ff?style=flat-square&labelColor=0b1521" alt="GPL-3.0-or-later" /></a>
<img src="https://img.shields.io/badge/sites-6%20adapters-9fd4ff?style=flat-square&labelColor=0b1521" alt="6 site adapters" />
<img src="https://img.shields.io/badge/catalog-~22k%20titles-9fd4ff?style=flat-square&labelColor=0b1521" alt="~22,000 AniList titles mirrored locally" />

<p><em>Tracks currently-airing anime, tells you the moment a new episode drops, and never loses your place — across six streaming sites, in English or Spanish.</em></p>

<p>
  <a href="https://github.com/Yearsuck/AnimeOnTrack/releases/latest"><b>⬇ Download for Windows</b></a>
  &nbsp;·&nbsp;
  <a href="#features">Features</a>
  &nbsp;·&nbsp;
  <a href="#why-the-odd-design-choices">How it works</a>
  &nbsp;·&nbsp;
  <a href="#getting-started">Build it yourself</a>
</p>

</div>

---

## Get it

Grab the latest `*_x64-setup.exe` (or the `.msi`) from the [**releases page**](https://github.com/Yearsuck/AnimeOnTrack/releases/latest) and run it. No account, no config file, no telemetry — everything lives in a local SQLite database on your machine.

On first launch, pick a site in **Settings** and let it scan. macOS and Linux builds are published too, but are **not functional yet** — the episode-detection scraper is Windows-only for now (see [`docs/cross-platform-porting.md`](docs/cross-platform-porting.md)).

---

## The premise

Pirate streaming sites carry almost everything that's currently airing, and they sit behind Cloudflare — no ordinary HTTP client gets past the `403`, because clearing the JavaScript challenge takes a real browser engine. **AnimeOnTrack** opens a hidden WebView2 window, waits for the challenge to clear, pulls the rendered HTML back into Rust, and tells you when something you follow has a new episode. You decide what to watch; the app decides when it needs to check.

It also keeps a full local mirror of the **AniList catalog (~22,000 titles)**, so browsing, filtering, and discovery are instant and offline — the streaming site is only ever touched when it genuinely has to be.

---

## A note on piracy

I'll say this plainly: I always advocate paying for the content you watch. Creators,
studios, and the people who make anime don't live on air — if you can support them
officially, you should, and I do.

But I also won't pretend the history is simple. Spanish- and English-language anime
piracy has, for decades, been the **first door into anime** for most fans outside Japan.
Without fansubs and pirate streaming, the medium simply would not be as big, as global,
or as loved as it is today — a lot of us are here *because* of that door. And there's a
harder truth on top of it: plenty of titles are only ever available fansubbed, never
licensed or translated officially in a given region — so for those, "just pay for it"
isn't an option that exists, pirate or not. You can't support what nobody is selling you.

So: support creators wherever you actually can, and understand this tool for what it is —
a tracker built by someone who loves the medium, not an excuse to skip the box set when
the box set exists.

---

## Supported sites

Six sites ship today, each behind the same pluggable `SiteAdapter`. Switch between them in **Settings** — your followed shows, seen episodes, watch progress, and backlog are one **shared, canonical** state across every site you use a show on, unified by its AniList id (or a normalized title when unlinked). Only the actual episode-playback links point at whichever site's copy you have open: switching the active site changes which URLs you click through to watch, not *which* shows you follow or how much you've watched. Every site still has a per-site mirror list, so when a domain goes down the app falls through to a working clone automatically.

| Site | Default domain | Adapter id |
|---|---|---|
| **AnimeYT** | `wwv.animeytx.net` | `animeytx` |
| **TioAnime** | `tioanime.com` | `tioanime` |
| **AnimeFLV** | `www4.animeflv.net` | `animeflv` |
| **JKanime** | `jkanime.net` | `jkanime` |
| **GogoAnime** | `gogoanime.by` | `gogoanime` |
| **AnimeLand** | `w7.animeland.tv` | `animeland` |

Adding a seventh is one `SiteInfo` row plus a `SiteAdapter` implementation — no other code changes.

---

## Features

| Area | What it does |
|---|---|
| **Pending** | Flat queue of new episodes across everything you follow. Sortable by how many episodes each series still has left to watch. |
| **Airing** | The live scraped airing schedule — follow a series with one click. Newest-release-first. |
| **Library** | Followed series grouped by derived status — **Watching** (includes airing shows you're caught up on), **Plan to watch**, **Completed** (finished *and* fully watched). Airing filter, progress bars, keyboard-accessible, one-click reclassify. |
| **Discover** | A swipe deck (Want / Seen / Discard) drawn from the local catalog, weighted toward your taste. Multi-level undo of your last ~5 cards, plus configurable per-genre, per-type, and release-date-range filters (default: everything up to today — set it into the future to include upcoming titles). |
| **Catalog** | The full AniList catalog, filterable by genre / format / score / length. Multi-select to batch-add many titles to *Want* or *Seen*; ℹ opens the AniList page. |
| **Stats** | Watch-time and completion tiles, a top-series ranking, an activity timeline, genre/type breakdowns, and an interactive 3D force graph of your library — all computed locally. Long-runners the site splits into per-arc rows (One Piece, Boruto…) roll up into one franchise. |
| **Backup** | One-click, end-to-end backup of your database to a hidden folder in **your** Google Drive, plus automatic daily backups. Restore onto a fresh machine by connecting the same account. |
| **Settings** | Language (English / Spanish), light / dark theme, active site, mirror list, Google Drive credentials, and a full-recheck escape hatch. |

**Watching is gap-free.** Marking an episode seen marks every earlier one seen; un-marking one un-marks every later one. No "I watched 10 but not 6–9" states — and it parses whatever numbering the site uses (`1x05`, `12.5`, …).

**Everything is reversible.** Unfollow, move between lists, un-mark "seen", return a Discover card to the deck — every classification has an inverse reachable from the UI.

**Links open in their own window.** Watching an episode or opening an info page spawns an app-owned window, separate from your everyday browser and its profile.

---

## What's new

- **v0.5.4** — Faster scraping: up to **4 concurrent** episode-list fetches (was 2), app-wide. Plus a crash fix — catalog linking no longer blocks the site switch that triggers it.
- **v0.5.3** — Switching to a site now catches it up automatically: shows followed elsewhere get their episode lists fetched and their AniList links resolved in the background, instead of waiting until that site happened to be the one open at startup. Also fixes AnimeLand storing URL slugs (`re-monster`) instead of real titles (`Re Monster`).
- **v0.5.2** — All six adapters re-validated against the live sites; JKanime's directory-page selector updated after a site redesign that had silently broken its listing.
- **v0.5.1** — Four fixes: shows that actually finished now get un-marked as airing (via AniList's status), pending episodes sort numerically instead of by scrape order, the Discover deck stops re-offering shows you already decided on another site, and saving deck filters applies immediately instead of after a tab switch.
- **v0.5.0** — **Release-date range filter** in Discover: pick any window, defaults to *everything up to today*, and can reach into the future for upcoming titles.
- **v0.4.3** — Real per-episode progress now syncs across sites continuously. Follow the same show on two sites, watch on one, the other updates automatically — no more stale "unwatched" entries when switching sites.
- **v0.4.2** — **Estadísticas** computed canonically across *every* site you use. Before this, Stats only reflected the active site's slice of your data (e.g. showing 0 "want to watch" when you had hundreds tracked under a different site).
- **v0.4.1** — Scraper remembers the last working mirror per site and tries it first next time. Cross-site airing entries use the freshest "next episode" timestamp across *all* sites that have the show. Catalog linking falls back to fuzzy title match (season markers, cross-language variants), collapsing duplicate entries into one canonical "En emisión"/library row.
- **Six sites**, up from three — JKanime, GogoAnime, and AnimeLand joined the adapter registry.
- **Google Drive backup & restore**, configurable entirely in-app: paste a Desktop OAuth client into Settings, no rebuild. Automatic daily backups; restoring onto a new machine finds the backup by name.
- **Stats accuracy pass.** Franchise roll-up fixes long-runners the site splits into per-arc rows (One Piece counted only one arc before), real-vs-estimated watch counts that never double-count, local-time activity buckets, and honest empty states.
- **A rebuilt design system.** Bundled Inter variable font, one consistent type/spacing/motion scale, normalized controls, loading skeletons, and a fully theme-aware light mode.

---

## Why the odd design choices

- **A WebView2 window instead of `reqwest`.** Cloudflare requires a real JS engine; a spoofed-user-agent HTTP client never clears the challenge. Only normal REST APIs (AniList, Google Drive) use `reqwest`.
- **Covers fetched one at a time, only for followed series.** Requesting ~150 posters at once reads as scraping abuse and gets rate-limited regardless of a valid session. One cover per followed series per refresh, decoded via an offscreen `<canvas>` and stored as a `data:` URI.
- **Mirror fallback doesn't trust a `200`.** A mirror can return HTTP 200 while rendering a totally different, incompatible site. Fallthrough continues until a mirror actually *parses* into data, not merely until the server answers.
- **Refresh skips series that can't have changed.** Using the airing schedule's own release metadata, a quiet refresh went from ~510s (scraping every followed series) to ~1.5s (typically one fetch). The skip rule is a pure, unit-tested function — a bug there would silently stop the app from doing its one job.
- **Concurrency is one app-wide ceiling, not per-command.** Every scraper window — refresh, discover, a one-off detail fetch — draws from a single semaphore (4 permits). Per-caller limits looked fine in isolation and still stacked into stalls and `ExecuteScript` timeouts when two flows ran at once. It's deliberately modest: unnaturally-parallel requests to one site are exactly what bot detection keys on, and the bottleneck is the site, not the PC.
- **Browsing and swiping never scrape.** The streaming site is hit only for the airing scan and on-demand for a single title (opening its detail, following it, or marking it seen). Discover, the Catalog, and the "Want" swipe stay entirely local against the SQLite catalog.
- **Backup credentials live in local SQLite, not baked into the build.** A Desktop OAuth client's id and secret are non-confidential by design (which is why the flow uses PKCE), so storing them next to the refresh token adds no exposure — and means no rebuild to set backup up.

---

## Tech stack

- **Shell:** [Tauri v2](https://tauri.app/) (Windows, WebView2)
- **Backend:** Rust — [`rusqlite`](https://github.com/rusqlite/rusqlite) (bundled SQLite), [`scraper`](https://github.com/causal-agent/scraper) for HTML parsing, `reqwest` for the AniList and Google Drive APIs
- **Frontend:** React 19 + TypeScript + Vite, a hand-written light/dark design system (no UI library) on a bundled Inter variable font, `react-force-graph-3d` / three.js for the stats graph
- **i18n:** a dependency-free catalog layer (English / Spanish), compile-time-checked for full key coverage

---

## Getting started

**Prerequisites**

- [Rust](https://rustup.rs/) (stable, `x86_64-pc-windows-msvc`)
- **Visual Studio 2022 Build Tools** with the C++ workload — `rusqlite`'s `bundled` feature compiles SQLite from C
- [Node.js](https://nodejs.org/) (with npm)

**Run it (dev, hot-reload)**

```bash
npm install
npm run tauri dev
```

**Build & check**

```bash
# Backend
cargo build   --manifest-path src-tauri/Cargo.toml
cargo test    --manifest-path src-tauri/Cargo.toml

# Frontend
npx tsc --noEmit
npm run build
```

On first launch, add a site in **Settings** and let it scan. The app's SQLite database
lives at `%APPDATA%\com.ernes.aot-scaffold\animeontrack.sqlite`. To enable cloud backup,
follow [`docs/google-drive-setup.md`](docs/google-drive-setup.md).

---

## Project layout

```
src-tauri/src/
  scraper_engine.rs   hidden-WebView2 fetch (Cloudflare) + cover decoding
  adapter/            SiteAdapter trait + 6 site implementations
  commands/           Tauri commands (the app's API surface)
  db/                 rusqlite schema + queries (series, episodes, catalog, stats)
  anilist.rs          AniList catalog sync + by-id backfill (reqwest)
  backup/             Google Drive backup: OAuth (PKCE), snapshot, staged restore
  matching.rs         fuzzy + exact title matching (site <-> catalog)
  player.rs           episode/info-link opening (app-owned window)
src/
  api.ts              typed wrapper over every invoke() call
  views/*.tsx         one file per screen
  i18n/               en/es/ca catalog + provider
  styles.css          the design system (tokens, components, light/dark)
docs/                     setup docs (Google Drive backup)
```

---

## Disclaimer

AnimeOnTrack is a personal, educational project. It scrapes third-party streaming sites
that the author does not operate, host, or endorse; it stores no media and streams nothing
itself — it only reads publicly-rendered pages to detect new episodes and opens links in a
window. You are responsible for how you use it and for complying with the laws and terms
that apply to you.

---

## License

Licensed under the **GNU General Public License v3.0 or later** — see [`LICENSE`](LICENSE).

You may use, study, share, and modify this software, but any distributed derivative must
also be released under the GPL. It comes with **no warranty** of any kind.

<div align="center">
<img src="https://capsule-render.vercel.app/api?type=waving&color=0:4aa8ff,100:0b1521&height=120&section=footer&animation=fadeIn" width="100%" alt="" />
</div>
