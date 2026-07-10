# Scraper performance: skip unchanged series, reuse hot windows

`refresh()` (`commands.rs`) walks every followed series (**119 today**), fetching each one's episode-list page through `scraper_engine::fetch_html`, which builds a **fresh WebView2 window per fetch**, navigates it, polls for readiness, extracts the HTML, and destroys the window. Chunks of 2 run concurrently (`REFRESH_CONCURRENCY`), globally capped by `SCRAPE_PERMITS` (2). It is slow, and the cost is per-series.

## Method: measure first, then optimize

This spec mandates **phase 0 before any optimization**. Do not tune what you have not measured.

Instrument `fetch_html` with `std::time::Instant` and `eprintln!` (same style as the existing `[scrape] poll N` logs), emitting one line per fetch with four numbers:
`window_build_ms`, `time_to_ready_ms` (+ poll count), `extract_ms`, `total_ms`.

Then run one full `refresh()` against the real site with the real 119 followed series, capture the log, and record in the final summary: total wall time, median/p90 per-fetch total, and the **share** of per-fetch time spent building the window vs. waiting for readiness vs. extracting.

That breakdown decides whether optimization B below is worth its risk. If window construction turns out to be, say, 5% of per-fetch time, **skip B and say so** — the win is elsewhere and a hot-window pool is pure added risk. Report the numbers either way.

## Verified facts (from `src-tauri/tests/fixtures/airing.html`, real captured markup)

Each airing card (`.bsx`) carries metadata the adapter currently throws away:

```html
<div class="bt">
  <span class="epx cndwn" data-cndwn="8859" data-rlsdt="1783350140">0d 2h 27m</span>
  <span class="sb Sub">2</span>
</div>
```

- `data-rlsdt` — **Unix timestamp of the next episode's release**. (Fixture captured 2026-07-06; its three values decode to 2026-07-06T15:02Z, 2026-07-06T18:44Z, 2026-07-07T14:20Z — all just ahead of capture time. Consistent with "next episode".)
- `data-cndwn` — seconds remaining until that release (redundant with `rlsdt`; ignore it, it's stale the moment it's parsed).
- `.sb` — the site's **episode count** for the series. Observed values: `2`, `14`, `??`. **Not always numeric** — `??` must parse to `None`, never panic or coerce to 0.

The implementer must confirm on live HTML that `data-rlsdt` behaves as described for a series whose next episode has **already** aired (does the card update, or does `rlsdt` go stale in the past?). If it goes stale in the past, that is exactly the "there may be a new episode" signal this design wants — but confirm rather than assume, and write down what you saw.

## Optimization A — skip series that cannot have changed (the big one)

The airing listing is **one fetch** and it already tells us, for every series on it, both the next-release time and the episode count. So:

1. Extend `SiteAdapter::parse_airing` (and `models::Series`) to carry `next_episode_at: Option<i64>` (unix) and `site_episode_count: Option<i64>`, parsed from `data-rlsdt` and `.sb`. Add both columns to `series` via `ensure_column`. `upsert_series` writes them; like `followed`, they are scan-owned.
2. `refresh()` starts by re-scanning the airing listing (1 fetch) to get fresh values, then for each followed series **skips the per-series fetch entirely** when either:
   - `next_episode_at` is in the future (no new episode can exist yet), **or**
   - `site_episode_count` is `Some(n)` and the DB already holds `n` episodes for that series.
3. A followed series **absent from the airing listing** (finished shows) has no fresh metadata. Fetch it at most once per `FINISHED_RECHECK` interval (e.g. 7 days, tracked by a `series.last_checked_at` column) instead of every refresh — a completed series does not sprout episodes.
4. Everything else (`update_series_url`, `new_episodes`, cover fetch, `backfill_series_genre_if_missing`) is unchanged and still runs only for series that were actually fetched. Covers and genre backfill keep their existing one-at-a-time discipline.
5. `emit_refresh_progress` must still report `X/Y` over **all** followed series, marking skipped ones as done immediately — the progress bar should visibly race through the skips, not stall.

Expected effect: a refresh with no new episodes anywhere drops from ~119 page fetches to **1**. This is the change that matters; it also reduces load on the site, which is the politeness constraint the whole codebase is built around.

**Correctness guard**: a bug here silently stops detecting new episodes — the app's entire purpose. So: add a `force: bool` argument to `refresh()` (default `false`, exposed as a "Forzar recomprobación completa" item in Settings) that ignores every skip rule and fetches all followed series, so the user always has an escape hatch, and so the skip logic can be A/B-verified against a full run.

## Optimization B — hot window pool (only if phase 0 justifies it)

Replace per-fetch window construction with a pool of `SCRAPE_CONCURRENCY` (2) long-lived hidden WebView2 windows, acquired/released like the current semaphore permits (the semaphore *becomes* the pool). Navigate an acquired window to the next URL instead of building a new one. Close pooled windows after an idle timeout (e.g. 60s) so a long-idle app isn't holding two renderers.

Note honestly: WebView2 windows in this app already **share a user-data folder**, therefore already share Cloudflare cookies. The win here is *not* "session reuse" — it is avoiding renderer-process/window construction cost. Do not claim otherwise.

**The trap that will bite you.** After `navigate()`, the old page's DOM is still live for a while. The existing readiness probe checks `document.readyState` and body size — both of which the **previous** page satisfies. It will return "ready" instantly with the wrong HTML, and the parse will silently succeed against stale content. Mitigations, all required:

- Include `location.href` in the probe's JSON and require it to match the requested URL (normalize trailing slash; allow the mirror's own redirects by comparing origin+path, and log when it differs).
- Before polling, wait for a navigation-started signal: poll until `document.readyState === 'loading'` **or** `location.href` already equals the target, whichever comes first, with a short ceiling.
- Add a unit-testable seam: extract the "is this probe result acceptable for URL X" decision into a pure function in `scraper_engine.rs` (input: probe JSON + expected URL; output: ready / not-ready / wrong-page) and test it directly, including the stale-previous-page case.

If phase 0 shows window construction is a minor cost, **do not implement B**. Write the measurement in the summary and move on. Partial credit for correctly declining an optimization.

## Optimization C — short-TTL HTML cache

An in-memory `HashMap<String, (Instant, String)>` in `AppState`, TTL ~10 minutes, consulted by `fetch_html`'s callers (not by `fetch_html` itself — keep the engine dumb). It does nothing for a single refresh (every series is a distinct URL) but makes "refresh, then immediately open a series' detail view" free, which is the common interaction.

Keep it small and bounded (e.g. 64 entries, evict oldest). Do not persist it to disk. Do not cache the airing listing under `force: true`.

## Acceptance criteria (verifiable)

1. `cargo test` passes, with new tests:
   - `animeytx.rs`: `parse_airing` extracts `next_episode_at` and `site_episode_count` from the real `airing.html` fixture; `.sb` value `??` yields `None` (add a fixture case if the current file lacks one); a card with no `.epx.cndwn` yields `None` rather than erroring.
   - `commands.rs`/pure helper: the skip decision — future `next_episode_at` → skip; past → fetch; matching `site_episode_count` → skip; `None`/`None` on a non-airing series → fetch only if `last_checked_at` older than the recheck interval; `force: true` → never skip.
   - `scraper_engine.rs` (only if B is implemented): the probe-acceptance function rejects a ready-looking probe whose `location.href` is the previous URL.
2. `npx tsc --noEmit`, `npm run build` pass.
3. **Measured**: phase-0 timings and post-change timings, both from real runs, reported as concrete numbers. A refresh with no new episodes performs **1** page fetch (verify by counting `[scrape]` log lines / observed scraper windows), versus ~119 before.
4. New episodes are still detected: with `force: false`, a series whose `data-rlsdt` has passed **is** fetched and its new episode appears in Pendientes.
5. `refresh(force: true)` fetches every followed series (regression escape hatch works).

## Live verification required

- Phase-0 log excerpt with the per-fetch breakdown, and the total wall time of a full pre-change refresh.
- Post-change: wall time of a no-op refresh and the count of scraper windows/fetches it performed.
- Evidence that at least one real new episode was still detected after the change (or, if none aired during testing, a `force: true` run reconciling identically to the pre-change behavior — say which one you did).
- Never more than 2 scraper windows at once, before or after.

## Explicitly out of scope

- Raising `SCRAPE_CONCURRENCY` above 2. The cap is a politeness/stability constraint, not a tuning knob.
- Any `reqwest`/`curl` path to the site. Cloudflare; WebView2 only.
- Bulk cover fetching (permanently out of scope, see CLAUDE.md).
- Persisting the HTML cache across app restarts.
