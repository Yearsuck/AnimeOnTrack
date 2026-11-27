use super::*;

/// True for a `UNIQUE(series_id, url)` violation on `episodes` specifically —
/// see `apply_episode_diff`'s doc comment for why this one constraint is
/// tolerated (skip the offending row) rather than aborting the whole diff.
///
/// The check used to be `ErrorCode::ConstraintViolation` alone, which is the
/// *class* of every constraint failure SQLite can raise: NOT NULL, CHECK,
/// FOREIGN KEY, PRIMARY KEY. A genuine data bug in a scraped episode (a NULL
/// number, a `series_id` pointing at a deleted series) therefore got quietly
/// logged as "url collides with another episode" and skipped, instead of
/// surfacing as the real error it is. Two things narrow it now:
///
/// - the **extended** result code must be `SQLITE_CONSTRAINT_UNIQUE` (2067),
///   which rusqlite carries on `ffi::Error::extended_code` — that alone rules
///   out every other constraint kind; and
/// - when SQLite supplies its message (it always does in practice), it must
///   name this exact constraint: "UNIQUE constraint failed: episodes.series_id,
///   episodes.url". A missing message falls back to the extended code alone
///   rather than misreporting a tolerable collision as a hard failure.
fn is_unique_violation(e: &anyhow::Error) -> bool {
    let Some(rusqlite::Error::SqliteFailure(err, msg)) = e.downcast_ref::<rusqlite::Error>() else {
        return false;
    };
    if err.code != rusqlite::ErrorCode::ConstraintViolation
        || err.extended_code != rusqlite::ffi::SQLITE_CONSTRAINT_UNIQUE
    {
        return false;
    }
    match msg {
        Some(m) => {
            let m = m.to_ascii_lowercase();
            m.contains("episodes.series_id") && m.contains("episodes.url")
        }
        None => true,
    }
}

/// Extract a comparable numeric value from an episode-number string, e.g.
/// "12" -> 12.0, "12.5" -> 12.5 (OVA/special numbering), "1x05" -> season 1
/// episode 5 packed as 100005.0 (season-prefixed numbering seen on
/// multi-cour series pages) so cascade comparisons order by (season,
/// episode) instead of just the season digit — packing just the season
/// would make every episode in the same season parse identically, and
/// un-marking one used to wipe every other episode in that season too.
/// Returns `None` when there are no leading digits at all (e.g. "OVA"), so
/// callers can fall back to an exact-string match instead of guessing an
/// ordering.
pub(crate) fn parse_ep_number(s: &str) -> Option<f64> {
    let trimmed = s.trim();
    if let Some((season, ep)) = trimmed.split_once(['x', 'X']) {
        if let (Ok(season), Ok(ep)) = (season.trim().parse::<f64>(), ep.trim().parse::<f64>()) {
            return Some(season * 100_000.0 + ep);
        }
    }
    let mut end = 0;
    let mut seen_dot = false;
    for (i, c) in trimmed.char_indices() {
        if c.is_ascii_digit() {
            end = i + c.len_utf8();
        } else if c == '.' && !seen_dot && end > 0 {
            seen_dot = true;
        } else {
            break;
        }
    }
    if end == 0 {
        None
    } else {
        trimmed[..end].parse().ok()
    }
}

/// Sibling episode on another source in the same canonical group.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SiblingEpisode {
    pub series_id: i64,
    pub source_id: i64,
    pub site_id: String,
    pub episode_id: i64,
    pub url: String,
}

/// Sibling series on another source that needs scraping for missing episodes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SiblingToScrape {
    pub series_id: i64,
    pub site_id: String,
    pub url: String,
}

/// Context for searching other sites for an episode that has no local series row.
#[derive(Debug, Clone, PartialEq)]
pub struct SearchContext {
    pub titles: (String, Option<String>),
    pub canonical_number: f64,
    pub group_site_ids: Vec<String>,
}

/// Sibling series acceptance guard: same non-null anilist_id OR equal normalized title.
/// Shared by `sibling_episodes_in` and `siblings_to_scrape_in` to prevent drift.
fn matches_sibling_guard(
    clicked_al_id: Option<i64>,
    clicked_title: &str,
    sib_al_id: Option<i64>,
    sib_title: &str,
) -> bool {
    let same_non_null_al = match (clicked_al_id, sib_al_id) {
        (Some(a), Some(b)) => a == b,
        _ => false,
    };
    let titles_equal = crate::matching::normalize_title(clicked_title)
        == crate::matching::normalize_title(sib_title);
    same_non_null_al || titles_equal
}

impl Db {
    /// Episode-row count for one series. Test-only: the production skip
    /// decision moved to `max_episode_number` (a row COUNT gets inflated by
    /// re-scraped duplicates), but the tests still assert against a raw count.
    #[cfg(test)]
    pub fn episode_count(&self, series_id: i64) -> Result<i64> {
        Ok(self.conn.query_row(
            "SELECT COUNT(*) FROM episodes WHERE series_id=?1",
            [series_id],
            |r| r.get(0),
        )?)
    }

    /// Highest numeric episode number we have for a series — the correct DB
    /// side of refresh()'s skip decision (see `refresh_plan::should_fetch_series`),
    /// unlike `episode_count` which is a row COUNT and gets inflated by
    /// recap/"0"/version-reupload rows (Bug B, 2026-07-12 airing-refresh fix).
    /// SQLite's `CAST(x AS INTEGER)` parses a leading integer and stops at the
    /// first non-digit, so `'13'`->13, `'1 | v2'`->1, `'0 | Recap'`->0,
    /// `'Recap'`->0. Returns 0 when the series has no episode rows.
    // Kept as a public single-series lookup even though `refresh()` now uses
    // the batched `max_episode_numbers_for_source` instead (see its doc
    // comment) — a real, useful primitive on its own, exercised directly by
    // the tests below.
    #[allow(dead_code)]
    pub fn max_episode_number(&self, series_id: i64) -> Result<i64> {
        Ok(self.conn.query_row(
            "SELECT COALESCE(MAX(CAST(number AS INTEGER)), 0) FROM episodes WHERE series_id=?1",
            [series_id],
            |r| r.get(0),
        )?)
    }

    /// Batched form of `max_episode_number` for every followed series of
    /// `source_id`, one query instead of one-per-series. `refresh()`'s skip
    /// decision used to call `max_episode_number` (plus
    /// `Db::last_checked_age_secs`) once per followed series before it even
    /// started fetching — for a library of 100+ shows that's 200+ separate
    /// short-lived `state.db` mutex lock/query round-trips on every refresh
    /// cycle (every app launch and every "Actualizar" click), each also
    /// contending with any UI-triggered command that needs the same lock.
    /// A series with zero episode rows has no entry in the returned map
    /// (there's no `GROUP BY` row for it) — callers must treat a missing key
    /// as 0, matching `max_episode_number`'s own COALESCE default.
    pub fn max_episode_numbers_for_source(
        &self,
        source_id: i64,
    ) -> Result<std::collections::HashMap<i64, i64>> {
        let mut stmt = self.conn.prepare(
            "SELECT e.series_id, MAX(CAST(e.number AS INTEGER))
             FROM episodes e JOIN series s ON s.id = e.series_id
             WHERE s.source_id=?1 AND s.followed=1
             GROUP BY e.series_id",
        )?;
        let rows = stmt
            .query_map([source_id], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<std::collections::HashMap<i64, i64>>>()?;
        Ok(rows)
    }

    /// The lowest-numbered episode's `released_at` for every airing series in
    /// `source_id` that has one — the DB side of the "Esta temporada" filter
    /// (see docs/superpowers/specs/2026-07-13-airing-this-season-design.md).
    /// Only 37/118 currently-airing series have any scraped episodes at all
    /// (episodes are fetched on-demand, not for the whole catalog — see
    /// [[project-scraping-scope]]), so most airing series are simply absent
    /// from the returned map; callers must treat that as "unknown", not "old".
    /// The correlated subquery picks one definite row per series (lowest
    /// `CAST(number AS INTEGER)`, ties broken by lowest `id`) regardless of
    /// insertion order, so a re-scraped or out-of-order episode list still
    /// resolves to episode "1". Rows with a NULL/empty `released_at` (~11 of
    /// 2195 site-wide) are excluded rather than reported as an unparseable date.
    pub fn first_episode_dates(&self, source_id: i64) -> Result<std::collections::HashMap<i64, String>> {
        let mut stmt = self.conn.prepare(
            "SELECT s.id, e.released_at
             FROM series s
             JOIN episodes e ON e.series_id = s.id
             WHERE s.source_id = ?1 AND s.is_airing = 1
               AND e.id = (
                 SELECT id FROM episodes e2
                 WHERE e2.series_id = s.id
                 ORDER BY CAST(e2.number AS INTEGER) ASC, e2.id ASC
                 LIMIT 1
               )
               AND e.released_at IS NOT NULL AND e.released_at != ''",
        )?;
        let rows = stmt
            .query_map([source_id], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows.into_iter().collect())
    }

    /// The episode *numbers* already stored for a series. This — not the URL —
    /// is an episode's stable identity: the sites move between domains
    /// (animeytx.net -> animeyt.cc) and even change the URL path shape, so
    /// de-duplicating scraped episodes by URL re-inserts the whole list as
    /// "new" on a domain change (the episode-duplication bug). The scan
    /// de-duplicates against this set instead.
    pub fn existing_episode_numbers(&self, series_id: i64) -> Result<std::collections::HashSet<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT number FROM episodes WHERE series_id=?1")?;
        let nums = stmt
            .query_map([series_id], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<std::collections::HashSet<_>>>()?;
        Ok(nums)
    }

    /// Remove the unaired-episode placeholders animeyt.cc rows were stored with
    /// before the adapter learned to skip `is-upcoming` cards. The fresh
    /// listing no longer contains them, so a row of an `animeytx` series that
    /// is unseen, undated, has no download and is the one slot right after the highest listed
    /// number (at most +1, so a partly rendered page cannot wipe real rows) can only be one of those placeholders (the site lists the next
    /// episode, never a gap). An empty listing, or one with a number that does
    /// not parse, proves nothing, so nothing is removed. Returns the rows dropped.
    pub fn drop_unaired_placeholders(&self, series_id: i64, listed: &[crate::models::Episode]) -> Result<usize> {
        let nums: Vec<f64> = listed.iter().filter_map(|e| e.number.parse::<f64>().ok()).collect();
        if nums.is_empty() || nums.len() != listed.len() {
            return Ok(0);
        }
        let highest = nums.into_iter().fold(f64::MIN, f64::max);
        let n = self.conn.execute(
            "DELETE FROM episodes
             WHERE series_id=?1 AND seen=0 AND released_at IS NULL AND download_path IS NULL AND download_status IS NULL
               AND CAST(number AS REAL) > ?2 AND CAST(number AS REAL) <= ?2 + 1
               AND EXISTS (SELECT 1 FROM series s JOIN sources src ON src.id=s.source_id
                           WHERE s.id=?1 AND src.site_id='animeytx')",
            (series_id, highest),
        )?;
        Ok(n)
    }

    /// Refresh the mutable metadata (url/title/released_at) of an episode
    /// identified by (series_id, number), leaving `seen`/`seen_at` untouched.
    /// Called during a re-scan for episodes whose number is already known, so a
    /// domain change updates the link in place instead of inserting a duplicate.
    pub fn refresh_episode_meta(
        &self,
        series_id: i64,
        number: &str,
        url: &str,
        title: Option<&str>,
        released_at: Option<&str>,
    ) -> Result<()> {
        self.conn.execute(
            "UPDATE episodes SET url=?3, title=?4, released_at=?5
             WHERE series_id=?1 AND number=?2",
            (series_id, number, url, title, released_at),
        )?;
        Ok(())
    }

    /// Insert episode if its `(series_id, url)` is new. Returns
    /// `(row id, was actually inserted)`.
    ///
    /// The `bool` is the whole point of the tuple: `ON CONFLICT DO NOTHING`
    /// followed by an unconditional `SELECT id` returns `Ok(id)` whether or
    /// not a row was written, so the id alone can't tell a genuinely new
    /// episode from one that already existed under this exact URL. Callers
    /// that count new episodes (`apply_episode_diff`, which feeds
    /// `refresh()`'s "N nuevos episodios" figure) reported one new episode
    /// per refresh, forever, for any episode whose stored *number* drifted
    /// from the scraped label while its URL stayed put — `new_episodes()`
    /// keys on number, so it offers such an episode as new, the INSERT then
    /// conflicts on URL and writes nothing, and the count was incremented
    /// regardless. `rows_affected()` from the INSERT is the only honest
    /// signal available here.
    pub fn insert_episode(&self, e: &crate::models::Episode) -> Result<(i64, bool)> {
        let rows_affected = self.conn.execute(
            "INSERT INTO episodes(series_id, number, title, url, released_at, seen)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(series_id, url) DO NOTHING",
            (
                e.series_id, &e.number, &e.title, &e.url,
                &e.released_at, e.seen as i64,
            ),
        )?;
        let id: i64 = self.conn.query_row(
            "SELECT id FROM episodes WHERE series_id=?1 AND url=?2",
            (e.series_id, &e.url),
            |r| r.get(0),
        )?;
        Ok((id, rows_affected > 0))
    }

    /// Set an episode's seen flag either way (lets the user un-mark).
    /// `seen_at` tracks alongside: stamped `datetime('now')` when marking
    /// seen for the first time, cleared back to NULL when un-marking.
    /// `COALESCE` preserves an existing `seen_at` on a redundant
    /// already-seen -> seen call — without it, re-marking an episode that
    /// was already seen (e.g. via the cascade below re-touching it) resets
    /// its watch date to today, silently rewriting watch-history stats.
    pub fn set_seen(&self, episode_id: i64, seen: bool) -> Result<()> {
        self.conn.execute(
            "UPDATE episodes SET seen=?1, seen_at = CASE WHEN ?1=1 THEN COALESCE(seen_at, datetime('now')) ELSE NULL END
             WHERE id=?2",
            (seen as i64, episode_id),
        )?;
        Ok(())
    }

    /// Enforce sequential watching: marking an episode seen also marks every
    /// earlier episode of that series seen (no gaps like "watched 10 but not
    /// 6-9"); un-marking an episode also un-marks every later one (you can't
    /// have watched what comes after something you're un-marking).
    ///
    /// Comparison happens in Rust via `parse_ep_number`, not SQLite's
    /// `CAST(... AS INTEGER)` — the two disagreed on numbers like "1x05"
    /// (season-prefixed) or "12.5" (specials): SQLite's cast still read a
    /// leading digit while Rust's old strict `i64::parse` failed to 0,
    /// which made un-marking such an episode wipe the *whole series'*
    /// watched history instead of just the later episodes.
    /// Wrapped in one transaction: a crash/panic partway through a long
    /// cascade (a 1000+-episode runner marked seen in one go via
    /// `mark_all_episodes_seen`) used to leave it half-applied, violating
    /// the gap-free-watching invariant this function exists to enforce.
    /// Every row-level `UPDATE` before this was its own implicit
    /// transaction — a wasted fsync per row on top of the atomicity gap.
    pub fn set_seen_cascade(&self, series_id: i64, number: &str, seen: bool) -> Result<()> {
        let Some(target) = parse_ep_number(number) else {
            // No leading digits at all: ordering is meaningless, so just
            // toggle the exact-matching episode(s) rather than cascade.
            // COALESCE, same reasoning as `set_seen`: don't reset an
            // already-seen episode's watch date to today.
            self.conn.execute(
                "UPDATE episodes SET seen=?1, seen_at = CASE WHEN ?1=1 THEN COALESCE(seen_at, datetime('now')) ELSE NULL END
                 WHERE series_id=?2 AND number=?3",
                (seen as i64, series_id, number),
            )?;
            return Ok(());
        };
        // Reuse an enclosing transaction (sync apply wraps a whole batch); SQLite has no nested BEGIN.
        let tx = if self.conn.is_autocommit() { Some(self.conn.unchecked_transaction()?) } else { None };
        let rows: Vec<(i64, String)> = {
            let mut stmt = self.conn.prepare("SELECT id, number FROM episodes WHERE series_id=?1")?;
            let collected = stmt
                .query_map([series_id], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            collected
        };
        for (id, num) in rows {
            let matches = match parse_ep_number(&num) {
                Some(v) if seen => v <= target,
                Some(v) => v >= target,
                None => false,
            };
            if matches {
                // seen_at cascades along with seen — every row the cascade
                // touches gets the same stamped/cleared treatment as the
                // one the user explicitly clicked, not just that one.
                // COALESCE preserves the real watch date of a row the
                // cascade re-touches that was *already* seen (e.g. marking
                // episode 12 seen also re-touches 1-11, which likely have
                // their own earlier seen_at from being watched over time) —
                // without it, every mark-seen click flattened the whole
                // series' watch history to today, corrupting every
                // date-bucketed stat (30-day heatmap, yearly activity,
                // hourly distribution, dusty-watchlist ranges).
                self.conn.execute(
                    "UPDATE episodes SET seen=?1, seen_at = CASE WHEN ?1=1 THEN COALESCE(seen_at, datetime('now')) ELSE NULL END
                     WHERE id=?2",
                    (seen as i64, id),
                )?;
            }
        }
        if let Some(tx) = tx {
            tx.commit()?;
        }
        Ok(())
    }

    /// Single-pass grouping of all series rows into canonical identity keys.
    ///
    /// Bridges:
    /// - Linked rows (`anilist_id` IS NOT NULL) map to `al:{id}`.
    /// - Unlinked rows map to `al:{id}` when their normalized title matches either
    ///   the catalog title or the local title of a linked row in the same catalog group.
    /// - Otherwise unlinked rows map to `t:{normalize_title}`.
    ///
    /// Executes a single pass over `series` LEFT JOIN `anilist_catalog` (no per-row queries).
    pub fn canonical_group_keys(&self) -> Result<std::collections::HashMap<i64, String>> {
        let mut stmt = self.conn.prepare(
            "SELECT s.id, s.anilist_id, s.title, c.title AS catalog_title
             FROM series s
             LEFT JOIN anilist_catalog c ON c.id = s.anilist_id
             ORDER BY s.id ASC",
        )?;
        struct Row {
            id: i64,
            anilist_id: Option<i64>,
            title: String,
            catalog_title: Option<String>,
        }
        let rows = stmt
            .query_map([], |r| {
                Ok(Row {
                    id: r.get(0)?,
                    anilist_id: r.get(1)?,
                    title: r.get(2)?,
                    catalog_title: r.get(3)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        let mut tkey_to_anilist: std::collections::HashMap<String, i64> =
            std::collections::HashMap::new();
        for r in &rows {
            if let Some(al_id) = r.anilist_id {
                let tkey_local = format!("t:{}", crate::matching::normalize_title(&r.title));
                tkey_to_anilist
                    .entry(tkey_local)
                    .and_modify(|id| *id = (*id).min(al_id))
                    .or_insert(al_id);
                if let Some(cat_title) = &r.catalog_title {
                    let tkey_cat = format!("t:{}", crate::matching::normalize_title(cat_title));
                    tkey_to_anilist
                        .entry(tkey_cat)
                        .and_modify(|id| *id = (*id).min(al_id))
                        .or_insert(al_id);
                }
            }
        }

        let mut out = std::collections::HashMap::with_capacity(rows.len());
        for r in rows {
            let key = match r.anilist_id {
                Some(id) => format!("al:{id}"),
                None => {
                    let tkey = format!("t:{}", crate::matching::normalize_title(&r.title));
                    if let Some(al_id) = tkey_to_anilist.get(&tkey).copied() {
                        format!("al:{al_id}")
                    } else {
                        tkey
                    }
                }
            };
            out.insert(r.id, key);
        }
        Ok(out)
    }

    /// Returns the series IDs for every row in the same canonical group as `series_id`.
    pub fn canonical_series_ids(&self, series_id: i64) -> Result<Vec<i64>> {
        let keys = self.canonical_group_keys()?;
        let target_key = match keys.get(&series_id) {
            Some(k) => k.clone(),
            None => return Ok(vec![]),
        };
        let mut out: Vec<i64> = keys
            .into_iter()
            .filter_map(|(id, k)| if k == target_key { Some(id) } else { None })
            .collect();
        out.sort_unstable();
        Ok(out)
    }

    #[cfg(test)]
    /// Mark seen/unseen across all series rows in the same canonical group
    /// that have at least one episode row.
    pub fn set_seen_cascade_canonical(
        &self,
        series_id: i64,
        number: &str,
        seen: bool,
    ) -> Result<()> {
        let sids = self.canonical_series_ids(series_id)?;
        self.set_seen_cascade_in(series_id, &sids, number, seen)
    }

    /// Same as `set_seen_cascade_canonical` for a group the caller already resolved, so the
    /// full-table group scan in `canonical_series_ids` is not repeated per click.
    pub fn set_seen_cascade_in(
        &self,
        clicked_id: i64,
        sids: &[i64],
        number: &str,
        seen: bool,
    ) -> Result<()> {
        let now = chrono::Utc::now().timestamp();
        let clicked_align = self.row_align(clicked_id, now)?;
        let canonical = parse_ep_number(number)
            .and_then(|n| clicked_align.to_canonical(n));

        if let Some(canon) = canonical {
            for &sid in sids {
                let has_ep: bool = self.conn.query_row(
                    "SELECT EXISTS(SELECT 1 FROM episodes WHERE series_id=?1)",
                    [sid],
                    |r| r.get(0),
                )?;
                if has_ep {
                    let align = self.row_align(sid, now)?;
                    let row_target = align.to_row(canon);
                    let row_number_string = super::episode_align::format_ep_number(row_target);
                    self.set_seen_cascade(sid, &row_number_string, seen)?;
                }
            }
        } else {
            // If the clicked number has no canonical value (earlier-season numbers of a cumulative row,
            // or non-numeric) fall back to cascading only that row with the raw number (today's behaviour).
            self.set_seen_cascade(clicked_id, number, seen)?;
        }
        Ok(())
    }

    /// Sibling episodes of `episode_id` across other sources in the same canonical group.
    ///
    /// Maps the episode number through canonical space (`RowAlign::to_canonical`)
    /// and resolves each sibling series' episode number via `RowAlign::to_row`.
    /// Sibling rows whose series has no episodes or no episode matching the aligned number are omitted.
    /// Results are returned in deterministic order: by `sid` ascending.
    pub fn sibling_episodes(&self, episode_id: i64) -> Result<Vec<SiblingEpisode>> {
        let series_id: i64 = match self.conn.query_row(
            "SELECT series_id FROM episodes WHERE id=?1",
            [episode_id],
            |r| r.get(0),
        ) {
            Ok(v) => v,
            Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(vec![]),
            Err(e) => return Err(e.into()),
        };

        let sids = self.canonical_series_ids(series_id)?;
        self.sibling_episodes_in(episode_id, &sids)
    }

    /// Same as `sibling_episodes` for a canonical group the caller already resolved, so
    /// `canonical_series_ids` is not repeated per episode.
    pub fn sibling_episodes_in(&self, episode_id: i64, sids: &[i64]) -> Result<Vec<SiblingEpisode>> {
        let clicked: Option<(i64, String, Option<i64>, String)> = self
            .conn
            .query_row(
                "SELECT e.series_id, e.number, s.anilist_id, s.title
                 FROM episodes e
                 JOIN series s ON s.id = e.series_id
                 WHERE e.id = ?1",
                [episode_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()?;

        let (series_id, number, clicked_al_id, clicked_title) = match clicked {
            Some(v) => v,
            None => return Ok(vec![]),
        };

        let mut sorted_sids = sids.to_vec();
        sorted_sids.sort_unstable();
        sorted_sids.dedup();

        let now = chrono::Utc::now().timestamp();
        let clicked_align = self.row_align(series_id, now)?;
        let canon = match parse_ep_number(&number).and_then(|n| clicked_align.to_canonical(n)) {
            Some(c) => c,
            None => return Ok(vec![]),
        };

        let mut out = Vec::new();
        for sid in sorted_sids {
            if sid == series_id {
                continue;
            }
            let align = self.row_align(sid, now)?;
            let row_target = align.to_row(canon);
            let row_number = super::episode_align::format_ep_number(row_target);

            let row: Option<(i64, String, Option<i64>, String, i64, String)> = self
                .conn
                .query_row(
                    "SELECT s.source_id, COALESCE(so.site_id, ''), s.anilist_id, s.title, e.id, e.url
                     FROM episodes e
                     JOIN series s ON s.id = e.series_id
                     LEFT JOIN sources so ON so.id = s.source_id
                     WHERE e.series_id = ?1 AND e.number = ?2
                     ORDER BY e.id ASC
                     LIMIT 1",
                    rusqlite::params![sid, row_number],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
                )
                .optional()?;

            if let Some((source_id, site_id, sib_al_id, sib_title, ep_id, url)) = row {
                if !matches_sibling_guard(clicked_al_id, &clicked_title, sib_al_id, &sib_title) {
                    continue;
                }

                out.push(SiblingEpisode {
                    series_id: sid,
                    source_id,
                    site_id,
                    episode_id: ep_id,
                    url,
                });
            }
        }
        Ok(out)
    }

    /// Sibling series across other sources in the same canonical group that do NOT yet
    /// have an episode with the aligned number (e.g. series with zero episodes or missing that number).
    ///
    /// Sibling series are filtered with the same acceptance guard as `sibling_episodes_in`
    /// (same non-null anilist_id OR equal `normalize_title`).
    /// Results are returned in deterministic order: by series_id ascending.
    pub fn siblings_to_scrape(&self, episode_id: i64) -> Result<Vec<SiblingToScrape>> {
        let series_id: i64 = match self.conn.query_row(
            "SELECT series_id FROM episodes WHERE id=?1",
            [episode_id],
            |r| r.get(0),
        ) {
            Ok(v) => v,
            Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(vec![]),
            Err(e) => return Err(e.into()),
        };

        let sids = self.canonical_series_ids(series_id)?;
        self.siblings_to_scrape_in(episode_id, &sids)
    }

    /// Same as `siblings_to_scrape` for a canonical group the caller already resolved, so
    /// `canonical_series_ids` is not repeated per episode.
    pub fn siblings_to_scrape_in(&self, episode_id: i64, sids: &[i64]) -> Result<Vec<SiblingToScrape>> {
        let clicked: Option<(i64, String, Option<i64>, String)> = self
            .conn
            .query_row(
                "SELECT e.series_id, e.number, s.anilist_id, s.title
                 FROM episodes e
                 JOIN series s ON s.id = e.series_id
                 WHERE e.id = ?1",
                [episode_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()?;

        let (series_id, number, clicked_al_id, clicked_title) = match clicked {
            Some(v) => v,
            None => return Ok(vec![]),
        };

        let mut sorted_sids = sids.to_vec();
        sorted_sids.sort_unstable();
        sorted_sids.dedup();

        let now = chrono::Utc::now().timestamp();
        let clicked_align = self.row_align(series_id, now)?;
        let canon = match parse_ep_number(&number).and_then(|n| clicked_align.to_canonical(n)) {
            Some(c) => c,
            None => return Ok(vec![]),
        };

        let mut out = Vec::new();
        for sid in sorted_sids {
            if sid == series_id {
                continue;
            }
            let align = self.row_align(sid, now)?;
            let row_target = align.to_row(canon);
            let row_number = super::episode_align::format_ep_number(row_target);

            let has_episode: bool = self.conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM episodes WHERE series_id = ?1 AND number = ?2)",
                rusqlite::params![sid, row_number],
                |r| r.get(0),
            )?;
            if has_episode {
                continue;
            }

            let row: Option<(String, Option<i64>, String, String)> = self
                .conn
                .query_row(
                    "SELECT COALESCE(so.site_id, ''), s.anilist_id, s.title, s.url
                     FROM series s
                     LEFT JOIN sources so ON so.id = s.source_id
                     WHERE s.id = ?1",
                    [sid],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                )
                .optional()?;

            if let Some((site_id, sib_al_id, sib_title, url)) = row {
                if !matches_sibling_guard(clicked_al_id, &clicked_title, sib_al_id, &sib_title) {
                    continue;
                }

                out.push(SiblingToScrape {
                    series_id: sid,
                    site_id,
                    url,
                });
            }
        }
        Ok(out)
    }

    /// Search context for finding an episode across other sites when local rows failed.
    /// Returns the primary/english search titles, canonical episode number, and all site IDs
    /// currently present in the episode's canonical series group.
    pub fn download_search_context(&self, episode_id: i64) -> Result<Option<SearchContext>> {
        let clicked: Option<(i64, String, Option<i64>, String)> = self
            .conn
            .query_row(
                "SELECT e.series_id, e.number, s.anilist_id, s.title
                 FROM episodes e
                 JOIN series s ON s.id = e.series_id
                 WHERE e.id = ?1",
                [episode_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()?;

        let (series_id, number, anilist_id, series_title) = match clicked {
            Some(v) => v,
            None => return Ok(None),
        };

        let now = chrono::Utc::now().timestamp();
        let clicked_align = self.row_align(series_id, now)?;
        let canonical_number = match parse_ep_number(&number).and_then(|n| clicked_align.to_canonical(n)) {
            Some(c) => c,
            None => return Ok(None),
        };

        let titles = if let Some(al_id) = anilist_id {
            match self.get_catalog_titles(al_id)? {
                Some((cat_title, romaji, english)) => {
                    let primary = romaji.filter(|s| !s.trim().is_empty()).unwrap_or(cat_title);
                    let eng = english.filter(|s| !s.trim().is_empty());
                    (primary, eng)
                }
                None => (series_title, None),
            }
        } else {
            (series_title, None)
        };

        let sids = self.canonical_series_ids(series_id)?;
        let mut group_site_ids = Vec::new();
        if !sids.is_empty() {
            let placeholders = vec!["?"; sids.len()].join(",");
            let sql = format!(
                "SELECT DISTINCT COALESCE(so.site_id, '')
                 FROM series s
                 LEFT JOIN sources so ON so.id = s.source_id
                 WHERE s.id IN ({placeholders})"
            );
            let mut stmt = self.conn.prepare(&sql)?;
            let params = rusqlite::params_from_iter(sids.iter());
            let rows = stmt.query_map(params, |r| r.get::<_, String>(0))?;
            for row in rows {
                let site_id = row?;
                if !site_id.trim().is_empty() {
                    group_site_ids.push(site_id);
                }
            }
            group_site_ids.sort();
            group_site_ids.dedup();
        }

        Ok(Some(SearchContext {
            titles,
            canonical_number,
            group_site_ids,
        }))
    }

    /// All episodes of a series (seen or not), oldest number first — the
    /// progress view for "which episode am I on".
    pub fn list_series_episodes(&self, series_id: i64) -> Result<Vec<crate::models::Episode>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, series_id, number, title, url, released_at, seen,
                    download_path, download_status
             FROM episodes WHERE series_id=?1
             ORDER BY CAST(number AS INTEGER) ASC, id ASC",
        )?;
        let rows = stmt
            .query_map([series_id], |r| {
                Ok(crate::models::Episode {
                    id: r.get(0)?,
                    series_id: r.get(1)?,
                    number: r.get(2)?,
                    title: r.get(3)?,
                    url: r.get(4)?,
                    released_at: r.get(5)?,
                    seen: r.get::<_, i64>(6)? != 0,
                    download_path: r.get(7)?,
                    download_status: r.get(8)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Mark every episode of `series_id` seen via the existing gap-free
    /// `set_seen_cascade`, targeting whichever episode number sorts highest
    /// by `parse_ep_number` — used when a catalog title decided `Seen`
    /// (`watched_externally=1`) gets linked to a real site series and its
    /// episode list is scraped in for the first time. Reusing the cascade
    /// (rather than a blanket `UPDATE ... SET seen=1`) keeps this consistent
    /// with the same season/episode-aware ordering the rest of the app's
    /// watch-tracking relies on. A series with no episodes yet is a silent
    /// no-op, not an error.
    pub fn mark_all_episodes_seen(&self, series_id: i64) -> Result<()> {
        let mut stmt = self.conn.prepare("SELECT number FROM episodes WHERE series_id=?1")?;
        let numbers: Vec<String> = stmt
            .query_map([series_id], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let highest = numbers.iter().max_by(|a, b| {
            let av = parse_ep_number(a).unwrap_or(f64::MIN);
            let bv = parse_ep_number(b).unwrap_or(f64::MIN);
            av.partial_cmp(&bv).unwrap_or(std::cmp::Ordering::Equal)
        });
        if let Some(number) = highest {
            self.set_seen_cascade(series_id, number, true)?;
        }
        Ok(())
    }

    /// Lowest-numbered unseen episode of a series, or `None` if every
    /// episode is seen (or there are none) — same ordering as
    /// `list_series_episodes` (`SeriesDetail`'s query): `CAST(number AS
    /// INTEGER) ASC, id ASC`. Deliberately reuses that exact ORDER BY rather
    /// than a different numeric parse, so this can never disagree with what
    /// `SeriesDetail` shows as "next" for the same series.
    pub fn next_unseen_episode(&self, series_id: i64) -> Result<Option<crate::models::NextEpisode>> {
        let mut stmt = self.conn.prepare(
            "SELECT number, title, url FROM episodes
             WHERE series_id=?1 AND seen=0
             ORDER BY CAST(number AS INTEGER) ASC, id ASC
             LIMIT 1",
        )?;
        let mut rows = stmt.query_map([series_id], |r| {
            Ok(crate::models::NextEpisode {
                number: r.get(0)?,
                title: r.get(1)?,
                url: r.get(2)?,
            })
        })?;
        rows.next().transpose().map_err(Into::into)
    }

    /// Every URL that already has a `series` row for this source — any
    /// `backlog_status` or `followed` value counts as "already decided", so
    /// the swipe deck never re-offers a card the user has already acted on.
    pub fn known_series_urls(&self, source_id: i64) -> Result<std::collections::HashSet<String>> {
        let mut stmt = self.conn.prepare("SELECT url FROM series WHERE source_id=?1")?;
        let urls = stmt
            .query_map([source_id], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<std::collections::HashSet<_>>>()?;
        Ok(urls)
    }

    /// Apply one series' freshly-scraped episode list in a single
    /// transaction: rewrite its URL if the serving mirror changed, refresh
    /// the mutable metadata of episodes already known (by number, not URL —
    /// see the call sites this replaces), insert genuinely new ones, and
    /// stamp `last_checked_at`. Returns the count of new episodes inserted.
    ///
    /// This used to be a loop of individually autocommitted statements —
    /// one `execute` per already-known episode plus one per new one, each
    /// its own implicit transaction/fsync. For a long-running show that's a
    /// couple dozen separate commits per series per refresh cycle, and on at
    /// least one real machine that pattern reliably stalled indefinitely
    /// partway through (zero CPU, not a SQLite-level lock — a concurrent
    /// `sqlite3` write to the very row it was "stuck" on succeeded
    /// instantly), consistent with a disk/AV write-rate heuristic tripping
    /// on many rapid small writes to the same file rather than anything
    /// SQLite itself was doing. One commit per series sidesteps that
    /// regardless of the exact cause, and is strictly fewer fsyncs either way.
    pub fn apply_episode_diff(
        &self,
        series_id: i64,
        new_series_url: Option<&str>,
        eps: &[crate::models::Episode],
    ) -> Result<i64> {
        let tx = self.conn.unchecked_transaction()?;
        if let Some(url) = new_series_url {
            self.update_series_url(series_id, url)?;
        }
        let known = self.existing_episode_numbers(series_id)?;
        for e in eps.iter().filter(|e| known.contains(&e.number)) {
            // A scraped episode can legitimately collide on
            // UNIQUE(series_id, url) with another episode already stored
            // under a different number — observed live on a real site
            // whose listing briefly mapped two distinct episode numbers to
            // the same href (a half-episode's page and its neighbor). That
            // is bad *source* data, not a reason to abort this series' whole
            // diff (which used to propagate the error and silently kill the
            // entire `refresh()` cycle — every series still queued after
            // this one, and the frontend's progress bar just froze on
            // whatever number it last reached, looking exactly like a hang).
            // Skip just this one episode's metadata refresh and keep going;
            // it's retried on the next cycle and self-heals once the source
            // page stops colliding.
            if let Err(e2) = self.refresh_episode_meta(series_id, &e.number, &e.url, e.title.as_deref(), e.released_at.as_deref()) {
                if is_unique_violation(&e2) {
                    eprintln!("[refresh] series {series_id} episode {}: skipping metadata refresh, url collides with another episode ({e2})", e.number);
                    continue;
                }
                return Err(e2);
            }
        }
        let mut inserted = 0i64;
        for mut e in crate::diff::new_episodes(eps, &known) {
            e.series_id = series_id;
            match self.insert_episode(&e) {
                // Only a row the INSERT actually wrote is a new episode.
                // `new_episodes()` keys on episode *number*, the UNIQUE
                // constraint keys on URL, and the two disagree whenever a
                // stored episode's number drifts from the site's current
                // label while its URL stays the same (e.g. the number
                // "13.5" used to be stored as "135", then "13"). Such an
                // episode is offered as new on every single refresh, the
                // INSERT does nothing each time, and counting it regardless
                // reported a phantom "1 nuevo episodio" forever.
                Ok((_, true)) => inserted += 1,
                Ok((_, false)) => {
                    eprintln!("[refresh] series {series_id} episode {}: not new after all, its url already belongs to a stored episode", e.number);
                }
                Err(e2) if is_unique_violation(&e2) => {
                    eprintln!("[refresh] series {series_id} episode {}: skipping insert, url collides with another episode ({e2})", e.number);
                }
                Err(e2) => return Err(e2),
            }
        }
        self.drop_unaired_placeholders(series_id, eps)?;
        self.set_last_checked_at(series_id)?;
        tx.commit()?;
        Ok(inserted)
    }

    /// Returns (episode_id, download_path) pairs for episodes that have a
    /// non-NULL download_path among the given IDs. Used by the auto-cleanup
    /// hook in `set_seen` / `set_seen_cascade` to know which files to delete.
    pub fn downloaded_paths(&self, episode_ids: &[i64]) -> Result<Vec<(i64, String)>> {
        let mut result = Vec::new();
        for &id in episode_ids {
            let path: Option<String> = self
                .conn
                .query_row(
                    "SELECT download_path FROM episodes WHERE id=?1 AND download_path IS NOT NULL",
                    [id],
                    |r| r.get(0),
                )
                .optional()?;
            if let Some(p) = path {
                result.push((id, p));
            }
        }
        Ok(result)
    }

    /// Downloads that were "downloading" when the app last stopped are not running any
    /// more (the tasks died with the process), so their status is cleared and the row
    /// goes back to "not downloaded". Returns how many rows were reset.
    pub fn clear_interrupted_downloads(&self) -> Result<usize> {
        Ok(self.conn.execute(
            "UPDATE episodes SET download_status = NULL WHERE download_status = 'downloading'",
            [],
        )?)
    }

    /// Clear the download_path and download_status columns for the given
    /// episode IDs — called after the on-disk file has been deleted.
    pub fn clear_download_columns(&self, episode_ids: &[i64]) -> Result<()> {
        for &id in episode_ids {
            self.conn.execute(
                "UPDATE episodes SET download_path = NULL, download_status = NULL WHERE id=?1",
                [id],
            )?;
        }
        Ok(())
    }

    /// Set the download status (and optionally the path) for an episode.
    pub fn set_download_status(
        &self,
        episode_id: i64,
        status: &str,
        path: Option<&str>,
    ) -> Result<()> {
        self.conn.execute(
            "UPDATE episodes SET download_status=?1, download_path=COALESCE(?2, download_path) WHERE id=?3",
            (status, path, episode_id),
        )?;
        Ok(())
    }

    /// Compute which episode IDs would be affected by a cascade marking
    /// `seen` or `unseen` from the given episode number. Used by
    /// `commands::seen` to know which downloads to auto-delete.
    pub fn episode_ids_in_cascade(
        &self,
        series_id: i64,
        number: &str,
        seen: bool,
    ) -> Result<Vec<i64>> {
        let target = parse_ep_number(number);
        let mut stmt = self
            .conn
            .prepare("SELECT id, number FROM episodes WHERE series_id=?1")?;
        let rows: Vec<(i64, String)> = stmt
            .query_map([series_id], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut ids = Vec::new();
        for (id, num) in rows {
            let matches = match (target, parse_ep_number(&num)) {
                (Some(t), Some(v)) if seen => v <= t,
                (Some(t), Some(v)) => v >= t,
                (None, _) => num == number,
                _ => false,
            };
            if matches {
                ids.push(id);
            }
        }
        Ok(ids)
    }

    /// Retrieve a single episode by its ID.
    pub fn get_episode(&self, episode_id: i64) -> Result<crate::models::Episode> {
        Ok(self.conn.query_row(
            "SELECT id, series_id, number, title, url, released_at, seen,
                    download_path, download_status
             FROM episodes WHERE id=?1",
            [episode_id],
            |r| {
                Ok(crate::models::Episode {
                    id: r.get(0)?,
                    series_id: r.get(1)?,
                    number: r.get(2)?,
                    title: r.get(3)?,
                    url: r.get(4)?,
                    released_at: r.get(5)?,
                    seen: r.get::<_, i64>(6)? != 0,
                    download_path: r.get(7)?,
                    download_status: r.get(8)?,
                })
            },
        )?)
    }

    /// Total size in bytes of all downloaded episode files on disk.
    pub fn downloads_disk_size(&self) -> Result<u64> {
        let mut stmt = self
            .conn
            .prepare("SELECT download_path FROM episodes WHERE download_path IS NOT NULL")?;
        let paths: Vec<String> = stmt
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let total = paths
            .iter()
            .filter_map(|p| std::fs::metadata(p).ok())
            .map(|m| m.len())
            .sum();
        Ok(total)
    }

    /// Delete ALL downloaded episode files from disk within `downloads_dir` and
    /// clear their DB columns. Returns how many files were cleaned up.
    pub fn clear_all_downloads(&self, downloads_dir: &std::path::Path) -> Result<usize> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, download_path FROM episodes WHERE download_path IS NOT NULL")?;
        let rows: Vec<(i64, String)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut cleaned = 0;
        let mut failed = 0;
        for (id, path) in &rows {
            let p = std::path::Path::new(path);
            let can_delete = crate::download_safety::is_safe_file_in_dir(downloads_dir, p);
            if can_delete {
                match std::fs::remove_file(p) {
                    Ok(()) => {
                        self.conn.execute(
                            "UPDATE episodes SET download_path = NULL, download_status = NULL WHERE id=?1",
                            [id],
                        )?;
                        cleaned += 1;
                        if let Some(parent) = p.parent() {
                            crate::download_safety::remove_empty_dir_in_dir(downloads_dir, parent);
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                        self.conn.execute(
                            "UPDATE episodes SET download_path = NULL, download_status = NULL WHERE id=?1",
                            [id],
                        )?;
                        cleaned += 1;
                    }
                    Err(e) => {
                        eprintln!("[download] clear_all_downloads: failed to remove {path}: {e}");
                        failed += 1;
                    }
                }
            } else if !p.exists() {
                self.conn.execute(
                    "UPDATE episodes SET download_path = NULL, download_status = NULL WHERE id=?1",
                    [id],
                )?;
                cleaned += 1;
            } else {
                eprintln!("[download] clear_all_downloads: skipping file outside downloads_dir: {path}");
                failed += 1;
            }
        }
        if failed > 0 {
            eprintln!("[download] clear_all_downloads: {failed} files could not be cleaned up");
        }
        Ok(cleaned)
    }

    /// Returns all non-NULL `download_path` values for episodes of the given series.
    pub fn series_downloaded_paths(&self, series_id: i64) -> Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT download_path FROM episodes WHERE series_id=?1 AND download_path IS NOT NULL")?;
        let paths: Vec<String> = stmt
            .query_map([series_id], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(paths)
    }

    /// Returns canonicalized `PathBuf`s for all episodes with `download_status = 'downloaded'`.
    pub fn downloaded_episode_canonical_paths(&self) -> Result<std::collections::HashSet<std::path::PathBuf>> {
        let mut stmt = self.conn.prepare(
            "SELECT download_path FROM episodes WHERE download_status = 'downloaded' AND download_path IS NOT NULL",
        )?;
        let paths: Vec<String> = stmt
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut set = std::collections::HashSet::new();
        for p in paths {
            let path = std::path::PathBuf::from(&p);
            if let Ok(canon) = path.canonicalize() {
                set.insert(canon);
            } else {
                set.insert(path);
            }
        }
        Ok(set)
    }
}

#[cfg(test)]
mod tests;
