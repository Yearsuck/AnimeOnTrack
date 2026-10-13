<a id="readme-top"></a>

<!-- PROJECT SHIELDS -->
<div align="center">

[![Latest Release][release-shield]][release-url]
[![Total Downloads][downloads-shield]][downloads-url]
[![Stars][stars-shield]][stars-url]
[![Forks][forks-shield]][forks-url]
[![Issues][issues-shield]][issues-url]
[![License][license-shield]][license-url]
[![Platform][platform-shield]][platform-url]

</div>

<!-- PROJECT LOGO & HEADER -->
<br />
<div align="center">
  <a href="https://github.com/Yearsuck/AnimeOnTrack">
    <img src="src-tauri/icons/128x128.png" alt="AnimeOnTrack Logo" width="96" height="96" />
  </a>

  <img src="https://capsule-render.vercel.app/api?type=waving&color=0:0b1521,100:4aa8ff&height=190&section=header&text=AnimeOnTrack&fontSize=52&fontColor=e9eff5&fontAlignY=36&animation=fadeIn&desc=Stop%20refreshing%20the%20site.%20Let%20the%20app%20refresh%20it.&descAlignY=58&descSize=15&descColor=9fd4ff" width="100%" alt="AnimeOnTrack Banner" />

  <p align="center">
    <strong>A high-performance, local-first anime tracker and offline downloader for Windows.</strong>
    <br />
    Tracks currently-airing anime, notifies you the second a new episode drops, downloads episodes for offline viewing, and unifies your progress across six streaming sites — without an account or cloud lock-in.
    <br />
    <br />
    <a href="https://github.com/Yearsuck/AnimeOnTrack/releases/latest"><strong>⬇ Download Installer (v0.7.0) »</strong></a>
    <br />
    <br />
    <a href="#about-the-project">About The Project</a>
    ·
    <a href="#key-features">Key Features</a>
    ·
    <a href="#supported-streaming-sites">Supported Sites</a>
    ·
    <a href="#getting-started">Getting Started</a>
    ·
    <a href="https://github.com/Yearsuck/AnimeOnTrack/issues/new?labels=bug">Report Bug</a>
    ·
    <a href="https://github.com/Yearsuck/AnimeOnTrack/issues/new?labels=enhancement">Request Feature</a>
  </p>
</div>

<!-- TABLE OF CONTENTS -->
<details>
  <summary><strong>Table of Contents</strong></summary>
  <ol>
    <li>
      <a href="#about-the-project">About The Project</a>
      <ul>
        <li><a href="#the-premise">The Premise</a></li>
        <li><a href="#built-with">Built With</a></li>
      </ul>
    </li>
    <li><a href="#key-features">Key Features</a></li>
    <li><a href="#supported-streaming-sites">Supported Streaming Sites</a></li>
    <li>
      <a href="#getting-started">Getting Started</a>
      <ul>
        <li><a href="#prebuilt-binaries">Prebuilt Binaries</a></li>
        <li><a href="#prerequisites">Prerequisites</a></li>
        <li><a href="#installation--development">Installation & Development</a></li>
        <li><a href="#build--test-suite">Build & Test Suite</a></li>
      </ul>
    </li>
    <li><a href="#architecture--engineering-highlights">Architecture & Engineering Highlights</a></li>
    <li><a href="#project-layout">Project Layout</a></li>
    <li><a href="#whats-new-in-v070">What's New in v0.7.0</a></li>
    <li><a href="#contributing">Contributing</a></li>
    <li><a href="#a-note-on-piracy">A Note on Piracy</a></li>
    <li><a href="#disclaimer">Disclaimer</a></li>
    <li><a href="#license">License</a></li>
    <li><a href="#acknowledgments">Acknowledgments</a></li>
  </ol>
</details>

---

## About The Project

### The Premise

Streaming sites host almost every title currently airing, but their catalogs and release schedules sit behind Cloudflare anti-bot verification. Standard HTTP clients hit `403 Forbidden` because passing the Turnstile/JavaScript challenge requires a real, modern rendering engine.

**AnimeOnTrack** embeds a hidden WebView2 browser instance to transparently solve challenges, parses the rendered DOM inside native Rust, and alerts you the instant an episode drops for any series in your library. You decide what to watch; the app handles the tracking.

Crucially, the app maintains an **offline mirror of the AniList catalog (~22,000 titles)** inside an embedded SQLite database. Browsing, filtering, franchise aggregation, and deck discovery run locally at sub-millisecond speeds — the streaming site is only queried when strictly required.

<p align="right">(<a href="#readme-top">back to top</a>)</p>

### Built With

* [![Tauri][tauri-shield]][tauri-url]
* [![Rust][rust-shield]][rust-url]
* [![React][react-shield]][react-url]
* [![TypeScript][typescript-shield]][typescript-url]
* [![Vite][vite-shield]][vite-url]
* [![SQLite][sqlite-shield]][sqlite-url]
* [![Three.js][threejs-shield]][threejs-url]

<p align="right">(<a href="#readme-top">back to top</a>)</p>

---

## Key Features

### 💾 Offline Episode Downloads & Auto-Cleanup
* **Background Downloader**: Download episodes directly for offline viewing with real-time transfer speeds, ETA, and progress bar tracking.
* **Auto-Delete on Seen**: Optional smart cleanup toggle — automatically removes the local media file once an episode is marked as watched to save disk space.
* **Pause, Resume & Cancel**: Full control over your active download queue directly from the UI.

### 🖼️ AniList Spotlight & Landscape Banners
* **Wide Cinematic Banners**: The Airing Spotlight carousel fetches high-definition landscape banners directly from AniList's GraphQL API.
* **Graceful Cover Fallback**: Automatic fallback to high-resolution portrait posters whenever an official banner is unavailable.

### ⚡ Intelligent Airing Detection
* **Sub-2-Second Quiet Scans**: Release schedule heuristics skip titles that cannot have updated yet, cutting refresh cycles from ~510s down to ~1.5s.
* **App-Wide Semaphore**: Scraper requests are capped to 4 concurrent permits, eliminating bot-detection triggers and WebView2 IPC stalls.

### 🔗 Canonical Tracking Across 6 Streaming Sites
* **Unified Watch History**: Followed series, watched episodes, and backlogs are shared canonically across all supported sites using AniList ID resolution.
* **Seamless Site Switching**: Switching between streaming providers updates video links without disturbing your library progress or statistics.
* **Automatic Mirror Failover**: Each adapter contains a fallback mirror list that automatically bypasses dead domains or DNS takedowns.

### 🃏 Discover Swipe Deck with Multi-Level Undo
* **Tinder-Style Swiping**: Rapidly sort recommendations into *Want*, *Seen*, or *Discard* with keyboard shortcuts or mouse gestures.
* **Taste-Weighted Algorithm**: Surfaces recommendations based on your completed and high-rated franchises.
* **Precision Filtering**: Filter by release date ranges (including future upcoming releases), score floor, formats (TV, Movie, OVA, ONA), and genres, backed by a ~5-step undo history.
* **Watchlist Manager**: The Want / Discarded / Seen lists open as a grid or dense list with score, year, format, status and genre filters, sorting, quick groups and bulk actions.

### 📊 Watch Insights & Interactive 3D Force Graph
* **Comprehensive Stats**: View total hours watched, completion percentages, activity timelines, and genre distributions.
* **Franchise Rollups**: Multi-season epics (e.g., *One Piece*, *Boruto*) consolidate into single franchises instead of fragmented arcs.
* **3D Knowledge Graph**: Explore your entire anime universe with an interactive, GPU-accelerated 3D force-directed graph powered by Three.js: node and link weights follow watched episodes, click to select, right-click for a context menu.
* **Yearly Activity Heatmap**: A full-year contribution-style grid with quartile-based intensity levels.

### ☁️ Encrypted Google Drive Backup
* **Zero Cloud Lock-In**: End-to-end database backups saved to a private, hidden folder in your own Google Drive.
* **PKCE OAuth**: Easy one-click setup directly within Settings without custom compilation or baked-in secrets.
* **Cross-Machine Migration**: Restoring on a fresh installation detects your snapshot and stages it safely.

### 🌐 Tri-Lingual Support
* Full localization in **English (`en`)**, **Spanish (`es`)**, and **Catalan (`ca`)**, verified at compile time with 100% key coverage.

<p align="right">(<a href="#readme-top">back to top</a>)</p>

---

## Supported Streaming Sites

Six adapters are bundled out of the box, all conforming to a pluggable `SiteAdapter` interface:

| Site | Default Domain | Adapter ID | Primary Language | Mirror Failover |
|:---|:---|:---|:---:|:---:|
| **AnimeYT** | `animeyt.cc` | `animeytx` | Spanish (Castilian / LatAm) | Automatic |
| **TioAnime** | `tioanime.com` | `tioanime` | Spanish (Castilian / LatAm) | Automatic |
| **AnimeFLV** | `www4.animeflv.net` | `animeflv` | Spanish (Castilian / LatAm) | Automatic |
| **JKanime** | `jkanime.net` | `jkanime` | Spanish (Castilian / LatAm) | Automatic |
| **GogoAnime** | `gogoanime.by` | `gogoanime` | English (Sub / Dub) | Automatic |
| **AnimeLand** | `w7.animeland.tv` | `animeland` | English (Dubbed) | Automatic |

> [!TIP]
> Adding support for a new site requires only a single `SiteInfo` registry entry and a `SiteAdapter` implementation — no frontend modifications required.

<p align="right">(<a href="#readme-top">back to top</a>)</p>

---

## Getting Started

### Prebuilt Binaries

1. Navigate to the [**Releases Page**](https://github.com/Yearsuck/AnimeOnTrack/releases/latest).
2. Download the latest Windows setup installer (`AnimeOnTrack_*_x64-setup.exe` or `.msi`).
3. Run the installer. On first launch, select your preferred streaming site in **Settings** and initiate your initial library scan.

> [!NOTE]
> macOS and Linux builds compile via Tauri stubs, but automated Cloudflare bypass scraping is currently exclusive to Windows via WebView2 (refer to [`docs/cross-platform-porting.md`](docs/cross-platform-porting.md)).

### Prerequisites

To build AnimeOnTrack from source, ensure your environment has:

* **[Rust](https://rustup.rs/)** (stable toolchain, `x86_64-pc-windows-msvc`)
* **[Node.js](https://nodejs.org/)** (v18+ with `npm`)
* **Visual Studio 2022 Build Tools** with the *Desktop development with C++* workload (required for compiling bundled SQLite from C via `rusqlite`)

### Installation & Development

1. Clone the repository and checkout the `develop` branch:
   ```bash
   git clone https://github.com/Yearsuck/AnimeOnTrack.git
   cd AnimeOnTrack
   ```

2. Install frontend dependencies:
   ```bash
   npm install
   ```

3. Launch the desktop application with live hot-reloading:
   ```bash
   npm run tauri dev
   ```

### Build & Test Suite

Verify both backend and frontend layers prior to opening pull requests:

```bash
# Rust backend: Run all 680+ automated unit & integration tests
cargo test --manifest-path src-tauri/Cargo.toml

# Rust backend: Strict linter check
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings

# Frontend: Typecheck & production bundle build
npx tsc --noEmit
npm run build
```

<p align="right">(<a href="#readme-top">back to top</a>)</p>

---

## Architecture & Engineering Highlights

```mermaid
flowchart TD
    subgraph UI ["Frontend (React 19 + TypeScript)"]
        Views["Views (Airing, Library, Discover, Downloads, Stats)"]
        Store["State & i18n (en / es / ca)"]
        ThreeGraph["3D Force Graph (Three.js)"]
    end

    subgraph Core ["Tauri IPC & Rust Engine"]
        Commands["Tauri Commands Layer"]
        Limiter["Concurrency Semaphore (4 Permits)"]
        Engine["Scraper Engine (WebView2)"]
        Downloader["Offline Downloader (Auto-Delete)"]
        AniList["AniList GraphQL Client"]
        GDoc["Google Drive Backup (PKCE)"]
    end

    subgraph Storage ["Local Storage"]
        DB[(Embedded SQLite DB)]
        Media[("Local Downloads / Video Cache")]
    end

    Views <--> Commands
    Commands --> Limiter
    Limiter --> Engine
    Commands --> Downloader
    Downloader --> Media
    Downloader --> DB
    Commands --> DB
    Commands --> AniList
    Commands --> GDoc
    Engine --> DB
```

* **WebView2 vs. `reqwest`**: Cloudflare Turnstile blocks spoofed User-Agents. AnimeOnTrack instantiates a hidden WebView2 window, lets the challenge execute legitimately, and harvests rendered HTML back into Rust.
* **Global Concurrency Semaphore**: All scraping tasks share a single 4-permit semaphore. This prevents rate-limiting IP bans and avoids IPC deadlocks.
* **Strict Gap-Free Progression**: Marking episode 12 as seen automatically marks episodes 1 through 11; un-marking episode 12 cascades forward. Numbering parsers gracefully handle notations like `1x05`, `12.5`, and multi-part specials.
* **Smart Airing Skip Predicate**: Refreshes skip series that cannot mathematically have updated based on their broadcast schedule, speeding up typical background checks to ~1.5s.
* **Local-First SQLite Database**: The application database lives locally at `%APPDATA%\com.animeontrack.app\animeontrack.sqlite`. No telemetry or mandatory cloud connections.

<p align="right">(<a href="#readme-top">back to top</a>)</p>

---

## Project Layout

```text
AnimeOnTrack/
├── src-tauri/
│   ├── src/
│   │   ├── adapter/         # SiteAdapter trait and 6 streaming implementations
│   │   ├── backup/          # Google Drive OAuth (PKCE), snapshot, and staged restore
│   │   ├── commands/        # Tauri command handlers (app API surface)
│   │   ├── db/              # rusqlite schema, migrations, queries, and catalog indexing
│   │   ├── anilist.rs       # AniList GraphQL sync, landscape banner & metadata resolver
│   │   ├── matching.rs      # Fuzzy and exact title matching algorithms
│   │   ├── player.rs        # Dedicated external player window lifecycle
│   │   └── scraper_engine.rs# Hidden WebView2 challenge solver & canvas cover decoder
│   ├── Cargo.toml           # Rust backend dependencies & build settings
│   └── tauri.conf.json      # Tauri application bundle configuration
├── src/
│   ├── components/          # Reusable design system primitives & UI widgets
│   ├── i18n/                # Zero-dependency catalog with full en/es/ca coverage
│   ├── views/               # Screen views (Airing, Library, Discover, Downloads, Stats)
│   ├── api.ts               # Strongly-typed wrapper over window.__TAURI__.invoke
│   └── styles.css           # Custom design system tokens, typography, and dark/light themes
├── docs/                    # Technical documentation and platform guides
└── package.json             # Frontend dependencies & scripts
```

<p align="right">(<a href="#readme-top">back to top</a>)</p>

---

## What's New in v0.7.0

* **Catalog filters**: score range (including everything under 60), release-date range with presets (2020s, 2010s, ...), status and sort, plus a score floor and date presets for the discover deck. "Hide watched" no longer freezes the app (about 18 s per call before, about 0.1 s now).
* **Discover lists redesigned**: grid or dense view, filters by score, year, format, status and genres, sorting, quick groups (airing now, top rated, short), bulk actions that keep failures selected for retry, and paged rendering for very long lists. The swipe deck no longer shows "no more anime" while its queue is still filling.
* **Statistics**: calmer layout with a headline KPI strip, a yearly activity heatmap that fills the row at any width, and a 3D graph without the root hub, with stronger weights, left-click selection (info panel and camera fly) and a right-click menu. Genre cards and graph nodes now show real covers.
* **Airing list cleanup**: finished OVAs, Specials and shows that stopped weeks ago are hidden unless you follow them or AniList confirms an upcoming episode.
* **Settings in tabs**: General, Episode sources, Data & backup and Maintenance, each with a plain-language description; mirrors, add-a-site and Google credentials live under "Advanced".

*(For detailed historical release notes, see the [Releases](https://github.com/Yearsuck/AnimeOnTrack/releases) tab).*

<p align="right">(<a href="#readme-top">back to top</a>)</p>

---

## Contributing

Contributions are what make the open source community such an amazing place to learn, inspire, and create. Any contributions you make are **greatly appreciated**.

This project follows the **Gitflow** branching workflow:
* `main`: Production-ready, tagged releases only (`vX.Y.Z`).
* `develop`: Active integration branch.
* `feat/*`, `fix/*`, `docs/*`: Short-lived feature and bugfix branches.

### Workflow

1. Fork the Project.
2. Create your Feature Branch off `develop`:
   ```bash
   git checkout develop
   git checkout -b feat/AmazingFeature
   ```
3. Commit your Changes using [Conventional Commits](https://www.conventionalcommits.org/):
   ```bash
   git commit -m "feat(scraper): add support for new streaming provider"
   ```
4. Verify tests and linting:
   ```bash
   cargo test --manifest-path src-tauri/Cargo.toml
   npm run build
   ```
5. Push to your branch:
   ```bash
   git push origin feat/AmazingFeature
   ```
6. Open a Pull Request targeting `develop`.

<p align="right">(<a href="#readme-top">back to top</a>)</p>

---

## A Note on Piracy

I'll say this plainly: I always advocate paying for the content you watch. Creators, studios, and the people who make anime don't live on air — if you can support them officially, you should, and I do.

Spanish- and English-language anime piracy has, for decades, been the **first door into anime** for most fans outside Japan. Without fansubs and community streaming, the medium simply would not be as global, accessible, or beloved as it is today. Furthermore, plenty of older and niche titles are never licensed or distributed officially in given regions — so for those, "just pay for it" simply isn't an option. You cannot support what nobody is selling you.

Support creators wherever you actually can, and understand this tool for what it is: a tracker built by someone who loves the medium, not an excuse to skip the official release when one exists.

<p align="right">(<a href="#readme-top">back to top</a>)</p>

---

## Disclaimer

AnimeOnTrack is an independent, open-source software project. It interacts with third-party streaming sites that the author does not operate, host, or endorse. The application stores no copyrighted media files on remote servers and hosts no streaming content — it parses publicly-rendered web pages to detect release updates and opens playback links in an isolated browser window. Users are solely responsible for their use of the software and compliance with local laws and regulations.

<p align="right">(<a href="#readme-top">back to top</a>)</p>

---

## License

Distributed under the **GNU General Public License v3.0 or later**. See [`LICENSE`](LICENSE) for more information.

```text
AnimeOnTrack - Cross-site anime tracker and manager
Copyright (C) 2026 Yearsuck and contributors

This program is free software: you can redistribute it and/or modify
it under the terms of the GNU General Public License as published by
the Free Software Foundation, either version 3 of the License, or
(at your option) any later version.
```

<p align="right">(<a href="#readme-top">back to top</a>)</p>

---

## Acknowledgments

* [othneildrew/Best-README-Template](https://github.com/othneildrew/Best-README-Template) — for the outstanding README structure and inspiration.
* [AniList API v2](https://anilist.gitbook.io/anilist-api-v2-docs/) — for providing comprehensive anime metadata and banner artwork.
* [Tauri](https://tauri.app/) — for delivering a lightweight, secure native desktop shell.
* [rusqlite](https://github.com/rusqlite/rusqlite) & [SQLite](https://www.sqlite.org/) — for rock-solid embedded local data persistence.
* [react-force-graph-3d](https://github.com/vasturiano/react-force-graph) & [Three.js](https://threejs.org/) — for 3D library visualization.
* [Capsule Render](https://github.com/kyechan99/capsule-render) — for dynamic SVG banner artwork.
* [Shields.io](https://shields.io/) — for dynamic GitHub status badges.

<p align="right">(<a href="#readme-top">back to top</a>)</p>

<div align="center">
  <img src="https://capsule-render.vercel.app/api?type=waving&color=0:4aa8ff,100:0b1521&height=110&section=footer&animation=fadeIn" width="100%" alt="Footer Banner" />
</div>

<!-- MARKDOWN LINKS & IMAGES -->
[release-shield]: https://img.shields.io/github/v/release/Yearsuck/AnimeOnTrack?style=for-the-badge&color=4aa8ff&labelColor=0b1521&label=release
[release-url]: https://github.com/Yearsuck/AnimeOnTrack/releases/latest
[downloads-shield]: https://img.shields.io/github/downloads/Yearsuck/AnimeOnTrack/total?style=for-the-badge&color=46d19e&labelColor=0b1521&label=downloads
[downloads-url]: https://github.com/Yearsuck/AnimeOnTrack/releases
[stars-shield]: https://img.shields.io/github/stars/Yearsuck/AnimeOnTrack.svg?style=for-the-badge&color=4aa8ff&labelColor=0b1521
[stars-url]: https://github.com/Yearsuck/AnimeOnTrack/stargazers
[forks-shield]: https://img.shields.io/github/forks/Yearsuck/AnimeOnTrack.svg?style=for-the-badge&color=4aa8ff&labelColor=0b1521
[forks-url]: https://github.com/Yearsuck/AnimeOnTrack/network/members
[issues-shield]: https://img.shields.io/github/issues/Yearsuck/AnimeOnTrack.svg?style=for-the-badge&color=4aa8ff&labelColor=0b1521
[issues-url]: https://github.com/Yearsuck/AnimeOnTrack/issues
[license-shield]: https://img.shields.io/badge/license-GPL--3.0--or--later-46d19e?style=for-the-badge&labelColor=0b1521
[license-url]: https://github.com/Yearsuck/AnimeOnTrack/blob/main/LICENSE
[platform-shield]: https://img.shields.io/badge/platform-Windows%2010%20%7C%2011-e9eff5?style=for-the-badge&logo=windows11&logoColor=0b1521&labelColor=0b1521
[platform-url]: https://github.com/Yearsuck/AnimeOnTrack/releases

[tauri-shield]: https://img.shields.io/badge/Tauri_2-24C8D8?style=for-the-badge&logo=tauri&logoColor=white
[tauri-url]: https://tauri.app/
[rust-shield]: https://img.shields.io/badge/Rust_stable-DEA584?style=for-the-badge&logo=rust&logoColor=black
[rust-url]: https://www.rust-lang.org/
[react-shield]: https://img.shields.io/badge/React_19-20232A?style=for-the-badge&logo=react&logoColor=61DAFB
[react-url]: https://react.dev/
[typescript-shield]: https://img.shields.io/badge/TypeScript-3178C6?style=for-the-badge&logo=typescript&logoColor=white
[typescript-url]: https://www.typescriptlang.org/
[vite-shield]: https://img.shields.io/badge/Vite-646CFF?style=for-the-badge&logo=vite&logoColor=white
[vite-url]: https://vitejs.dev/
[sqlite-shield]: https://img.shields.io/badge/SQLite_3-003B57?style=for-the-badge&logo=sqlite&logoColor=white
[sqlite-url]: https://www.sqlite.org/
[threejs-shield]: https://img.shields.io/badge/Three.js-000000?style=for-the-badge&logo=three.js&logoColor=white
[threejs-url]: https://threejs.org/
