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
                    c.episodes AS c_episodes,
                    (SELECT MAX(CAST(e.number AS REAL)) FROM episodes e WHERE e.series_id = s.id) AS max_ep,
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
            /// AniList says the linked (same-season) entry is over
            /// (FINISHED/CANCELLED): a site still listing it is replaying it.
            catalog_ended: bool,
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
                let mut catalog_ended = false;
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
                        // ...unless the site already lists an episode past AniList's
                        // total: a split cour on one site page linked to the finished
                        // first cour (site evidence beats the catalog, as in
                        // `sync_status_from_catalog`).
                        let c_episodes: Option<i64> = r.get("c_episodes")?;
                        let site_count: Option<i64> = r.get("site_episode_count")?;
                        let max_ep: Option<f64> = r.get("max_ep")?;
                        let site_max = site_count.map(|n| n as f64).into_iter().chain(max_ep).fold(None, |m: Option<f64>, v| Some(m.map_or(v, |m| m.max(v))));
                        let past_total = matches!((c_episodes, site_max), (Some(total), Some(seen)) if seen > total as f64);
                        catalog_ended = !past_total && matches!(status.as_deref(), Some("FINISHED" | "CANCELLED"));
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
                    catalog_ended,
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
                // AniList is the source of truth: a linked, same-season entry it
                // marks FINISHED/CANCELLED is not airing, whatever date the
                // site's listing carries (sites replay or re-date finished shows).
                let ended = group.iter().any(|m| m.catalog_ended) && !group.iter().any(|m| m.catalog_active);
                if side_content || stale || ended {
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

/// Compute the AniList-derived upper bound on aired canonical episode numbers.
///
/// Returns `Some(cap)` when AniList provides airing information (status, total episodes,
/// upcoming episode countdown, or episode air dates in `anilist_episode_map`), or `None`
/// when no AniList data is available (in which case no cap is applied, preserving existing
/// behaviour).
///
/// Rules:
/// - If no AniList catalog data is present (status, episodes, and next_episode are all None),
///   the episode map alone is not trusted without catalog presence: return `None`.
/// - `next_aired`: if `next_episode` is `Some(n)`, then `n` when `next_airing_at <= now`
///   (stale catalog entry whose air date has passed), else `n - 1`.
/// - If `status == Some("FINISHED")` and `episodes == Some(t)` with `t > 0`:
///   `cap = max(t, map_aired, next_aired)`.
/// - Otherwise: `cap = max(map_aired, next_aired)` when a countdown is known, else `None`
///   (the map alone undercounts when ani.zip lacks dates, so it never sets a cap by itself).
///
/// `now` and `next_airing_at` are unix seconds.
pub(crate) fn anilist_aired_cap(
    status: Option<&str>,
    episodes: Option<i64>,
    next_episode: Option<i64>,
    next_airing_at: Option<i64>,
    map_aired: Option<i64>,
    now: i64,
) -> Option<f64> {
    if status.is_none() && episodes.is_none() && next_episode.is_none() {
        return None;
    }

    // A catalog row whose countdown is already in the past is stale: episode
    // `n` has aired, and if the row was not refreshed for weeks, later ones
    // have too. Assume the usual weekly cadence so a stale row never hides an
    // episode that is really out.
    let next_aired = next_episode.map(|n| match next_airing_at {
        Some(ts) if ts <= now => n + (now - ts) / (7 * 86_400),
        _ => n - 1,
    });

    let known_map = map_aired.filter(|&c| c > 0);
    let known_next = next_aired.filter(|&c| c > 0);

    if status == Some("FINISHED") {
        if let Some(t) = episodes.filter(|&c| c > 0) {
            let mut cap = t;
            if let Some(m) = known_map {
                cap = cap.max(m);
            }
            if let Some(n) = known_next {
                cap = cap.max(n);
            }
            return Some(cap as f64);
        }
    }

    // The episode map only RAISES a countdown-based cap: it counts dated
    // entries only, and ani.zip often lacks dates for some aired episodes, so
    // on its own it undercounts and would hide episodes that are out.
    match (known_map, known_next) {
        (Some(m), Some(n)) => Some(m.max(n) as f64),
        (None, Some(n)) => Some(n as f64),
        (_, None) => None,
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
                    s.watched_externally, s.followed, so.site_id, s.slug,
                    c.status, c.episodes, c.next_episode, c.next_airing_at
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
            synthetic: bool,
            catalog_status: Option<String>,
            catalog_episodes: Option<i64>,
            catalog_next_episode: Option<i64>,
            catalog_next_airing_at: Option<i64>,
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
                    synthetic: r.get::<_, String>(8)?.starts_with("anilist-"),
                    catalog_status: r.get(9)?,
                    catalog_episodes: r.get(10)?,
                    catalog_next_episode: r.get(11)?,
                    catalog_next_airing_at: r.get(12)?,
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
        let mut align_anilist_ids: std::collections::BTreeSet<i64> = std::collections::BTreeSet::new();
        for group in groups.values() {
            if group.iter().any(|r| r.followed) {
                let catalog_status = group.iter().find_map(|r| r.catalog_status.as_deref());
                let is_finished_or_cancelled = matches!(catalog_status, Some("FINISHED" | "CANCELLED"));
                if is_finished_or_cancelled && group.iter().any(|r| r.watched_externally) {
                    continue;
                }
                for r in group {
                    if let Some(al) = r.anilist_id {
                        align_anilist_ids.insert(al);
                    }
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

        // Episode-map facts for every AniList id involved, in two statements
        // (replaces the per-group and per-member lookups).
        let align_ids: Vec<i64> = align_anilist_ids.into_iter().collect();
        let preload = self.align_preload(&align_ids, now)?;

        let mut show_results = Vec::new();

        for (_group_key, group) in groups {
            if !group.iter().any(|r| r.followed) {
                continue;
            }
            let catalog_status = group.iter().find_map(|r| r.catalog_status.as_deref());
            let is_finished_or_cancelled = matches!(catalog_status, Some("FINISHED" | "CANCELLED"));
            if is_finished_or_cancelled && group.iter().any(|r| r.watched_externally) {
                continue;
            }
            let candidate_members: Vec<&Row> =
                group.iter().filter(|r| !r.watched_externally && !r.synthetic).collect();
            if candidate_members.is_empty() {
                continue;
            }

            // AniList aired cap (read-time only). Groups linked to an AniList id compute a cap
            // from the catalog's episode count, upcoming episode countdown, and/or episode map.
            let group_al_id = group.iter().find_map(|r| r.anilist_id).or_else(|| {
                _group_key.strip_prefix("al:").and_then(|s| s.parse::<i64>().ok())
            });
            let group_cap = if let Some(al_id) = group_al_id {
                let map_aired = preload.aired(al_id);
                let catalog_episodes = group.iter().find_map(|r| r.catalog_episodes);
                let catalog_next_episode = group.iter().find_map(|r| r.catalog_next_episode);
                let catalog_next_airing_at = group.iter().find_map(|r| r.catalog_next_airing_at);
                anilist_aired_cap(
                    catalog_status,
                    catalog_episodes,
                    catalog_next_episode,
                    catalog_next_airing_at,
                    map_aired,
                    now,
                )
            } else {
                None
            };

            // Compute row_align once for every candidate member.
            let mut member_align: std::collections::HashMap<i64, super::episode_align::RowAlign> =
                std::collections::HashMap::new();
            for m in &candidate_members {
                let align = preload.row_align(
                    m.anilist_id,
                    eps_by_series
                        .get(&m.id)
                        .into_iter()
                        .flatten()
                        .map(|e| e.number.as_str()),
                );
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
                    // A row whose lowest episode is already past the cap numbers
                    // the season differently (cumulative numbering with an unknown
                    // offset, e.g. 13-24 for a 12-episode entry): the cap does not
                    // describe it, so it is not applied to that row at all.
                    let member_cap = group_cap.filter(|&cap| {
                        eps.iter()
                            .filter_map(|e| crate::db::episodes::parse_ep_number(&e.number))
                            .filter_map(|v| align.to_canonical(v))
                            .fold(f64::INFINITY, f64::min)
                            .floor()
                            <= cap
                    });
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
                                if let Some(cap) = member_cap {
                                    if canon.floor() > cap {
                                        continue;
                                    }
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
mod tests;
