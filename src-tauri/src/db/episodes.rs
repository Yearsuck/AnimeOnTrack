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
    /// side of refresh()'s skip decision (see `commands::should_fetch_series`),
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
        let tx = self.conn.unchecked_transaction()?;
        let rows: Vec<(i64, String)> = {
            let mut stmt = tx.prepare("SELECT id, number FROM episodes WHERE series_id=?1")?;
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
                tx.execute(
                    "UPDATE episodes SET seen=?1, seen_at = CASE WHEN ?1=1 THEN COALESCE(seen_at, datetime('now')) ELSE NULL END
                     WHERE id=?2",
                    (seen as i64, id),
                )?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Returns the series IDs for every row in the same canonical group as `series_id`.
    pub fn canonical_series_ids(&self, series_id: i64) -> Result<Vec<i64>> {
        let (al_id, title): (Option<i64>, String) = match self.conn.query_row(
            "SELECT anilist_id, title FROM series WHERE id=?1",
            [series_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        ) {
            Ok(v) => v,
            Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(vec![]),
            Err(e) => return Err(e.into()),
        };

        let mut stmt = self.conn.prepare(
            "SELECT s.id, s.anilist_id, s.title, c.title AS catalog_title
             FROM series s
             LEFT JOIN anilist_catalog c ON c.id = s.anilist_id"
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
            if let (Some(al_id), Some(cat_title)) = (r.anilist_id, &r.catalog_title) {
                let tkey = format!("t:{}", crate::matching::normalize_title(cat_title));
                tkey_to_anilist
                    .entry(tkey)
                    .and_modify(|id| *id = (*id).min(al_id))
                    .or_insert(al_id);
            }
        }

        let target_key = match al_id {
            Some(id) => format!("al:{id}"),
            None => {
                let tkey = format!("t:{}", crate::matching::normalize_title(&title));
                if let Some(al_id) = tkey_to_anilist.get(&tkey).copied() {
                    format!("al:{al_id}")
                } else {
                    tkey
                }
            }
        };

        let mut out = Vec::new();
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
            if key == target_key {
                out.push(r.id);
            }
        }
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
                let same_non_null_al = match (clicked_al_id, sib_al_id) {
                    (Some(a), Some(b)) => a == b,
                    _ => false,
                };
                let titles_equal = crate::matching::normalize_title(&clicked_title)
                    == crate::matching::normalize_title(&sib_title);

                if !same_non_null_al && !titles_equal {
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

    /// Delete ALL downloaded episode files from disk and clear their DB
    /// columns. Returns how many files were cleaned up.
    pub fn clear_all_downloads(&self) -> Result<usize> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, download_path FROM episodes WHERE download_path IS NOT NULL")?;
        let rows: Vec<(i64, String)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut cleaned = 0;
        for (id, path) in &rows {
            let _ = std::fs::remove_file(path);
            self.conn.execute(
                "UPDATE episodes SET download_path = NULL, download_status = NULL WHERE id=?1",
                [id],
            )?;
            cleaned += 1;
        }
        Ok(cleaned)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::test_support::*;

    // ---- is_unique_violation must name ONE constraint, not a whole class ----

    /// The real thing it is meant to tolerate: two episodes of the same series
    /// on the same URL.
    #[test]
    fn is_unique_violation_accepts_the_episodes_url_collision() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
        let sid = db.upsert_series(src, &mk_airing("a", "A", None)).unwrap();
        let err: anyhow::Error = db
            .conn
            .execute(
                "INSERT INTO episodes(series_id, number, url) VALUES(?1, '1', 'u'), (?1, '2', 'u')",
                [sid],
            )
            .unwrap_err()
            .into();
        assert!(is_unique_violation(&err), "expected a UNIQUE(series_id, url) hit, got: {err}");
    }

    /// Everything else that raises `ErrorCode::ConstraintViolation` must NOT be
    /// swallowed as "url collides with another episode" — a scraped episode
    /// with a NULL number, or one pointing at a series that doesn't exist, is a
    /// genuine data bug and has to surface.
    #[test]
    fn is_unique_violation_rejects_other_constraint_failures() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
        let sid = db.upsert_series(src, &mk_airing("a", "A", None)).unwrap();

        // NOT NULL on episodes.number.
        let not_null: anyhow::Error = db
            .conn
            .execute("INSERT INTO episodes(series_id, number, url) VALUES(?1, NULL, 'n')", [sid])
            .unwrap_err()
            .into();
        assert!(!is_unique_violation(&not_null), "NOT NULL must not read as a url collision");

        // A different table's UNIQUE constraint (settings.key is the PK) is
        // also not this one — the message check, not just the extended code.
        db.set_setting("k", "v").unwrap();
        let other_table: anyhow::Error = db
            .conn
            .execute("INSERT INTO settings(key, value) VALUES('k', 'v2')", [])
            .unwrap_err()
            .into();
        assert!(!is_unique_violation(&other_table), "another table's constraint is not ours");

        // And a plain non-SQLite error is never a match.
        assert!(!is_unique_violation(&anyhow::anyhow!("something else entirely")));
    }

    #[test]
    fn apply_episode_diff_refreshes_known_inserts_new_and_reports_the_new_count() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "old.example", "animeytx").unwrap();
        let sid = db.upsert_series(src, &mk_airing("a", "A", None)).unwrap();
        db.insert_episode(&crate::models::Episode {
            id: 0, series_id: sid, number: "1".into(), title: None,
            url: "https://old.example/a-1".into(), released_at: None, seen: true, download_path: None, download_status: None,
        }).unwrap();

        let scraped = vec![
            crate::models::Episode {
                id: 0, series_id: 0, number: "1".into(), title: Some("Ep 1".into()),
                url: "https://new.example/a-1".into(), released_at: Some("hoy".into()), seen: false, download_path: None, download_status: None,
            },
            crate::models::Episode {
                id: 0, series_id: 0, number: "2".into(), title: None,
                url: "https://new.example/a-2".into(), released_at: None, seen: false, download_path: None, download_status: None,
            },
        ];
        let inserted = db.apply_episode_diff(sid, Some("https://new.example"), &scraped).unwrap();
        assert_eq!(inserted, 1, "only episode 2 is new");

        let eps = db.list_series_episodes(sid).unwrap();
        assert_eq!(eps.len(), 2);
        assert!(eps[0].seen, "refreshing episode 1's link must not touch its seen state");
        assert_eq!(eps[0].url, "https://new.example/a-1", "known episode's link is refreshed in place");
        assert_eq!(db.get_series_url(sid).unwrap().as_deref(), Some("https://new.example"));
        assert!(db.last_checked_age_secs(sid).unwrap().is_some(), "last_checked_at must be stamped");
    }

    #[test]
    fn apply_episode_diff_repro_5006_constraint_clash() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "tio.example", "tioanime").unwrap();
        let sid = db.upsert_series(src, &mk_airing("shingeki", "Shingeki", None)).unwrap();
        db.insert_episode(&crate::models::Episode {
            id: 0, series_id: sid, number: "135".into(), title: None,
            url: "https://tioanime.com/ver/shingeki-no-kyojin-13.5".into(), released_at: None, seen: true, download_path: None, download_status: None,
        }).unwrap();
        db.insert_episode(&crate::models::Episode {
            id: 0, series_id: sid, number: "13".into(), title: None,
            url: "https://tioanime.com/ver/shingeki-no-kyojin-13".into(), released_at: None, seen: true, download_path: None, download_status: None,
        }).unwrap();

        let scraped = vec![
            crate::models::Episode {
                id: 0, series_id: 0, number: "135".into(), title: None,
                url: "https://tioanime.com/ver/shingeki-no-kyojin-13.5".into(), released_at: None, seen: false, download_path: None, download_status: None,
            },
            crate::models::Episode {
                id: 0, series_id: 0, number: "13".into(), title: None,
                // Same url the "135" episode already owns — the exact
                // constraint clash observed live for series 5006.
                url: "https://tioanime.com/ver/shingeki-no-kyojin-13.5".into(), released_at: None, seen: false, download_path: None, download_status: None,
            },
        ];
        // Must skip the colliding episode and finish normally — this used
        // to propagate the constraint error and abort the whole `refresh()`
        // cycle for every series still queued behind this one.
        let inserted = db.apply_episode_diff(sid, None, &scraped).unwrap();
        assert_eq!(inserted, 0, "both numbers were already known, nothing new to insert");
    }

    /// The counting bug: `new_episodes()` keys on episode *number*, the
    /// UNIQUE constraint keys on URL. When a stored episode's number no
    /// longer matches the site's label (here: "13.5" was stored as "135" by
    /// an older `digits_in`, and the adapter now emits "13.5"), the diff
    /// offers it as new, the INSERT conflicts on the unchanged URL and writes
    /// nothing — and `apply_episode_diff` used to count it as one new episode
    /// anyway, on every refresh, forever, for an episode it never inserted.
    #[test]
    fn apply_episode_diff_does_not_count_an_episode_whose_url_already_exists() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("TioAnime", "tio.example", "tioanime").unwrap();
        let sid = db.upsert_series(src, &mk_airing("shingeki", "Shingeki", None)).unwrap();
        // Stored under the OLD number for this URL.
        db.insert_episode(&crate::models::Episode {
            id: 0, series_id: sid, number: "135".into(), title: None,
            url: "https://tioanime.com/ver/shingeki-no-kyojin-13.5".into(),
            released_at: None, seen: false, download_path: None, download_status: None,
        }).unwrap();

        // The site now labels that same URL "13.5" — an unknown *number*, so
        // the diff offers it as new, but its URL is already taken.
        let scraped = vec![crate::models::Episode {
            id: 0, series_id: 0, number: "13.5".into(), title: None,
            url: "https://tioanime.com/ver/shingeki-no-kyojin-13.5".into(),
            released_at: None, seen: false, download_path: None, download_status: None,
        }];
        let inserted = db.apply_episode_diff(sid, None, &scraped).unwrap();
        assert_eq!(inserted, 0, "nothing was actually written, so nothing is new");
        assert_eq!(db.episode_count(sid).unwrap(), 1, "and no duplicate row appeared");

        // Repeating the refresh must stay at zero — this is the part that
        // used to report a phantom new episode on every single cycle.
        assert_eq!(db.apply_episode_diff(sid, None, &scraped).unwrap(), 0);
    }

    /// A genuinely-new fractional episode (its own number AND its own URL)
    /// still counts, and lands as its own row next to the integer episode it
    /// used to collide with once `digits_in` stopped truncating it.
    #[test]
    fn apply_episode_diff_stores_a_fractional_episode_as_its_own_row() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("TioAnime", "tio.example", "tioanime").unwrap();
        let sid = db.upsert_series(src, &mk_airing("shingeki", "Shingeki", None)).unwrap();
        db.insert_episode(&crate::models::Episode {
            id: 0, series_id: sid, number: "13".into(), title: None,
            url: "https://tioanime.com/ver/shingeki-no-kyojin-13".into(),
            released_at: None, seen: true, download_path: None, download_status: None,
        }).unwrap();

        let scraped = vec![
            crate::models::Episode {
                id: 0, series_id: 0, number: "13".into(), title: None,
                url: "https://tioanime.com/ver/shingeki-no-kyojin-13".into(),
                released_at: None, seen: false, download_path: None, download_status: None,
            },
            crate::models::Episode {
                id: 0, series_id: 0, number: "13.5".into(), title: None,
                url: "https://tioanime.com/ver/shingeki-no-kyojin-13.5".into(),
                released_at: None, seen: false, download_path: None, download_status: None,
            },
        ];
        let inserted = db.apply_episode_diff(sid, None, &scraped).unwrap();
        assert_eq!(inserted, 1, "only the half-episode is new");

        let eps = db.list_series_episodes(sid).unwrap();
        assert_eq!(eps.len(), 2);
        let thirteen = eps.iter().find(|e| e.number == "13").unwrap();
        assert_eq!(
            thirteen.url, "https://tioanime.com/ver/shingeki-no-kyojin-13",
            "episode 13's url must not be rewritten to the half-episode's href"
        );
        assert!(thirteen.seen, "and its watch state is untouched");
        assert!(eps.iter().any(|e| e.number == "13.5"));

        // Re-scraping the same list is a no-op — the fractional number is now
        // a known number, so it is neither re-inserted nor re-counted.
        assert_eq!(db.apply_episode_diff(sid, None, &scraped).unwrap(), 0);
        assert_eq!(db.episode_count(sid).unwrap(), 2);

        // The cascade orders it strictly between 13 and 14: marking 13.5 seen
        // must not touch a later episode.
        db.insert_episode(&crate::models::Episode {
            id: 0, series_id: sid, number: "14".into(), title: None,
            url: "https://tioanime.com/ver/shingeki-no-kyojin-14".into(),
            released_at: None, seen: false, download_path: None, download_status: None,
        }).unwrap();
        db.set_seen_cascade(sid, "13.5", true).unwrap();
        let eps = db.list_series_episodes(sid).unwrap();
        assert!(eps.iter().find(|e| e.number == "13.5").unwrap().seen);
        assert!(!eps.iter().find(|e| e.number == "14").unwrap().seen);
    }

    #[test]
    fn apply_episode_diff_leaves_series_url_alone_when_none_is_given() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
        let sid = db.upsert_series(src, &mk_airing("a", "A", None)).unwrap();
        let original_url = db.get_series_url(sid).unwrap();

        db.apply_episode_diff(sid, None, &[]).unwrap();

        assert_eq!(db.get_series_url(sid).unwrap(), original_url);
    }

    #[test]
    fn episode_count_counts_only_this_series() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
        let sid_a = db.upsert_series(src, &mk_airing("a", "A", None)).unwrap();
        let sid_b = db.upsert_series(src, &mk_airing("b", "B", None)).unwrap();
        for i in 1..=3 {
            db.insert_episode(&crate::models::Episode {
                id: 0, series_id: sid_a, number: i.to_string(), title: None,
                url: format!("https://site/a-{i}"), released_at: None, seen: false, download_path: None, download_status: None,
            }).unwrap();
        }
        assert_eq!(db.episode_count(sid_a).unwrap(), 3);
        assert_eq!(db.episode_count(sid_b).unwrap(), 0);
    }

    /// `max_episode_number` must reflect the highest real episode number, not
    /// the row count — recap/"0"/version-reupload rows must never inflate it
    /// (Bug B in the 2026-07-12 airing-refresh-missing-episodes fix: a row
    /// COUNT was being compared against the site's next-episode-number badge,
    /// so a recap row cancelled the detection margin and a real new episode
    /// got silently skipped).
    #[test]
    fn max_episode_number_ignores_recap_and_version_rows() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();

        // Wistoria-shaped: "0|Recap", "1".."12" -> 13 rows, highest real = 12.
        let sid_a = db.upsert_series(src, &mk_airing("a", "A", None)).unwrap();
        db.insert_episode(&crate::models::Episode {
            id: 0, series_id: sid_a, number: "0 | Recap".to_string(), title: None,
            url: "https://site/a-recap".to_string(), released_at: None, seen: false, download_path: None, download_status: None,
        }).unwrap();
        for i in 1..=12 {
            db.insert_episode(&crate::models::Episode {
                id: 0, series_id: sid_a, number: i.to_string(), title: None,
                url: format!("https://site/a-{i}"), released_at: None, seen: false, download_path: None, download_status: None,
            }).unwrap();
        }
        assert_eq!(db.episode_count(sid_a).unwrap(), 13, "sanity: row count is 13");
        assert_eq!(db.max_episode_number(sid_a).unwrap(), 12);

        // Tomb Raider King-shaped: "1 | v2", "2 | v1" -> highest real = 2.
        let sid_b = db.upsert_series(src, &mk_airing("b", "B", None)).unwrap();
        db.insert_episode(&crate::models::Episode {
            id: 0, series_id: sid_b, number: "1 | v2".to_string(), title: None,
            url: "https://site/b-1".to_string(), released_at: None, seen: false, download_path: None, download_status: None,
        }).unwrap();
        db.insert_episode(&crate::models::Episode {
            id: 0, series_id: sid_b, number: "2 | v1".to_string(), title: None,
            url: "https://site/b-2".to_string(), released_at: None, seen: false, download_path: None, download_status: None,
        }).unwrap();
        assert_eq!(db.max_episode_number(sid_b).unwrap(), 2);

        // No episodes at all -> 0.
        let sid_c = db.upsert_series(src, &mk_airing("c", "C", None)).unwrap();
        assert_eq!(db.max_episode_number(sid_c).unwrap(), 0);
    }

    /// The batched form must agree with `max_episode_number` per series, be
    /// scoped to followed rows of the given source, and simply omit a
    /// series with zero episode rows (callers default a missing key to 0).
    #[test]
    fn max_episode_numbers_for_source_matches_the_per_series_version() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
        let other_src = db.upsert_source("TioAnime", "t", "tioanime").unwrap();

        let sid_a = db.upsert_series(src, &mk_airing("a", "A", None)).unwrap();
        db.set_followed(sid_a, true).unwrap();
        db.insert_episode(&crate::models::Episode {
            id: 0, series_id: sid_a, number: "5".into(), title: None,
            url: "https://site/a-5".into(), released_at: None, seen: false, download_path: None, download_status: None,
        }).unwrap();

        // Followed but no episodes yet -> absent from the map.
        let sid_b = db.upsert_series(src, &mk_airing("b", "B", None)).unwrap();
        db.set_followed(sid_b, true).unwrap();

        // Not followed -> excluded even though it has episodes.
        let sid_c = db.upsert_series(src, &mk_airing("c", "C", None)).unwrap();
        db.insert_episode(&crate::models::Episode {
            id: 0, series_id: sid_c, number: "9".into(), title: None,
            url: "https://site/c-9".into(), released_at: None, seen: false, download_path: None, download_status: None,
        }).unwrap();

        // Followed on a different source -> excluded from this source's map.
        let sid_other = db.upsert_series(other_src, &mk_airing("d", "D", None)).unwrap();
        db.set_followed(sid_other, true).unwrap();
        db.insert_episode(&crate::models::Episode {
            id: 0, series_id: sid_other, number: "3".into(), title: None,
            url: "https://site/d-3".into(), released_at: None, seen: false, download_path: None, download_status: None,
        }).unwrap();

        let map = db.max_episode_numbers_for_source(src).unwrap();
        assert_eq!(map.get(&sid_a).copied(), Some(5));
        assert_eq!(map.get(&sid_b), None, "no episode rows -> absent, not 0");
        assert_eq!(map.get(&sid_c), None, "not followed -> excluded");
        assert_eq!(map.get(&sid_other), None, "different source -> excluded");
    }

    // ---- "Esta temporada" first-episode date (2026-07-13) ----

    /// The lowest-numbered episode must win regardless of insertion order —
    /// the site's episode list isn't always scraped/inserted in number order.
    #[test]
    fn first_episode_dates_picks_lowest_numbered_episode_out_of_order() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
        let sid = db.upsert_series(src, &mk_airing("a", "A", None)).unwrap();

        // Inserted out of order: 3, 1, 2.
        db.insert_episode(&crate::models::Episode {
            id: 0, series_id: sid, number: "3".into(), title: None,
            url: "https://site/a-3".into(), released_at: Some("agosto 1, 2026".into()), seen: false, download_path: None, download_status: None,
        }).unwrap();
        db.insert_episode(&crate::models::Episode {
            id: 0, series_id: sid, number: "1".into(), title: None,
            url: "https://site/a-1".into(), released_at: Some("junio 29, 2026".into()), seen: false, download_path: None, download_status: None,
        }).unwrap();
        db.insert_episode(&crate::models::Episode {
            id: 0, series_id: sid, number: "2".into(), title: None,
            url: "https://site/a-2".into(), released_at: Some("julio 15, 2026".into()), seen: false, download_path: None, download_status: None,
        }).unwrap();

        let dates = db.first_episode_dates(src).unwrap();
        assert_eq!(dates.get(&sid), Some(&"junio 29, 2026".to_string()));
    }

    #[test]
    fn first_episode_dates_omits_series_with_null_released_at() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
        let sid = db.upsert_series(src, &mk_airing("a", "A", None)).unwrap();
        db.insert_episode(&crate::models::Episode {
            id: 0, series_id: sid, number: "1".into(), title: None,
            url: "https://site/a-1".into(), released_at: None, seen: false, download_path: None, download_status: None,
        }).unwrap();

        let dates = db.first_episode_dates(src).unwrap();
        assert!(!dates.contains_key(&sid));
    }

    #[test]
    fn first_episode_dates_omits_non_airing_and_other_source_series() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
        let other_src = db.upsert_source("Other", "c", "othersite").unwrap();

        let mut not_airing = mk_airing("na", "NotAiring", None);
        not_airing.is_airing = false;
        let sid_not_airing = db.upsert_series(src, &not_airing).unwrap();
        db.insert_episode(&crate::models::Episode {
            id: 0, series_id: sid_not_airing, number: "1".into(), title: None,
            url: "https://site/na-1".into(), released_at: Some("mayo 1, 2026".into()), seen: false, download_path: None, download_status: None,
        }).unwrap();

        let sid_other = db.upsert_series(other_src, &mk_airing("o", "Other", None)).unwrap();
        db.insert_episode(&crate::models::Episode {
            id: 0, series_id: sid_other, number: "1".into(), title: None,
            url: "https://other/o-1".into(), released_at: Some("mayo 1, 2026".into()), seen: false, download_path: None, download_status: None,
        }).unwrap();

        let dates = db.first_episode_dates(src).unwrap();
        assert!(!dates.contains_key(&sid_not_airing));
        assert!(!dates.contains_key(&sid_other));
    }

    #[test]
    fn first_episode_dates_includes_unfollowed_series_with_episodes() {
        // The DB layer has no followed requirement: unfollowed airing series
        // with episode data should be included in first_episode_dates results.
        // The filter logic lives in the frontend (AiringGrid.tsx), not here.
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();

        // Create an unfollowed airing series (never call set_followed).
        let sid = db.upsert_series(src, &mk_airing("unfollowed", "Unfollowed", None)).unwrap();
        assert_eq!(
            db.conn
                .query_row("SELECT followed FROM series WHERE id=?1", [sid], |r| r.get::<_, i64>(0))
                .unwrap(),
            0,
            "series must be unfollowed by default"
        );

        // Insert an episode with a non-null released_at.
        db.insert_episode(&crate::models::Episode {
            id: 0, series_id: sid, number: "1".into(), title: None,
            url: "https://site/unfollowed-1".into(), released_at: Some("julio 15, 2026".into()), seen: false, download_path: None, download_status: None,
        }).unwrap();

        // Unfollowed series with episode data IS included in first_episode_dates.
        let dates = db.first_episode_dates(src).unwrap();
        assert_eq!(
            dates.get(&sid),
            Some(&"julio 15, 2026".to_string()),
            "unfollowed series with episodes must be included"
        );
    }

    #[test]
    fn seen_cascade_handles_non_integer_episode_numbers() {
        // Regression: episode numbers like "1x05" (season-prefixed) used to
        // fail Rust's strict i64 parse (-> 0) while SQLite's CAST still read
        // the leading digit, so un-marking one used to wipe every episode's
        // seen flag in the series instead of just the later ones.
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "https://wwv.animeytx.net", "animeytx").unwrap();
        let s = crate::models::Series {
            id: 0, slug: "x".into(), title: "X".into(),
            url: "u".into(), cover_url: None, is_airing: true, followed: false, next_episode_at: None, site_episode_count: None,
        };
        let sid = db.upsert_series(src, &s).unwrap();

        let mk = |n: &str, url: &str| crate::models::Episode {
            id: 0, series_id: sid, number: n.into(), title: None,
            url: url.into(), released_at: None, seen: false, download_path: None, download_status: None,
        };
        db.insert_episode(&mk("1x01", "https://site/e1")).unwrap();
        db.insert_episode(&mk("1x02", "https://site/e2")).unwrap();
        db.insert_episode(&mk("1x03", "https://site/e3")).unwrap();

        // marking "1x02" seen cascades to "1x01" too, but not "1x03"
        db.set_seen_cascade(sid, "1x02", true).unwrap();
        let eps = db.list_series_episodes(sid).unwrap();
        assert!(eps.iter().find(|e| e.number == "1x01").unwrap().seen);
        assert!(eps.iter().find(|e| e.number == "1x02").unwrap().seen);
        assert!(!eps.iter().find(|e| e.number == "1x03").unwrap().seen);

        // un-marking "1x02" un-marks "1x02"/"1x03" but must leave "1x01" alone
        db.set_seen_cascade(sid, "1x02", false).unwrap();
        let eps = db.list_series_episodes(sid).unwrap();
        assert!(eps.iter().find(|e| e.number == "1x01").unwrap().seen);
        assert!(!eps.iter().find(|e| e.number == "1x02").unwrap().seen);
        assert!(!eps.iter().find(|e| e.number == "1x03").unwrap().seen);
    }

    #[test]
    fn set_seen_cascade_stamps_and_clears_seen_at() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "https://wwv.animeytx.net", "animeytx").unwrap();
        let s = crate::models::Series {
            id: 0, slug: "x".into(), title: "X".into(),
            url: "u".into(), cover_url: None, is_airing: true, followed: false, next_episode_at: None, site_episode_count: None,
        };
        let sid = db.upsert_series(src, &s).unwrap();
        let mk = |n: &str, url: &str| crate::models::Episode {
            id: 0, series_id: sid, number: n.into(), title: None,
            url: url.into(), released_at: None, seen: false, download_path: None, download_status: None,
        };
        let (e1, _) = db.insert_episode(&mk("1", "https://site/e1")).unwrap();
        let (e2, _) = db.insert_episode(&mk("2", "https://site/e2")).unwrap();

        let seen_at = |id: i64| -> Option<String> {
            db.conn
                .query_row("SELECT seen_at FROM episodes WHERE id=?1", [id], |r| r.get(0))
                .unwrap()
        };
        assert!(seen_at(e1).is_none());
        assert!(seen_at(e2).is_none());

        // marking "2" seen cascades to "1" — both rows get seen_at, not just
        // the one explicitly clicked.
        db.set_seen_cascade(sid, "2", true).unwrap();
        assert!(seen_at(e1).is_some());
        assert!(seen_at(e2).is_some());

        // un-marking "2" clears seen_at on the cascade target too
        db.set_seen_cascade(sid, "2", false).unwrap();
        assert!(seen_at(e1).is_some(), "e1 stays seen, keeps its seen_at");
        assert!(seen_at(e2).is_none(), "e2 un-marked, seen_at cleared");
    }

    #[test]
    fn set_seen_cascade_preserves_seen_at_of_episodes_already_seen() {
        // The reported bug: watching episodes 1-11 over separate days, each
        // with its own real seen_at, then marking episode 12 seen used to
        // stamp every one of the 12 rows with today's date — flattening the
        // whole series' watch history and corrupting every date-bucketed
        // stat (30-day heatmap, yearly activity, hourly distribution).
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "https://wwv.animeytx.net", "animeytx").unwrap();
        let s = crate::models::Series {
            id: 0, slug: "x".into(), title: "X".into(),
            url: "u".into(), cover_url: None, is_airing: true, followed: false, next_episode_at: None, site_episode_count: None,
        };
        let sid = db.upsert_series(src, &s).unwrap();
        let (e1, _) = db
            .insert_episode(&crate::models::Episode {
                id: 0, series_id: sid, number: "1".into(), title: None,
                url: "https://site/e1".into(), released_at: None, seen: false, download_path: None, download_status: None,
            })
            .unwrap();
        let (e2, _) = db
            .insert_episode(&crate::models::Episode {
                id: 0, series_id: sid, number: "2".into(), title: None,
                url: "https://site/e2".into(), released_at: None, seen: false, download_path: None, download_status: None,
            })
            .unwrap();

        // Episode 1 was watched three weeks ago — give it a real, old seen_at.
        db.set_seen_cascade(sid, "1", true).unwrap();
        db.conn
            .execute(
                "UPDATE episodes SET seen_at = datetime('now', '-21 days') WHERE id=?1",
                [e1],
            )
            .unwrap();
        let old_seen_at: String = db
            .conn
            .query_row("SELECT seen_at FROM episodes WHERE id=?1", [e1], |r| r.get(0))
            .unwrap();

        // Marking episode 2 seen today re-touches episode 1 via the cascade
        // (1 <= 2) — its original watch date must survive unchanged.
        db.set_seen_cascade(sid, "2", true).unwrap();
        let seen_at_after: String = db
            .conn
            .query_row("SELECT seen_at FROM episodes WHERE id=?1", [e1], |r| r.get(0))
            .unwrap();
        assert_eq!(seen_at_after, old_seen_at, "cascade must not reset an already-seen episode's watch date");
        let e2_seen_at: Option<String> = db
            .conn
            .query_row("SELECT seen_at FROM episodes WHERE id=?1", [e2], |r| r.get(0))
            .unwrap();
        assert!(e2_seen_at.is_some(), "the newly-seen episode still gets stamped");
    }

    #[test]
    fn set_seen_stamps_and_clears_seen_at() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "https://wwv.animeytx.net", "animeytx").unwrap();
        let s = crate::models::Series {
            id: 0, slug: "x".into(), title: "X".into(),
            url: "u".into(), cover_url: None, is_airing: true, followed: false, next_episode_at: None, site_episode_count: None,
        };
        let sid = db.upsert_series(src, &s).unwrap();
        let (e1, _) = db
            .insert_episode(&crate::models::Episode {
                id: 0, series_id: sid, number: "1".into(), title: None,
                url: "https://site/e1".into(), released_at: None, seen: false, download_path: None, download_status: None,
            })
            .unwrap();

        db.set_seen(e1, true).unwrap();
        let seen_at: Option<String> = db
            .conn
            .query_row("SELECT seen_at FROM episodes WHERE id=?1", [e1], |r| r.get(0))
            .unwrap();
        assert!(seen_at.is_some());

        db.set_seen(e1, false).unwrap();
        let seen_at: Option<String> = db
            .conn
            .query_row("SELECT seen_at FROM episodes WHERE id=?1", [e1], |r| r.get(0))
            .unwrap();
        assert!(seen_at.is_none());
    }

    #[test]
    fn library_next_episode_agrees_with_series_detail_ordering() {
        // "9", "10", "10.5" — the design doc's canonical case for why next-
        // episode ordering must reuse list_series_episodes' ORDER BY
        // (CAST(number AS INTEGER) ASC, id ASC) rather than a numeric parse:
        // a naive string sort would put "10" before "9".
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "https://wwv.animeytx.net", "animeytx").unwrap();
        let s = crate::models::Series {
            id: 0, slug: "x".into(), title: "X".into(),
            url: "u".into(), cover_url: None, is_airing: true, followed: true, next_episode_at: None, site_episode_count: None,
        };
        let sid = db.upsert_series(src, &s).unwrap();
        // upsert_series never sets `followed` (scan path must not un-follow);
        // list_library filters on followed=1, so follow it explicitly.
        db.set_followed(sid, true).unwrap();
        let mk = |n: &str, url: &str| crate::models::Episode {
            id: 0, series_id: sid, number: n.into(), title: None,
            url: url.into(), released_at: None, seen: false, download_path: None, download_status: None,
        };
        // Insert out of numeric order to make sure ordering isn't just
        // reflecting insertion order.
        db.insert_episode(&mk("10", "https://site/e10")).unwrap();
        db.insert_episode(&mk("9", "https://site/e9")).unwrap();
        db.insert_episode(&mk("10.5", "https://site/e10.5")).unwrap();

        let detail_order = db.list_series_episodes(sid).unwrap();
        let first_unseen_in_detail_order = detail_order.iter().find(|e| !e.seen).unwrap();

        let items = db.list_library(src).unwrap();
        let item = items.iter().find(|it| it.series.id == sid).unwrap();
        let next = item.next_episode.as_ref().expect("has an unseen episode");
        assert_eq!(next.number, first_unseen_in_detail_order.number);
        assert_eq!(next.number, "9", "lowest by SeriesDetail's own ordering, not string sort");

        // Mark "9" seen; next should become "10" (not "10.5" — 10 sorts
        // before 10.5 under CAST(number AS INTEGER) since both cast to 10,
        // then id ASC breaks the tie in insertion order: "10" was inserted
        // before "10.5").
        db.set_seen_cascade(sid, "9", true).unwrap();
        let items = db.list_library(src).unwrap();
        let item = items.iter().find(|it| it.series.id == sid).unwrap();
        assert_eq!(item.next_episode.as_ref().unwrap().number, "10");
    }

    #[test]
    fn library_next_episode_none_when_fully_seen() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "https://wwv.animeytx.net", "animeytx").unwrap();
        let s = crate::models::Series {
            id: 0, slug: "x".into(), title: "X".into(),
            url: "u".into(), cover_url: None, is_airing: true, followed: true, next_episode_at: None, site_episode_count: None,
        };
        let sid = db.upsert_series(src, &s).unwrap();
        db.set_followed(sid, true).unwrap();
        db.insert_episode(&crate::models::Episode {
            id: 0, series_id: sid, number: "1".into(), title: None,
            url: "https://site/e1".into(), released_at: None, seen: false, download_path: None, download_status: None,
        })
        .unwrap();
        db.set_seen_cascade(sid, "1", true).unwrap();

        let items = db.list_library(src).unwrap();
        let item = items.iter().find(|it| it.series.id == sid).unwrap();
        assert!(item.next_episode.is_none());
        assert_eq!(item.total_episodes, 1);
        assert_eq!(item.seen_episodes, 1);
    }

    #[test]
    fn library_next_episode_none_and_no_panic_with_zero_episodes() {
        // A followed-but-never-scraped (or scrape-failed) series: total=0.
        // Must not divide by zero anywhere and must yield next_episode=None.
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "https://wwv.animeytx.net", "animeytx").unwrap();
        let s = crate::models::Series {
            id: 0, slug: "x".into(), title: "X".into(),
            url: "u".into(), cover_url: None, is_airing: true, followed: true, next_episode_at: None, site_episode_count: None,
        };
        let sid = db.upsert_series(src, &s).unwrap();
        db.set_followed(sid, true).unwrap();

        let items = db.list_library(src).unwrap();
        assert_eq!(items.len(), 1);
        assert!(items[0].next_episode.is_none());
        assert_eq!(items[0].total_episodes, 0);
        assert_eq!(items[0].seen_episodes, 0);
    }

    #[test]
    fn insert_episode_dedups_and_marks_seen() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "https://wwv.animeytx.net", "animeytx").unwrap();
        let s = crate::models::Series {
            id: 0, slug: "x".into(), title: "X".into(),
            url: "u".into(), cover_url: None, is_airing: true, followed: false, next_episode_at: None, site_episode_count: None,
        };
        let sid = db.upsert_series(src, &s).unwrap();
        db.set_followed(sid, true).unwrap();

        let ep = crate::models::Episode {
            id: 0, series_id: sid, number: "1".into(), title: None,
            url: "https://site/ep1".into(), released_at: None, seen: false, download_path: None, download_status: None,
        };
        let (eid, was_new) = db.insert_episode(&ep).unwrap();
        assert!(was_new, "the first insert really does write a row");
        // same url again => no new row
        let (eid_dup, was_new_again) = db.insert_episode(&ep).unwrap();
        assert_eq!(eid, eid_dup);
        assert!(
            !was_new_again,
            "a conflicting insert writes nothing and must report itself as not-new"
        );

        assert_eq!(db.pending_count(src, &[]).unwrap(), 1);
        db.set_seen(eid, true).unwrap();
        assert_eq!(db.pending_count(src, &[]).unwrap(), 0);
    }

    #[test]
    fn existing_episode_numbers_and_refresh_meta() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
        let s = crate::models::Series {
            id: 0, slug: "x".into(), title: "X".into(),
            url: "u".into(), cover_url: None, is_airing: true, followed: true, next_episode_at: None, site_episode_count: None,
        };
        let sid = db.upsert_series(src, &s).unwrap();
        db.insert_episode(&crate::models::Episode {
            id: 0, series_id: sid, number: "1".into(), title: None,
            url: "https://wwv.animeytx.net/anime/x-capitulo-1/".into(), released_at: None, seen: true, download_path: None, download_status: None,
        }).unwrap();

        let nums = db.existing_episode_numbers(sid).unwrap();
        assert!(nums.contains("1"));
        assert_eq!(nums.len(), 1);

        // A re-scan on a new domain refreshes the URL in place, leaving seen.
        db.refresh_episode_meta(sid, "1", "https://animeyt.cc/999/anime/x-capitulo-1/", None, None).unwrap();
        let (url, seen): (String, i64) = db.conn.query_row(
            "SELECT url, seen FROM episodes WHERE series_id=?1 AND number='1'",
            [sid], |r| Ok((r.get(0)?, r.get(1)?)),
        ).unwrap();
        assert_eq!(url, "https://animeyt.cc/999/anime/x-capitulo-1/");
        assert_eq!(seen, 1, "refreshing the link must not clear seen");
        // Still exactly one row for episode 1 — no duplicate.
        let n: i64 = db.conn.query_row(
            "SELECT COUNT(*) FROM episodes WHERE series_id=?1 AND number='1'", [sid], |r| r.get(0),
        ).unwrap();
        assert_eq!(n, 1);
    }

    #[test]
    fn mark_all_episodes_seen_marks_every_episode_via_cascade() {
        // Pins the idempotence fix: a row marked "Ya lo vi" (catalog "Seen")
        // after episodes are scraped in for a title the user decided
        // `Seen` on the catalog deck, every episode must end up seen, via the
        // same gap-free `set_seen_cascade` the rest of the app's watch-tracking
        // uses (not a separate blanket UPDATE).
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
        let s = crate::models::Series {
            id: 0, slug: "x".into(), title: "X".into(),
            url: "u".into(), cover_url: None, is_airing: false, followed: false, next_episode_at: None, site_episode_count: None,
        };
        let sid = db.upsert_series(src, &s).unwrap();

        for n in 1..=3 {
            db.insert_episode(&crate::models::Episode {
                id: 0, series_id: sid, number: n.to_string(), title: None,
                url: format!("e{n}"), released_at: None, seen: false, download_path: None, download_status: None,
            }).unwrap();
        }

        db.mark_all_episodes_seen(sid).unwrap();

        let eps = db.list_series_episodes(sid).unwrap();
        assert_eq!(eps.len(), 3);
        assert!(eps.iter().all(|e| e.seen), "every episode must be marked seen");
    }

    #[test]
    fn mark_all_episodes_seen_is_a_no_op_on_a_series_with_no_episodes() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
        let s = crate::models::Series {
            id: 0, slug: "x".into(), title: "X".into(),
            url: "u".into(), cover_url: None, is_airing: false, followed: false, next_episode_at: None, site_episode_count: None,
        };
        let sid = db.upsert_series(src, &s).unwrap();
        db.mark_all_episodes_seen(sid).unwrap(); // must not error on zero episodes
        assert!(db.list_series_episodes(sid).unwrap().is_empty());
    }

    #[test]
    fn known_series_urls_reflects_any_decided_row() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
        let s = crate::models::Series {
            id: 0, slug: "x".into(), title: "X".into(),
            url: "https://site/tv/x/".into(), cover_url: None, is_airing: false, followed: false, next_episode_at: None, site_episode_count: None,
        };
        let sid = db.upsert_series(src, &s).unwrap();
        db.set_backlog_status(sid, Some("discarded")).unwrap();

        let known = db.known_series_urls(src).unwrap();
        assert!(known.contains("https://site/tv/x/"));
        assert_eq!(known.len(), 1);
    }

    #[test]
    fn download_bookkeeping_and_cascade_detection() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
        let sid = db.upsert_series(src, &mk_airing("a", "A", None)).unwrap();

        let (ep1_id, _) = db.insert_episode(&crate::models::Episode {
            id: 0, series_id: sid, number: "1".into(), title: None,
            url: "https://site/a-1".into(), released_at: None, seen: false,
            download_path: None, download_status: None,
        }).unwrap();
        let (ep2_id, _) = db.insert_episode(&crate::models::Episode {
            id: 0, series_id: sid, number: "2".into(), title: None,
            url: "https://site/a-2".into(), released_at: None, seen: false,
            download_path: None, download_status: None,
        }).unwrap();

        // Initially no downloads
        assert_eq!(db.downloaded_paths(&[ep1_id, ep2_id]).unwrap().len(), 0);

        // Set ep1 downloaded
        let temp_file = std::env::temp_dir().join(format!("aot_test_dl_{}.mp4", ep1_id));
        std::fs::write(&temp_file, b"test-video").unwrap();
        let path_str = temp_file.to_str().unwrap();

        db.set_download_status(ep1_id, "downloaded", Some(path_str)).unwrap();

        let ep1 = db.get_episode(ep1_id).unwrap();
        assert_eq!(ep1.download_status.as_deref(), Some("downloaded"));
        assert_eq!(ep1.download_path.as_deref(), Some(path_str));

        let paths = db.downloaded_paths(&[ep1_id, ep2_id]).unwrap();
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0], (ep1_id, path_str.to_string()));

        // Check cascade affected IDs: marking episode 2 seen affects 1 and 2
        let cascade_ids = db.episode_ids_in_cascade(sid, "2", true).unwrap();
        assert!(cascade_ids.contains(&ep1_id));
        assert!(cascade_ids.contains(&ep2_id));

        // Clean up
        db.clear_download_columns(&[ep1_id]).unwrap();
        assert_eq!(db.downloaded_paths(&[ep1_id]).unwrap().len(), 0);
        let _ = std::fs::remove_file(&temp_file);
    }

    #[test]
    fn set_seen_cascade_canonical_syncs_across_canonical_group() {
        let db = Db::open(":memory:").unwrap();
        let src_a = db.upsert_source("AnimeYT", "https://a.example", "animeytx").unwrap();
        let src_b = db.upsert_source("TioAnime", "https://b.example", "tioanime").unwrap();

        let mk = |slug: &str, title: &str| crate::models::Series {
            id: 0,
            slug: slug.into(),
            title: title.into(),
            url: format!("https://site/{slug}"),
            cover_url: None,
            is_airing: true,
            followed: true,
            next_episode_at: None,
            site_episode_count: None,
        };

        let row_a = db.upsert_series(src_a, &mk("show-a", "Shared Show")).unwrap();
        db.set_followed(row_a, true).unwrap();
        let row_b = db.upsert_series(src_b, &mk("show-b", "Shared Show")).unwrap();
        db.set_followed(row_b, true).unwrap();

        for i in 1..=3 {
            db.insert_episode(&crate::models::Episode {
                id: 0,
                series_id: row_a,
                number: i.to_string(),
                title: None,
                url: format!("https://a/{i}"),
                released_at: None,
                seen: false,
                download_path: None,
                download_status: None,
            })
            .unwrap();
        }
        for i in 1..=4 {
            db.insert_episode(&crate::models::Episode {
                id: 0,
                series_id: row_b,
                number: i.to_string(),
                title: None,
                url: format!("https://b/{i}"),
                released_at: None,
                seen: false,
                download_path: None,
                download_status: None,
            })
            .unwrap();
        }

        // Marking 3 seen on A cascades 1-3 on BOTH rows
        db.set_seen_cascade_canonical(row_a, "3", true).unwrap();
        let eps_a = db.list_series_episodes(row_a).unwrap();
        let eps_b = db.list_series_episodes(row_b).unwrap();
        assert!(eps_a.iter().all(|e| e.seen), "A has 1-3 seen");
        assert_eq!(eps_b.iter().filter(|e| e.seen).count(), 3, "B has 1-3 seen");
        assert!(!eps_b.iter().find(|e| e.number == "4").unwrap().seen, "B's ep 4 is still unseen");

        // Un-marking 2 marks 2-4 unseen on both
        db.set_seen_cascade_canonical(row_a, "2", false).unwrap();
        let eps_a = db.list_series_episodes(row_a).unwrap();
        let eps_b = db.list_series_episodes(row_b).unwrap();
        assert!(eps_a.iter().find(|e| e.number == "1").unwrap().seen, "A's ep 1 stays seen");
        assert!(!eps_a.iter().find(|e| e.number == "2").unwrap().seen, "A's ep 2 unseen");
        assert!(!eps_a.iter().find(|e| e.number == "3").unwrap().seen, "A's ep 3 unseen");

        assert!(eps_b.iter().find(|e| e.number == "1").unwrap().seen, "B's ep 1 stays seen");
        assert!(!eps_b.iter().find(|e| e.number == "2").unwrap().seen, "B's ep 2 unseen");
        assert!(!eps_b.iter().find(|e| e.number == "3").unwrap().seen, "B's ep 3 unseen");
        assert!(!eps_b.iter().find(|e| e.number == "4").unwrap().seen, "B's ep 4 unseen");
    }

    #[test]
    fn set_seen_cascade_canonical_tensei_fixture() {
        let db = Db::open(":memory:").unwrap();
        let now = chrono::Utc::now().timestamp();
        let src_a = db.upsert_source("AnimeYT", "https://animeyt.cc", "animeytx").unwrap();
        let src_b = db.upsert_source("TioAnime", "https://tioanime.com", "tioanime").unwrap();

        let entries = vec![
            crate::anizip::MapEntry { episode: 1, season: 3, absolute: Some(25), air_at: Some(now - 14 * 86400) },
            crate::anizip::MapEntry { episode: 2, season: 3, absolute: Some(26), air_at: Some(now - 7 * 86400) },
            crate::anizip::MapEntry { episode: 3, season: 3, absolute: Some(27), air_at: Some(now) },
            crate::anizip::MapEntry { episode: 4, season: 3, absolute: Some(28), air_at: Some(now + 7 * 86400) },
            crate::anizip::MapEntry { episode: 5, season: 3, absolute: Some(29), air_at: Some(now + 14 * 86400) },
            crate::anizip::MapEntry { episode: 6, season: 3, absolute: Some(30), air_at: Some(now + 21 * 86400) },
            crate::anizip::MapEntry { episode: 7, season: 3, absolute: Some(31), air_at: Some(now + 28 * 86400) },
            crate::anizip::MapEntry { episode: 8, season: 3, absolute: Some(32), air_at: Some(now + 35 * 86400) },
        ];
        db.replace_episode_map(185756, &entries, now).unwrap();

        let mk = |slug: &str, title: &str| crate::models::Series {
            id: 0, slug: slug.into(), title: title.into(), url: format!("https://site/{slug}"),
            cover_url: None, is_airing: true, followed: true, next_episode_at: None, site_episode_count: None,
        };

        let row_a = db.upsert_series(src_a, &mk("tensei-a", "Tensei")).unwrap();
        db.set_anilist_id(row_a, 185756).unwrap();
        db.set_followed(row_a, true).unwrap();
        for i in 1..=3 {
            db.insert_episode(&crate::models::Episode {
                id: 0, series_id: row_a, number: i.to_string(), title: None,
                url: format!("https://animeyt.cc/tv/tensei/{i}"), released_at: None, seen: false,
                download_path: None, download_status: None,
            }).unwrap();
        }

        let row_b = db.upsert_series(src_b, &mk("tensei-b", "Tensei")).unwrap();
        db.set_anilist_id(row_b, 185756).unwrap();
        db.set_followed(row_b, true).unwrap();
        for i in 1..=28 {
            db.insert_episode(&crate::models::Episode {
                id: 0, series_id: row_b, number: i.to_string(), title: None,
                url: format!("https://tioanime.com/ver/tensei-{i}"), released_at: None, seen: false,
                download_path: None, download_status: None,
            }).unwrap();
        }

        // Marking animeyt "2" seen marks TioAnime 1..26 seen (and animeyt 1..2)
        db.set_seen_cascade_canonical(row_a, "2", true).unwrap();
        let eps_a = db.list_series_episodes(row_a).unwrap();
        let eps_b = db.list_series_episodes(row_b).unwrap();
        assert!(eps_a.iter().filter(|e| e.seen).all(|e| e.number == "1" || e.number == "2"));
        assert_eq!(eps_a.iter().filter(|e| e.seen).count(), 2);
        assert_eq!(eps_b.iter().filter(|e| e.seen).count(), 26, "TioAnime 1..26 seen");
        assert!(!eps_b.iter().find(|e| e.number == "27").unwrap().seen);
        assert!(!eps_b.iter().find(|e| e.number == "28").unwrap().seen);

        // Un-marking animeyt "2" unseen marks TioAnime 26..28 unseen and leaves 1..25 seen
        db.set_seen_cascade_canonical(row_a, "2", false).unwrap();
        let eps_a = db.list_series_episodes(row_a).unwrap();
        let eps_b = db.list_series_episodes(row_b).unwrap();
        assert_eq!(eps_a.iter().filter(|e| e.seen).count(), 1, "animeyt has 1 seen");
        assert!(eps_a.iter().find(|e| e.number == "1").unwrap().seen);
        assert_eq!(eps_b.iter().filter(|e| e.seen).count(), 25, "TioAnime leaves 1..25 seen");
        assert!(!eps_b.iter().find(|e| e.number == "26").unwrap().seen, "TioAnime 26 unseen");
        assert!(!eps_b.iter().find(|e| e.number == "27").unwrap().seen, "TioAnime 27 unseen");
        assert!(!eps_b.iter().find(|e| e.number == "28").unwrap().seen, "TioAnime 28 unseen");

        // Marking TioAnime "27" seen marks animeyt 1..3 seen
        db.set_seen_cascade_canonical(row_b, "27", true).unwrap();
        let eps_a = db.list_series_episodes(row_a).unwrap();
        let eps_b = db.list_series_episodes(row_b).unwrap();
        assert_eq!(eps_a.iter().filter(|e| e.seen).count(), 3, "animeyt 1..3 seen");
        assert_eq!(eps_b.iter().filter(|e| e.seen).count(), 27, "TioAnime 1..27 seen");
        assert!(!eps_b.iter().find(|e| e.number == "28").unwrap().seen);

        // Marking an earlier-season number on cumulative row (<= offset 24) cascades ONLY that row
        db.set_seen_cascade_canonical(row_b, "20", false).unwrap();
        let eps_a = db.list_series_episodes(row_a).unwrap();
        let eps_b = db.list_series_episodes(row_b).unwrap();
        // Animeyt is completely untouched (still 1..3 seen)
        assert_eq!(eps_a.iter().filter(|e| e.seen).count(), 3, "animeyt untouched by earlier-season cascade");
        // TioAnime has 1..19 seen
        assert_eq!(eps_b.iter().filter(|e| e.seen).count(), 19);
    }

    #[test]
    fn interrupted_downloads_are_reset_but_finished_ones_are_kept() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("T", "https://tioanime.com", "tioanime").unwrap();
        let series = crate::models::Series {
            id: 0, slug: "s".into(), title: "S".into(), url: "https://tioanime.com/anime/s".into(),
            cover_url: None, is_airing: true, followed: true, next_episode_at: None, site_episode_count: None,
        };
        let sid = db.upsert_series(src, &series).unwrap();
        let mk = |n: &str| crate::models::Episode {
            id: 0, series_id: sid, number: n.into(), title: None, url: format!("https://tioanime.com/ver/s-{n}"),
            released_at: None, seen: false, download_path: None, download_status: None,
        };
        let (a, _) = db.insert_episode(&mk("1")).unwrap();
        let (b, _) = db.insert_episode(&mk("2")).unwrap();
        let (c, _) = db.insert_episode(&mk("3")).unwrap();
        db.set_download_status(a, "downloading", None).unwrap();
        db.set_download_status(b, "downloaded", Some("/x/2.mp4")).unwrap();
        db.set_download_status(c, "failed", None).unwrap();
        assert_eq!(db.clear_interrupted_downloads().unwrap(), 1);
        assert_eq!(db.get_episode(a).unwrap().download_status, None);
        assert_eq!(db.get_episode(b).unwrap().download_status.as_deref(), Some("downloaded"));
        assert_eq!(db.get_episode(c).unwrap().download_status.as_deref(), Some("failed"));
    }

    #[test]
    fn sibling_episodes_unlinked_two_sources_reciprocal() {
        let db = Db::open(":memory:").unwrap();
        let src_a = db.upsert_source("AnimeYT", "https://animeyt.cc", "animeytx").unwrap();
        let src_b = db.upsert_source("TioAnime", "https://tioanime.com", "tioanime").unwrap();

        let mk = |slug: &str, title: &str| crate::models::Series {
            id: 0,
            slug: slug.into(),
            title: title.into(),
            url: format!("https://site/{slug}"),
            cover_url: None,
            is_airing: true,
            followed: true,
            next_episode_at: None,
            site_episode_count: None,
        };

        let row_a = db.upsert_series(src_a, &mk("show-a", "Shared Show")).unwrap();
        let row_b = db.upsert_series(src_b, &mk("show-b", "Shared Show")).unwrap();

        let (ep_a1, _) = db.insert_episode(&crate::models::Episode {
            id: 0,
            series_id: row_a,
            number: "1".into(),
            title: None,
            url: "https://a/1".into(),
            released_at: None,
            seen: false,
            download_path: None,
            download_status: None,
        }).unwrap();

        let (ep_b1, _) = db.insert_episode(&crate::models::Episode {
            id: 0,
            series_id: row_b,
            number: "1".into(),
            title: None,
            url: "https://b/1".into(),
            released_at: None,
            seen: false,
            download_path: None,
            download_status: None,
        }).unwrap();

        let sibs_of_a = db.sibling_episodes(ep_a1).unwrap();
        assert_eq!(
            sibs_of_a,
            vec![SiblingEpisode {
                series_id: row_b,
                source_id: src_b,
                site_id: "tioanime".into(),
                episode_id: ep_b1,
                url: "https://b/1".into(),
            }]
        );

        let sibs_of_b = db.sibling_episodes(ep_b1).unwrap();
        assert_eq!(
            sibs_of_b,
            vec![SiblingEpisode {
                series_id: row_a,
                source_id: src_a,
                site_id: "animeytx".into(),
                episode_id: ep_a1,
                url: "https://a/1".into(),
            }]
        );
    }

    #[test]
    fn sibling_episodes_cumulative_offset_tensei_fixture() {
        let db = Db::open(":memory:").unwrap();
        let now = chrono::Utc::now().timestamp();
        let src_a = db.upsert_source("AnimeYT", "https://animeyt.cc", "animeytx").unwrap();
        let src_b = db.upsert_source("TioAnime", "https://tioanime.com", "tioanime").unwrap();

        let entries = vec![
            crate::anizip::MapEntry { episode: 1, season: 3, absolute: Some(25), air_at: Some(now - 14 * 86400) },
            crate::anizip::MapEntry { episode: 2, season: 3, absolute: Some(26), air_at: Some(now - 7 * 86400) },
            crate::anizip::MapEntry { episode: 3, season: 3, absolute: Some(27), air_at: Some(now) },
            crate::anizip::MapEntry { episode: 4, season: 3, absolute: Some(28), air_at: Some(now + 7 * 86400) },
        ];
        db.replace_episode_map(185756, &entries, now).unwrap();

        let mk = |slug: &str, title: &str| crate::models::Series {
            id: 0, slug: slug.into(), title: title.into(), url: format!("https://site/{slug}"),
            cover_url: None, is_airing: true, followed: true, next_episode_at: None, site_episode_count: None,
        };

        let row_a = db.upsert_series(src_a, &mk("tensei-a", "Tensei")).unwrap();
        db.set_anilist_id(row_a, 185756).unwrap();
        let (ep_a3, _) = db.insert_episode(&crate::models::Episode {
            id: 0, series_id: row_a, number: "3".into(), title: None,
            url: "https://animeyt.cc/tv/tensei/3".into(), released_at: None, seen: false,
            download_path: None, download_status: None,
        }).unwrap();

        let row_b = db.upsert_series(src_b, &mk("tensei-b", "Tensei")).unwrap();
        db.set_anilist_id(row_b, 185756).unwrap();
        let mut ep_b20_id = 0;
        let mut ep_b27_id = 0;
        for i in 1..=28 {
            let (id, _) = db.insert_episode(&crate::models::Episode {
                id: 0, series_id: row_b, number: i.to_string(), title: None,
                url: format!("https://tioanime.com/ver/tensei-{i}"), released_at: None, seen: false,
                download_path: None, download_status: None,
            }).unwrap();
            if i == 20 { ep_b20_id = id; }
            if i == 27 { ep_b27_id = id; }
        }

        // Ep 3 on AnimeYT aligns to Ep 27 on TioAnime
        let sibs = db.sibling_episodes(ep_a3).unwrap();
        assert_eq!(
            sibs,
            vec![SiblingEpisode {
                series_id: row_b,
                source_id: src_b,
                site_id: "tioanime".into(),
                episode_id: ep_b27_id,
                url: "https://tioanime.com/ver/tensei-27".into(),
            }]
        );

        // Ep 20 on TioAnime is earlier season (<= offset 24) -> no canonical alignment -> empty vec
        let sibs_earlier = db.sibling_episodes(ep_b20_id).unwrap();
        assert!(sibs_earlier.is_empty());
    }

    #[test]
    fn sibling_episodes_no_matching_episode_returns_empty() {
        let db = Db::open(":memory:").unwrap();
        let src_a = db.upsert_source("AnimeYT", "https://animeyt.cc", "animeytx").unwrap();
        let src_b = db.upsert_source("TioAnime", "https://tioanime.com", "tioanime").unwrap();

        let mk = |slug: &str, title: &str| crate::models::Series {
            id: 0, slug: slug.into(), title: title.into(), url: format!("https://site/{slug}"),
            cover_url: None, is_airing: true, followed: true, next_episode_at: None, site_episode_count: None,
        };

        let row_a = db.upsert_series(src_a, &mk("show-a", "Unfinished Show")).unwrap();
        let row_b = db.upsert_series(src_b, &mk("show-b", "Unfinished Show")).unwrap();

        // Episode 5 on row_a, but row_b only has episode 1
        let (ep_a5, _) = db.insert_episode(&crate::models::Episode {
            id: 0, series_id: row_a, number: "5".into(), title: None,
            url: "https://a/5".into(), released_at: None, seen: false,
            download_path: None, download_status: None,
        }).unwrap();
        let (_ep_b1, _) = db.insert_episode(&crate::models::Episode {
            id: 0, series_id: row_b, number: "1".into(), title: None,
            url: "https://b/1".into(), released_at: None, seen: false,
            download_path: None, download_status: None,
        }).unwrap();

        let sibs = db.sibling_episodes(ep_a5).unwrap();
        assert!(sibs.is_empty());

        // Sibling series with zero episodes also yields empty
        let src_c = db.upsert_source("JKAnime", "https://jkanime.net", "jkanime").unwrap();
        let _row_c = db.upsert_series(src_c, &mk("show-c", "Unfinished Show")).unwrap();
        let sibs_again = db.sibling_episodes(ep_a5).unwrap();
        assert!(sibs_again.is_empty());
    }

    #[test]
    fn sibling_episodes_bridged_different_titles_rejected() {
        let db = Db::open(":memory:").unwrap();
        let src_a = db.upsert_source("AnimeYT", "https://animeyt.cc", "animeytx").unwrap();
        let src_b = db.upsert_source("TioAnime", "https://tioanime.com", "tioanime").unwrap();

        // Seed catalog row with title "Bridged Title"
        db.conn.execute(
            "INSERT INTO anilist_catalog (id, title, url, sort_order) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![500, "Bridged Title", "https://anilist.co/anime/500", 0],
        ).unwrap();

        let mk = |slug: &str, title: &str| crate::models::Series {
            id: 0, slug: slug.into(), title: title.into(), url: format!("https://site/{slug}"),
            cover_url: None, is_airing: true, followed: true, next_episode_at: None, site_episode_count: None,
        };

        // row_a is unlinked (None) but its title matches the catalog row ("Bridged Title")
        let row_a = db.upsert_series(src_a, &mk("show-a", "Bridged Title")).unwrap();
        // row_b is linked to AniList 500 but has a different title
        let row_b = db.upsert_series(src_b, &mk("show-b", "Different Local Title")).unwrap();
        db.set_anilist_id(row_b, 500).unwrap();

        // Verify canonical_series_ids groups them
        let group = db.canonical_series_ids(row_a).unwrap();
        assert!(group.contains(&row_a) && group.contains(&row_b));

        let (ep_a1, _) = db.insert_episode(&crate::models::Episode {
            id: 0, series_id: row_a, number: "1".into(), title: None,
            url: "https://a/1".into(), released_at: None, seen: false,
            download_path: None, download_status: None,
        }).unwrap();
        let (ep_b1, _) = db.insert_episode(&crate::models::Episode {
            id: 0, series_id: row_b, number: "1".into(), title: None,
            url: "https://b/1".into(), released_at: None, seen: false,
            download_path: None, download_status: None,
        }).unwrap();

        // Even though canonical_series_ids grouped row_a and row_b,
        // sibling_episodes rejects row_b because row_a is unlinked and titles differ.
        let sibs_a = db.sibling_episodes(ep_a1).unwrap();
        assert!(sibs_a.is_empty(), "different titles with unlinked row must be rejected");

        let sibs_b = db.sibling_episodes(ep_b1).unwrap();
        assert!(sibs_b.is_empty(), "reciprocal rejection for linked row with different title");
    }

    #[test]
    fn sibling_episodes_same_anilist_id_different_titles_are_siblings() {
        let db = Db::open(":memory:").unwrap();
        let src_a = db.upsert_source("AnimeYT", "https://animeyt.cc", "animeytx").unwrap();
        let src_b = db.upsert_source("TioAnime", "https://tioanime.com", "tioanime").unwrap();

        let mk = |slug: &str, title: &str| crate::models::Series {
            id: 0, slug: slug.into(), title: title.into(), url: format!("https://site/{slug}"),
            cover_url: None, is_airing: true, followed: true, next_episode_at: None, site_episode_count: None,
        };

        let row_a = db.upsert_series(src_a, &mk("show-a", "Attack on Titan")).unwrap();
        db.set_anilist_id(row_a, 999).unwrap();
        let row_b = db.upsert_series(src_b, &mk("show-b", "Shingeki no Kyojin")).unwrap();
        db.set_anilist_id(row_b, 999).unwrap();

        let (ep_a1, _) = db.insert_episode(&crate::models::Episode {
            id: 0, series_id: row_a, number: "1".into(), title: None,
            url: "https://a/1".into(), released_at: None, seen: false,
            download_path: None, download_status: None,
        }).unwrap();
        let (ep_b1, _) = db.insert_episode(&crate::models::Episode {
            id: 0, series_id: row_b, number: "1".into(), title: None,
            url: "https://b/1".into(), released_at: None, seen: false,
            download_path: None, download_status: None,
        }).unwrap();

        let sibs_a = db.sibling_episodes(ep_a1).unwrap();
        assert_eq!(sibs_a.len(), 1);
        assert_eq!(sibs_a[0].series_id, row_b);
        assert_eq!(sibs_a[0].episode_id, ep_b1);

        let sibs_b = db.sibling_episodes(ep_b1).unwrap();
        assert_eq!(sibs_b.len(), 1);
        assert_eq!(sibs_b[0].series_id, row_a);
        assert_eq!(sibs_b[0].episode_id, ep_a1);
    }

    #[test]
    fn sibling_episodes_in_matches_sibling_episodes() {
        let db = Db::open(":memory:").unwrap();
        let src_a = db.upsert_source("AnimeYT", "https://animeyt.cc", "animeytx").unwrap();
        let src_b = db.upsert_source("TioAnime", "https://tioanime.com", "tioanime").unwrap();

        let mk = |slug: &str, title: &str| crate::models::Series {
            id: 0, slug: slug.into(), title: title.into(), url: format!("https://site/{slug}"),
            cover_url: None, is_airing: true, followed: true, next_episode_at: None, site_episode_count: None,
        };

        let row_a = db.upsert_series(src_a, &mk("show-a", "Same Title")).unwrap();
        let row_b = db.upsert_series(src_b, &mk("show-b", "Same Title")).unwrap();

        let (ep_a1, _) = db.insert_episode(&crate::models::Episode {
            id: 0, series_id: row_a, number: "1".into(), title: None,
            url: "https://a/1".into(), released_at: None, seen: false,
            download_path: None, download_status: None,
        }).unwrap();
        let (ep_b1, _) = db.insert_episode(&crate::models::Episode {
            id: 0, series_id: row_b, number: "1".into(), title: None,
            url: "https://b/1".into(), released_at: None, seen: false,
            download_path: None, download_status: None,
        }).unwrap();

        let sids = db.canonical_series_ids(row_a).unwrap();
        let sibs_via_in = db.sibling_episodes_in(ep_a1, &sids).unwrap();
        let sibs_direct = db.sibling_episodes(ep_a1).unwrap();
        assert_eq!(sibs_via_in, sibs_direct);
        assert_eq!(sibs_via_in.len(), 1);
        assert_eq!(sibs_via_in[0].episode_id, ep_b1);
    }
}
