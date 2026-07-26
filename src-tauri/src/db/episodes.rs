use super::*;

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
    pub fn max_episode_number(&self, series_id: i64) -> Result<i64> {
        Ok(self.conn.query_row(
            "SELECT COALESCE(MAX(CAST(number AS INTEGER)), 0) FROM episodes WHERE series_id=?1",
            [series_id],
            |r| r.get(0),
        )?)
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

    /// Insert episode if its (series_id, url) is new. Returns the row id either way.
    pub fn insert_episode(&self, e: &crate::models::Episode) -> Result<i64> {
        self.conn.execute(
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
        Ok(id)
    }

    /// Set an episode's seen flag either way (lets the user un-mark).
    /// `seen_at` tracks alongside: stamped `datetime('now')` when marking
    /// seen, cleared back to NULL when un-marking.
    pub fn set_seen(&self, episode_id: i64, seen: bool) -> Result<()> {
        self.conn.execute(
            "UPDATE episodes SET seen=?1, seen_at = CASE WHEN ?1=1 THEN datetime('now') ELSE NULL END
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
    pub fn set_seen_cascade(&self, series_id: i64, number: &str, seen: bool) -> Result<()> {
        let Some(target) = parse_ep_number(number) else {
            // No leading digits at all: ordering is meaningless, so just
            // toggle the exact-matching episode(s) rather than cascade.
            self.conn.execute(
                "UPDATE episodes SET seen=?1, seen_at = CASE WHEN ?1=1 THEN datetime('now') ELSE NULL END
                 WHERE series_id=?2 AND number=?3",
                (seen as i64, series_id, number),
            )?;
            return Ok(());
        };
        let mut stmt = self
            .conn
            .prepare("SELECT id, number FROM episodes WHERE series_id=?1")?;
        let rows: Vec<(i64, String)> = stmt
            .query_map([series_id], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
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
                self.conn.execute(
                    "UPDATE episodes SET seen=?1, seen_at = CASE WHEN ?1=1 THEN datetime('now') ELSE NULL END
                     WHERE id=?2",
                    (seen as i64, id),
                )?;
            }
        }
        Ok(())
    }

    /// All episodes of a series (seen or not), oldest number first — the
    /// progress view for "which episode am I on".
    pub fn list_series_episodes(&self, series_id: i64) -> Result<Vec<crate::models::Episode>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, series_id, number, title, url, released_at, seen
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::test_support::*;

    #[test]
    fn episode_count_counts_only_this_series() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
        let sid_a = db.upsert_series(src, &mk_airing("a", "A", None)).unwrap();
        let sid_b = db.upsert_series(src, &mk_airing("b", "B", None)).unwrap();
        for i in 1..=3 {
            db.insert_episode(&crate::models::Episode {
                id: 0, series_id: sid_a, number: i.to_string(), title: None,
                url: format!("https://site/a-{i}"), released_at: None, seen: false,
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
            url: "https://site/a-recap".to_string(), released_at: None, seen: false,
        }).unwrap();
        for i in 1..=12 {
            db.insert_episode(&crate::models::Episode {
                id: 0, series_id: sid_a, number: i.to_string(), title: None,
                url: format!("https://site/a-{i}"), released_at: None, seen: false,
            }).unwrap();
        }
        assert_eq!(db.episode_count(sid_a).unwrap(), 13, "sanity: row count is 13");
        assert_eq!(db.max_episode_number(sid_a).unwrap(), 12);

        // Tomb Raider King-shaped: "1 | v2", "2 | v1" -> highest real = 2.
        let sid_b = db.upsert_series(src, &mk_airing("b", "B", None)).unwrap();
        db.insert_episode(&crate::models::Episode {
            id: 0, series_id: sid_b, number: "1 | v2".to_string(), title: None,
            url: "https://site/b-1".to_string(), released_at: None, seen: false,
        }).unwrap();
        db.insert_episode(&crate::models::Episode {
            id: 0, series_id: sid_b, number: "2 | v1".to_string(), title: None,
            url: "https://site/b-2".to_string(), released_at: None, seen: false,
        }).unwrap();
        assert_eq!(db.max_episode_number(sid_b).unwrap(), 2);

        // No episodes at all -> 0.
        let sid_c = db.upsert_series(src, &mk_airing("c", "C", None)).unwrap();
        assert_eq!(db.max_episode_number(sid_c).unwrap(), 0);
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
            url: "https://site/a-3".into(), released_at: Some("agosto 1, 2026".into()), seen: false,
        }).unwrap();
        db.insert_episode(&crate::models::Episode {
            id: 0, series_id: sid, number: "1".into(), title: None,
            url: "https://site/a-1".into(), released_at: Some("junio 29, 2026".into()), seen: false,
        }).unwrap();
        db.insert_episode(&crate::models::Episode {
            id: 0, series_id: sid, number: "2".into(), title: None,
            url: "https://site/a-2".into(), released_at: Some("julio 15, 2026".into()), seen: false,
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
            url: "https://site/a-1".into(), released_at: None, seen: false,
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
            url: "https://site/na-1".into(), released_at: Some("mayo 1, 2026".into()), seen: false,
        }).unwrap();

        let sid_other = db.upsert_series(other_src, &mk_airing("o", "Other", None)).unwrap();
        db.insert_episode(&crate::models::Episode {
            id: 0, series_id: sid_other, number: "1".into(), title: None,
            url: "https://other/o-1".into(), released_at: Some("mayo 1, 2026".into()), seen: false,
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
            url: "https://site/unfollowed-1".into(), released_at: Some("julio 15, 2026".into()), seen: false,
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
            url: url.into(), released_at: None, seen: false,
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
            url: url.into(), released_at: None, seen: false,
        };
        let e1 = db.insert_episode(&mk("1", "https://site/e1")).unwrap();
        let e2 = db.insert_episode(&mk("2", "https://site/e2")).unwrap();

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
    fn set_seen_stamps_and_clears_seen_at() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "https://wwv.animeytx.net", "animeytx").unwrap();
        let s = crate::models::Series {
            id: 0, slug: "x".into(), title: "X".into(),
            url: "u".into(), cover_url: None, is_airing: true, followed: false, next_episode_at: None, site_episode_count: None,
        };
        let sid = db.upsert_series(src, &s).unwrap();
        let e1 = db
            .insert_episode(&crate::models::Episode {
                id: 0, series_id: sid, number: "1".into(), title: None,
                url: "https://site/e1".into(), released_at: None, seen: false,
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
            url: url.into(), released_at: None, seen: false,
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
            url: "https://site/e1".into(), released_at: None, seen: false,
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
            url: "https://site/ep1".into(), released_at: None, seen: false,
        };
        let eid = db.insert_episode(&ep).unwrap();
        // same url again => no new row
        let eid_dup = db.insert_episode(&ep).unwrap();
        assert_eq!(eid, eid_dup);

        assert_eq!(db.pending_count(src).unwrap(), 1);
        db.set_seen(eid, true).unwrap();
        assert_eq!(db.pending_count(src).unwrap(), 0);
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
            url: "https://wwv.animeytx.net/anime/x-capitulo-1/".into(), released_at: None, seen: true,
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
                url: format!("e{n}"), released_at: None, seen: false,
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
}
