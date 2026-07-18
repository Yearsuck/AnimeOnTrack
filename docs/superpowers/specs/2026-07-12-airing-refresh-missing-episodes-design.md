# Airing refresh misses new episodes/anime — root cause & fix

**Date:** 2026-07-12
**Branch to implement on:** `feat/airing-refresh-detection`
**Type:** BUG (correctness). Root cause found with live DB evidence before any fix.
**Status:** approved (autonomous batch)

## Symptom

On "En emisión" → Actualizar, not all newly-released episodes of followed series are
detected. Quiet refresh (`refresh(false)`) silently stops updating some series; only
"Forzar recomprobación completa" (`refresh(true)`) recovers them.

## How the skip logic works (verified, `src-tauri/src/commands.rs`)

`refresh` (L746) does ONE airing-listing scan, upserts the scanned series (so their
`next_episode_at`/`site_episode_count` are fresh), then for every **followed** series calls
the pure fn `should_fetch_series` (L685) to decide whether its episode page can even have
changed, fetching only those that can. Signature:

```
should_fetch_series(force, listing_scanned, on_listing, next_episode_at,
                    site_episode_count, db_episode_count, last_checked_age_secs, now_unix)
```

Caller (L793-810) passes `db.episode_count(s.id)` — a **COUNT(*) of episode rows** — as
`db_episode_count`. The on-listing decision (L704-710):

```
if next_episode_at.is_some_and(|t| t <= now_unix) { return true; }
match site_episode_count {
    Some(next_number) => next_number > db_episode_count + 1,
    None => next_episode_at.is_none(),
}
```

Badge semantics (memory / CLAUDE): `.sb` = the **next** (upcoming) episode number = posted+1;
`data-rlsdt` = next release timestamp.

## Root cause — TWO bugs, both proven against the live DB

Live DB inspection (`%APPDATA%\com.ernes.aot-scaffold\animeontrack.sqlite`, 132 followed):

**Fact 0 — the countdown fast-path never fires.** `0` followed series *ever* have
`next_episode_at <= now` (the site rolls `data-rlsdt` forward the instant an episode posts —
0/all in the past). So detection rests ENTIRELY on the badge branch.

**Bug A — NULL badge (`??`) + future countdown = permanent skip.**
When `.sb` is non-numeric ("??"), `site_episode_count` is `None`, and the branch
`None => next_episode_at.is_none()` skips whenever a (future) countdown exists. Since the
countdown is always future, such a series is skipped **forever** on quiet refresh.
Live proof: *One Piece: Arco de Elbaph* (203 episodes, airing, on-listing, badge NULL,
`next_episode_at` in the future) — permanently skipped until a force refresh.

**Bug B — row COUNT used as if it were the max episode number.**
`site_episode_count(next) > db_episode_count(rows) + 1`. `db_episode_count` is a COUNT of
episode rows, but the site badge is a *next-episode-number*. Recaps, "0" episodes, specials,
and version re-uploads insert extra rows, so `COUNT(rows) > highest_real_episode_number`,
which cancels the `+1` detection margin. Live proof:
- *Tsue to Tsurugi no Wistoria Temporada 2*: episode rows = `0|Recap, 1..12` → COUNT=13,
  highest real ep = 12, badge = 13 (next). Caught-up skip is fine (`13 > 13+1` false). But
  when ep 13 posts, badge → 14 and `14 > 13+1=14` is **false** → **ep 13 is missed**, purely
  because the recap row inflated the count.
- *Tomb Raider King*: rows `1 | v2`, `2 | v1` (version re-uploads) — same inflation class.
Distribution among the 24 numeric-badge followed airing series: 21 have `badge = count+1`
(clean, correct), 3 have `badge = count` (all explained by a recap/`0`/version row, NOT a
different badge convention — verified by dumping their episode numbers).

## Fix

### Bug B — compare against the max episode NUMBER, not the row count

New method in `src-tauri/src/db.rs` (next to `episode_count`, L958):
```rust
/// Highest numeric episode number we have for a series (leading-int parse;
/// recaps/"0"/specials/version rows never inflate it), or 0 if none.
pub fn max_episode_number(&self, series_id: i64) -> Result<i64> {
    Ok(self.conn.query_row(
        "SELECT COALESCE(MAX(CAST(number AS INTEGER)), 0) FROM episodes WHERE series_id=?1",
        [series_id], |r| r.get(0),
    )?)
}
```
(SQLite `CAST('13' AS INTEGER)=13`, `CAST('1 | v2')=1`, `CAST('0 | Recap')=0`, `CAST('Recap')=0`.)

In `refresh` (L793-810) pass `db.max_episode_number(s.id)` instead of `db.episode_count(s.id)`.
Rename the `should_fetch_series` param `db_episode_count` → `db_max_episode_number` (its doc
comment too). The comparison expression is unchanged, so the existing numeric-badge unit
tests still pass (they feed raw integers). Keep `db.episode_count` (used elsewhere / has its
own test).

### Bug A — unknown badge falls back to a bounded time recheck

Add a constant near `OFF_LISTING_RECHECK_SECS` (L650):
```rust
/// On-listing followed series whose airing card shows a non-numeric "??" badge
/// give no count signal; the future countdown can't rule out a fresh episode
/// (it always rolls forward), so re-fetch at most this often instead of skipping.
const UNKNOWN_BADGE_RECHECK_SECS: i64 = 6 * 3600;
```
Change the `None` arm:
```rust
None => match last_checked_age_secs {
    None => true,
    Some(age) => age >= UNKNOWN_BADGE_RECHECK_SECS,
},
```
Cheap: only the handful of "??" series (≤7 observed) ever hit this, at most once per 6h.

### Keep

The `next_episode_at <= now` positive fast-path stays (correct, just dormant today).

## Explicitly rejected

- **Lowering the `+1` threshold to `> db_count`**: would re-fetch every caught-up
  `badge=count+1` series on *every* refresh, destroying the 510s→1.5s quiet-refresh win.
  `max_episode_number` is the correct fix, not threshold-lowering.
- **Blanket periodic recheck of all on-listing followed series**: bounds staleness but
  re-fetches caught-up series periodically → same perf regression. Not needed; the two
  targeted fixes cover every observed miss.

## Acceptance criteria (verifiable without live UI)

1. `cargo test --manifest-path src-tauri/Cargo.toml` green.
2. New `db.rs` unit test: `max_episode_number` on a series with rows `"0|Recap","1".."12"`
   returns `12` (not `13`); on `"1 | v2","2 | v1"` returns `2`; on an empty series returns `0`.
3. Updated/added `should_fetch_series` tests in `commands.rs` (mod tests ~L2303):
   - The existing test that asserts `(false,true,true,Some(NOW+DAY),None,5,None,NOW)` **skips**
     must FLIP to assert **fetch** (Bug A fix: unknown badge + never checked → fetch). Update it
     and its comment.
   - Add: unknown badge + future countdown + `last_checked_age < 6h` → skip; `>= 6h` → fetch.
   - Add a regression comment/test that with `db_max_episode_number` the Wistoria case
     (`badge=14`, `db_max=12`) fetches (`14 > 13`), while caught-up (`badge=13`,`db_max=12`)
     skips (`13 > 13` false).
4. Quiet-refresh perf unchanged: with all followed series caught-up and numeric badges, the
   fetch set is empty (only the 1 listing scan). Argue this from the code; no live timing needed.

## Verify live (NOT tool-reachable — state honestly)

Ask the user to relaunch, Actualizar in En emisión, and confirm One Piece: Elbaph and any
recap-carrying series now pick up their newest episode without a Forzar. Cannot be
screenshot-verified here.

## Out of scope

Scraping engine itself (perfect — untouched). Multi-site matching (Task 6). Non-`animeytx`
adapters share `should_fetch_series` and benefit automatically; no adapter change needed.
