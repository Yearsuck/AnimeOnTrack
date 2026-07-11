# README.md — polished English rewrite

**Date:** 2026-07-11
**Branch to implement on:** `feat/readme` (from `develop`, **LAST** — after every other
task merges, so the README documents the final, real state).
**Task:** #8. Documentation only (`README.md`, maybe `docs/` screenshots/gifs).

## Problem

`README.md` needs a polished, professional English rewrite reflecting what the app actually
does today (after the July 2026 batch + tasks #1–#7). Honest about the real code state — no
aspirational features.

## Approach

The implementer should research current best-practice OSS READMEs (WebSearch/WebFetch:
structure, badges, screenshots/GIFs, section ordering, "awesome-readme" patterns) and use
any available documentation skill. Keep it truthful to the codebase — verify each claimed
feature exists before writing it.

## Real features to cover (verify against code before claiming)

- **What it is:** Windows desktop app (Tauri v2 + Rust + React/TS) that tracks currently-
  airing anime scraped from a Cloudflare-fronted pirate streaming site, plus a full local
  AniList catalog. Follows series, detects new episodes into a pending-to-watch queue.
- **Scraping engine:** hidden WebView2 window drives past Cloudflare, returns rendered HTML
  to Rust (`scraper_engine.rs`); condition-based readiness; covers fetched one-at-a-time for
  followed series only. (See CLAUDE.md — do not overstate; explain the constraint honestly.)
- **Multi-site adapters:** `SiteAdapter` registry (animeytx / tioanime / animeflv), per-site
  library, site switcher in Settings.
- **Full AniList catalog** (~22k titles) synced locally, filterable Catálogo with
  multi-select batch actions (task #6).
- **Descubrir (swipe):** taste-weighted deck, catalog-only, multi-level undo cache and
  configurable genre/type bans (task #3).
- **Library:** derived status sections (Viendo incl. caught-up airing, Plan, Completadas)
  with airing filter (task #4), keyboard-accessible.
- **Pending queue:** gap-free watch enforcement, sortable by episodes-remaining (task #5).
- **Reversibility:** every classification is undoable/movable (task #2).
- **Links** open in an app-owned window, independent of the user's browser (task #7).
- **Stats:** genre/type stats + a 3D force graph.
- **i18n:** Spanish + English, switchable in Ajustes (task #1).
- **Scraping scope** honesty: site is hit only for the airing scan + on-demand
  (detail/follow/seen); browsing/swiping never scrapes.

## Structure (suggested, adapt to research)

Title + one-line tagline + badges → hero screenshot/GIF → Features → Screenshots →
Tech stack → How the scraping works (the interesting part) → Getting started (build/run:
`npm run tauri dev`, cargo/npm commands from CLAUDE.md) → Project layout → Data/where the
DB lives → Legal/ethical note (pirate-site scraping — a plain disclaimer) → License.

## Acceptance criteria

1. README is in English, well-structured, renders cleanly on GitHub (verify Markdown).
2. Every feature listed exists in the code (spot-check claims).
3. Build/run instructions match CLAUDE.md and actually work.
4. Includes at least one real screenshot/GIF of the app (capture from the running app if
   feasible; otherwise leave a clearly-marked placeholder and note it in the summary).
5. Honest legal/ethical disclaimer about the scraped source.

## Live verification

Render/preview the Markdown (or push-preview locally) to confirm layout, badges, and image
links resolve. If screenshots are captured from the running app, note which are real vs
placeholder in the completion summary.

## Out of scope

- Publishing to any registry / pushing to origin.
- CI badges for pipelines that don't exist (only real badges).
