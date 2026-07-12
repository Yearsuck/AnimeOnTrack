<div align="center">

<img src="https://capsule-render.vercel.app/api?type=waving&color=0:0b1521,100:4aa8ff&height=210&section=header&text=AnimeOnTrack&fontSize=56&fontColor=e9eff5&fontAlignY=38&animation=fadeIn&desc=Stop%20refreshing%20the%20site.%20Let%20the%20app%20refresh%20it.&descAlignY=58&descSize=16&descColor=9fd4ff" width="100%" alt="AnimeOnTrack" />

<img src="https://img.shields.io/badge/Tauri-2-4aa8ff?style=for-the-badge&logo=tauri&logoColor=e9eff5&labelColor=0b1521" alt="Tauri 2" />
<img src="https://img.shields.io/badge/Rust-stable-4aa8ff?style=for-the-badge&logo=rust&logoColor=e9eff5&labelColor=0b1521" alt="Rust" />
<img src="https://img.shields.io/badge/React-19-4aa8ff?style=for-the-badge&logo=react&logoColor=e9eff5&labelColor=0b1521" alt="React 19" />
<img src="https://img.shields.io/badge/TypeScript-strict-4aa8ff?style=for-the-badge&logo=typescript&logoColor=e9eff5&labelColor=0b1521" alt="TypeScript" />
<img src="https://img.shields.io/badge/SQLite-embedded-46d19e?style=for-the-badge&logo=sqlite&logoColor=0b1521&labelColor=0b1521" alt="SQLite" />
<img src="https://img.shields.io/badge/i18n-EN%20%C2%B7%20ES-46d19e?style=for-the-badge&labelColor=0b1521" alt="i18n EN/ES" />
<img src="https://img.shields.io/badge/platform-Windows-e9eff5?style=for-the-badge&logo=windows11&logoColor=0b1521&labelColor=0b1521" alt="Windows" />

<p><em>A Windows desktop app that tracks currently-airing anime, tells you the moment a new episode drops, and never loses your place — in English or Spanish.</em></p>

</div>

---

## The premise

There's a pirate streaming site with almost everything that's currently airing. It sits behind Cloudflare, so no ordinary HTTP client gets past the `403` — you need a real browser engine to solve the JavaScript challenge. **AnimeOnTrack** opens a hidden WebView2 window, waits for the challenge to clear, pulls the rendered HTML back into Rust, and tells you when something you follow has a new episode. You decide what to watch; the app decides when it needs to check.

It also keeps a full local mirror of the **AniList catalog (~22,000 titles)** so you can browse, filter, and discover instantly and offline — the pirate site is only ever touched when it actually has to be.

---

## Features

| Area | What it does |
|---|---|
| **Pending** | Flat queue of new episodes across everything you follow. Sortable by how many episodes each series still has left to watch. |
| **Airing** | The live scraped airing schedule — follow a series with one click. Newest-release-first. |
| **Library** | Followed series grouped by derived status — **Watching** (includes airing shows you're caught up on), **Plan to watch**, **Completed** (finished *and* fully watched). Airing filter, progress bars, keyboard-accessible, one-click reclassify. |
| **Discover** | A swipe deck (Want / Seen / Discard) drawn from the local catalog, weighted toward your taste. Multi-level undo of your last ~5 cards, plus configurable per-genre and per-type bans. |
| **Catalog** | The full AniList catalog, filterable by genre / format / score / length. Multi-select to batch-add many titles to *Want* or *Seen* at once; ℹ opens the AniList page. |
| **Stats** | Genre and type breakdowns plus an interactive 3D force graph of your library. |
| **Settings** | Language (English / Spanish), active site, mirror list, and a full-recheck escape hatch. |

**Watching is gap-free.** Marking an episode seen marks every earlier one seen; un-marking one un-marks every later one. No "I watched 10 but not 6–9" states — and it parses whatever numbering the site uses (`1x05`, `12.5`, …).

**Everything is reversible.** Unfollow, move between lists, un-mark "seen", return a Discover card to the deck — every classification has an inverse reachable from the UI.

**Links open in their own window.** Watching an episode or opening an info page spawns an app-owned window, separate from your everyday browser and its profile.

---

## Multi-site

The scraping layer is a pluggable `SiteAdapter` registry. Three sites ship today
(`animeytx`, `tioanime`, `animeflv`), each with its **own library** — switching sites in
Settings shows that site's followed series and progress without touching the others. A
per-site mirror list means that when the primary domain goes down, the app automatically
falls through to a working clone.

---

## Why the odd design choices

- **A WebView2 window instead of `reqwest`.** Cloudflare requires a real JS engine; a spoofed-user-agent HTTP client never clears the challenge. Only normal REST APIs (AniList) use `reqwest`.
- **Covers fetched one at a time, only for followed series.** Requesting ~150 posters at once reads as scraping abuse and gets rate-limited regardless of a valid session. One cover per followed series per refresh, decoded via an offscreen `<canvas>` and stored as a `data:` URI.
- **Mirror fallback doesn't trust a `200`.** A mirror can return HTTP 200 while rendering a totally different, incompatible site. Fallthrough continues until a mirror actually *parses* into data, not merely until the server answers.
- **Refresh skips series that can't have changed.** Using the airing schedule's own release metadata, a quiet refresh went from ~510s (scraping every followed series) to ~1.5s (typically one fetch). The skip rule is a pure, unit-tested function — a bug there would silently stop the app from doing its one job.
- **Browsing and swiping never scrape.** The pirate site is hit only for the airing scan and on-demand for a single title (opening its detail, following it, or marking it seen). Discover, the Catalog, and the "Want" swipe stay entirely local against the SQLite catalog.

---

## Tech stack

- **Shell:** [Tauri v2](https://tauri.app/) (Windows, WebView2)
- **Backend:** Rust — [`rusqlite`](https://github.com/rusqlite/rusqlite) (bundled SQLite), [`scraper`](https://github.com/causal-agent/scraper) for HTML parsing, `reqwest` for the AniList API
- **Frontend:** React 19 + TypeScript + Vite, a hand-written dark design system (no UI library), `react-force-graph-3d` / three.js for the stats graph
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
lives at `%APPDATA%\com.ernes.aot-scaffold\animeontrack.sqlite`.

---

## Project layout

```
src-tauri/src/
  scraper_engine.rs   hidden-WebView2 fetch (Cloudflare) + cover decoding
  adapter/            SiteAdapter trait + animeytx / tioanime / animeflv
  commands.rs         Tauri commands (the app's API surface)
  db.rs               rusqlite schema + queries
  anilist.rs          AniList catalog sync (reqwest)
  player.rs           episode/info-link opening (app-owned window)
src/
  api.ts              typed wrapper over every invoke() call
  views/*.tsx         one file per screen
  i18n/               en/es catalog + provider
  styles.css          the design system
docs/superpowers/specs/   design docs for every feature
```

---

## Disclaimer

AnimeOnTrack is a personal, educational project. It scrapes a third-party streaming site
that the author does not operate, host, or endorse; it stores no media and streams nothing
itself — it only reads publicly-rendered pages to detect new episodes and opens links in a
window. You are responsible for how you use it and for complying with the laws and terms
that apply to you. Please support creators through official, licensed channels.

---

## License

No license has been chosen yet — this is a personal project, so all rights are reserved by
the author until one is added.

<div align="center">
<img src="https://capsule-render.vercel.app/api?type=waving&color=0:4aa8ff,100:0b1521&height=120&section=footer&animation=fadeIn" width="100%" alt="" />
</div>
