# Descubrir recommendation engine (taste-scored, not random) — design

**Date:** 2026-07-12
**Branch to implement on:** `feat/discover-recommendation`
**Depends on:** Task 4 (`feat/discover-exclude-followed`, merged) — reuses its batch
`survivors` vec and engaged-title exclusion set.
**Status:** approved (autonomous batch)

## Problem

The Descubrir deck cards are effectively random within a taste-weighted genre. The user wants
recommendations driven by the genres AND the format (type) they watch most — a good, optimized,
local (no-scrape) algorithm that doesn't collapse into a single genre.

## What exists (verified)

- `discover_catalog_card` (`commands.rs` L1733): builds candidate genres
  (`filter_candidate_genres`), gets `get_genre_affinity` (HashMap<genre,score>: +2 followed,
  +1 want, −1.5 discarded, canonicalized via `genres::canonical_genre`), then loops up to
  `MAX_GENRE_ATTEMPTS=5`: `weighted_pick_index` (swipe.rs:30, time-seeded; uniform fallback when
  all weights ≤0) picks a genre, `random_catalog_anime_in_genre` returns a card.
- After Task 4, `random_catalog_anime_in_genre` fetches a batch (`LIMIT 40`), filters into a
  `survivors` vec (excluded by anilist_id + normalized engaged titles + quality floor + format
  bans), and currently returns the first survivor.
- `get_type_stats` (db.rs:667): `[(kind, count)]` over followed series — the format signal.
- `CatalogAnime` carries `format`, `genres` (via `list_catalog_genres`), `popularity`,
  `average_score`.

## Design — two-layer taste scoring

Keep the cheap indexed genre-weighted pick for the OUTER genre choice; replace the pure-random
inner choice with a per-candidate score over the `survivors` batch.

### 1. Genre-weight dampening (avoid single-genre collapse)

`weighted_pick_index` uses raw affinity sums, so one dominant genre (many follows) swamps the
outer pick. Before feeding weights in, apply **sub-linear dampening**: `w' = max(0, score)^0.6`.
Rationale: an exponent in (0,1) compresses large leads (a genre with score 20 vs 4 becomes
6.03 vs 2.30 instead of 20 vs 4 — still preferred, no longer monopolizing) while preserving
order. 0.6 chosen as a middle ground; unit-tested via the distribution property below, tunable
by one constant `GENRE_WEIGHT_EXPONENT = 0.6`. Negative/zero scores stay 0 (uniform fallback
still fires for a cold start, unchanged).

### 2. Format (type) affinity

New `db` helper reuse: build a format-affinity map from `get_type_stats` — normalize counts to
weights in [0,1]: `format_affinity[kind] = count / max_count`. A brand-new user (no follows) →
empty map → format term contributes 0 (pure genre/quality behavior).

### 3. Per-candidate score over the survivors batch

New **pure** fn `score_candidate(cand, genre_affinity, format_affinity, chosen_genre) -> f64`
(in `commands.rs` or a small `recommend.rs` module, unit-testable without DB/State):
```
genre_term  = Σ over cand.genres of dampened(max(0, genre_affinity[g]))   // taste overlap,
              excluding the already-chosen outer genre to reward SECONDARY overlap
format_term = format_affinity.get(cand.format).unwrap_or(0.0)
quality_term= (cand.average_score.unwrap_or(0) as f64) / 100.0            // 0..1 mild tiebreak
score = W_GENRE*genre_term + W_FORMAT*format_term + W_QUALITY*quality_term
```
Weights (constants, documented): `W_GENRE=1.0, W_FORMAT=0.6, W_QUALITY=0.25`. Genre overlap
leads; format meaningfully nudges; quality is a gentle tiebreak so a beloved-genre obscure title
still beats a mediocre one but genre/format dominate.

### 4. Non-deterministic pick among the top of the batch

Do NOT strict-argmax (repetitive, always the same card until decided). Instead: sort survivors
by `score_candidate` desc, take the top `RECOMMEND_TOP_K = 8` (or all if fewer), and
`weighted_pick_index` over THEIR scores (add a small epsilon so all top-k stay eligible). Keeps
the deck fresh while biasing hard toward taste. Reuses the existing time-seeded RNG (no new dep).

### 5. Wiring

`random_catalog_anime_in_genre` gains the two affinity maps + chosen genre and returns the
weighted-top-k pick instead of `.first()`. `discover_catalog_card` builds `genre_affinity`
(already does) and `format_affinity` (new, once per call) and threads both down. Cold start
(both maps empty / all ≤0) → behaviour reduces to the current quality-floored uniform pick.

### 6. (Optional, if cheap) Descubrir "Recomendado / Aleatorio" toggle

A segmented control in `SwipeView` persisted in localStorage; "Aleatorio" passes empty affinity
maps to fall back to random. Implement only if it doesn't balloon scope; otherwise default to
recommend and note as skipped. New i18n keys both catalogs if added.

## Acceptance criteria (verifiable without live UI)

1. `cargo test --manifest-path src-tauri/Cargo.toml` green.
2. Pure-fn unit tests for the scorer:
   - A candidate whose genres/format match the user's top taste outscores a same-genre
     candidate that doesn't.
   - Format affinity changes ranking: two candidates equal on genres, the one whose format the
     user watches more scores higher.
   - Dampening property: given one genre with a huge affinity and several moderate ones, the
     dampened outer weights do NOT make the huge genre's selection probability ≥ some
     collapse threshold (assert the huge genre's dampened weight / total < e.g. 0.7, whereas
     raw would exceed it) — i.e. no single-genre monopoly.
   - Cold start (empty maps) degrades to non-panicking uniform behavior.
3. Respects Task-4 exclusions (engaged titles never scored/returned) and bans — assert an
   engaged/banned title never surfaces even if it would score highest.
4. `npx tsc --noEmit` + `npm run build` (frontend only if the optional toggle is added).

## Perf

All local against the already-synced `anilist_catalog` (no scraping — honors
project-scraping-scope). One extra `get_type_stats` query + scoring a ≤40-row batch per card:
negligible.

## Verify live (NOT tool-reachable — state honestly)

Relaunch, swipe the deck, confirm cards skew toward the user's dominant genres/formats without
being all one genre. Cannot be screenshot-verified here.

## Out of scope

Cross-site (Task 6). Catalog browse ranking. Any AniList network call at swipe time.
