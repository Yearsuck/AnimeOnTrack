use super::*;

/// Ordering for the pending queue, by how many episodes each series still
/// has left to watch. `RemainingAsc` = fewest-left first (quick wins).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PendingSort {
    RemainingAsc,
    RemainingDesc,
}

impl Db {
    /// "En emisión" default order: most-recently-released first.
    /// `next_episode_at` is the *next* episode's release time; for a weekly
    /// series that's roughly "last release + 7 days", so the series whose
    /// episode dropped most recently has the furthest-away next episode —
    /// descending order therefore reads as newest-release-first (see the
    /// airing-sort-order design doc; non-weekly series break the inference
    /// and that's accepted). NULLs (no countdown on the card / never seen on
    /// the airing listing) sort last; `title` is a stable tie-break so
    /// equal/NULL timestamps never reorder run to run.
    pub fn list_airing(&self, source_id: i64) -> Result<Vec<crate::models::Series>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, slug, title, url, cover_url, is_airing, followed, next_episode_at, site_episode_count
             FROM series WHERE source_id=?1 AND is_airing=1
             ORDER BY next_episode_at IS NULL, next_episode_at DESC, title",
        )?;
        let rows = stmt
            .query_map([source_id], Self::row_to_series)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Unseen episodes of currently-followed series only, scoped to
    /// `source_id` — unfollowing a series must drop its episodes out of the
    /// pending count immediately, and (multi-site) a different site's
    /// followed series must never leak into this site's pending count.
    pub fn pending_count(&self, source_id: i64) -> Result<i64> {
        let n: i64 = self.conn.query_row(
            "SELECT count(*) FROM episodes e JOIN series s ON s.id = e.series_id
             WHERE e.seen=0 AND s.followed=1 AND s.source_id=?1",
            [source_id],
            |r| r.get(0),
        )?;
        Ok(n)
    }

    /// Unseen episodes of currently-followed series, joined with their
    /// series, newest first, scoped to `source_id` (see `pending_count`).
    /// Unseen episodes of currently-followed series, ordered so each series'
    /// episodes stay contiguous (grouped in the UI) and the *groups* come out
    /// sorted by how many pending episodes each series has — see `PendingSort`.
    /// `COUNT(*) OVER (PARTITION BY s.id)` is the per-series remaining count;
    /// `s.title` then `e.added_at DESC` keep ordering stable within a group and
    /// across equal-count series.
    pub fn list_pending(
        &self,
        source_id: i64,
        sort: PendingSort,
    ) -> Result<Vec<(crate::models::Series, crate::models::Episode)>> {
        let dir = match sort {
            PendingSort::RemainingAsc => "ASC",
            PendingSort::RemainingDesc => "DESC",
        };
        let sql = format!(
            "SELECT s.id, s.slug, s.title, s.url, s.cover_url, s.is_airing, s.followed,
                    e.id, e.series_id, e.number, e.title, e.url, e.released_at, e.seen,
                    s.next_episode_at, s.site_episode_count,
                    COUNT(*) OVER (PARTITION BY s.id) AS remaining
             FROM episodes e JOIN series s ON s.id = e.series_id
             WHERE e.seen=0 AND s.followed=1 AND s.source_id=?1
             ORDER BY remaining {dir}, s.title, e.added_at DESC"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt
            .query_map([source_id], |r| {
                let series = crate::models::Series {
                    id: r.get(0)?,
                    slug: r.get(1)?,
                    title: r.get(2)?,
                    url: r.get(3)?,
                    cover_url: r.get(4)?,
                    is_airing: r.get::<_, i64>(5)? != 0,
                    followed: r.get::<_, i64>(6)? != 0,
                    next_episode_at: r.get("next_episode_at")?,
                    site_episode_count: r.get("site_episode_count")?,
                };
                let ep = crate::models::Episode {
                    id: r.get(7)?,
                    series_id: r.get(8)?,
                    number: r.get(9)?,
                    title: r.get(10)?,
                    url: r.get(11)?,
                    released_at: r.get(12)?,
                    seen: r.get::<_, i64>(13)? != 0,
                };
                Ok((series, ep))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_support::*;

    #[test]
    fn list_airing_orders_newest_first_nulls_last_title_tiebreak() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
        db.upsert_series(src, &mk_airing("older", "Older", Some(1_000_000))).unwrap();
        db.upsert_series(src, &mk_airing("newer", "Newer", Some(2_000_000))).unwrap();
        db.upsert_series(src, &mk_airing("nodate", "NoDate", None)).unwrap();
        db.upsert_series(src, &mk_airing("tie-b", "Tie B", Some(1_000_000))).unwrap();

        let titles: Vec<String> = db.list_airing(src).unwrap().into_iter().map(|s| s.title).collect();
        assert_eq!(titles, vec!["Newer", "Older", "Tie B", "NoDate"]);
    }

    #[test]
    fn pending_count_and_list_pending_are_scoped_per_source() {
        let db = Db::open(":memory:").unwrap();
        let src_a = db.upsert_source("AnimeYT", "https://a.example", "animeytx").unwrap();
        let src_b = db.upsert_source("TioAnime", "https://b.example", "tioanime").unwrap();

        let mk = |slug: &str| crate::models::Series {
            id: 0, slug: slug.into(), title: slug.into(), url: format!("u-{slug}"),
            cover_url: None, is_airing: true, followed: false, next_episode_at: None, site_episode_count: None,
        };
        let sid_a = db.upsert_series(src_a, &mk("a")).unwrap();
        db.set_followed(sid_a, true).unwrap();
        let sid_b = db.upsert_series(src_b, &mk("b")).unwrap();
        db.set_followed(sid_b, true).unwrap();

        db.insert_episode(&crate::models::Episode {
            id: 0, series_id: sid_a, number: "1".into(), title: None,
            url: "https://a.example/ep1".into(), released_at: None, seen: false,
        })
        .unwrap();
        db.insert_episode(&crate::models::Episode {
            id: 0, series_id: sid_b, number: "1".into(), title: None,
            url: "https://b.example/ep1".into(), released_at: None, seen: false,
        })
        .unwrap();
        db.insert_episode(&crate::models::Episode {
            id: 0, series_id: sid_b, number: "2".into(), title: None,
            url: "https://b.example/ep2".into(), released_at: None, seen: false,
        })
        .unwrap();

        assert_eq!(db.pending_count(src_a).unwrap(), 1);
        assert_eq!(db.pending_count(src_b).unwrap(), 2);
        assert_eq!(db.list_pending(src_a, PendingSort::RemainingAsc).unwrap().len(), 1);
        assert_eq!(db.list_pending(src_b, PendingSort::RemainingAsc).unwrap().len(), 2);
        assert!(db
            .list_pending(src_a, PendingSort::RemainingAsc)
            .unwrap()
            .iter()
            .all(|(s, _)| s.id == sid_a));
    }

    #[test]
    fn list_pending_orders_groups_by_remaining_count() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "https://a.example", "animeytx").unwrap();
        let mk = |slug: &str| crate::models::Series {
            id: 0, slug: slug.into(), title: slug.into(), url: format!("u-{slug}"),
            cover_url: None, is_airing: true, followed: false, next_episode_at: None, site_episode_count: None,
        };
        // "few" has 1 pending episode, "many" has 3.
        let few = db.upsert_series(src, &mk("few")).unwrap();
        db.set_followed(few, true).unwrap();
        let many = db.upsert_series(src, &mk("many")).unwrap();
        db.set_followed(many, true).unwrap();
        db.insert_episode(&crate::models::Episode {
            id: 0, series_id: few, number: "1".into(), title: None,
            url: "few/1".into(), released_at: None, seen: false,
        }).unwrap();
        for n in 1..=3 {
            db.insert_episode(&crate::models::Episode {
                id: 0, series_id: many, number: n.to_string(), title: None,
                url: format!("many/{n}"), released_at: None, seen: false,
            }).unwrap();
        }

        // Ascending: the 1-episode series' rows come before the 3-episode one.
        let asc = db.list_pending(src, PendingSort::RemainingAsc).unwrap();
        assert_eq!(asc.first().unwrap().0.id, few);
        assert_eq!(asc.last().unwrap().0.id, many);
        // Descending: reversed.
        let desc = db.list_pending(src, PendingSort::RemainingDesc).unwrap();
        assert_eq!(desc.first().unwrap().0.id, many);
        assert_eq!(desc.last().unwrap().0.id, few);
        // Each series' episodes stay contiguous (no interleaving).
        let ids: Vec<i64> = asc.iter().map(|(s, _)| s.id).collect();
        assert_eq!(ids, vec![few, many, many, many]);
    }
}
