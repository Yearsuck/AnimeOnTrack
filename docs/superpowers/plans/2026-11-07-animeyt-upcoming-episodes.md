# AnimeYT: unaired ("PRÓXIMO") episodes shown as pending

Date: 2026-11-07. Status: implemented in PR (2 commits). Commit 2 cleans inside `apply_episode_diff` (all call sites), gated to animeytx.

## Problem
animeyt.cc lists the next, unaired episode on `/tv/<slug>/` as a normal `article.aniyt-episode-card` that
links to a real post, but the card carries the class `is-upcoming` (visible text "PRÓXIMO · N ... Próximo") and
the post only shows a countdown. `animeytx::new_episodes` (`src-tauri/src/adapter/animeytx.rs:162`) keeps every
card, so the app inserts the unaired episode as unseen and it shows in the pending list.

Evidence (user DB copy, 2026-11-07): animeytx has one episode more than tioanime/animeflv for Shinja Zero
(5 vs 4), Overgeared/Tempal (7 vs 6), Kashita Maryoku (6 vs 5); the extra row is the only unseen one, has a
URL and no `released_at`. The user confirmed the page only shows the countdown. Checked live: the first card
of `/tv/shinja-zero-...` has classes `aniyt-episode-card is-upcoming` and text "PRÓXIMO · 5".
Not verified: whether `is-upcoming` is in the server HTML or added by page JS (the scraper reads the rendered
DOM through WebView2, so either works); Goblin ("completely corrupted") has clean data, to re-check after this fix.

Agent research reports (not trusted): the "+1 in the schedule" cause is wrong (deliberate, documented);
season grouping in `merged_pending_episodes` and `distinct_extension` are real code smells but do not
explain these rows. Out of scope here, noted only.

## Changes (one `fix/animeyt-upcoming-episodes` branch, 2 atomic commits)
1. `fix(animeytx): skip unaired episode cards` — in `new_episodes`, skip cards whose class list contains
   `is-upcoming`. Test with a new fixture trimmed from the real page (one `is-upcoming` card + aired cards);
   assert it is absent. TDD.
2. `fix(episodes): drop stored unaired placeholders` — in `apply_episode_diff`: for animeytx series, delete rows that are unseen, undated, without download path or status, and are the slot right after the highest listed number (at most +1, so a partly rendered page cannot wipe real rows).
   No bulk update of live data. Needs a backup of the DB before the user runs it on real data (sandbox first via
   `tools/sandbox`).

## Verification
`cargo test`, `cargo clippy --all-targets -- -D warnings`, `npx tsc --noEmit`; run on the sandbox copy of the
real DB and check the 3 series lose only the extra episode.

## Delegation
agy `gemini-3.8-flash-high` (Opus has no quota): commit 1 is mechanical; commit 2 touches the DB, so I write
it here.
