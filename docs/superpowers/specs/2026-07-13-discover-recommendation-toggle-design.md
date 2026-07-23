# Discover — activate the recommendation engine (Recomendado / Aleatorio toggle)

**Date:** 2026-07-13
**Branch to implement on:** `feat/discover-recommendation-toggle`
**Type:** feature (expose existing engine). Backend param + frontend + i18n.
**Status:** approved (autonomous batch)

## Problem

The local recommendation engine (`src-tauri/src/recommend.rs`) is already wired into
`discover_catalog_card` (genre affinity + format affinity + quality → `pick_recommended`), but
there is **no UI to turn it on/off**, and the deck is *always* in recommended mode. The user
perceives "there is no recommendation algorithm" because nothing signals it and there's no
contrast with a plain-random mode. Add a **Recomendado / Aleatorio** toggle, persisted, that
selects between taste-weighted and uniform-random picking. 100% local — never scrapes (see
[[project-scraping-scope]]).

## What exists (verified)

- `discover_catalog_card` (`commands.rs:1816`) always builds `affinity =
  get_genre_affinity(src)` and `format_affinity = format_affinity_from_type_stats(get_type_stats)`,
  then does a taste-weighted **outer** genre pick (`dampen_genre_weight` weights →
  `weighted_pick_index`) and calls `db.random_catalog_anime_in_genre(genre, banned_formats,
  excluded, affinity, format_affinity)`.
- `random_catalog_anime_in_genre` (`db.rs:1584`) pulls a `BATCH_SIZE=40` **`ORDER BY RANDOM()`**
  batch (already exclusion/ban/quality-floored), backfills genres, then `recommend::pick_recommended`
  scores + samples top-`RECOMMEND_TOP_K`. **With empty affinity maps the score reduces to the
  quality term**, so "empty maps" alone is NOT uniform-random — it still biases toward high
  `average_score`. Genuine random needs to bypass scoring.
- `swipe::weighted_pick_index` falls back to **uniform** when all weights ≤ 0 (verified test
  `weighted_pick_index_falls_back_to_uniform_when_all_non_positive`) — so an empty genre-affinity
  map already makes the outer genre pick uniform.
- Frontend: `discoverCatalogCard = () => invoke("discover_catalog_card")` (`api.ts:143`), called
  by `fillQueue` in `Descubrir.tsx`. Persistence pattern to mirror: `src/theme.ts` / `src/i18n`
  (localStorage key, module-level getter/setter).

## Design

### Backend — a `recommended: bool` command param

- `discover_catalog_card(state, recommended: bool)`:
  - `recommended == true`: current behavior unchanged (build both affinity maps, taste-weighted).
  - `recommended == false`: pass **empty** `genre_affinity`/`format_affinity` maps → the outer
    genre pick degrades to uniform (via the all-≤0 fallback), and thread `recommended` into
    `random_catalog_anime_in_genre` so the inner pick bypasses scoring.
- `random_catalog_anime_in_genre(..., recommended: bool)`: after building `survivors`, when
  `recommended == false` return `survivors.into_iter().next()` (the batch is already
  `ORDER BY RANDOM()`, so the first survivor is a uniform random pick) instead of
  `pick_recommended(...)`. Keeps all exclusion/ban/quality-floor logic identical — only the
  final selection differs. Update the doc comment.
- Tauri maps the camelCase JS arg `recommended` to the snake_case Rust param.
- Add/adjust unit tests: `random_catalog_anime_in_genre(recommended=false)` returns a survivor and
  never an excluded title (reuse the existing seeded-DB test harness for this fn if present); the
  recommend-mode path stays covered by existing `recommend.rs` tests.

### Frontend — persisted toggle

- New `src/discoverMode.ts` mirroring `theme.ts`: localStorage key `aot.discoverMode`, values
  `"recommended" | "random"`, default `"recommended"` (the engine is the point), module getter
  `getDiscoverMode()` / setter `setDiscoverMode()` + a `useDiscoverMode()` hook returning
  `[mode, setMode]` (re-render on change via a tiny event or state, same shape as `useLang`).
- `discoverCatalogCard` gains a `recommended: boolean` arg → `invoke("discover_catalog_card",
  { recommended })`. `fillQueue` passes `getDiscoverMode() === "recommended"`.
- **Changing the mode must clear the prefetch queue** so the next cards reflect the new mode
  immediately: on toggle, reset `queueRef.current = []` and `fillQueue()` again (don't discard the
  on-screen card). Wire this in `Descubrir.tsx`.
- UI for T3: a small **segmented control** (`.tabs`/segmented style already in the design system)
  placed in the swipe stage header near `TasteChips`, labelled Recomendado | Aleatorio. **Note:**
  T8 will relocate this (plus the genre/format filters) into a swipe-side panel — keep the control
  self-contained (a `<DiscoverModeToggle>` component) so T8 can move it without rewiring.

### i18n (es.ts + en.ts)

- `discover.modeRecommended` = "Recomendado" / "Recommended"
- `discover.modeRandom` = "Aleatorio" / "Random"
- `discover.modeAria` = "Modo del mazo" / "Deck mode"
- optional `discover.modeHint` explaining recommended uses your genres/formats locally.

## Acceptance criteria

- Toggle visible in Descubrir swipe view; selection persists across app restarts (localStorage).
- In **Recomendado**, the deck favors the user's affinity genres/formats (unchanged behavior). In
  **Aleatorio**, picks are uniform (no quality/affinity bias) — verifiable by a unit test on
  `random_catalog_anime_in_genre(recommended=false)` and by the outer pick using empty maps.
- Switching modes refreshes the upcoming deck (queue cleared + refilled) without a page reload.
- Never scrapes in either mode. `cargo test` green; `npx tsc --noEmit`; `npm run build` clean.

## Live verification (user)

Relaunch: toggle to Aleatorio, swipe a while (variety unbiased by score); toggle to Recomendado
(more on-taste); restart app → mode persisted. Chrome harness can preview the toggle markup.
