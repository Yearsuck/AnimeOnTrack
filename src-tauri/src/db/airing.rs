use super::*;

/// A followed canonical show that lacks episodes on all tracked sites, or is
/// behind what AniList says has aired.
#[derive(Debug, Clone, PartialEq)]
pub struct EpisodeSourceCandidate {
    pub canon_key: String,
    pub anilist_id: Option<i64>,
    pub title: String,
    pub known_rows: Vec<(i64 /*series id*/, String /*site_id*/, String /*series url*/)>,
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

/// An unfollowed airing row whose last scraped release is older than this has
/// stopped (finished or dropped) even if the site still lists it.
const STALE_AIRING_SECS: i64 = 21 * 86400;

/// A chosen pending episode: (episode id, owning series id, ranking score).
type ChosenEpisode = (i64, i64, (usize, i64, i64));

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
    /// otherwise, if status is RELEASING, the catalog date aired within the last 7
    /// days, and no scraped upcoming date exists, it rolls over to next week so the show
    /// stays in the weekly grid; otherwise the scraped value. A catalog time that has already passed is a
    /// snapshot taken before the episode aired (the site has usually rolled to
    /// next week by then), so it never beats the freshly scraped value.
    pub fn list_airing_with_rolled(
        &self,
        active_source_id: i64,
    ) -> Result<(Vec<crate::models::Series>, std::collections::HashSet<i64>)> {
        let mut stmt = self.conn.prepare(
            "SELECT s.id, s.source_id, s.anilist_id, s.slug, s.url,
                    COALESCE(c.cover_url, s.cover_url) AS cover_url,
                    COALESCE(c.title, s.title) AS title,
                    s.followed, s.next_episode_at AS scraped_next,
                    c.next_airing_at AS catalog_next,
                    s.title AS site_title, c.title AS c_title,
                    c.title_romaji AS c_romaji, c.title_english AS c_english,
                    s.site_episode_count, c.format AS c_format, c.status AS c_status,
                    (SELECT COUNT(*) FROM episodes e WHERE e.series_id = s.id) AS total_ep
             FROM series s LEFT JOIN anilist_catalog c ON c.id = s.anilist_id
             WHERE s.is_airing = 1",
        )?;
        struct AiringRow {
            source_id: i64,
            anilist_id: Option<i64>,
            followed: bool,
            total_ep: i64,
            /// AniList format of the linked catalog entry (OVA/SPECIAL/...),
            /// only when it is the same season as the scraped row.
            catalog_format: Option<String>,
            /// AniList says the linked (same-season) entry is still releasing
            /// or on hiatus, so a stale scraped date alone must not hide it.
            catalog_active: bool,
            /// The catalog confirms an upcoming episode of this very season.
            catalog_upcoming: bool,
            next_episode_estimated: bool,
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
                let mut catalog_upcoming = false;
                let mut catalog_active = false;
                let mut next_episode_estimated = false;
                let mut catalog_format: Option<String> = None;
                if anilist_id.is_some() && r.get::<_, Option<String>>("c_title")?.is_some() {
                    let site_title: String = r.get("site_title")?;
                    let titles: Vec<String> = ["c_title", "c_romaji", "c_english"]
                        .into_iter()
                        .filter_map(|col| r.get::<_, Option<String>>(col).transpose())
                        .collect::<rusqlite::Result<_>>()?;
                    let refs: Vec<&str> = titles.iter().map(String::as_str).collect();
                    if crate::matching::season_consistent(&site_title, &refs) {
                        catalog_format = r.get("c_format")?;
                        let status: Option<String> = r.get("c_status")?;
                        catalog_active = matches!(status.as_deref(), Some("RELEASING" | "HIATUS"));
                        if let Some(cat) = catalog_next.filter(|&cat| cat > now) {
                            effective_next = Some(cat);
                            catalog_upcoming = true;
                        } else if let Some(cat) = catalog_next {
                            const SEVEN_DAYS_SECS: i64 = 7 * 86400;
                            let scraped_upcoming = scraped_next.is_some_and(|s| s > now);
                            if status.as_deref() == Some("RELEASING")
                                && cat <= now
                                && now - cat < SEVEN_DAYS_SECS
                                && !scraped_upcoming
                            {
                                let mut rolled = cat;
                                while rolled <= now {
                                    rolled += SEVEN_DAYS_SECS;
                                }
                                effective_next = Some(rolled);
                                next_episode_estimated = true;
                            }
                        }
                    }
                }

                Ok(AiringRow {
                    source_id: r.get("source_id")?,
                    anilist_id,
                    followed,
                    total_ep: r.get("total_ep")?,
                    catalog_format,
                    catalog_active,
                    catalog_upcoming,
                    next_episode_estimated,
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

        let group_keys = self.canonical_group_keys()?;
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
            // The catalog bridge only ever upgrades an unlinked row to the `al:` key of
            // the show it demonstrably is; everything else keeps the franchise key so
            // season variants of unlinked rows still collapse ("Temporada 2" vs "2nd Season").
            let key = match group_keys.get(&row.series.id) {
                Some(k) if k.starts_with("al:") => k.clone(),
                _ => match row.anilist_id {
                    Some(id) => format!("al:{id}"),
                    None => format!("t:{}", crate::matching::franchise_dedup_key(&row.series.title)),
                },
            };
            if !groups.contains_key(&key) {
                order.push(key.clone());
            }
            groups.entry(key).or_default().push(row);
        }

        let mut out: Vec<crate::models::Series> = Vec::with_capacity(order.len());
        let mut rolled_series_ids = std::collections::HashSet::new();
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
            // The shown date is estimated only when it comes from a rolled member and no
            // member with a real date (scraped or AniList) confirms the same moment.
            let from_freshest = |est: bool| {
                group
                    .iter()
                    .any(|m| m.next_episode_estimated == est && m.series.next_episode_at == freshest_next_episode_at)
            };
            let next_date_rolled = from_freshest(true) && !from_freshest(false);
            // A site keeps listing OVAs, "Specials" and shows that stopped
            // weeks ago on its airing page. Hide them at read time (nothing is
            // rewritten) unless the user follows them or AniList confirms an
            // upcoming episode of this season.
            if !followed_any && !group.iter().any(|m| m.catalog_upcoming) {
                let side_content = group.iter().any(|m| {
                    matches!(m.catalog_format.as_deref(), Some("OVA" | "SPECIAL" | "MUSIC" | "MOVIE"))
                        || crate::matching::is_special_title(&m.series.title)
                });
                let stale = !group.iter().any(|m| m.catalog_active)
                    && freshest_next_episode_at.is_some_and(|t| t < now - STALE_AIRING_SECS);
                if side_content || stale {
                    continue;
                }
            }
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
            if next_date_rolled {
                rolled_series_ids.insert(series.id);
            }
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
        Ok((out, rolled_series_ids))
    }

    pub fn list_airing(&self, active_source_id: i64) -> Result<Vec<crate::models::Series>> {
        Ok(self.list_airing_with_rolled(active_source_id)?.0)
    }
}

fn site_rank(site_id: Option<&str>, priority: &[String]) -> usize {
    match site_id {
        Some(s) => priority.iter().position(|p| p == s).unwrap_or(priority.len()),
        None => priority.len(),
    }
}

#[derive(Clone, PartialEq, Eq, Hash)]
enum EpKey {
    Numeric(u64), // f64::to_bits()
    Text(String),
}

impl EpKey {
    fn cmp_key(&self, other: &Self) -> std::cmp::Ordering {
        match (self, other) {
            (EpKey::Numeric(a), EpKey::Numeric(b)) => {
                let fa = f64::from_bits(*a);
                let fb = f64::from_bits(*b);
                fa.partial_cmp(&fb).unwrap_or(std::cmp::Ordering::Equal)
            }
            (EpKey::Numeric(_), EpKey::Text(_)) => std::cmp::Ordering::Less,
            (EpKey::Text(_), EpKey::Numeric(_)) => std::cmp::Ordering::Greater,
            (EpKey::Text(a), EpKey::Text(b)) => a.cmp(b),
        }
    }
}

impl Db {
    fn merged_pending_episodes(&self, priority: &[String]) -> Result<Vec<(i64, Vec<i64>)>> {
        self.merged_pending_episodes_at(priority, chrono::Utc::now().timestamp())
    }

    pub(crate) fn merged_pending_episodes_at(
        &self,
        priority: &[String],
        now: i64,
    ) -> Result<Vec<(i64, Vec<i64>)>> {
        let mut stmt = self.conn.prepare(
            "SELECT s.id, s.source_id, s.anilist_id, s.title, c.title AS catalog_title,
                    s.watched_externally, s.followed, so.site_id
             FROM series s
             LEFT JOIN sources so ON so.id = s.source_id
             LEFT JOIN anilist_catalog c ON c.id = s.anilist_id",
        )?;
        struct Row {
            id: i64,
            anilist_id: Option<i64>,
            title: String,
            catalog_title: Option<String>,
            watched_externally: bool,
            followed: bool,
            site_id: Option<String>,
        }
        let rows = stmt
            .query_map([], |r| {
                Ok(Row {
                    id: r.get(0)?,
                    anilist_id: r.get(2)?,
                    title: r.get(3)?,
                    catalog_title: r.get(4)?,
                    watched_externally: r.get::<_, i64>(5)? != 0,
                    followed: r.get::<_, i64>(6)? != 0,
                    site_id: r.get(7)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        let mut groups: std::collections::HashMap<String, Vec<Row>> =
            std::collections::HashMap::new();

        // normalized catalog title -> AniList id (lowest id wins, so grouping is deterministic)
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
                    if !r.watched_externally {
                        candidate_ids.push(r.id);
                    }
                }
            }
        }

        if candidate_ids.is_empty() {
            return Ok(vec![]);
        }

        struct EpRow {
            id: i64,
            series_id: i64,
            number: String,
            seen: bool,
        }

        let mut eps_by_series: std::collections::HashMap<i64, Vec<EpRow>> =
            std::collections::HashMap::new();
        for chunk in candidate_ids.chunks(5000) {
            let ids_str = chunk.iter().map(|id| id.to_string()).collect::<Vec<_>>().join(",");
            let sql = format!(
                "SELECT id, series_id, number, seen FROM episodes WHERE series_id IN ({ids_str})"
            );
            let mut ep_stmt = self.conn.prepare(&sql)?;
            let ep_rows = ep_stmt.query_map([], |r| {
                Ok(EpRow {
                    id: r.get(0)?,
                    series_id: r.get(1)?,
                    number: r.get(2)?,
                    seen: r.get::<_, i64>(3)? != 0,
                })
            })?;
            for ep in ep_rows {
                let ep = ep?;
                eps_by_series.entry(ep.series_id).or_default().push(ep);
            }
        }

        let mut show_results = Vec::new();

        for (_group_key, group) in groups {
            if !group.iter().any(|r| r.followed) {
                continue;
            }
            let candidate_members: Vec<&Row> =
                group.iter().filter(|r| !r.watched_externally).collect();
            if candidate_members.is_empty() {
                continue;
            }

            // Compute row_align once for every candidate member.
            let mut member_align: std::collections::HashMap<i64, super::episode_align::RowAlign> =
                std::collections::HashMap::new();
            for m in &candidate_members {
                let align = self.row_align(m.id, now)?;
                member_align.insert(m.id, align);
            }

            // W = the highest canonical episode number that is seen=1 over all members (0.0 if none).
            let mut w: f64 = 0.0;
            for m in &candidate_members {
                let align = &member_align[&m.id];
                if let Some(eps) = eps_by_series.get(&m.id) {
                    for ep in eps {
                        if ep.seen {
                            if let Some(num) = crate::db::episodes::parse_ep_number(&ep.number) {
                                if let Some(canon) = align.to_canonical(num) {
                                    if canon > w {
                                        w = canon;
                                    }
                                }
                            }
                        }
                    }
                }
            }

            // For each distinct canonical episode number > W that exists unseen in some member, choose the
            // member whose site ranks first in priority (rank of the member's sources.site_id;
            // unknown site = last; ties by lowest series id). That member's episode row is the chosen one.
            let mut best_by_key: std::collections::HashMap<EpKey, ChosenEpisode> =
                std::collections::HashMap::new();

            for m in &candidate_members {
                let align = &member_align[&m.id];
                let rank = site_rank(m.site_id.as_deref(), priority);
                if let Some(eps) = eps_by_series.get(&m.id) {
                    for ep in eps {
                        if ep.seen {
                            continue;
                        }
                        let key = match crate::db::episodes::parse_ep_number(&ep.number) {
                            Some(v) => {
                                let Some(canon) = align.to_canonical(v) else {
                                    // Episodes whose canonical number is None (earlier-season numbers of a cumulative row) are never pending
                                    continue;
                                };
                                if canon <= w {
                                    continue;
                                }
                                EpKey::Numeric(canon.to_bits())
                            }
                            None => EpKey::Text(ep.number.trim().to_string()),
                        };
                        let score = (rank, m.id, ep.id);
                        match best_by_key.get(&key) {
                            Some((_, _, cur_score)) if *cur_score <= score => {}
                            _ => {
                                best_by_key.insert(key, (ep.id, m.id, score));
                            }
                        }
                    }
                }
            }

            if best_by_key.is_empty() {
                continue;
            }

            // Representative series = the member that owns the most chosen episodes,
            // ties broken by priority rank then id.
            let mut member_chosen_counts: std::collections::HashMap<i64, usize> =
                std::collections::HashMap::new();
            for (_, series_id, _) in best_by_key.values() {
                *member_chosen_counts.entry(*series_id).or_default() += 1;
            }

            let rep = candidate_members
                .iter()
                .min_by_key(|m| {
                    let count = member_chosen_counts.get(&m.id).copied().unwrap_or(0);
                    let rank = site_rank(m.site_id.as_deref(), priority);
                    (std::cmp::Reverse(count), rank, m.id)
                })
                .unwrap();

            let mut sorted_keys: Vec<EpKey> = best_by_key.keys().cloned().collect();
            sorted_keys.sort_by(|a, b| a.cmp_key(b));
            let chosen_episodes: Vec<i64> =
                sorted_keys.into_iter().map(|k| best_by_key[&k].0).collect();

            show_results.push((rep.id, chosen_episodes));
        }

        Ok(show_results)
    }


    /// Returns followed canonical shows with no row that has episodes on ANY site
    /// (old rule), or followed releasing shows behind what AniList says has aired,
    /// minus those with a fresh miss marker.
    pub fn shows_needing_episode_source(&self, _active_source_id: i64, now: i64) -> Result<Vec<EpisodeSourceCandidate>> {
        let mut stmt = self.conn.prepare(
            "SELECT s.id, COALESCE(src.site_id, '') AS site_id, s.url, s.anilist_id, s.title,
                    c.title AS catalog_title, s.followed,
                    COUNT(e.id) AS total_ep,
                    COALESCE(MAX(CAST(e.number AS INTEGER)), 0) AS max_ep,
                    c.status AS catalog_status,
                    c.next_episode AS catalog_next_episode,
                    s.watched_externally
             FROM series s
             LEFT JOIN sources src ON src.id = s.source_id
             LEFT JOIN anilist_catalog c ON c.id = s.anilist_id
             LEFT JOIN episodes e ON e.series_id = s.id
             GROUP BY s.id"
        )?;
        struct Row {
            id: i64,
            site_id: String,
            url: String,
            anilist_id: Option<i64>,
            title: String,
            catalog_title: Option<String>,
            followed: bool,
            total_ep: i64,
            max_ep: i64,
            catalog_status: Option<String>,
            catalog_next_episode: Option<i64>,
            watched_ext: bool,
        }
        let rows = stmt.query_map([], |r| Ok(Row {
            id: r.get(0)?,
            site_id: r.get(1)?,
            url: r.get(2)?,
            anilist_id: r.get(3)?,
            title: r.get(4)?,
            catalog_title: r.get(5)?,
            followed: r.get::<_, i64>(6)? != 0,
            total_ep: r.get(7)?,
            max_ep: r.get(8)?,
            catalog_status: r.get(9)?,
            catalog_next_episode: r.get(10)?,
            watched_ext: r.get::<_, i64>(11)? != 0,
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
            if group.iter().any(|r| r.watched_ext) {
                continue; // Watched externally
            }
            if self.has_recent_episode_fallback_miss(&key, now)? {
                continue; // Fresh miss marker
            }

            // Must be linked to a catalog entry
            if !key.starts_with("al:") {
                continue;
            }

            let valid_rep = group.iter().find(|r| r.anilist_id.is_some() && !r.watched_ext);
            let rep = match valid_rep {
                Some(r) => r,
                None => continue,
            };

            let no_episodes_anywhere = group.iter().all(|r| r.total_ep == 0);
            if no_episodes_anywhere {
                let valid_status = rep.catalog_status.as_deref() == Some("RELEASING") ||
                                   rep.catalog_status.as_deref() == Some("FINISHED") ||
                                   rep.catalog_status.as_deref() == Some("HIATUS");
                if valid_status {
                    candidates.push(EpisodeSourceCandidate {
                        canon_key: key,
                        anilist_id: rep.anilist_id,
                        title: rep.title.clone(),
                        known_rows: Vec::new(),
                    });
                }
            } else {
                if rep.catalog_status.as_deref() != Some("RELEASING") {
                    continue;
                }
                let next_ep = match rep.catalog_next_episode {
                    Some(n) => n,
                    None => continue,
                };
                let expected = next_ep - 1;
                if expected <= 0 {
                    continue;
                }
                let max_ep = group.iter().map(|r| r.max_ep).max().unwrap_or(0);
                if max_ep < expected {
                    let mut known_rows: Vec<(i64, String, String)> = group.iter()
                        .filter(|r| !r.url.trim().is_empty())
                        .map(|r| (r.id, r.site_id.clone(), r.url.clone()))
                        .collect();
                    known_rows.sort_by_key(|r| r.0);
                    candidates.push(EpisodeSourceCandidate {
                        canon_key: key,
                        anilist_id: rep.anilist_id,
                        title: rep.title.clone(),
                        known_rows,
                    });
                }
            }
        }

        // Sort for stable tests
        candidates.sort_by(|a, b| a.title.cmp(&b.title));

        Ok(candidates)
    }

    /// Unseen episodes of currently-followed series only, merged across
    /// all sites by priority order.
    pub fn pending_count(&self, _source_id: i64, priority: &[String]) -> Result<i64> {
        let shows = self.merged_pending_episodes(priority)?;
        let count: usize = shows.iter().map(|(_, eps)| eps.len()).sum();
        Ok(count as i64)
    }

    /// Unseen episodes of currently-followed series, merged across all sites
    /// by priority order, grouped and ordered.
    pub fn list_pending(
        &self,
        _source_id: i64,
        sort: PendingSort,
        priority: &[String],
    ) -> Result<Vec<(crate::models::Series, crate::models::Episode)>> {
        let shows = self.merged_pending_episodes(priority)?;
        if shows.is_empty() {
            return Ok(vec![]);
        }

        let rep_ids: Vec<i64> = shows.iter().map(|(rep_id, _)| *rep_id).collect();
        let all_ep_ids: Vec<i64> = shows.iter().flat_map(|(_, eps)| eps.iter().copied()).collect();

        let mut reps_by_id = std::collections::HashMap::new();
        for chunk in rep_ids.chunks(5000) {
            let ids_str = chunk.iter().map(|id| id.to_string()).collect::<Vec<_>>().join(",");
            let sql = format!(
                "SELECT s.id, s.slug, COALESCE(c.title, s.title) AS title, s.url,
                        COALESCE(
                            c.cover_url,
                            CASE WHEN s.cover_url LIKE 'file:%' OR s.cover_url LIKE 'data:%' OR s.cover_url LIKE 'https://%anilist.co%' THEN s.cover_url ELSE NULL END,
                            s.cover_url
                        ) AS cover_url,
                        s.is_airing, s.followed,
                        s.next_episode_at, s.site_episode_count
                 FROM series s
                 LEFT JOIN anilist_catalog c ON c.id = s.anilist_id
                 WHERE s.id IN ({ids_str})"
            );
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map([], |r| {
                Ok(crate::models::Series {
                    id: r.get(0)?,
                    slug: r.get(1)?,
                    title: r.get(2)?,
                    url: r.get(3)?,
                    cover_url: r.get(4)?,
                    is_airing: r.get::<_, i64>(5)? != 0,
                    followed: true,
                    next_episode_at: r.get(7)?,
                    site_episode_count: r.get(8)?,
                })
            })?;
            for s in rows {
                let s = s?;
                reps_by_id.insert(s.id, s);
            }
        }

        let mut episodes_by_id = std::collections::HashMap::new();
        for chunk in all_ep_ids.chunks(5000) {
            let ids_str = chunk.iter().map(|id| id.to_string()).collect::<Vec<_>>().join(",");
            let sql = format!(
                "SELECT id, series_id, number, title, url, released_at, seen, download_path, download_status
                 FROM episodes WHERE id IN ({ids_str})"
            );
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map([], |r| {
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
            })?;
            for ep in rows {
                let ep = ep?;
                episodes_by_id.insert(ep.id, ep);
            }
        }

        struct ShowGroup {
            series: crate::models::Series,
            remaining: usize,
            episodes: Vec<crate::models::Episode>,
        }

        let mut show_groups = Vec::new();
        for (rep_id, ep_ids) in shows {
            let Some(series) = reps_by_id.get(&rep_id).cloned() else {
                continue;
            };
            let mut episodes = Vec::new();
            for ep_id in ep_ids {
                if let Some(ep) = episodes_by_id.get(&ep_id).cloned() {
                    episodes.push(ep);
                }
            }
            episodes.sort_by(|a, b| {
                let num_a = crate::db::episodes::parse_ep_number(&a.number);
                let num_b = crate::db::episodes::parse_ep_number(&b.number);
                match (num_a, num_b) {
                    (Some(va), Some(vb)) => va.partial_cmp(&vb).unwrap_or(std::cmp::Ordering::Equal),
                    (Some(_), None) => std::cmp::Ordering::Less,
                    (None, Some(_)) => std::cmp::Ordering::Greater,
                    (None, None) => a.number.cmp(&b.number),
                }
                .then_with(|| a.id.cmp(&b.id))
            });
            let remaining = episodes.len();
            show_groups.push(ShowGroup {
                series,
                remaining,
                episodes,
            });
        }

        show_groups.sort_by(|a, b| {
            let rem_cmp = match sort {
                PendingSort::RemainingAsc => a.remaining.cmp(&b.remaining),
                PendingSort::RemainingDesc => b.remaining.cmp(&a.remaining),
            };
            rem_cmp
                .then_with(|| a.series.title.cmp(&b.series.title))
                .then_with(|| a.series.id.cmp(&b.series.id))
        });

        let mut out = Vec::new();
        for group in show_groups {
            for ep in group.episodes {
                out.push((group.series.clone(), ep));
            }
        }

        Ok(out)
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
            id: 0, series_id: c2b, number: "1".into(), title: None, url: "u/1".into(), released_at: None, seen: false,
            download_path: None, download_status: None,
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
    fn shows_needing_episode_source_releasing_behind_ani_list_is_candidate() {
        let db = Db::open(":memory:").unwrap();
        let src_a = db.upsert_source("AnimeYT", "https://a.example", "animeytx").unwrap();
        let now = 10_000_000;

        let mk = |slug: &str, title: &str, url: &str| crate::models::Series {
            id: 0, slug: slug.into(), title: title.into(), url: url.into(),
            cover_url: None, is_airing: true, followed: false, next_episode_at: None, site_episode_count: None,
        };

        // Catalog: RELEASING, next_episode = 6 (expected 5)
        db.upsert_catalog_anime(&crate::db::test_support::catalog_anime(100, "Airing Show", &[]), 0).unwrap();
        db.conn.execute(
            "UPDATE anilist_catalog SET status = 'RELEASING', next_episode = 6 WHERE id = 100",
            [],
        ).unwrap();

        // Series on site A, followed, linked to 100
        let s_a = db.upsert_series(src_a, &mk("airing-show", "Airing Show", "https://a.example/tv/airing-show/")).unwrap();
        db.set_anilist_id(s_a, 100).unwrap();
        db.set_followed(s_a, true).unwrap();

        // Episodes 1 to 4 on site A (highest is 4 < 5)
        for ep in 1..=4 {
            db.insert_episode(&crate::models::Episode {
                id: 0, series_id: s_a, number: ep.to_string(), title: None,
                url: format!("https://a.example/ep-{ep}"), released_at: None, seen: false,
                download_path: None, download_status: None,
            }).unwrap();
        }

        let candidates = db.shows_needing_episode_source(src_a, now).unwrap();
        assert_eq!(candidates.len(), 1);
        let c = &candidates[0];
        assert_eq!(c.canon_key, "al:100");
        assert_eq!(c.anilist_id, Some(100));
        assert_eq!(c.title, "Airing Show");
        assert_eq!(c.known_rows, vec![(s_a, "animeytx".to_string(), "https://a.example/tv/airing-show/".to_string())]);
    }

    #[test]
    fn shows_needing_episode_source_caught_up_is_not_candidate() {
        let db = Db::open(":memory:").unwrap();
        let src_a = db.upsert_source("AnimeYT", "https://a.example", "animeytx").unwrap();
        let now = 10_000_000;

        let mk = |slug: &str, title: &str| crate::models::Series {
            id: 0, slug: slug.into(), title: title.into(), url: format!("u-{slug}"),
            cover_url: None, is_airing: true, followed: false, next_episode_at: None, site_episode_count: None,
        };

        db.upsert_catalog_anime(&crate::db::test_support::catalog_anime(100, "Airing Show", &[]), 0).unwrap();
        db.conn.execute(
            "UPDATE anilist_catalog SET status = 'RELEASING', next_episode = 6 WHERE id = 100",
            [],
        ).unwrap();

        let s_a = db.upsert_series(src_a, &mk("airing-show", "Airing Show")).unwrap();
        db.set_anilist_id(s_a, 100).unwrap();
        db.set_followed(s_a, true).unwrap();

        // Episodes 1 to 5 on A (5 == expected 5)
        for ep in 1..=5 {
            db.insert_episode(&crate::models::Episode {
                id: 0, series_id: s_a, number: ep.to_string(), title: None,
                url: format!("https://a.example/ep-{ep}"), released_at: None, seen: false,
                download_path: None, download_status: None,
            }).unwrap();
        }

        let candidates = db.shows_needing_episode_source(src_a, now).unwrap();
        assert!(candidates.is_empty(), "caught-up show (ep 5 == expected 5) must not be candidate");
    }

    #[test]
    fn shows_needing_episode_source_finished_or_null_next_with_episodes_not_candidate() {
        let db = Db::open(":memory:").unwrap();
        let src_a = db.upsert_source("AnimeYT", "https://a.example", "animeytx").unwrap();
        let now = 10_000_000;

        let mk = |slug: &str, title: &str| crate::models::Series {
            id: 0, slug: slug.into(), title: title.into(), url: format!("u-{slug}"),
            cover_url: None, is_airing: true, followed: false, next_episode_at: None, site_episode_count: None,
        };

        // FINISHED show with episodes
        db.upsert_catalog_anime(&crate::db::test_support::catalog_anime(201, "Finished Show", &[]), 0).unwrap();
        db.conn.execute(
            "UPDATE anilist_catalog SET status = 'FINISHED', next_episode = 6 WHERE id = 201",
            [],
        ).unwrap();
        let s1 = db.upsert_series(src_a, &mk("finished-show", "Finished Show")).unwrap();
        db.set_anilist_id(s1, 201).unwrap();
        db.set_followed(s1, true).unwrap();
        db.insert_episode(&crate::models::Episode {
            id: 0, series_id: s1, number: "1".into(), title: None,
            url: "u/1".into(), released_at: None, seen: false, download_path: None, download_status: None,
        }).unwrap();

        // RELEASING show with next_episode NULL and episodes present
        db.upsert_catalog_anime(&crate::db::test_support::catalog_anime(202, "Null Next Show", &[]), 0).unwrap();
        db.conn.execute(
            "UPDATE anilist_catalog SET status = 'RELEASING', next_episode = NULL WHERE id = 202",
            [],
        ).unwrap();
        let s2 = db.upsert_series(src_a, &mk("null-next-show", "Null Next Show")).unwrap();
        db.set_anilist_id(s2, 202).unwrap();
        db.set_followed(s2, true).unwrap();
        db.insert_episode(&crate::models::Episode {
            id: 0, series_id: s2, number: "1".into(), title: None,
            url: "u/2".into(), released_at: None, seen: false, download_path: None, download_status: None,
        }).unwrap();

        let candidates = db.shows_needing_episode_source(src_a, now).unwrap();
        assert!(candidates.is_empty(), "FINISHED with eps and NULL next_episode with eps must not be candidates");
    }

    #[test]
    fn shows_needing_episode_source_fresh_group_miss_marker_skips_candidate() {
        let db = Db::open(":memory:").unwrap();
        let src_a = db.upsert_source("AnimeYT", "https://a.example", "animeytx").unwrap();
        let now = 10_000_000;

        let mk = |slug: &str, title: &str| crate::models::Series {
            id: 0, slug: slug.into(), title: title.into(), url: format!("u-{slug}"),
            cover_url: None, is_airing: true, followed: false, next_episode_at: None, site_episode_count: None,
        };

        db.upsert_catalog_anime(&crate::db::test_support::catalog_anime(300, "Miss Show", &[]), 0).unwrap();
        db.conn.execute(
            "UPDATE anilist_catalog SET status = 'RELEASING', next_episode = 6 WHERE id = 300",
            [],
        ).unwrap();
        let s = db.upsert_series(src_a, &mk("miss-show", "Miss Show")).unwrap();
        db.set_anilist_id(s, 300).unwrap();
        db.set_followed(s, true).unwrap();
        db.insert_episode(&crate::models::Episode {
            id: 0, series_id: s, number: "1".into(), title: None,
            url: "u/1".into(), released_at: None, seen: false, download_path: None, download_status: None,
        }).unwrap();

        db.record_episode_fallback_miss("al:300", now - 3600).unwrap();

        let candidates = db.shows_needing_episode_source(src_a, now).unwrap();
        assert!(candidates.is_empty(), "show with fresh miss marker must not be candidate");
    }

    #[test]
    fn shows_needing_episode_source_zero_episodes_old_behaviour_intact() {
        let db = Db::open(":memory:").unwrap();
        let src_a = db.upsert_source("AnimeYT", "https://a.example", "animeytx").unwrap();
        let now = 10_000_000;

        let mk = |slug: &str, title: &str| crate::models::Series {
            id: 0, slug: slug.into(), title: title.into(), url: format!("u-{slug}"),
            cover_url: None, is_airing: true, followed: false, next_episode_at: None, site_episode_count: None,
        };

        db.upsert_catalog_anime(&crate::db::test_support::catalog_anime(400, "Zero Eps Show", &[]), 0).unwrap();
        db.conn.execute(
            "UPDATE anilist_catalog SET status = 'FINISHED' WHERE id = 400",
            [],
        ).unwrap();
        let s = db.upsert_series(src_a, &mk("zero-eps", "Zero Eps Show")).unwrap();
        db.set_anilist_id(s, 400).unwrap();
        db.set_followed(s, true).unwrap();

        let candidates = db.shows_needing_episode_source(src_a, now).unwrap();
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].canon_key, "al:400");
        assert!(candidates[0].known_rows.is_empty(), "old behaviour: empty known_rows");
    }

    #[test]
    fn shows_needing_episode_source_unfollowed_or_watched_externally_never_candidate() {
        let db = Db::open(":memory:").unwrap();
        let src_a = db.upsert_source("AnimeYT", "https://a.example", "animeytx").unwrap();
        let now = 10_000_000;

        let mk = |slug: &str, title: &str| crate::models::Series {
            id: 0, slug: slug.into(), title: title.into(), url: format!("u-{slug}"),
            cover_url: None, is_airing: true, followed: false, next_episode_at: None, site_episode_count: None,
        };

        // Unfollowed behind show
        db.upsert_catalog_anime(&crate::db::test_support::catalog_anime(501, "Unfollowed", &[]), 0).unwrap();
        db.conn.execute(
            "UPDATE anilist_catalog SET status = 'RELEASING', next_episode = 6 WHERE id = 501",
            [],
        ).unwrap();
        let s1 = db.upsert_series(src_a, &mk("unfollowed", "Unfollowed")).unwrap();
        db.set_anilist_id(s1, 501).unwrap();
        db.insert_episode(&crate::models::Episode {
            id: 0, series_id: s1, number: "1".into(), title: None,
            url: "u/1".into(), released_at: None, seen: false, download_path: None, download_status: None,
        }).unwrap();

        // Followed but watched_externally show
        db.upsert_catalog_anime(&crate::db::test_support::catalog_anime(502, "Watched Ext", &[]), 0).unwrap();
        db.conn.execute(
            "UPDATE anilist_catalog SET status = 'RELEASING', next_episode = 6 WHERE id = 502",
            [],
        ).unwrap();
        let s2 = db.upsert_series(src_a, &mk("watched-ext", "Watched Ext")).unwrap();
        db.set_anilist_id(s2, 502).unwrap();
        db.set_followed(s2, true).unwrap();
        db.conn.execute("UPDATE series SET watched_externally = 1 WHERE id = ?1", [s2]).unwrap();
        db.insert_episode(&crate::models::Episode {
            id: 0, series_id: s2, number: "1".into(), title: None,
            url: "u/2".into(), released_at: None, seen: false, download_path: None, download_status: None,
        }).unwrap();

        let candidates = db.shows_needing_episode_source(src_a, now).unwrap();
        assert!(candidates.is_empty(), "unfollowed or watched_externally must never be a candidate");
    }

    #[test]
    fn list_airing_orders_newest_first_nulls_last_title_tiebreak() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
        let base = chrono::Utc::now().timestamp() - 5 * 86400;
        db.upsert_series(src, &mk_airing("older", "Older", Some(base))).unwrap();
        db.upsert_series(src, &mk_airing("newer", "Newer", Some(base + 86400))).unwrap();
        db.upsert_series(src, &mk_airing("nodate", "NoDate", None)).unwrap();
        db.upsert_series(src, &mk_airing("tie-b", "Tie B", Some(base))).unwrap();

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
            url: "u/1".into(), released_at: None, seen: false, download_path: None, download_status: None,
        }).unwrap();

        // 2. show only on another site
        let only_other = db.upsert_series(src_b, &mk("only-b", "Only Other")).unwrap();
        db.set_followed(only_other, true).unwrap();
        db.insert_episode(&crate::models::Episode {
            id: 0, series_id: only_other, number: "1".into(), title: None,
            url: "u/1".into(), released_at: None, seen: false, download_path: None, download_status: None,
        }).unwrap();

        // 3. show on two other sites (the one with more episodes wins)
        let two_b = db.upsert_series(src_b, &mk("two-b", "Two Others")).unwrap();
        db.set_anilist_id(two_b, 100).unwrap();
        db.set_followed(two_b, true).unwrap();
        db.insert_episode(&crate::models::Episode { id: 0, series_id: two_b, number: "1".into(), title: None, url: "u/1".into(), released_at: None, seen: false, download_path: None, download_status: None }).unwrap();

        let two_c = db.upsert_series(src_c, &mk("two-c", "Two Others")).unwrap();
        db.set_anilist_id(two_c, 100).unwrap();
        // C has 2 episodes, so C wins
        db.insert_episode(&crate::models::Episode { id: 0, series_id: two_c, number: "1".into(), title: None, url: "u/1".into(), released_at: None, seen: false, download_path: None, download_status: None }).unwrap();
        db.insert_episode(&crate::models::Episode { id: 0, series_id: two_c, number: "2".into(), title: None, url: "u/2".into(), released_at: None, seen: false, download_path: None, download_status: None }).unwrap();

        // 4. show on active site with 0 eps, but another site has eps
        let active_zero = db.upsert_series(src_a, &mk("zero-a", "Active Zero")).unwrap();
        db.set_anilist_id(active_zero, 200).unwrap();
        db.set_followed(active_zero, true).unwrap();

        let zero_b = db.upsert_series(src_b, &mk("zero-b", "Active Zero")).unwrap();
        db.set_anilist_id(zero_b, 200).unwrap();
        db.insert_episode(&crate::models::Episode { id: 0, series_id: zero_b, number: "1".into(), title: None, url: "u/1".into(), released_at: None, seen: false, download_path: None, download_status: None }).unwrap();

        // 5. unfollowed rows never appear
        let unfollowed = db.upsert_series(src_b, &mk("unf", "Unfollowed")).unwrap();
        db.insert_episode(&crate::models::Episode { id: 0, series_id: unfollowed, number: "1".into(), title: None, url: "u/1".into(), released_at: None, seen: false, download_path: None, download_status: None }).unwrap();

        // Expected for active source (A), merged per episode number across sites:
        // Active Has Eps: 1 (A)
        // Only Other: 1 (B)
        // Two Others: 2 (episode 1 from B, the lowest id on a priority tie; episode 2 only exists on C)
        // Active Zero: 1 (B, because A has none)
        // Total pending count = 1 + 1 + 2 + 1 = 5.
        assert_eq!(db.pending_count(src_a, &[]).unwrap(), 5);

        let pending = db.list_pending(src_a, PendingSort::RemainingAsc, &[]).unwrap();
        assert_eq!(pending.len(), 5);
        // The episode keeps its own row; the series is the group's representative.
        let owners: Vec<i64> = pending.iter().map(|(_, e)| e.series_id).collect();
        assert!(owners.contains(&active_has_eps));
        assert!(owners.contains(&only_other));
        assert!(owners.contains(&two_c), "episode 2 only exists on the third site");
        assert!(owners.contains(&two_b), "episode 1 comes from the lowest id on a tie");
        assert!(owners.contains(&zero_b));
        assert!(!owners.contains(&active_zero));
        assert!(!owners.contains(&unfollowed));
        let two_others: Vec<&str> = pending
            .iter()
            .filter(|(s, _)| s.title == "Two Others")
            .map(|(_, e)| e.number.as_str())
            .collect();
        assert_eq!(two_others, vec!["1", "2"], "one entry per episode number");
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
            url: "few/1".into(), released_at: None, seen: false, download_path: None, download_status: None,
        }).unwrap();
        for n in 1..=3 {
            db.insert_episode(&crate::models::Episode {
                id: 0, series_id: many, number: n.to_string(), title: None,
                url: format!("many/{n}"), released_at: None, seen: false, download_path: None, download_status: None,
            }).unwrap();
        }

        // Ascending: the 1-episode series' rows come before the 3-episode one.
        let asc = db.list_pending(src, PendingSort::RemainingAsc, &[]).unwrap();
        assert_eq!(asc.first().unwrap().0.id, few);
        assert_eq!(asc.last().unwrap().0.id, many);
        // Descending: reversed.
        let desc = db.list_pending(src, PendingSort::RemainingDesc, &[]).unwrap();
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
                url: format!("show/{n}"), released_at: None, seen: false, download_path: None, download_status: None,
            }).unwrap();
        }

        let pending = db.list_pending(src, PendingSort::RemainingAsc, &[]).unwrap();
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

    fn set_catalog_format(db: &Db, id: i64, format: &str) {
        db.conn.execute("UPDATE anilist_catalog SET format = ?1 WHERE id = ?2", (format, id)).unwrap();
    }

    fn airing_titles(db: &Db, source: i64) -> Vec<String> {
        db.list_airing(source).unwrap().into_iter().map(|s| s.title).collect()
    }

    #[test]
    fn list_airing_hides_unfollowed_ova_without_an_upcoming_episode() {
        let db = Db::open(":memory:").unwrap();
        let a = db.upsert_source("AnimeYT", "https://a", "animeytx").unwrap();
        db.upsert_catalog_anime(&crate::db::test_support::catalog_anime(1, "Noble Rose", &[]), 0).unwrap();
        set_catalog_format(&db, 1, "OVA");
        let sid = db.upsert_series(a, &mk_airing("nr", "Noble Rose", None)).unwrap();
        db.set_anilist_id(sid, 1).unwrap();
        assert!(airing_titles(&db, a).is_empty(), "OVA with no future catalog airing is hidden");

        db.set_followed(sid, true).unwrap();
        assert_eq!(airing_titles(&db, a), vec!["Noble Rose".to_string()], "followed stays");
    }

    #[test]
    fn list_airing_keeps_an_ova_that_has_a_future_catalog_airing() {
        let db = Db::open(":memory:").unwrap();
        let a = db.upsert_source("AnimeYT", "https://a", "animeytx").unwrap();
        db.upsert_catalog_anime(&crate::db::test_support::catalog_anime(1, "Noble Rose", &[]), 0).unwrap();
        set_catalog_format(&db, 1, "OVA");
        let now = chrono::Utc::now().timestamp();
        db.conn.execute("UPDATE anilist_catalog SET next_airing_at = ?1 WHERE id = 1", [now + 86400]).unwrap();
        let sid = db.upsert_series(a, &mk_airing("nr", "Noble Rose", None)).unwrap();
        db.set_anilist_id(sid, 1).unwrap();
        assert_eq!(airing_titles(&db, a).len(), 1);
    }

    #[test]
    fn list_airing_hides_unlinked_specials_and_stale_unfollowed_rows() {
        let db = Db::open(":memory:").unwrap();
        let a = db.upsert_source("AnimeYT", "https://a", "animeytx").unwrap();
        let now = chrono::Utc::now().timestamp();
        let day = 86400;
        db.upsert_series(a, &mk_airing("sp", "Slime 3rd Season Specials", Some(now - day))).unwrap();
        db.upsert_series(a, &mk_airing("stale", "Stale Show", Some(now - 30 * day))).unwrap();
        db.upsert_series(a, &mk_airing("fresh", "Fresh Show", Some(now - 6 * day))).unwrap();
        db.upsert_series(a, &mk_airing("nodate", "Undated Show", None)).unwrap();
        let followed = db.upsert_series(a, &mk_airing("fstale", "Followed Stale", Some(now - 60 * day))).unwrap();
        db.set_followed(followed, true).unwrap();

        let mut titles = airing_titles(&db, a);
        titles.sort();
        assert_eq!(titles, vec!["Followed Stale", "Fresh Show", "Undated Show"]);
    }

    #[test]
    fn list_airing_keeps_a_stale_scraped_row_when_the_catalog_says_releasing() {
        let db = Db::open(":memory:").unwrap();
        let a = db.upsert_source("AnimeYT", "https://a", "animeytx").unwrap();
        let now = chrono::Utc::now().timestamp();
        let mut anime = crate::db::test_support::catalog_anime(3, "Biweekly Show", &[]);
        anime.status = Some("RELEASING".into());
        db.upsert_catalog_anime(&anime, 0).unwrap();
        let sid = db.upsert_series(a, &mk_airing("bw", "Biweekly Show", Some(now - 40 * 86400))).unwrap();
        db.set_anilist_id(sid, 3).unwrap();
        assert_eq!(airing_titles(&db, a).len(), 1);
    }

    #[test]
    fn list_airing_keeps_a_stale_scraped_row_when_the_catalog_has_a_future_airing() {
        let db = Db::open(":memory:").unwrap();
        let a = db.upsert_source("AnimeYT", "https://a", "animeytx").unwrap();
        let now = chrono::Utc::now().timestamp();
        db.upsert_catalog_anime(&crate::db::test_support::catalog_anime(2, "Slow Show", &[]), 0).unwrap();
        db.conn.execute("UPDATE anilist_catalog SET next_airing_at = ?1 WHERE id = 2", [now + 3 * 86400]).unwrap();
        let sid = db.upsert_series(a, &mk_airing("slow", "Slow Show", Some(now - 40 * 86400))).unwrap();
        db.set_anilist_id(sid, 2).unwrap();
        assert_eq!(airing_titles(&db, a).len(), 1);
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
            id: 0, series_id: id_b, number: "1".into(), title: None, url: "e1".into(), released_at: None, seen: false,
            download_path: None, download_status: None,
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
            download_path: None, download_status: None,
        }).unwrap();

        let row_b = db.upsert_series(src_b, &mk("oth", "Show")).unwrap();
        db.set_followed(row_b, true).unwrap();
        db.insert_episode(&crate::models::Episode {
            id: 0, series_id: row_b, number: "1".into(), title: None,
            url: "u/1b".into(), released_at: None, seen: false, // UNSEEN!
            download_path: None, download_status: None,
        }).unwrap();

        // Active site has episodes, so it stays authoritative. Since its eps are seen, pending_count is 0.
        let priority = vec!["animeytx".to_string(), "tioanime".to_string()];
        assert_eq!(db.pending_count(src_a, &priority).unwrap(), 0);
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
            download_path: None, download_status: None,
        }).unwrap();

        let priority = vec!["animeytx".to_string()];
        assert_eq!(db.pending_count(src_a, &priority).unwrap(), 0, "watched_externally should yield 0 pending");
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
            download_path: None, download_status: None,
        }).unwrap();

        // A has NO episodes but IS followed.
        let row_a = db.upsert_series(src_a, &mk("act", "Show")).unwrap();
        db.set_followed(row_a, true).unwrap(); // Marks canonical group followed!

        let priority = vec!["animeytx".to_string(), "tioanime".to_string()];
        // Since the canonical group is followed, and A has no episodes, it falls back to B.
        assert_eq!(db.pending_count(src_a, &priority).unwrap(), 1);
        let pending = db.list_pending(src_a, PendingSort::RemainingAsc, &priority).unwrap();
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
            download_path: None, download_status: None,
        }).unwrap();
        db.insert_episode(&crate::models::Episode {
            id: 0, series_id: row, number: "2".into(), title: None,
            url: "u/2".into(), released_at: None, seen: false,
            download_path: None, download_status: None,
        }).unwrap();

        let priority = vec!["animeytx".to_string()];
        let count = db.pending_count(src_a, &priority).unwrap();
        let pending = db.list_pending(src_a, PendingSort::RemainingAsc, &priority).unwrap();
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

        let priority = vec!["animeytx".to_string(), "tioanime".to_string()];
        let start = std::time::Instant::now();
        let count = db.pending_count(src_a, &priority).unwrap();
        let elapsed = start.elapsed();

        let start2 = std::time::Instant::now();
        let list = db.list_pending(src_a, PendingSort::RemainingAsc, &priority).unwrap();
        let elapsed2 = start2.elapsed();

        assert!(elapsed.as_millis() < 1500, "pending_count took (debug build, limit 1500) {} ms", elapsed.as_millis());
        assert!(elapsed2.as_millis() < 1500, "list_pending took {} ms", elapsed2.as_millis());
        assert_eq!(count as usize, list.len());
    }

    #[test]
    fn list_pending_prefers_catalog_title_and_cover_for_linked_series() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "https://a.example", "animeytx").unwrap();

        db.upsert_catalog_anime(&catalog_anime(1001, "As a Reincarnated Aristocrat", &[]), 0).unwrap();

        let s = crate::models::Series {
            id: 0,
            slug: "tensei-kizoku".into(),
            title: "Tensei Kizoku".into(),
            url: "https://site/tensei-kizoku".into(),
            cover_url: Some("file:C:/covers/tensei.jpg".into()),
            is_airing: true,
            followed: true,
            next_episode_at: None,
            site_episode_count: None,
        };
        let sid = db.upsert_series(src, &s).unwrap();
        db.set_followed(sid, true).unwrap();
        db.set_anilist_id(sid, 1001).unwrap();
        db.insert_episode(&crate::models::Episode {
            id: 0,
            series_id: sid,
            number: "1".into(),
            title: None,
            url: "https://site/tensei-kizoku/1".into(),
            released_at: None,
            seen: false,
            download_path: None,
            download_status: None,
        }).unwrap();

        let priority = vec!["animeytx".to_string()];
        let pending = db.list_pending(src, PendingSort::RemainingAsc, &priority).unwrap();
        assert_eq!(pending.len(), 1);
        let (series, _) = &pending[0];
        assert_eq!(series.title, "As a Reincarnated Aristocrat");
        assert_eq!(series.cover_url.as_deref(), Some("https://cdn/1001.jpg"));
    }

    #[test]
    fn list_pending_keeps_site_title_and_cover_for_unlinked_series() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "https://a.example", "animeytx").unwrap();

        let s = crate::models::Series {
            id: 0,
            slug: "unlinked-show".into(),
            title: "Unlinked Site Title".into(),
            url: "https://site/unlinked".into(),
            cover_url: Some("file:C:/covers/unlinked.jpg".into()),
            is_airing: true,
            followed: true,
            next_episode_at: None,
            site_episode_count: None,
        };
        let sid = db.upsert_series(src, &s).unwrap();
        db.set_followed(sid, true).unwrap();
        db.insert_episode(&crate::models::Episode {
            id: 0,
            series_id: sid,
            number: "1".into(),
            title: None,
            url: "https://site/unlinked/1".into(),
            released_at: None,
            seen: false,
            download_path: None,
            download_status: None,
        }).unwrap();

        let priority = vec!["animeytx".to_string()];
        let pending = db.list_pending(src, PendingSort::RemainingAsc, &priority).unwrap();
        assert_eq!(pending.len(), 1);
        let (series, _) = &pending[0];
        assert_eq!(series.title, "Unlinked Site Title");
        assert_eq!(series.cover_url.as_deref(), Some("file:C:/covers/unlinked.jpg"));
    }

    #[test]
    fn list_pending_linked_series_with_null_catalog_cover_falls_back_to_site_cover() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "https://a.example", "animeytx").unwrap();

        let mut cat = catalog_anime(1002, "Catalog Title", &[]);
        cat.cover_url = None;
        db.upsert_catalog_anime(&cat, 0).unwrap();

        let s = crate::models::Series {
            id: 0,
            slug: "null-cat-cover".into(),
            title: "Site Title".into(),
            url: "https://site/null-cat-cover".into(),
            cover_url: Some("file:C:/covers/site.jpg".into()),
            is_airing: true,
            followed: true,
            next_episode_at: None,
            site_episode_count: None,
        };
        let sid = db.upsert_series(src, &s).unwrap();
        db.set_followed(sid, true).unwrap();
        db.set_anilist_id(sid, 1002).unwrap();
        db.insert_episode(&crate::models::Episode {
            id: 0,
            series_id: sid,
            number: "1".into(),
            title: None,
            url: "https://site/null-cat-cover/1".into(),
            released_at: None,
            seen: false,
            download_path: None,
            download_status: None,
        }).unwrap();

        let priority = vec!["animeytx".to_string()];
        let pending = db.list_pending(src, PendingSort::RemainingAsc, &priority).unwrap();
        assert_eq!(pending.len(), 1);
        let (series, _) = &pending[0];
        assert_eq!(series.title, "Catalog Title");
        assert_eq!(series.cover_url.as_deref(), Some("file:C:/covers/site.jpg"));
    }

    #[test]
    fn merged_pending_priority_and_link_selection() {
        let db = Db::open(":memory:").unwrap();
        let src_a = db.upsert_source("AnimeYT", "https://a.example", "animeytx").unwrap();
        let src_b = db.upsert_source("TioAnime", "https://b.example", "tioanime").unwrap();

        let mk = |slug: &str, title: &str| crate::models::Series {
            id: 0, slug: slug.into(), title: title.into(), url: format!("https://site/{slug}"),
            cover_url: None, is_airing: true, followed: true, next_episode_at: None, site_episode_count: None,
        };

        let row_a = db.upsert_series(src_a, &mk("act", "Show")).unwrap();
        db.set_followed(row_a, true).unwrap();
        for i in 1..=3 {
            db.insert_episode(&crate::models::Episode {
                id: 0, series_id: row_a, number: i.to_string(), title: None,
                url: format!("https://a/{i}"), released_at: None, seen: false,
                download_path: None, download_status: None,
            }).unwrap();
        }

        let row_b = db.upsert_series(src_b, &mk("oth", "Show")).unwrap();
        db.set_followed(row_b, true).unwrap();
        for i in 1..=4 {
            db.insert_episode(&crate::models::Episode {
                id: 0, series_id: row_b, number: i.to_string(), title: None,
                url: format!("https://b/{i}"), released_at: None, seen: false,
                download_path: None, download_status: None,
            }).unwrap();
        }

        // Priority [A, B]: pending = A1, A2, A3, B4 under representative A
        let p_ab = vec!["animeytx".to_string(), "tioanime".to_string()];
        let pending_ab = db.list_pending(src_a, PendingSort::RemainingAsc, &p_ab).unwrap();
        assert_eq!(pending_ab.len(), 4);
        assert_eq!(pending_ab[0].0.id, row_a); // representative is A
        assert_eq!(pending_ab[1].0.id, row_a);
        assert_eq!(pending_ab[2].0.id, row_a);
        assert_eq!(pending_ab[3].0.id, row_a);

        assert_eq!(pending_ab[0].1.url, "https://a/1");
        assert_eq!(pending_ab[1].1.url, "https://a/2");
        assert_eq!(pending_ab[2].1.url, "https://a/3");
        assert_eq!(pending_ab[3].1.url, "https://b/4");

        // Priority [B, A]: pending = B1, B2, B3, B4 under representative B
        let p_ba = vec!["tioanime".to_string(), "animeytx".to_string()];
        let pending_ba = db.list_pending(src_a, PendingSort::RemainingAsc, &p_ba).unwrap();
        assert_eq!(pending_ba.len(), 4);
        assert_eq!(pending_ba[0].0.id, row_b); // representative is B
        assert_eq!(pending_ba[0].1.url, "https://b/1");
        assert_eq!(pending_ba[1].1.url, "https://b/2");
        assert_eq!(pending_ba[2].1.url, "https://b/3");
        assert_eq!(pending_ba[3].1.url, "https://b/4");
    }

    #[test]
    fn merged_pending_with_seen_watermark() {
        let db = Db::open(":memory:").unwrap();
        let src_a = db.upsert_source("AnimeYT", "https://a.example", "animeytx").unwrap();
        let src_b = db.upsert_source("TioAnime", "https://b.example", "tioanime").unwrap();

        let mk = |slug: &str, title: &str| crate::models::Series {
            id: 0, slug: slug.into(), title: title.into(), url: format!("https://site/{slug}"),
            cover_url: None, is_airing: true, followed: true, next_episode_at: None, site_episode_count: None,
        };

        let row_a = db.upsert_series(src_a, &mk("act", "Show")).unwrap();
        db.set_followed(row_a, true).unwrap();
        for i in 1..=3 {
            db.insert_episode(&crate::models::Episode {
                id: 0, series_id: row_a, number: i.to_string(), title: None,
                url: format!("https://a/{i}"), released_at: None, seen: i <= 2, // 1 and 2 seen!
                download_path: None, download_status: None,
            }).unwrap();
        }

        let row_b = db.upsert_series(src_b, &mk("oth", "Show")).unwrap();
        db.set_followed(row_b, true).unwrap();
        for i in 1..=4 {
            db.insert_episode(&crate::models::Episode {
                id: 0, series_id: row_b, number: i.to_string(), title: None,
                url: format!("https://b/{i}"), released_at: None, seen: false, // unseen on B!
                download_path: None, download_status: None,
            }).unwrap();
        }

        // Priority [A, B]: user has seen 1-2 (on A).
        // Pending must be 3 (from A) and 4 (from B).
        // Neither 1 nor 2 should reappear even though unseen on B!
        let priority = vec!["animeytx".to_string(), "tioanime".to_string()];
        let pending = db.list_pending(src_a, PendingSort::RemainingAsc, &priority).unwrap();
        assert_eq!(pending.len(), 2);
        assert_eq!(pending[0].1.number, "3");
        assert_eq!(pending[0].1.url, "https://a/3");
        assert_eq!(pending[1].1.number, "4");
        assert_eq!(pending[1].1.url, "https://b/4");
    }

    #[test]
    fn merged_pending_only_other_site_has_episodes() {
        let db = Db::open(":memory:").unwrap();
        let src_a = db.upsert_source("AnimeYT", "https://a.example", "animeytx").unwrap();
        let src_b = db.upsert_source("TioAnime", "https://b.example", "tioanime").unwrap();

        let mk = |slug: &str, title: &str| crate::models::Series {
            id: 0, slug: slug.into(), title: title.into(), url: format!("https://site/{slug}"),
            cover_url: None, is_airing: true, followed: true, next_episode_at: None, site_episode_count: None,
        };

        let row_a = db.upsert_series(src_a, &mk("act", "Show")).unwrap();
        db.set_followed(row_a, true).unwrap();

        let row_b = db.upsert_series(src_b, &mk("oth", "Show")).unwrap();
        db.set_followed(row_b, true).unwrap();
        db.insert_episode(&crate::models::Episode {
            id: 0, series_id: row_b, number: "1".into(), title: None,
            url: "https://b/1".into(), released_at: None, seen: false,
            download_path: None, download_status: None,
        }).unwrap();

        let priority = vec!["animeytx".to_string(), "tioanime".to_string()];
        let pending = db.list_pending(src_a, PendingSort::RemainingAsc, &priority).unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].0.id, row_b);
        assert_eq!(pending[0].1.url, "https://b/1");
    }

    #[test]
    fn missing_site_in_priority_ranks_last() {
        let db = Db::open(":memory:").unwrap();
        let src_a = db.upsert_source("AnimeYT", "https://a.example", "animeytx").unwrap();
        let src_b = db.upsert_source("Other", "https://b.example", "unknown_site").unwrap();

        let mk = |slug: &str, title: &str| crate::models::Series {
            id: 0, slug: slug.into(), title: title.into(), url: format!("https://site/{slug}"),
            cover_url: None, is_airing: true, followed: true, next_episode_at: None, site_episode_count: None,
        };

        let row_a = db.upsert_series(src_a, &mk("act", "Show")).unwrap();
        db.set_followed(row_a, true).unwrap();
        db.insert_episode(&crate::models::Episode {
            id: 0, series_id: row_a, number: "1".into(), title: None,
            url: "https://a/1".into(), released_at: None, seen: false,
            download_path: None, download_status: None,
        }).unwrap();

        let row_b = db.upsert_series(src_b, &mk("oth", "Show")).unwrap();
        db.set_followed(row_b, true).unwrap();
        db.insert_episode(&crate::models::Episode {
            id: 0, series_id: row_b, number: "1".into(), title: None,
            url: "https://b/1".into(), released_at: None, seen: false,
            download_path: None, download_status: None,
        }).unwrap();

        // Priority only contains animeytx. unknown_site is missing, so ranks last.
        let priority = vec!["animeytx".to_string()];
        let pending = db.list_pending(src_a, PendingSort::RemainingAsc, &priority).unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].1.url, "https://a/1");
    }

    #[test]
    fn no_duplicate_episode_numbers() {
        let db = Db::open(":memory:").unwrap();
        let src_a = db.upsert_source("AnimeYT", "https://a.example", "animeytx").unwrap();
        let src_b = db.upsert_source("TioAnime", "https://b.example", "tioanime").unwrap();

        let mk = |slug: &str, title: &str| crate::models::Series {
            id: 0, slug: slug.into(), title: title.into(), url: format!("https://site/{slug}"),
            cover_url: None, is_airing: true, followed: true, next_episode_at: None, site_episode_count: None,
        };

        let row_a = db.upsert_series(src_a, &mk("act", "Show")).unwrap();
        db.set_followed(row_a, true).unwrap();
        db.insert_episode(&crate::models::Episode {
            id: 0, series_id: row_a, number: "1".into(), title: None,
            url: "https://a/1".into(), released_at: None, seen: false,
            download_path: None, download_status: None,
        }).unwrap();

        let row_b = db.upsert_series(src_b, &mk("oth", "Show")).unwrap();
        db.set_followed(row_b, true).unwrap();
        db.insert_episode(&crate::models::Episode {
            id: 0, series_id: row_b, number: "1".into(), title: None,
            url: "https://b/1".into(), released_at: None, seen: false,
            download_path: None, download_status: None,
        }).unwrap();

        let priority = vec!["animeytx".to_string(), "tioanime".to_string()];
        let pending = db.list_pending(src_a, PendingSort::RemainingAsc, &priority).unwrap();
        assert_eq!(pending.len(), 1, "Must never have the same episode number twice");
    }

    #[test]
    fn unfollowed_and_watched_externally_shows_never_appear() {
        let db = Db::open(":memory:").unwrap();
        let src_a = db.upsert_source("AnimeYT", "https://a.example", "animeytx").unwrap();

        let mk = |slug: &str, title: &str| crate::models::Series {
            id: 0, slug: slug.into(), title: title.into(), url: format!("https://site/{slug}"),
            cover_url: None, is_airing: true, followed: false, next_episode_at: None, site_episode_count: None,
        };

        // Unfollowed
        let s_unfollowed = db.upsert_series(src_a, &mk("unf", "Unfollowed")).unwrap();
        db.insert_episode(&crate::models::Episode {
            id: 0, series_id: s_unfollowed, number: "1".into(), title: None,
            url: "https://a/unf/1".into(), released_at: None, seen: false,
            download_path: None, download_status: None,
        }).unwrap();

        // Watched externally
        let s_ext = db.upsert_series(src_a, &mk("ext", "Watched Externally")).unwrap();
        db.set_followed(s_ext, true).unwrap();
        db.conn.execute("UPDATE series SET watched_externally=1 WHERE id=?1", [s_ext]).unwrap();
        db.insert_episode(&crate::models::Episode {
            id: 0, series_id: s_ext, number: "1".into(), title: None,
            url: "https://a/ext/1".into(), released_at: None, seen: false,
            download_path: None, download_status: None,
        }).unwrap();

        let priority = vec!["animeytx".to_string()];
        assert_eq!(db.pending_count(src_a, &priority).unwrap(), 0);
        let pending = db.list_pending(src_a, PendingSort::RemainingAsc, &priority).unwrap();
        assert!(pending.is_empty());
    }

    #[test]
    fn merged_pending_canonical_alignment_tensei_fixture() {
        let db = Db::open(":memory:").unwrap();
        let now = 1_700_000_000;
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
        for i in 1..=27 {
            db.insert_episode(&crate::models::Episode {
                id: 0, series_id: row_b, number: i.to_string(), title: None,
                url: format!("https://tioanime.com/ver/tensei-{i}"), released_at: None, seen: false,
                download_path: None, download_status: None,
            }).unwrap();
        }

        let priority = vec!["animeytx".to_string(), "tioanime".to_string()];
        let pending = db.merged_pending_episodes_at(&priority, now).unwrap();
        assert_eq!(pending.len(), 1);
        let (rep_id, ep_ids) = &pending[0];
        assert_eq!(*rep_id, row_a);
        assert_eq!(ep_ids.len(), 3, "exactly 3 pending episodes (canonical 1..3), all from animeyt");
        for ep_id in ep_ids {
            let ep = db.get_episode(*ep_id).unwrap();
            assert_eq!(ep.series_id, row_a);
        }

        // Now if TioAnime also has "28" (canonical 4, aired later), it appears as the fourth, from TioAnime
        db.insert_episode(&crate::models::Episode {
            id: 0, series_id: row_b, number: "28".into(), title: None,
            url: "https://tioanime.com/ver/tensei-28".into(), released_at: None, seen: false,
            download_path: None, download_status: None,
        }).unwrap();

        let pending2 = db.merged_pending_episodes_at(&priority, now).unwrap();
        assert_eq!(pending2.len(), 1);
        let (rep_id2, ep_ids2) = &pending2[0];
        assert_eq!(*rep_id2, row_a);
        assert_eq!(ep_ids2.len(), 4, "four pending episodes now");
        let ep4 = db.get_episode(ep_ids2[3]).unwrap();
        assert_eq!(ep4.series_id, row_b);
        assert_eq!(ep4.number, "28");
        assert_eq!(ep4.url, "https://tioanime.com/ver/tensei-28");
    }

    #[test]
    fn linked_and_unlinked_same_title_rows_collapse_into_one_airing_card() {
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

        // row_a is linked to AniList 9999
        let row_a = db.upsert_series(src_a, &mk("frieren-a", "Sousou no Frieren")).unwrap();
        db.set_anilist_id(row_a, 9999).unwrap();

        // row_b is unlinked (None) but shares the exact same title
        let _row_b = db.upsert_series(src_b, &mk("frieren-b", "Sousou no Frieren")).unwrap();

        let airing = db.list_airing(src_a).unwrap();
        assert_eq!(airing.len(), 1, "linked and unlinked same-title rows collapse into one airing card");
        assert_eq!(airing[0].title, "Sousou no Frieren");
    }

    #[test]
    fn list_airing_rollover_when_no_scraped_upcoming_date() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "https://a.example", "animeytx").unwrap();
        let now = chrono::Utc::now().timestamp();
        let day = 86400;

        // 1. RELEASING, next_airing_at = now - 2 days, no scraped date
        // -> effective_next == next_airing_at + 7 days
        let mut a1 = crate::db::test_support::catalog_anime(101, "Rolled Show", &[]);
        a1.status = Some("RELEASING".into());
        a1.next_airing_at = Some(now - 2 * day);
        a1.next_episode = Some(5);
        db.upsert_catalog_anime(&a1, 0).unwrap();
        let s1 = db.upsert_series(src, &mk_airing("rolled", "Rolled Show", None)).unwrap();
        db.set_anilist_id(s1, 101).unwrap();

        let airing1 = db.list_airing(src).unwrap();
        assert_eq!(airing1.len(), 1);
        assert_eq!(airing1[0].next_episode_at, Some(now - 2 * day + 7 * day));

        // 2. now - 9 days (>= 7 days ago) -> no rolled date (None)
        db.conn.execute("UPDATE anilist_catalog SET next_airing_at = ?1 WHERE id = 101", [now - 9 * day]).unwrap();
        let airing2 = db.list_airing(src).unwrap();
        assert_eq!(airing2.len(), 1);
        assert_eq!(airing2[0].next_episode_at, None);

        // 3. FINISHED status -> no rolled date (None)
        db.conn.execute(
            "UPDATE anilist_catalog SET status = 'FINISHED', next_airing_at = ?1 WHERE id = 101",
            [now - 2 * day],
        ).unwrap();
        let airing3 = db.list_airing(src).unwrap();
        assert_eq!(airing3.len(), 1);
        assert_eq!(airing3[0].next_episode_at, None);

        // 4. Must NOT happen: catalog date > now is replaced by a rolled date
        db.conn.execute(
            "UPDATE anilist_catalog SET status = 'RELEASING', next_airing_at = ?1 WHERE id = 101",
            [now + 2 * day],
        ).unwrap();
        let airing4 = db.list_airing(src).unwrap();
        assert_eq!(airing4.len(), 1);
        assert_eq!(airing4[0].next_episode_at, Some(now + 2 * day));

        // 5. Must NOT happen: a scraped upcoming date is overridden by the roll-over
        db.conn.execute("UPDATE anilist_catalog SET next_airing_at = ?1 WHERE id = 101", [now - 2 * day]).unwrap();
        db.conn.execute("UPDATE series SET next_episode_at = ?1 WHERE id = ?2", [now + 3 * day, s1]).unwrap();
        let airing5 = db.list_airing(src).unwrap();
        assert_eq!(airing5.len(), 1);
        assert_eq!(airing5[0].next_episode_at, Some(now + 3 * day));

        // 6. catalog_upcoming is false for rolled date: unfollowed OVA is hidden
        db.conn.execute("UPDATE series SET next_episode_at = NULL, followed = 0 WHERE id = ?1", [s1]).unwrap();
        db.conn.execute(
            "UPDATE anilist_catalog SET format = 'OVA', next_airing_at = ?1 WHERE id = 101",
            [now - 2 * day],
        ).unwrap();
        let airing_ova_rolled = db.list_airing(src).unwrap();
        assert!(airing_ova_rolled.is_empty(), "unfollowed OVA with rolled date must stay hidden (catalog_upcoming false)");

        // But with real future catalog date (catalog_upcoming true), unfollowed OVA is shown
        db.conn.execute("UPDATE anilist_catalog SET next_airing_at = ?1 WHERE id = 101", [now + 2 * day]).unwrap();
        let airing_ova_future = db.list_airing(src).unwrap();
        assert_eq!(airing_ova_future.len(), 1, "unfollowed OVA with future date must show (catalog_upcoming true)");
    }

    #[test]
    fn a_rolled_date_confirmed_by_a_scraped_one_is_not_estimated() {
        let db = Db::open(":memory:").unwrap();
        let a = db.upsert_source("AnimeYT", "https://a.example", "animeytx").unwrap();
        let b = db.upsert_source("TioAnime", "https://b.example", "tioanime").unwrap();
        let now = chrono::Utc::now().timestamp();
        let day = 86400;
        let mut cat = crate::db::test_support::catalog_anime(101, "Twin Show", &[]);
        cat.status = Some("RELEASING".into());
        cat.next_airing_at = Some(now - 2 * day);
        cat.next_episode = Some(5);
        db.upsert_catalog_anime(&cat, 0).unwrap();
        // Site A has no date of its own (its date is the +7 d roll); site B scraped exactly that moment.
        let sa = db.upsert_series(a, &crate::db::test_support::mk_airing("twin-a", "Twin Show", None)).unwrap();
        db.set_anilist_id(sa, 101).unwrap();
        let sb = db
            .upsert_series(b, &crate::db::test_support::mk_airing("twin-b", "Twin Show", Some(now + 5 * day)))
            .unwrap();
        db.set_anilist_id(sb, 101).unwrap();

        let (series, rolled) = db.list_airing_with_rolled(a).unwrap();
        assert_eq!(series.len(), 1);
        assert_eq!(series[0].next_episode_at, Some(now + 5 * day));
        assert!(rolled.is_empty(), "a real date confirms the rolled one");
    }

    #[test]
    fn test_list_airing_next_episode_estimated() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "https://a.example", "animeytx").unwrap();
        let now = chrono::Utc::now().timestamp();
        let day = 86400;

        // RELEASING, next_airing_at = now - 2 days, no scraped date -> next_episode_estimated == true
        let mut a1 = crate::db::test_support::catalog_anime(101, "Rolled Show", &[]);
        a1.status = Some("RELEASING".into());
        a1.next_airing_at = Some(now - 2 * day);
        a1.next_episode = Some(5);
        db.upsert_catalog_anime(&a1, 0).unwrap();
        let s1 = db.upsert_series(src, &crate::db::test_support::mk_airing("rolled", "Rolled Show", None)).unwrap();
        db.set_anilist_id(s1, 101).unwrap();

        let (series1, rolled1) = db.list_airing_with_rolled(src).unwrap();
        assert_eq!(series1.len(), 1);
        assert!(rolled1.contains(&s1));

        // Future catalog date -> rolled does not contain s1
        db.conn.execute(
            "UPDATE anilist_catalog SET next_airing_at = ?1 WHERE id = 101",
            [now + 2 * day],
        ).unwrap();
        let (series2, rolled2) = db.list_airing_with_rolled(src).unwrap();
        assert_eq!(series2.len(), 1);
        assert!(!rolled2.contains(&s1));
    }
}
