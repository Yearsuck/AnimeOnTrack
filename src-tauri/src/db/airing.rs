use super::*;

/// Ordering for the pending queue, by how many episodes each series still
/// has left to watch. `RemainingAsc` = fewest-left first (quick wins).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PendingSort {
    RemainingAsc,
    RemainingDesc,
}

impl Db {
    /// The **canonical** "En emisión" list — the union of every site's airing
    /// shows, deduped to one entry per canonical identity (`anilist_id`, else
    /// normalized title), so it's identical whichever site is active (the user's
    /// site-agnostic model: identity from AniList, only the *pending episodes*
    /// come from the active site). Each entry shows **AniList metadata** (its
    /// catalog title and cover) when the show is linked to the catalog, falling
    /// back to the scraped values; `followed` is true if followed on any site.
    ///
    /// `active_source_id` is a *preference*: when a show airs on several sites
    /// the member on the active site is the representative (so opening it targets
    /// the site whose episodes you'll watch), else any member.
    ///
    /// Order: newest-release-first via `next_episode_at` (see the airing-sort
    /// design doc), NULLs last, `title` as a stable tie-break.
    pub fn list_airing(&self, active_source_id: i64) -> Result<Vec<crate::models::Series>> {
        let mut stmt = self.conn.prepare(
            "SELECT s.id, s.source_id, s.anilist_id, s.slug, s.url,
                    COALESCE(c.cover_url, s.cover_url) AS cover_url,
                    COALESCE(c.title, s.title) AS title,
                    s.followed, s.next_episode_at, s.site_episode_count
             FROM series s LEFT JOIN anilist_catalog c ON c.id = s.anilist_id
             WHERE s.is_airing = 1",
        )?;
        struct AiringRow {
            source_id: i64,
            anilist_id: Option<i64>,
            followed: bool,
            series: crate::models::Series,
        }
        let rows: Vec<AiringRow> = stmt
            .query_map([], |r| {
                let followed = r.get::<_, i64>("followed")? != 0;
                Ok(AiringRow {
                    source_id: r.get("source_id")?,
                    anilist_id: r.get("anilist_id")?,
                    followed,
                    series: crate::models::Series {
                        id: r.get("id")?,
                        slug: r.get("slug")?,
                        title: r.get("title")?,
                        url: r.get("url")?,
                        cover_url: r.get("cover_url")?,
                        is_airing: true,
                        followed,
                        next_episode_at: r.get("next_episode_at")?,
                        site_episode_count: r.get("site_episode_count")?,
                    },
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        // Group by canonical identity; a group is followed if *any* member is.
        let mut order: Vec<String> = Vec::new();
        let mut groups: std::collections::HashMap<String, Vec<AiringRow>> =
            std::collections::HashMap::new();
        for row in rows {
            // Linked rows dedup by AniList id. Unlinked rows dedup by franchise
            // key (season markers + spacing stripped) so the same show under two
            // sites' title variants ("…2nd Season" vs "…Temporada 2") collapses
            // to one entry even when neither is matched to the catalog.
            let key = match row.anilist_id {
                Some(id) => format!("al:{id}"),
                None => format!("t:{}", crate::matching::franchise_dedup_key(&row.series.title)),
            };
            if !groups.contains_key(&key) {
                order.push(key.clone());
            }
            groups.entry(key).or_default().push(row);
        }

        let mut out: Vec<crate::models::Series> = Vec::with_capacity(order.len());
        for key in &order {
            let group = groups.remove(key).unwrap();
            let followed_any = group.iter().any(|m| m.followed);
            // Freshest `next_episode_at` across every member, not just the
            // representative: a group can pair an up-to-date site with one
            // that's stale or currently unreachable (mirror down, not
            // rescanned recently), and the sort order must reflect whichever
            // site actually saw a new episode most recently, not whichever
            // happens to be active — otherwise a stale active-site member
            // silently drags the whole show to the bottom of the list.
            let freshest_next_episode_at = group.iter().filter_map(|m| m.series.next_episode_at).max();
            // Representative: prefer the active-site member (its episodes are the
            // ones you'll actually open), then a followed member, then stable id.
            let rep = group
                .into_iter()
                .min_by(|a, b| {
                    let rank = |m: &AiringRow| {
                        (
                            (m.source_id != active_source_id) as u8,
                            (!m.followed) as u8,
                            m.series.id,
                        )
                    };
                    rank(a).cmp(&rank(b))
                })
                .unwrap();
            let mut series = rep.series;
            series.followed = followed_any;
            series.next_episode_at = freshest_next_episode_at;
            out.push(series);
        }

        out.sort_by(|a, b| {
            let a_null = a.next_episode_at.is_none();
            let b_null = b.next_episode_at.is_none();
            a_null
                .cmp(&b_null)
                .then(b.next_episode_at.cmp(&a.next_episode_at))
                .then_with(|| a.title.to_lowercase().cmp(&b.title.to_lowercase()))
        });
        Ok(out)
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
             WHERE e.seen=0 AND s.followed=1 AND s.watched_externally=0 AND s.source_id=?1
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

    #[test]
    fn list_airing_is_a_canonical_union_deduped_across_sites() {
        let db = Db::open(":memory:").unwrap();
        let a = db.upsert_source("AnimeYT", "https://a", "animeytx").unwrap();
        let b = db.upsert_source("TioAnime", "https://b", "tioanime").unwrap();
        let mk = |slug: &str, title: &str| crate::models::Series {
            id: 0, slug: slug.into(), title: title.into(), url: format!("u-{slug}"),
            cover_url: None, is_airing: true, followed: false, next_episode_at: None, site_episode_count: None,
        };
        // Catalog provides the canonical AniList display title for both sites.
        db.upsert_catalog_anime(&crate::db::test_support::catalog_anime(21, "One Piece", &["Action"]), 0).unwrap();
        db.upsert_catalog_anime(&crate::db::test_support::catalog_anime(99, "AnimeYT Only", &["Action"]), 0).unwrap();
        // Same show (same anilist id) airing on both sites + one site-only show.
        let one_a = db.upsert_series(a, &mk("op-a", "ONE PIECE")).unwrap();
        db.set_anilist_id(one_a, 21).unwrap();
        let one_b = db.upsert_series(b, &mk("op-b", "one piece latino")).unwrap();
        db.set_anilist_id(one_b, 21).unwrap();
        let solo = db.upsert_series(a, &mk("solo", "AnimeYT Only")).unwrap();
        db.set_anilist_id(solo, 99).unwrap();

        // Identical set regardless of the active site: One Piece (once) + solo.
        let from_a = db.list_airing(a).unwrap();
        let from_b = db.list_airing(b).unwrap();
        assert_eq!(from_a.len(), 2, "deduped: One Piece appears once, not twice");
        let titles_a: Vec<&str> = from_a.iter().map(|s| s.title.as_str()).collect();
        let titles_b: Vec<&str> = from_b.iter().map(|s| s.title.as_str()).collect();
        assert_eq!(titles_a, titles_b, "same airing set on either active site");
        // Representative prefers the active site (so its episodes open there).
        let op_from_b = from_b.iter().find(|s| s.title.eq_ignore_ascii_case("one piece")).unwrap();
        assert_eq!(op_from_b.id, one_b, "active-site member is the representative");
    }

    #[test]
    fn list_airing_uses_the_freshest_next_episode_at_across_merged_members() {
        let db = Db::open(":memory:").unwrap();
        let a = db.upsert_source("AnimeYT", "https://a", "animeytx").unwrap();
        let b = db.upsert_source("TioAnime", "https://b", "tioanime").unwrap();
        let mk = |slug: &str, title: &str, next: Option<i64>| crate::models::Series {
            id: 0, slug: slug.into(), title: title.into(), url: format!("u-{slug}"),
            cover_url: None, is_airing: true, followed: false, next_episode_at: next, site_episode_count: None,
        };
        // Active site's member (a) is stale/missing next_episode_at (e.g. its
        // mirror has been down and hasn't rescanned successfully); site b's
        // member has fresh data. The merged entry must surface b's timestamp
        // instead of silently falling back to a's None just because a is the
        // representative (active-site) member — otherwise a stale site drags
        // an otherwise-current show to the bottom of "En emisión".
        let one_a = db.upsert_series(a, &mk("op-a", "One Piece", None)).unwrap();
        db.set_anilist_id(one_a, 21).unwrap();
        let one_b = db.upsert_series(b, &mk("op-b", "One Piece", Some(1_800_000_000))).unwrap();
        db.set_anilist_id(one_b, 21).unwrap();

        let airing = db.list_airing(a).unwrap();
        assert_eq!(airing.len(), 1);
        let entry = &airing[0];
        assert_eq!(entry.id, one_a, "active-site member is still the representative for id/url");
        assert_eq!(
            entry.next_episode_at,
            Some(1_800_000_000),
            "but the sort-relevant timestamp is the freshest across the whole group"
        );
    }

    #[test]
    fn list_airing_dedups_unlinked_season_variants_across_sites() {
        let db = Db::open(":memory:").unwrap();
        let a = db.upsert_source("AnimeYT", "https://a", "animeytx").unwrap();
        let b = db.upsert_source("TioAnime", "https://b", "tioanime").unwrap();
        let mk = |slug: &str, title: &str| crate::models::Series {
            id: 0, slug: slug.into(), title: title.into(), url: format!("u-{slug}"),
            cover_url: None, is_airing: true, followed: false, next_episode_at: None, site_episode_count: None,
        };
        // Same show, NO anilist id on either, titles differ only by the season
        // marker's language — must still collapse to one airing entry.
        db.upsert_series(a, &mk("hm-a", "Hell Mode: Yarikomizuki no Gamer Temporada 2")).unwrap();
        db.upsert_series(b, &mk("hm-b", "Hell Mode: Yarikomizuki no Gamer 2nd Season")).unwrap();
        let airing = db.list_airing(a).unwrap();
        assert_eq!(airing.len(), 1, "unlinked season variants dedup to one entry");
    }

    #[test]
    fn list_airing_marks_followed_when_any_site_follows_it() {
        let db = Db::open(":memory:").unwrap();
        let a = db.upsert_source("AnimeYT", "https://a", "animeytx").unwrap();
        let b = db.upsert_source("TioAnime", "https://b", "tioanime").unwrap();
        let mk = |slug: &str, title: &str| crate::models::Series {
            id: 0, slug: slug.into(), title: title.into(), url: format!("u-{slug}"),
            cover_url: None, is_airing: true, followed: false, next_episode_at: None, site_episode_count: None,
        };
        let one_a = db.upsert_series(a, &mk("op-a", "ONE PIECE")).unwrap();
        db.set_anilist_id(one_a, 21).unwrap();
        let one_b = db.upsert_series(b, &mk("op-b", "One Piece")).unwrap();
        db.set_anilist_id(one_b, 21).unwrap();
        // Followed only on AnimeYT.
        db.set_followed(one_a, true).unwrap();

        // From TioAnime's perspective the canonical entry is still "followed".
        let op = db.list_airing(b).unwrap().into_iter().find(|s| s.title.eq_ignore_ascii_case("one piece")).unwrap();
        assert!(op.followed, "followed on any site => followed in the canonical list");
    }

    #[test]
    fn set_followed_canonical_follows_the_show_on_every_site() {
        let db = Db::open(":memory:").unwrap();
        let a = db.upsert_source("AnimeYT", "https://a", "animeytx").unwrap();
        let b = db.upsert_source("TioAnime", "https://b", "tioanime").unwrap();
        let mk = |slug: &str, title: &str| crate::models::Series {
            id: 0, slug: slug.into(), title: title.into(), url: format!("u-{slug}"),
            cover_url: None, is_airing: true, followed: false, next_episode_at: None, site_episode_count: None,
        };
        let one_a = db.upsert_series(a, &mk("op-a", "ONE PIECE")).unwrap();
        db.set_anilist_id(one_a, 21).unwrap();
        let one_b = db.upsert_series(b, &mk("op-b", "One Piece")).unwrap();
        db.set_anilist_id(one_b, 21).unwrap();

        // Following on B follows the AnimeYT row too (same anilist id).
        let changed = db.set_followed_canonical(one_b, true).unwrap();
        assert_eq!(changed, 2);
        assert!(db.list_followed(a).unwrap().iter().any(|s| s.id == one_a));
        assert!(db.list_followed(b).unwrap().iter().any(|s| s.id == one_b));

        // No-anilist rows fall back to normalized-title matching.
        let x1 = db.upsert_series(a, &mk("x1", "Some  Show")).unwrap();
        let x2 = db.upsert_series(b, &mk("x2", "some show")).unwrap();
        assert_eq!(db.set_followed_canonical(x1, true).unwrap(), 2);
        assert!(db.list_followed(b).unwrap().iter().any(|s| s.id == x2));
    }
}
