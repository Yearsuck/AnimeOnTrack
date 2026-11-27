//! Pure skip/fetch planning for `refresh` (see `commands::scan::refresh`).
//! No Tauri, DB or network: `refresh` gathers the inputs from the DB, asks
//! `plan_refresh` which followed series need their episode page fetched, then
//! does the I/O. Checked by `tools/arch-check.mjs` (pure-module rule).

/// How long a followed series that is *absent* from the airing listing goes
/// between episode-list rechecks. The spec drafted this as 7 days on the
/// assumption "absent from the listing = finished", but live verification
/// (2026-07-10) disproved that: the schedule listing only carries ~77 series
/// while 114 are marked airing, and two followed shows absent from it
/// (Tensei Slime T4, Ryoumin 0-nin) got real new episodes the same day. So
/// absent series are rechecked daily — new episodes there arrive at most
/// ~24h late instead of ~7 days, at the cost of one full off-listing sweep
/// per day (first refresh of the day), with every later refresh that day
/// still hitting the 1-fetch fast path.
const OFF_LISTING_RECHECK_SECS: i64 = 24 * 3600;

/// On-listing followed series whose airing card shows a non-numeric "??"
/// badge give no count signal at all; the future countdown can't rule out a
/// fresh episode either (live-verified: it's never observed in the past,
/// since the site rolls it forward the instant an episode posts), so an
/// unknown badge used to mean an indefinite skip (Bug A, 2026-07-12
/// airing-refresh-missing-episodes fix — proven live against "One Piece:
/// Arco de Elbaph", permanently stuck until a force refresh). Re-fetch at
/// most this often instead. Cheap: only a handful of "??" series were ever
/// observed live (<=7), at most once per 6h each.
const UNKNOWN_BADGE_RECHECK_SECS: i64 = 6 * 3600;

/// The skip decision at the heart of refresh()'s optimization A (see
/// docs/superpowers/specs/2026-07-10-scraper-performance-design.md): given a
/// followed series' fresh airing-listing metadata, does its episode-list
/// page need fetching this cycle? Pure so it can be unit-tested without a
/// scraper or DB — a bug here silently stops detecting new episodes, which
/// is the app's entire purpose.
///
/// Live-verified semantics of the card metadata (2026-07-10, real site):
/// the `.sb` badge is the **upcoming episode's number**, not the count of
/// posted episodes — 8/8 followed series that were provably up to date
/// (fetched moments earlier, no new episodes) all showed `.sb == db+1`, and
/// the fixture agrees (Liar Game: `.sb`=14, newest posted episode 13). And
/// no live card (0/77) carried a past `data-rlsdt`; just-aired series
/// either roll to next week's timestamp or leave the listing entirely.
///
/// Rules, in order:
/// - `force` (the Settings "Forzar recomprobación completa" escape hatch)
///   always fetches.
/// - `listing_scanned == false` (the airing-listing scan itself failed, so
///   there is no fresh metadata) always fetches — behave exactly like the
///   pre-skip-logic refresh rather than trusting stale signals.
/// - Off the listing: fetch only when never checked or the last check is
///   older than `OFF_LISTING_RECHECK_SECS` (see its comment — absent does
///   NOT mean finished on this site).
/// - On the listing with a past countdown: the episode aired and the card
///   hasn't rolled over — fetch, whatever the badge says.
/// - Badge present: fetch iff `badge > db_max+1` (site has posted something
///   we don't have; `db_max` is the highest real episode NUMBER, not a row
///   count — see `Db::max_episode_number`'s doc for why a row count is
///   wrong). `db_max+1` (up to date) and `<= db_max` (our numbering equal or
///   ahead) both skip. Deliberately *not* the spec's literal "badge == db
///   skips": that rule assumed badge = posted count, which the live data
///   disproves — under it, every current weekly series re-fetches forever.
/// - Badge unknown (`"??"`): no count signal at all, and the countdown can't
///   substitute for one (it's never observed in the past — see
///   `UNKNOWN_BADGE_RECHECK_SECS`'s doc), so fall back to a bounded recheck
///   instead of skipping indefinitely (Bug A, 2026-07-12 fix).
#[allow(clippy::too_many_arguments)]
fn should_fetch_series(
    force: bool,
    listing_scanned: bool,
    on_listing: bool,
    next_episode_at: Option<i64>,
    site_episode_count: Option<i64>,
    db_max_episode_number: i64,
    last_checked_age_secs: Option<i64>,
    now_unix: i64,
) -> bool {
    if force || !listing_scanned {
        return true;
    }
    if !on_listing {
        return match last_checked_age_secs {
            None => true,
            Some(age) => age >= OFF_LISTING_RECHECK_SECS,
        };
    }
    if next_episode_at.is_some_and(|t| t <= now_unix) {
        return true;
    }
    match site_episode_count {
        Some(next_number) => next_number > db_max_episode_number + 1,
        None => match last_checked_age_secs {
            None => true,
            Some(age) => age >= UNKNOWN_BADGE_RECHECK_SECS,
        },
    }
}

/// How long a failed episode-list fetch holds off the time-based rechecks.
/// Short on purpose: one bad cycle (a Cloudflare 503, a timeout) must not
/// push the next real check a whole `OFF_LISTING_RECHECK_SECS` away; a page
/// that is gone for good is then retried every couple of hours instead of
/// on every refresh.
const FAILED_FETCH_RETRY_SECS: i64 = 2 * 3600;

/// Age fed to `should_fetch_series`'s time-based rechecks. A failed fetch
/// (every mirror errored or parsed empty, e.g. a page that now 404s) used to
/// leave no trace, so a series off the listing that had never been fetched
/// OK was fetched again on every refresh. A failure younger than
/// `FAILED_FETCH_RETRY_SECS` now counts as a recent attempt; an older one is
/// ignored and the last successful check decides, as before. Signals of a
/// new episode (badge ahead, past countdown) do not look at this age.
fn effective_check_age(checked: Option<i64>, failed: Option<i64>) -> Option<i64> {
    match failed {
        Some(f) if f < FAILED_FETCH_RETRY_SECS => Some(checked.map_or(f, |c| c.min(f))),
        _ => checked,
    }
}

/// Everything the skip decision needs about one followed series, read from
/// the DB / this refresh's listing scan by the caller.
#[derive(Debug, Clone, PartialEq)]
pub struct SeriesInput {
    pub id: i64,
    /// The series' slug appeared in this refresh's airing-listing scan.
    pub on_listing: bool,
    pub next_episode_at: Option<i64>,
    pub site_episode_count: Option<i64>,
    /// Highest real episode number in the DB (0 when no rows).
    pub db_max_episode_number: i64,
    /// Seconds since the last successful episode-list check, if any.
    pub checked_age_secs: Option<i64>,
    /// Seconds since the last failed fetch, if any.
    pub failed_age_secs: Option<i64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RefreshInputs {
    /// The airing-listing scan succeeded this cycle (fresh metadata exists).
    pub listing_scanned: bool,
    /// Followed series in the order they are processed.
    pub series: Vec<SeriesInput>,
}

/// Series ids partitioned by decision, each in input order.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RefreshPlan {
    pub to_fetch: Vec<i64>,
    pub skipped: Vec<i64>,
}

/// Partition the followed series into skipped vs to-fetch (see
/// `should_fetch_series` for the rules).
pub fn plan_refresh(inputs: &RefreshInputs, now_unix: i64, force: bool) -> RefreshPlan {
    let mut plan = RefreshPlan::default();
    for s in &inputs.series {
        let age = effective_check_age(s.checked_age_secs, s.failed_age_secs);
        if should_fetch_series(
            force,
            inputs.listing_scanned,
            s.on_listing,
            s.next_episode_at,
            s.site_episode_count,
            s.db_max_episode_number,
            age,
            now_unix,
        ) {
            plan.to_fetch.push(s.id);
        } else {
            plan.skipped.push(s.id);
        }
    }
    plan
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_783_400_000;
    const DAY: i64 = 86_400;

    /// A failed fetch counts as an attempt: the age-based rechecks wait from
    /// whichever happened last, so a dead URL is not re-fetched every cycle.
    #[test]
    fn effective_check_age_takes_the_most_recent_attempt() {
        assert_eq!(effective_check_age(None, None), None);
        assert_eq!(effective_check_age(Some(100), None), Some(100));
        assert_eq!(effective_check_age(None, Some(50)), Some(50));
        assert_eq!(effective_check_age(Some(100), Some(50)), Some(50));
        assert_eq!(effective_check_age(Some(10), Some(5000)), Some(10));
        // An old failure no longer counts: the last success decides, so one
        // transient failure cannot delay the next real check by a full window.
        assert_eq!(effective_check_age(None, Some(FAILED_FETCH_RETRY_SECS)), None);
        assert_eq!(
            effective_check_age(Some(25 * 3600), Some(FAILED_FETCH_RETRY_SECS + 1)),
            Some(25 * 3600)
        );
    }

    /// The real case from the sandbox: an off-listing series whose page 404s
    /// was never stamped as checked, so it was re-fetched on every refresh.
    #[test]
    fn off_listing_series_that_just_failed_is_not_refetched_until_the_recheck() {
        let failed_an_hour_ago = effective_check_age(None, Some(3600));
        assert!(!should_fetch_series(false, true, false, None, None, 0, failed_an_hour_ago, NOW));
        let failed_hours_ago = effective_check_age(None, Some(FAILED_FETCH_RETRY_SECS));
        assert!(should_fetch_series(false, true, false, None, None, 0, failed_hours_ago, NOW));
        // Last good check 25 h ago, one failure 1 h ago: waits only for the
        // short retry window, not another 24 h.
        let transient = effective_check_age(Some(25 * 3600), Some(3600));
        assert!(!should_fetch_series(false, true, false, None, None, 0, transient, NOW));
        let transient_later = effective_check_age(Some(27 * 3600), Some(FAILED_FETCH_RETRY_SECS + 60));
        assert!(should_fetch_series(false, true, false, None, None, 0, transient_later, NOW));
        // A new-episode signal still wins over a recent failure.
        assert!(should_fetch_series(false, true, true, None, Some(9), 5, failed_an_hour_ago, NOW));
    }

    #[test]
    fn skip_when_next_episode_in_future_and_no_count_conflict() {
        // .sb = db_max+1: up to date under the verified next-episode-number
        // semantics (8/8 provably-current live series showed exactly this).
        assert!(!should_fetch_series(
            false, true, true, Some(NOW + DAY), Some(6), 5, None, NOW
        ));
        // .sb = db_max: db numbering equal/ahead of the badge — also nothing new.
        assert!(!should_fetch_series(
            false, true, true, Some(NOW + DAY), Some(5), 5, None, NOW
        ));
    }

    /// Bug A (2026-07-12 airing-refresh-missing-episodes fix): a non-numeric
    /// "??" badge gives no count signal at all, and the site's countdown is
    /// *never* observed in the past (it always rolls forward the instant an
    /// episode posts, live-verified), so "future countdown -> skip" would
    /// skip such a series FOREVER — proven live against "One Piece: Arco de
    /// Elbaph" (203 episodes, airing, on-listing, badge NULL, future
    /// next_episode_at, permanently stuck until a force refresh). Fall back
    /// to a bounded recheck instead of an indefinite skip.
    #[test]
    fn unknown_badge_with_future_countdown_uses_bounded_recheck() {
        // Never checked before -> fetch (matches the old test's flipped case:
        // unknown badge + never checked used to assert skip, which was the
        // bug).
        assert!(should_fetch_series(
            false, true, true, Some(NOW + DAY), None, 5, None, NOW
        ));
        // Checked well within the 6h window -> skip.
        assert!(!should_fetch_series(
            false, true, true, Some(NOW + DAY), None, 5, Some(3600), NOW
        ));
        // Checked exactly 6h ago -> fetch (boundary is inclusive).
        assert!(should_fetch_series(
            false, true, true, Some(NOW + DAY), None, 5, Some(6 * 3600), NOW
        ));
        // Checked well past 6h ago -> fetch.
        assert!(should_fetch_series(
            false, true, true, Some(NOW + DAY), None, 5, Some(7 * 3600), NOW
        ));
    }

    /// Bug B regression (2026-07-12 fix): `db_max_episode_number` must be the
    /// highest real episode NUMBER, not a row COUNT — recap/"0"/version rows
    /// inflate a row count and silently cancel the `+1` detection margin.
    /// Live proof: Tsue to Tsurugi no Wistoria Temporada 2 has episode rows
    /// "0|Recap", 1..12 (13 rows, highest real number 12); when episode 13
    /// posts the badge becomes 14.
    #[test]
    fn regression_recap_row_no_longer_masks_a_new_episode() {
        // Caught up: badge 13 (next-episode-number) vs max number 12 -> skip.
        assert!(!should_fetch_series(
            false, true, true, Some(NOW + DAY), Some(13), 12, None, NOW
        ));
        // Episode 13 just posted: badge rolls to 14 vs max number still 12 ->
        // fetch. Under the old row-COUNT bug (13) this was `14 > 13+1=14` =
        // false, i.e. the miss this fix corrects.
        assert!(should_fetch_series(
            false, true, true, Some(NOW + DAY), Some(14), 12, None, NOW
        ));
    }

    #[test]
    fn fetch_when_next_episode_in_past_and_count_unknown() {
        assert!(should_fetch_series(
            false, true, true, Some(NOW - 3600), None, 5, None, NOW
        ));
    }

    /// A countdown sitting in the past means the episode aired and the card
    /// hasn't rolled over — always fetch, regardless of what the badge says
    /// (the badge may not have rolled yet either).
    #[test]
    fn fetch_when_countdown_in_past_even_if_badge_looks_current() {
        assert!(should_fetch_series(
            false, true, true, Some(NOW - 3600), Some(6), 5, None, NOW
        ));
        assert!(should_fetch_series(
            false, true, true, Some(NOW - 3600), Some(5), 5, None, NOW
        ));
    }

    /// .sb greater than db+1 means the site has posted episodes we don't
    /// have (badge = next episode's number, so badge > db+1 ⇒ posted > db).
    /// This must win even over a future countdown: the site rolls the
    /// countdown to next week when it posts, so "future next_episode_at"
    /// alone would skip a freshly-posted episode forever.
    #[test]
    fn fetch_when_badge_says_site_is_ahead_even_with_future_countdown() {
        assert!(should_fetch_series(
            false, true, true, Some(NOW + DAY), Some(7), 5, None, NOW
        ));
    }

    #[test]
    fn fetch_when_no_signal_at_all_on_listing() {
        // On the listing but no countdown span and a non-numeric count
        // ("??") — no basis to skip.
        assert!(should_fetch_series(false, true, true, None, None, 5, None, NOW));
    }

    /// Followed series absent from the airing listing: fetch at most once
    /// per OFF_LISTING_RECHECK interval (24h — live verification showed the
    /// listing only carries ~77 series and genuinely-airing followed shows
    /// can be absent from it, so "absent = finished" is false and a 7-day
    /// interval would delay real episodes up to a week).
    #[test]
    fn off_listing_series_fetches_only_when_recheck_interval_elapsed() {
        // Never checked → fetch.
        assert!(should_fetch_series(false, true, false, None, None, 5, None, NOW));
        // Checked 1 hour ago → skip.
        assert!(!should_fetch_series(
            false, true, false, None, None, 5, Some(3600), NOW
        ));
        // Checked 25 hours ago → fetch again.
        assert!(should_fetch_series(
            false, true, false, None, None, 5, Some(25 * 3600), NOW
        ));
    }

    #[test]
    fn force_always_fetches() {
        // force=true overrides every skip rule, including the strongest ones.
        assert!(should_fetch_series(
            true, true, true, Some(NOW + DAY), Some(5), 5, Some(0), NOW
        ));
        assert!(should_fetch_series(true, true, false, None, None, 5, Some(0), NOW));
    }

    #[test]
    fn listing_scan_failure_falls_back_to_fetching_everything() {
        // If the airing-listing scan failed there is no fresh metadata; the
        // refresh must behave exactly like the pre-skip-logic code and fetch
        // every followed series rather than trusting stale skip signals.
        assert!(should_fetch_series(
            false, false, true, Some(NOW + DAY), Some(5), 5, Some(0), NOW
        ));
    }

    fn input(id: i64, on_listing: bool, next: Option<i64>, badge: Option<i64>, db_max: i64) -> SeriesInput {
        SeriesInput {
            id,
            on_listing,
            next_episode_at: next,
            site_episode_count: badge,
            db_max_episode_number: db_max,
            checked_age_secs: None,
            failed_age_secs: None,
        }
    }

    /// Table: (name, listing_scanned, force, series, expected to_fetch ids,
    /// expected skipped ids).
    #[test]
    fn plan_refresh_partitions_in_input_order() {
        let up_to_date = |id| input(id, true, Some(NOW + DAY), Some(6), 5);
        let ahead = |id| input(id, true, Some(NOW + DAY), Some(9), 5);
        let off_never = |id| input(id, false, None, None, 5);
        let mut off_recent = input(0, false, None, None, 5);
        off_recent.checked_age_secs = Some(3600);
        let off_recent = |id| SeriesInput { id, ..off_recent.clone() };
        let mut off_failed = input(0, false, None, None, 5);
        off_failed.failed_age_secs = Some(600);
        off_failed.checked_age_secs = Some(30 * 3600);
        let off_failed = |id| SeriesInput { id, ..off_failed.clone() };

        type Case = (&'static str, bool, bool, Vec<SeriesInput>, Vec<i64>, Vec<i64>);
        let cases: Vec<Case> = vec![
            ("empty", true, false, vec![], vec![], vec![]),
            (
                "force fetches everything",
                true,
                true,
                vec![up_to_date(1), off_recent(2), ahead(3)],
                vec![1, 2, 3],
                vec![],
            ),
            (
                "listing scan failed fetches everything",
                false,
                false,
                vec![up_to_date(1), off_recent(2)],
                vec![1, 2],
                vec![],
            ),
            (
                "up to date and recently checked are skipped",
                true,
                false,
                vec![up_to_date(1), off_recent(2), off_failed(3)],
                vec![],
                vec![1, 2, 3],
            ),
            (
                "mixed keeps input order in both lists",
                true,
                false,
                vec![up_to_date(1), ahead(2), off_recent(3), off_never(4), up_to_date(5)],
                vec![2, 4],
                vec![1, 3, 5],
            ),
            (
                "past countdown fetches",
                true,
                false,
                vec![input(1, true, Some(NOW - 1), Some(6), 5)],
                vec![1],
                vec![],
            ),
            (
                "unknown badge never checked fetches",
                true,
                false,
                vec![input(1, true, Some(NOW + DAY), None, 5)],
                vec![1],
                vec![],
            ),
        ];
        for (name, scanned, force, series, want_fetch, want_skip) in cases {
            let plan = plan_refresh(&RefreshInputs { listing_scanned: scanned, series }, NOW, force);
            assert_eq!(plan.to_fetch, want_fetch, "to_fetch: {name}");
            assert_eq!(plan.skipped, want_skip, "skipped: {name}");
        }
    }

    #[test]
    fn plan_refresh_uses_failure_age_for_time_based_rechecks() {
        let mut s = input(1, false, None, None, 0);
        s.failed_age_secs = Some(3600);
        let inputs = RefreshInputs { listing_scanned: true, series: vec![s.clone()] };
        assert_eq!(plan_refresh(&inputs, NOW, false).skipped, vec![1]);
        // An old failure no longer counts: never checked -> fetch.
        s.failed_age_secs = Some(FAILED_FETCH_RETRY_SECS);
        let inputs = RefreshInputs { listing_scanned: true, series: vec![s] };
        assert_eq!(plan_refresh(&inputs, NOW, false).to_fetch, vec![1]);
    }
}
