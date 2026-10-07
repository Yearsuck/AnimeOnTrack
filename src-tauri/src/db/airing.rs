use super::*;

/// A followed canonical show that lacks episodes on all tracked sites.
#[derive(Debug, Clone, PartialEq)]
pub struct EpisodeSourceCandidate {
    pub canon_key: String,
    pub anilist_id: Option<i64>,
    pub title: String,
}

/// Ordering for the pending queue, by how many episodes each series still
/// has left to watch. `RemainingAsc` = fewest-left first (quick wins).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PendingSort {
    RemainingAsc,
    RemainingDesc,
}

/// Whether `cover_url` is something the frontend's CSP (`img-src 'self'
/// data: asset: https://asset.localhost https://*.anilist.co`) will actually
/// render, rather than silently block — a `data:` URI (fetched by
/// `refresh()`), a local cached cover (`file:` path served via the `asset:`
/// protocol), or an AniList CDN URL (already resolved through
/// `anilist_catalog.cover_url` by `list_airing`'s own COALESCE). Anything
/// else is a scraped site's raw remote thumbnail.
pub(crate) fn is_csp_displayable_cover(cover_url: Option<&str>) -> bool {
    let Some(u) = cover_url else { return false };
    if u.starts_with("data:") || u.starts_with("file:") || u.starts_with("asset:") {
        return true;
    }
    url::Url::parse(u)
        .ok()
        .and_then(|parsed| parsed.host_str().map(|h| h.to_ascii_lowercase()))
        .is_some_and(|host| host == "anilist.co" || host.ends_with(".anilist.co"))
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
    ///
    /// `next_episode_at` is AniList's `next_airing_at` when the series is linked
    /// to a catalog entry of the SAME season (`season_consistent`, the check
    /// `sync_status_from_catalog` applies) and that time is still in the future;
    /// otherwise the scraped value. A catalog time that has already passed is a
    /// snapshot taken before the episode aired (the site has usually rolled to
    /// next week by then), so it never beats the freshly scraped value.
    pub fn list_airing(&self, active_source_id: i64) -> Result<Vec<crate::models::Series>> {
        let mut stmt = self.conn.prepare(
            "SELECT s.id, s.source_id, s.anilist_id, s.slug, s.url,
                    COALESCE(c.cover_url, s.cover_url) AS cover_url,
                    COALESCE(c.title, s.title) AS title,
                    s.followed, s.next_episode_at AS scraped_next,
                    c.next_airing_at AS catalog_next,
                    s.title AS site_title, c.title AS c_title,
                    c.title_romaji AS c_romaji, c.title_english AS c_english,
                    s.site_episode_count,
                    (SELECT COUNT(*) FROM episodes e WHERE e.series_id = s.id) AS total_ep
             FROM series s LEFT JOIN anilist_catalog c ON c.id = s.anilist_id
             WHERE s.is_airing = 1",
        )?;
        struct AiringRow {
            source_id: i64,
            anilist_id: Option<i64>,
            followed: bool,
            total_ep: i64,
            series: crate::models::Series,
        }
        let now = chrono::Utc::now().timestamp();
        let rows: Vec<AiringRow> = stmt
            .query_map([], |r| {
                let followed = r.get::<_, i64>("followed")? != 0;
                let anilist_id: Option<i64> = r.get("anilist_id")?;
                let scraped_next: Option<i64> = r.get("scraped_next")?;
                let catalog_next: Option<i64> = r.get("catalog_next")?;

                let mut effective_next = scraped_next;
                if let (Some(_), Some(cat)) = (anilist_id, catalog_next) {
                    if cat > now {
                        let site_title: String = r.get("site_title")?;
                        let titles: Vec<String> = ["c_title", "c_romaji", "c_english"]
                            .into_iter()
                            .filter_map(|col| r.get::<_, Option<String>>(col).transpose())
                            .collect::<rusqlite::Result<_>>()?;
                        let refs: Vec<&str> = titles.iter().map(String::as_str).collect();
                        if crate::matching::season_consistent(&site_title, &refs) {
                            effective_next = Some(cat);
                        }
                    }
                }

                Ok(AiringRow {
                    source_id: r.get("source_id")?,
                    anilist_id,
                    followed,
                    total_ep: r.get("total_ep")?,
                    series: crate::models::Series {
                        id: r.get("id")?,
                        slug: r.get("slug")?,
                        title: r.get("title")?,
                        url: r.get("url")?,
                        cover_url: r.get("cover_url")?,
                        is_airing: true,
                        followed,
                        next_episode_at: effective_next,
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
            // A live-action adaptation has no AniList entry to say whether it is
            // still airing, and this is an anime tracker: unless the user follows
            // it, it does not belong in the airing list.
            if row.anilist_id.is_none() && !row.followed && crate::matching::is_live_action(&row.series.title) {
                continue;
            }
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
            // A show airing on several sites is only cover-fetched on
            // whichever one is currently active (see commands/scan.rs's
            // airing_series_needing_cover_fetch — WebView2 scraping is
            // inherently one-active-site-at-a-time), so a representative
            // picked from a site that's never been active would show a
            // blank card forever even once a sibling site's copy has a real
            // photo. Borrow one from any group member before the
            // active-site pick below discards the rest of the group — the
            // representative still decides url/id/episodes (its site is the
            // one you'll actually open), only the cover can come from
            // elsewhere.
            let borrowed_cover = group
                .iter()
                .find(|m| is_csp_displayable_cover(m.series.cover_url.as_deref()))
                .and_then(|m| m.series.cover_url.clone());
            // Representative: prefer a member with total_ep > 0 (even on non-active sites)
            // if the active site has 0 episodes. Then prefer the active site, then a followed member, then stable id.
            let rep = group
                .into_iter()
                .min_by(|a, b| {
                    let rank = |m: &AiringRow| {
                        let is_active_with_episodes = m.source_id == active_source_id && m.total_ep > 0;
                        let has_episodes = m.total_ep > 0;
                        (
                            (!is_active_with_episodes) as u8,
                            (!has_episodes) as u8,
                            (m.source_id != active_source_id) as u8,
                            (!m.followed) as u8,
                            m.series.id,
                        )
                    };
                    rank(a).cmp(&rank(b))
                })
                .unwrap();
            let mut series = rep.series;
            if !is_csp_displayable_cover(series.cover_url.as_deref()) {
                if let Some(cover) = borrowed_cover {
                    series.cover_url = Some(cover);
                }
            }
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

    fn get_fallback_series_ids(&self, active_source_id: i64) -> Result<Vec<i64>> {
        let mut stmt = self.conn.prepare(
            "SELECT s.id, s.source_id, s.anilist_id, s.title, c.title AS catalog_title,
                    s.watched_externally, s.followed
             FROM series s
             LEFT JOIN anilist_catalog c ON c.id = s.anilist_id"
        )?;
        struct Row { id: i64, source_id: i64, anilist_id: Option<i64>, title: String, catalog_title: Option<String>, watched_externally: bool, followed: bool }
        let rows = stmt.query_map([], |r| Ok(Row {
            id: r.get(0)?, source_id: r.get(1)?, anilist_id: r.get(2)?, title: r.get(3)?, catalog_title: r.get(4)?,
            watched_externally: r.get::<_, i64>(5)? != 0,
            followed: r.get::<_, i64>(6)? != 0,
        }))?.collect::<rusqlite::Result<Vec<_>>>()?;

        let mut groups: std::collections::HashMap<String, Vec<Row>> = std::collections::HashMap::new();

        // normalized catalog title -> AniList id (lowest id wins, so grouping is deterministic)
        let mut tkey_to_anilist: std::collections::HashMap<String, i64> = std::collections::HashMap::new();
        for r in &rows {
            if let (Some(al_id), Some(cat_title)) = (r.anilist_id, &r.catalog_title) {
                let tkey = format!("t:{}", crate::matching::normalize_title(cat_title));
                tkey_to_anilist.entry(tkey).and_modify(|id| *id = (*id).min(al_id)).or_insert(al_id);
            }
        }

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
            groups.entry(key).or_default().push(r);
        }

        let mut candidate_ids = Vec::new();
        for group in groups.values() {
            if group.iter().any(|r| r.followed) {
                for r in group {
                    candidate_ids.push(r.id);
                }
            }
        }

        if candidate_ids.is_empty() {
            return Ok(vec![]);
        }

        let ids_str = candidate_ids.iter().map(|id| id.to_string()).collect::<Vec<_>>().join(",");
        let mut ep_stmt = self.conn.prepare(&format!(
            "SELECT series_id, COUNT(id), SUM(CASE WHEN seen = 0 THEN 1 ELSE 0 END)
             FROM episodes WHERE series_id IN ({}) GROUP BY series_id", ids_str
        ))?;
        let mut ep_counts = std::collections::HashMap::new();
        let ep_rows = ep_stmt.query_map([], |r| Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, i64>(1)?,
            r.get::<_, Option<i64>>(2)?.unwrap_or(0)
        )))?;
        for (sid, total, unseen) in ep_rows.flatten() {
            ep_counts.insert(sid, (total, unseen));
        }

        let mut winners = Vec::new();
        for (_, group) in groups {
            if !group.iter().any(|r| r.followed) {
                continue;
            }

            let active_member = group.iter().find(|r| r.source_id == active_source_id);
            if let Some(active) = active_member {
                let (active_total, active_unseen) = ep_counts.get(&active.id).copied().unwrap_or((0, 0));
                if active_total > 0 {
                    if active_unseen > 0 && !active.watched_externally {
                        winners.push(active.id);
                    }
                    continue;
                }
            }

            let winner = group.into_iter().filter(|r| r.source_id != active_source_id && !r.watched_externally).max_by(|a, b| {
                let (a_total, a_unseen) = ep_counts.get(&a.id).copied().unwrap_or((0, 0));
                let (b_total, b_unseen) = ep_counts.get(&b.id).copied().unwrap_or((0, 0));
                let rank = |total: i64, unseen: i64, id: i64| (total, total - unseen, -id);
                rank(a_total, a_unseen, a.id).cmp(&rank(b_total, b_unseen, b.id))
            });
            if let Some(w) = winner {
                let (_, w_unseen) = ep_counts.get(&w.id).copied().unwrap_or((0, 0));
                if w_unseen > 0 {
                    winners.push(w.id);
                }
            }
        }
        Ok(winners)
    }

    /// Returns followed canonical shows with no row that has episodes on ANY site,
    /// minus those with a fresh miss marker.
    pub fn shows_needing_episode_source(&self, _active_source_id: i64, now: i64) -> Result<Vec<EpisodeSourceCandidate>> {
        let mut stmt = self.conn.prepare(
            "SELECT s.id, s.anilist_id, s.title, c.title AS catalog_title, s.followed,
                    COUNT(e.id) AS total_ep, c.status AS catalog_status,
                    s.watched_externally
             FROM series s
             LEFT JOIN anilist_catalog c ON c.id = s.anilist_id
             LEFT JOIN episodes e ON e.series_id = s.id
             GROUP BY s.id"
        )?;
        struct Row { anilist_id: Option<i64>, title: String, catalog_title: Option<String>, followed: bool, total_ep: i64, catalog_status: Option<String>, watched_ext: bool }
        let rows = stmt.query_map([], |r| Ok(Row {
            anilist_id: r.get(1)?, title: r.get(2)?, catalog_title: r.get(3)?,
            followed: r.get::<_, i64>(4)? != 0, total_ep: r.get(5)?,
            catalog_status: r.get(6)?, watched_ext: r.get::<_, i64>(7)? != 0
        }))?.collect::<rusqlite::Result<Vec<_>>>()?;

        let mut groups: std::collections::HashMap<String, Vec<Row>> = std::collections::HashMap::new();

        // normalized catalog title -> AniList id (lowest id wins, so grouping is deterministic)
        let mut tkey_to_anilist: std::collections::HashMap<String, i64> = std::collections::HashMap::new();
        for r in &rows {
            if let (Some(al_id), Some(cat_title)) = (r.anilist_id, &r.catalog_title) {
                let tkey = format!("t:{}", crate::matching::normalize_title(cat_title));
                tkey_to_anilist.entry(tkey).and_modify(|id| *id = (*id).min(al_id)).or_insert(al_id);
            }
        }

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
            groups.entry(key).or_default().push(r);
        }

        let mut candidates = Vec::new();
        for (key, group) in groups {
            if !group.iter().any(|r| r.followed) {
                continue; // Not followed
            }
            if group.iter().any(|r| r.total_ep > 0) {
                continue; // Has episodes on at least one site
            }
            if self.has_recent_episode_fallback_miss(&key, now)? {
                continue; // Fresh miss marker
            }

            // Must be linked to a catalog entry
            if !key.starts_with("al:") {
                continue;
            }

            let mut valid_rep = None;
            for r in group {
                if r.anilist_id.is_none() { continue; }
                if r.watched_ext { continue; }

                let valid_status = r.catalog_status.as_deref() == Some("RELEASING") ||
                                   r.catalog_status.as_deref() == Some("FINISHED") ||
                                   r.catalog_status.as_deref() == Some("HIATUS");
                if valid_status {
                    valid_rep = Some(r);
                    break;
                }
            }

            if let Some(rep) = valid_rep {
                candidates.push(EpisodeSourceCandidate {
                    canon_key: key,
                    anilist_id: rep.anilist_id,
                    title: rep.title,
                });
            }
        }

        // Sort for stable tests
        candidates.sort_by(|a, b| a.title.cmp(&b.title));

        Ok(candidates)
    }

    /// Unseen episodes of currently-followed series only, falling back
    /// to other sites if the active site has no episodes for the show.
    pub fn pending_count(&self, source_id: i64) -> Result<i64> {
        let winners = self.get_fallback_series_ids(source_id)?;
        if winners.is_empty() { return Ok(0); }
        let ids_str = winners.iter().map(|id| id.to_string()).collect::<Vec<_>>().join(",");
        let sql = format!(
            "SELECT count(*) FROM episodes e JOIN series s ON s.id = e.series_id
             WHERE e.seen=0 AND s.id IN ({ids_str})"
        );
        let n: i64 = self.conn.query_row(&sql, [], |r| r.get(0))?;
        Ok(n)
    }

    /// Unseen episodes of currently-followed series, grouped and ordered.
    /// Falls back to other sites if the active site has no episodes.
    pub fn list_pending(
        &self,
        source_id: i64,
        sort: PendingSort,
    ) -> Result<Vec<(crate::models::Series, crate::models::Episode)>> {
        let winners = self.get_fallback_series_ids(source_id)?;
        if winners.is_empty() { return Ok(vec![]); }
        let ids_str = winners.iter().map(|id| id.to_string()).collect::<Vec<_>>().join(",");
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
             WHERE e.seen=0 AND s.watched_externally=0 AND s.id IN ({ids_str})
             ORDER BY remaining {dir}, s.title, CAST(e.number AS INTEGER) ASC, e.id ASC"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt
            .query_map([], |r| {
                let series = crate::models::Series {
                    id: r.get(0)?,
                    slug: r.get(1)?,
                    title: r.get(2)?,
                    url: r.get(3)?,
                    cover_url: r.get(4)?,
                    is_airing: r.get::<_, i64>(5)? != 0,
                    // Force followed to true since we selected it because *some* canonical row is followed
                    followed: true,
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
    fn shows_needing_episode_source_selection_and_expiry() {
        let db = Db::open(":memory:").unwrap();
        let src_a = db.upsert_source("AnimeYT", "https://a", "animeytx").unwrap();
        let src_b = db.upsert_source("TioAnime", "https://b", "tioanime").unwrap();
        let now = 10_000_000;

        let mk = |slug: &str, title: &str| crate::models::Series {
            id: 0, slug: slug.into(), title: title.into(), url: format!("u-{slug}"),
            cover_url: None, is_airing: true, followed: false, next_episode_at: None, site_episode_count: None,
        };

        // Helper to insert a catalog anime
        let insert_catalog = |id, status| {
            db.upsert_catalog_anime(&crate::db::test_support::catalog_anime(id, "Title", &[]), 0).unwrap();
            db.conn.execute("UPDATE anilist_catalog SET status=?1 WHERE id=?2", (status, id)).unwrap();
        };

        // Candidate 1: Followed, 0 eps, RELEASING -> Candidate
        insert_catalog(10, "RELEASING");
        let c1 = db.upsert_series(src_a, &mk("c1", "Candidate One")).unwrap();
        db.set_anilist_id(c1, 10).unwrap();
        db.set_followed(c1, true).unwrap();

        // Followed, 0 eps on A, but has eps on B -> NOT a Candidate
        insert_catalog(20, "RELEASING");
        let c2a = db.upsert_series(src_a, &mk("c2a", "Candidate Two")).unwrap();
        db.set_anilist_id(c2a, 20).unwrap();
        db.set_followed(c2a, true).unwrap();
        let c2b = db.upsert_series(src_b, &mk("c2b", "Candidate Two")).unwrap();
        db.set_anilist_id(c2b, 20).unwrap();
        db.insert_episode(&crate::models::Episode {
            id: 0, series_id: c2b, number: "1".into(), title: None, url: "u/1".into(), released_at: None, seen: false
        }).unwrap();

        // Not followed -> NOT a Candidate
        insert_catalog(30, "RELEASING");
        let c3 = db.upsert_series(src_a, &mk("c3", "Candidate Three")).unwrap();
        db.set_anilist_id(c3, 30).unwrap();

        // Followed, 0 eps, RELEASING, but fresh miss marker -> NOT a Candidate
        insert_catalog(40, "RELEASING");
        let c4 = db.upsert_series(src_a, &mk("c4", "Candidate Four")).unwrap();
        db.set_anilist_id(c4, 40).unwrap();
        db.set_followed(c4, true).unwrap();
        db.record_episode_fallback_miss("al:40", now - 3600).unwrap(); // 1 hour ago

        // Followed, 0 eps, RELEASING, miss marker is expired (4 days ago) -> Candidate again
        insert_catalog(50, "RELEASING");
        let c5 = db.upsert_series(src_a, &mk("c5", "Candidate Five")).unwrap();
        db.set_anilist_id(c5, 50).unwrap();
        db.set_followed(c5, true).unwrap();
        db.record_episode_fallback_miss("al:50", now - 4 * 24 * 3600).unwrap();

        // Followed, 0 eps, but NOT_YET_RELEASED -> NOT a Candidate
        insert_catalog(60, "NOT_YET_RELEASED");
        let c6 = db.upsert_series(src_a, &mk("c6", "Candidate Six")).unwrap();
        db.set_anilist_id(c6, 60).unwrap();
        db.set_followed(c6, true).unwrap();

        // Followed, 0 eps, RELEASING, but watched_externally -> NOT a Candidate
        insert_catalog(70, "RELEASING");
        let c7 = db.upsert_series(src_a, &mk("c7", "Candidate Seven")).unwrap();
        db.set_anilist_id(c7, 70).unwrap();
        db.set_followed(c7, true).unwrap();
        db.conn.execute("UPDATE series SET watched_externally=1 WHERE id=?", [c7]).unwrap();

        // Followed from the AniList catalog (synthetic anilist-N row), 0 eps, RELEASING: a
        // Candidate (the real case: Kanojo no Tomodachi was followed this way).
        insert_catalog(80, "RELEASING");
        let c8 = db.upsert_series(src_a, &mk("anilist-80", "Candidate Eight")).unwrap();
        db.set_anilist_id(c8, 80).unwrap();
        db.set_followed(c8, true).unwrap();

        let candidates = db.shows_needing_episode_source(src_a, now).unwrap();
        let titles: Vec<&str> = candidates.iter().map(|c| c.title.as_str()).collect();
        assert_eq!(titles, vec!["Candidate Eight", "Candidate Five", "Candidate One"]);
    }

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
    fn pending_count_and_list_pending_fallback_to_other_sites() {
        let db = Db::open(":memory:").unwrap();
        let src_a = db.upsert_source("AnimeYT", "https://a.example", "animeytx").unwrap(); // active
        let src_b = db.upsert_source("TioAnime", "https://b.example", "tioanime").unwrap();
        let src_c = db.upsert_source("JKAnime", "https://c.example", "jkanime").unwrap();

        let mk = |slug: &str, title: &str| crate::models::Series {
            id: 0, slug: slug.into(), title: title.into(), url: format!("u-{slug}"),
            cover_url: None, is_airing: true, followed: false, next_episode_at: None, site_episode_count: None,
        };

        // 1. show on active site with episodes (unchanged)
        let active_has_eps = db.upsert_series(src_a, &mk("act", "Active Has Eps")).unwrap();
        db.set_followed(active_has_eps, true).unwrap();
        db.insert_episode(&crate::models::Episode {
            id: 0, series_id: active_has_eps, number: "1".into(), title: None,
            url: "u/1".into(), released_at: None, seen: false,
        }).unwrap();

        // 2. show only on another site
        let only_other = db.upsert_series(src_b, &mk("only-b", "Only Other")).unwrap();
        db.set_followed(only_other, true).unwrap();
        db.insert_episode(&crate::models::Episode {
            id: 0, series_id: only_other, number: "1".into(), title: None,
            url: "u/1".into(), released_at: None, seen: false,
        }).unwrap();

        // 3. show on two other sites (the one with more episodes wins)
        let two_b = db.upsert_series(src_b, &mk("two-b", "Two Others")).unwrap();
        db.set_anilist_id(two_b, 100).unwrap();
        db.set_followed(two_b, true).unwrap();
        db.insert_episode(&crate::models::Episode { id: 0, series_id: two_b, number: "1".into(), title: None, url: "u/1".into(), released_at: None, seen: false }).unwrap();

        let two_c = db.upsert_series(src_c, &mk("two-c", "Two Others")).unwrap();
        db.set_anilist_id(two_c, 100).unwrap();
        // C has 2 episodes, so C wins
        db.insert_episode(&crate::models::Episode { id: 0, series_id: two_c, number: "1".into(), title: None, url: "u/1".into(), released_at: None, seen: false }).unwrap();
        db.insert_episode(&crate::models::Episode { id: 0, series_id: two_c, number: "2".into(), title: None, url: "u/2".into(), released_at: None, seen: false }).unwrap();

        // 4. show on active site with 0 eps, but another site has eps
        let active_zero = db.upsert_series(src_a, &mk("zero-a", "Active Zero")).unwrap();
        db.set_anilist_id(active_zero, 200).unwrap();
        db.set_followed(active_zero, true).unwrap();

        let zero_b = db.upsert_series(src_b, &mk("zero-b", "Active Zero")).unwrap();
        db.set_anilist_id(zero_b, 200).unwrap();
        db.insert_episode(&crate::models::Episode { id: 0, series_id: zero_b, number: "1".into(), title: None, url: "u/1".into(), released_at: None, seen: false }).unwrap();

        // 5. unfollowed rows never appear
        let unfollowed = db.upsert_series(src_b, &mk("unf", "Unfollowed")).unwrap();
        db.insert_episode(&crate::models::Episode { id: 0, series_id: unfollowed, number: "1".into(), title: None, url: "u/1".into(), released_at: None, seen: false }).unwrap();

        // Expected counts for active source (A):
        // Active Has Eps: 1
        // Only Other: 1 (from B)
        // Two Others: 2 (from C, because 2 > 1)
        // Active Zero: 1 (from B, because A has 0)
        // Total pending count = 1 + 1 + 2 + 1 = 5.
        assert_eq!(db.pending_count(src_a).unwrap(), 5);

        let pending = db.list_pending(src_a, PendingSort::RemainingAsc).unwrap();
        assert_eq!(pending.len(), 5);
        let ids: Vec<i64> = pending.iter().map(|(s, _)| s.id).collect();
        assert!(ids.contains(&active_has_eps));
        assert!(ids.contains(&only_other));
        assert!(ids.contains(&two_c));
        assert!(!ids.contains(&two_b));
        assert!(ids.contains(&zero_b));
        assert!(!ids.contains(&active_zero));
        assert!(!ids.contains(&unfollowed));
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
    fn list_pending_orders_episodes_numerically_within_a_series_not_by_insertion_order() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "https://a.example", "animeytx").unwrap();
        let mk = |slug: &str| crate::models::Series {
            id: 0, slug: slug.into(), title: slug.into(), url: format!("u-{slug}"),
            cover_url: None, is_airing: true, followed: false, next_episode_at: None, site_episode_count: None,
        };
        let sid = db.upsert_series(src, &mk("show")).unwrap();
        db.set_followed(sid, true).unwrap();
        // Inserted out of numeric order — a re-scan/backfill shape. If the
        // ordering were still keyed on insertion time (or a plain string
        // sort), "10" would surface before "2".
        for n in ["10", "2", "1"] {
            db.insert_episode(&crate::models::Episode {
                id: 0, series_id: sid, number: n.into(), title: None,
                url: format!("show/{n}"), released_at: None, seen: false,
            }).unwrap();
        }

        let pending = db.list_pending(src, PendingSort::RemainingAsc).unwrap();
        let numbers: Vec<&str> = pending.iter().map(|(_, e)| e.number.as_str()).collect();
        assert_eq!(numbers, vec!["1", "2", "10"], "numeric order, not insertion order or string order");
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
    fn list_airing_borrows_a_displayable_cover_from_a_sibling_site_when_the_representative_has_none() {
        // The reported bug: covers are only ever fetched for the currently
        // active site's own rows (refresh() is inherently one-active-site-
        // at-a-time), but list_airing is cross-site — a show airing on
        // several sites can pick a representative from whichever site is
        // active even when THAT site's own copy has never had its cover
        // fetched, while a sibling site's copy already has a real one. The
        // card showed the fallback tile forever even after the active
        // site's own backlog had long since converged.
        let db = Db::open(":memory:").unwrap();
        let a = db.upsert_source("AnimeYT", "https://a", "animeytx").unwrap();
        let b = db.upsert_source("TioAnime", "https://b", "tioanime").unwrap();
        let mk = |slug: &str, cover: &str| crate::models::Series {
            id: 0, slug: slug.into(), title: "Shared Show".into(), url: format!("u-{slug}"),
            cover_url: Some(cover.into()), is_airing: true, followed: false, next_episode_at: None, site_episode_count: None,
        };
        // Unlinked on both sides (no anilist_id) — same franchise_dedup_key
        // from the identical title is what groups them.
        let on_a = db.upsert_series(a, &mk("shared-a", "data:image/png;base64,AAAA")).unwrap();
        let on_b = db.upsert_series(b, &mk("shared-b", "https://tioanime.com/portadas/1.jpg")).unwrap();

        // B is active, so B's own row (blocked cover) is the representative
        // for url/id purposes — but the displayed cover must come from A.
        let out = db.list_airing(b).unwrap();
        assert_eq!(out.len(), 1, "deduped to one canonical entry");
        assert_eq!(out[0].id, on_b, "active-site row is still the representative for opening episodes");
        assert_eq!(out[0].cover_url.as_deref(), Some("data:image/png;base64,AAAA"), "cover borrowed from sibling site A");

        // If the representative's OWN cover is already displayable, nothing
        // is borrowed — its own value wins unchanged.
        let out_from_a = db.list_airing(a).unwrap();
        assert_eq!(out_from_a[0].id, on_a);
        assert_eq!(out_from_a[0].cover_url.as_deref(), Some("data:image/png;base64,AAAA"));
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

    #[test]
    fn is_csp_displayable_cover_handles_all_supported_schemes() {
        assert!(!is_csp_displayable_cover(None));
        assert!(!is_csp_displayable_cover(Some("")));
        assert!(is_csp_displayable_cover(Some("data:image/jpeg;base64,1234")));
        assert!(is_csp_displayable_cover(Some("file:covers/abc.jpg")));
        assert!(is_csp_displayable_cover(Some("file:///C:/Users/app/covers/abc.jpg")));
        assert!(is_csp_displayable_cover(Some("asset://localhost/covers/abc.jpg")));
        assert!(is_csp_displayable_cover(Some("https://s4.anilist.co/file/anilistcdn/media/anime/cover/medium/bx123.jpg")));
        assert!(is_csp_displayable_cover(Some("https://anilist.co/img/cover.jpg")));

        // Scraped remote covers must NOT be considered CSP displayable
        assert!(!is_csp_displayable_cover(Some("https://i0.wp.com/animeflv.net/cover.jpg")));
        assert!(!is_csp_displayable_cover(Some("https://cdn.jkdesa.com/cover.jpg")));
        assert!(!is_csp_displayable_cover(Some("https://animeflv.net/cover.jpg")));
        assert!(!is_csp_displayable_cover(Some("https://tioanime.com/cover.jpg")));
        assert!(!is_csp_displayable_cover(Some("https://w7.animeland.tv/cover.jpg")));
        assert!(!is_csp_displayable_cover(Some("https://wwv.animeytx.net/cover.jpg")));
    }

    #[test]
    fn list_airing_uses_catalog_next_airing_at_when_linked_and_future() {
        let db = Db::open(":memory:").unwrap();
        let a = db.upsert_source("AnimeYT", "https://a", "animeytx").unwrap();
        let now = chrono::Utc::now().timestamp();

        db.upsert_catalog_anime(&crate::db::test_support::catalog_anime(21, "One Piece", &[]), 0).unwrap();
        db.conn.execute("UPDATE anilist_catalog SET next_airing_at = ?1 WHERE id = 21", [now + 86400]).unwrap();

        let id = db.upsert_series(a, &mk_airing("op", "One Piece", Some(now + 100))).unwrap();
        db.set_anilist_id(id, 21).unwrap();

        db.upsert_series(a, &mk_airing("other", "Other", Some(now + 50000))).unwrap();

        let airing = db.list_airing(a).unwrap();
        assert_eq!(airing.len(), 2);
        assert_eq!(airing[0].title, "One Piece");
        assert_eq!(airing[0].next_episode_at, Some(now + 86400));
        assert_eq!(airing[1].title, "Other");
        assert_eq!(airing[1].next_episode_at, Some(now + 50000));
    }

    #[test]
    fn list_airing_keeps_scraped_value_when_catalog_value_is_null_or_unlinked() {
        let db = Db::open(":memory:").unwrap();
        let a = db.upsert_source("AnimeYT", "https://a", "animeytx").unwrap();
        let now = chrono::Utc::now().timestamp();

        db.upsert_catalog_anime(&crate::db::test_support::catalog_anime(21, "One Piece", &[]), 0).unwrap();

        let id1 = db.upsert_series(a, &mk_airing("op", "One Piece", Some(now + 100))).unwrap();
        db.set_anilist_id(id1, 21).unwrap();

        db.upsert_series(a, &mk_airing("other", "Other", Some(now + 200))).unwrap();

        let airing = db.list_airing(a).unwrap();
        let op = airing.iter().find(|s| s.title == "One Piece").unwrap();
        assert_eq!(op.next_episode_at, Some(now + 100));

        let other = airing.iter().find(|s| s.title == "Other").unwrap();
        assert_eq!(other.next_episode_at, Some(now + 200));
    }

    #[test]
    fn list_airing_merges_two_sites_with_catalog_value() {
        let db = Db::open(":memory:").unwrap();
        let a = db.upsert_source("AnimeYT", "https://a", "animeytx").unwrap();
        let b = db.upsert_source("TioAnime", "https://b", "tioanime").unwrap();
        let now = chrono::Utc::now().timestamp();

        db.upsert_catalog_anime(&crate::db::test_support::catalog_anime(21, "One Piece", &[]), 0).unwrap();
        db.conn.execute("UPDATE anilist_catalog SET next_airing_at = ?1 WHERE id = 21", [now + 86400]).unwrap();

        let id_a = db.upsert_series(a, &mk_airing("op-a", "One Piece", Some(now + 100))).unwrap();
        db.set_anilist_id(id_a, 21).unwrap();

        let id_b = db.upsert_series(b, &mk_airing("op-b", "One Piece", Some(now + 200))).unwrap();
        db.set_anilist_id(id_b, 21).unwrap();

        let airing = db.list_airing(a).unwrap();
        assert_eq!(airing.len(), 1);
        assert_eq!(airing[0].next_episode_at, Some(now + 86400));
    }

    #[test]
    fn list_airing_falls_back_to_scraped_when_catalog_value_is_stale() {
        let db = Db::open(":memory:").unwrap();
        let a = db.upsert_source("AnimeYT", "https://a", "animeytx").unwrap();
        let now = chrono::Utc::now().timestamp();

        db.upsert_catalog_anime(&crate::db::test_support::catalog_anime(21, "One Piece", &[]), 0).unwrap();
        db.conn.execute("UPDATE anilist_catalog SET next_airing_at = ?1 WHERE id = 21", [now - 3600]).unwrap();

        let id1 = db.upsert_series(a, &mk_airing("op", "One Piece", Some(now + 100))).unwrap();
        db.set_anilist_id(id1, 21).unwrap();

        let airing = db.list_airing(a).unwrap();
        assert_eq!(airing[0].next_episode_at, Some(now + 100));
    }

    #[test]
    fn list_airing_ordering_with_mixed_entries() {
        let db = Db::open(":memory:").unwrap();
        let a = db.upsert_source("AnimeYT", "https://a", "animeytx").unwrap();
        let now = chrono::Utc::now().timestamp();

        db.upsert_catalog_anime(&crate::db::test_support::catalog_anime(1, "Linked Future", &[]), 0).unwrap();
        db.conn.execute("UPDATE anilist_catalog SET next_airing_at = ?1 WHERE id = 1", [now + 10000]).unwrap();
        let id1 = db.upsert_series(a, &mk_airing("s1", "Linked Future", Some(0))).unwrap();
        db.set_anilist_id(id1, 1).unwrap();

        db.upsert_catalog_anime(&crate::db::test_support::catalog_anime(2, "Linked Null", &[]), 0).unwrap();
        let id2 = db.upsert_series(a, &mk_airing("s2", "Linked Null", Some(now + 5000))).unwrap();
        db.set_anilist_id(id2, 2).unwrap();

        db.upsert_series(a, &mk_airing("s3", "Unlinked Future", Some(now + 20000))).unwrap();
        db.upsert_series(a, &mk_airing("s4", "Unlinked Null", None)).unwrap();

        let airing = db.list_airing(a).unwrap();
        let titles: Vec<&str> = airing.iter().map(|s| s.title.as_str()).collect();
        assert_eq!(titles, vec!["Unlinked Future", "Linked Future", "Linked Null", "Unlinked Null"]);
    }

    #[test]
    fn list_airing_ignores_a_catalog_time_for_a_different_season() {
        let db = Db::open(":memory:").unwrap();
        let a = db.upsert_source("AnimeYT", "https://a", "animeytx").unwrap();
        let now = chrono::Utc::now().timestamp();

        // Only season 1 exists in the catalog; the site row is season 2.
        db.upsert_catalog_anime(&crate::db::test_support::catalog_anime(7, "Show", &[]), 0).unwrap();
        db.conn.execute("UPDATE anilist_catalog SET next_airing_at = ?1 WHERE id = 7", [now + 86400]).unwrap();
        let id = db.upsert_series(a, &mk_airing("show-2", "Show Temporada 2", Some(now + 100))).unwrap();
        db.set_anilist_id(id, 7).unwrap();

        let airing = db.list_airing(a).unwrap();
        assert_eq!(airing[0].next_episode_at, Some(now + 100), "a wrong-season link must not lend its countdown");
    }

    #[test]
    fn list_airing_prefers_a_newer_scraped_time_over_a_catalog_time_already_passed() {
        let db = Db::open(":memory:").unwrap();
        let a = db.upsert_source("AnimeYT", "https://a", "animeytx").unwrap();
        let now = chrono::Utc::now().timestamp();

        db.upsert_catalog_anime(&crate::db::test_support::catalog_anime(21, "One Piece", &[]), 0).unwrap();
        // episode aired 3 h ago; the catalog snapshot is stale, the site already shows next week
        db.conn.execute("UPDATE anilist_catalog SET next_airing_at = ?1 WHERE id = 21", [now - 3 * 3600]).unwrap();
        let id = db.upsert_series(a, &mk_airing("op", "One Piece", Some(now + 6 * 86400))).unwrap();
        db.set_anilist_id(id, 21).unwrap();

        let airing = db.list_airing(a).unwrap();
        assert_eq!(airing[0].next_episode_at, Some(now + 6 * 86400));
    }

    #[test]
    fn list_airing_keeps_the_scraped_time_of_an_unlinked_row_even_if_a_catalog_row_shares_its_title() {
        let db = Db::open(":memory:").unwrap();
        let a = db.upsert_source("AnimeYT", "https://a", "animeytx").unwrap();
        let now = chrono::Utc::now().timestamp();

        db.upsert_catalog_anime(&crate::db::test_support::catalog_anime(21, "One Piece", &[]), 0).unwrap();
        db.conn.execute("UPDATE anilist_catalog SET next_airing_at = ?1 WHERE id = 21", [now + 86400]).unwrap();
        db.upsert_series(a, &mk_airing("op", "One Piece", Some(now + 100))).unwrap(); // not linked

        let airing = db.list_airing(a).unwrap();
        assert_eq!(airing[0].next_episode_at, Some(now + 100));
    }

    #[test]
    fn list_airing_hides_an_unfollowed_live_action_without_a_catalog_entry() {
        let db = Db::open(":memory:").unwrap();
        let a = db.upsert_source("AnimeYT", "https://a", "animeytx").unwrap();
        db.upsert_series(a, &mk_airing("op-live", "One Piece: Live Action (2023)", None)).unwrap();
        let anime = db.upsert_series(a, &mk_airing("op", "Some Anime", None)).unwrap();
        let followed_live = db.upsert_series(a, &mk_airing("fl", "Another: Live Action", None)).unwrap();
        db.set_followed(followed_live, true).unwrap();

        let titles: Vec<String> = db.list_airing(a).unwrap().into_iter().map(|s| s.title).collect();
        assert!(titles.contains(&"Some Anime".to_string()));
        assert!(titles.contains(&"Another: Live Action".to_string()), "a followed one stays");
        assert!(!titles.iter().any(|t| t.contains("One Piece: Live Action")));
        let _ = anime;
    }

    #[test]
    fn list_airing_prefers_row_with_episodes_over_active_site_with_none() {
        let db = Db::open(":memory:").unwrap();
        let a = db.upsert_source("AnimeYT", "https://a", "animeytx").unwrap();
        let b = db.upsert_source("TioAnime", "https://b", "tioanime").unwrap();

        // Active site row has no episodes
        let _id_a = db.upsert_series(a, &mk_airing("op-a", "One Piece", None)).unwrap();
        // Non-active site row has an episode
        let id_b = db.upsert_series(b, &mk_airing("op-b", "One Piece", None)).unwrap();
        db.insert_episode(&crate::models::Episode {
            id: 0, series_id: id_b, number: "1".into(), title: None, url: "e1".into(), released_at: None, seen: false
        }).unwrap();

        let airing = db.list_airing(a).unwrap();
        assert_eq!(airing.len(), 1);
        assert_eq!(airing[0].url, "https://site/tv/op-b/", "representative should be the one with episodes");
    }

    #[test]
    fn active_row_with_all_seen_stays_authoritative_over_other_site_with_unseen() {
        let db = Db::open(":memory:").unwrap();
        let src_a = db.upsert_source("AnimeYT", "https://a.example", "animeytx").unwrap(); // active
        let src_b = db.upsert_source("TioAnime", "https://b.example", "tioanime").unwrap();

        let mk = |slug: &str, title: &str| crate::models::Series {
            id: 0, slug: slug.into(), title: title.into(), url: format!("u-{slug}"),
            cover_url: None, is_airing: true, followed: false, next_episode_at: None, site_episode_count: None,
        };

        let row_a = db.upsert_series(src_a, &mk("act", "Show")).unwrap();
        db.set_followed(row_a, true).unwrap();
        db.insert_episode(&crate::models::Episode {
            id: 0, series_id: row_a, number: "1".into(), title: None,
            url: "u/1".into(), released_at: None, seen: true, // SEEN!
        }).unwrap();

        let row_b = db.upsert_series(src_b, &mk("oth", "Show")).unwrap();
        db.set_followed(row_b, true).unwrap();
        db.insert_episode(&crate::models::Episode {
            id: 0, series_id: row_b, number: "1".into(), title: None,
            url: "u/1b".into(), released_at: None, seen: false, // UNSEEN!
        }).unwrap();

        // Active site has episodes, so it stays authoritative. Since its eps are seen, pending_count is 0.
        assert_eq!(db.pending_count(src_a).unwrap(), 0);
    }

    #[test]
    fn watched_externally_is_excluded_from_pending() {
        let db = Db::open(":memory:").unwrap();
        let src_a = db.upsert_source("AnimeYT", "https://a.example", "animeytx").unwrap();

        let row = db.upsert_series(src_a, &mk_airing("act", "Show", None)).unwrap();
        db.set_followed(row, true).unwrap();
        db.conn.execute("UPDATE series SET watched_externally = 1 WHERE id = ?", [row]).unwrap();

        db.insert_episode(&crate::models::Episode {
            id: 0, series_id: row, number: "1".into(), title: None,
            url: "u/1".into(), released_at: None, seen: false,
        }).unwrap();

        assert_eq!(db.pending_count(src_a).unwrap(), 0, "watched_externally should yield 0 pending");
    }

    #[test]
    fn group_where_only_non_winner_row_is_followed() {
        let db = Db::open(":memory:").unwrap();
        let src_a = db.upsert_source("AnimeYT", "https://a.example", "animeytx").unwrap();
        let src_b = db.upsert_source("TioAnime", "https://b.example", "tioanime").unwrap();

        let mk = |slug: &str, title: &str| crate::models::Series {
            id: 0, slug: slug.into(), title: title.into(), url: format!("u-{slug}"),
            cover_url: None, is_airing: true, followed: false, next_episode_at: None, site_episode_count: None,
        };

        // B has episodes but is NOT followed.
        let row_b = db.upsert_series(src_b, &mk("oth", "Show")).unwrap();
        db.insert_episode(&crate::models::Episode {
            id: 0, series_id: row_b, number: "1".into(), title: None,
            url: "u/1b".into(), released_at: None, seen: false,
        }).unwrap();

        // A has NO episodes but IS followed.
        let row_a = db.upsert_series(src_a, &mk("act", "Show")).unwrap();
        db.set_followed(row_a, true).unwrap(); // Marks canonical group followed!

        // Since the canonical group is followed, and A has no episodes, it falls back to B.
        assert_eq!(db.pending_count(src_a).unwrap(), 1);
        let pending = db.list_pending(src_a, PendingSort::RemainingAsc).unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].0.id, row_b);
    }

    #[test]
    fn pending_count_equals_list_pending_length() {
        let db = Db::open(":memory:").unwrap();
        let src_a = db.upsert_source("AnimeYT", "https://a.example", "animeytx").unwrap();

        let row = db.upsert_series(src_a, &mk_airing("act", "Show", None)).unwrap();
        db.set_followed(row, true).unwrap();
        db.insert_episode(&crate::models::Episode {
            id: 0, series_id: row, number: "1".into(), title: None,
            url: "u/1".into(), released_at: None, seen: false,
        }).unwrap();
        db.insert_episode(&crate::models::Episode {
            id: 0, series_id: row, number: "2".into(), title: None,
            url: "u/2".into(), released_at: None, seen: false,
        }).unwrap();

        let count = db.pending_count(src_a).unwrap();
        let pending = db.list_pending(src_a, PendingSort::RemainingAsc).unwrap();
        assert_eq!(count as usize, pending.len());
    }

    #[test]
    fn timing_test_real_size() {
        let db = Db::open(":memory:").unwrap();
        let src_a = db.upsert_source("AnimeYT", "https://a.example", "animeytx").unwrap();
        let src_b = db.upsert_source("TioAnime", "https://b.example", "tioanime").unwrap();

        // Generate 6k series
        let tx = db.conn.unchecked_transaction().unwrap();
        for i in 0..6000 {
            let followed = if i % 3 == 0 { 1 } else { 0 }; // 2k followed
            let src = if i % 2 == 0 { src_a } else { src_b };
            tx.execute(
                "INSERT INTO series(source_id, slug, title, url, is_airing, followed, watched_externally)
                 VALUES(?1, ?2, ?3, ?4, 0, ?5, 0)",
                (src, format!("s-{i}"), format!("Show {i}"), format!("u/{i}"), followed)
            ).unwrap();
            let sid = tx.last_insert_rowid();
            // 20 episodes each => 120k episodes
            for j in 1..=20 {
                tx.execute(
                    "INSERT INTO episodes(series_id, number, url, seen) VALUES(?1, ?2, ?3, 0)",
                    (sid, j.to_string(), format!("u/{i}/{j}"))
                ).unwrap();
            }
        }
        tx.commit().unwrap();

        let start = std::time::Instant::now();
        let count = db.pending_count(src_a).unwrap();
        let elapsed = start.elapsed();

        let start2 = std::time::Instant::now();
        let list = db.list_pending(src_a, PendingSort::RemainingAsc).unwrap();
        let elapsed2 = start2.elapsed();

        assert!(elapsed.as_millis() < 1500, "pending_count took (debug build, limit 1500) {} ms", elapsed.as_millis());
        assert!(elapsed2.as_millis() < 1500, "list_pending took {} ms", elapsed2.as_millis());
        assert_eq!(count as usize, list.len());
    }
}
