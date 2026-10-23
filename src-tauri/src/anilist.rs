//! Thin client for AniList's public GraphQL API — the source for the
//! "Catálogo" tab's full anime catalog (independent of what's scraped off
//! the tracked site) and for feeding catalog cards into Descubrir.
//!
//! Chosen over MyAnimeList/Jikan, Kitsu and AniDB: no API key or OAuth
//! registration needed for public read queries, a single GraphQL request
//! returns exactly the fields we want (title/cover/genres/format/episodes)
//! for a whole page instead of N+1 REST round-trips, and cover images are
//! served reliably off AniList's own CDN in multiple sizes.
//!
//! ## Why the catalog is fetched in partitions, not one popularity crawl
//!
//! AniList hard-caps offset pagination at `page * perPage <= 5000` per
//! query, and `pageInfo.total`/`lastPage` always report a fixed, fake value
//! regardless of the real result set (verified live 2026-07-10) — only
//! `pageInfo.hasNextPage` is trustworthy. There's no `id_greater` cursor to
//! walk past the cap, and `seasonYear` is unreliable (`seasonYear: 1917`
//! returned 2017 movies — verified). `startDate_greater`/`startDate_lesser`
//! (FuzzyDateInt, e.g. `19161231`) are exact and are the only reliable way
//! to slice the ~21,000-title catalog into partitions each guaranteed to
//! stay under the 5000-row cap. See `build_partitions`.
//!
//! Every partition query sorts `sort: ID` — popularity ordering shifts live
//! between requests and would skip/duplicate rows mid-partition crawl;
//! popularity is fetched as a field and stored, then used for the *local*
//! display ordering instead (see `db::list_catalog`).

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::time::Duration;

const ENDPOINT: &str = "https://graphql.anilist.co";

/// Field selection shared by every catalog query — the single source of
/// truth for what a `media` node returns. Used to live hand-duplicated in
/// both `CATALOG_QUERY` and `BY_IDS_QUERY`; a field added/renamed in one copy
/// and not the other would silently make the by-ids backfill
/// (`commands::backfill_catalog_metadata`) return partial data relative to
/// the main crawl, with no compiler or test signal tying the two together.
const MEDIA_FIELDS: &str = r#"id
      title { romaji english }
      synonyms
      coverImage { large }
      bannerImage
      format
      genres
      episodes
      duration
      averageScore
      popularity
      siteUrl
      status
      studios(isMain: true) { nodes { name } }
      startDate { year month day }
      nextAiringEpisode { airingAt episode }
      tags { name rank isMediaSpoiler }
      relations { edges { relationType node { id type } } }"#;

/// AniList's own per-request page cap. `fetch_by_ids_multi` chunks must never be handed
/// more ids than this — anything past the cap is silently dropped by the API,
/// which would look like "these ids don't exist" rather than an error.
pub const MAX_IDS_PER_REQUEST: usize = 50;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CatalogAnime {
    pub id: i64,
    pub title: String,
    /// Original (non-English) romanized title, kept alongside the display
    /// `title` so `matching::best_match` can try both when looking this
    /// title up on the scraped site — the site often lists shows under
    /// their romaji title even when AniList's `title` collapsed to English.
    /// `None` for rows synced before this field existed (backfills on the
    /// next incremental sync) or when AniList itself has no romaji title.
    #[serde(default)]
    pub title_romaji: Option<String>,
    /// English title, kept alongside `title` for the same reason as
    /// `title_romaji` — `title` prefers English already, so this is usually
    /// redundant with it, but kept distinct so callers never have to guess
    /// which one `title` actually is.
    #[serde(default)]
    pub title_english: Option<String>,
    /// Alternative titles AniList lists, used only for matching.
    #[serde(default)]
    pub synonyms: Vec<String>,
    pub cover_url: Option<String>,
    #[serde(default)]
    pub banner_url: Option<String>,
    pub format: Option<String>,
    pub genres: Vec<String>,
    pub episodes: Option<i64>,
    pub average_score: Option<i64>,
    pub popularity: Option<i64>,
    pub url: String,
    /// AniList's own vocabulary (`RELEASING`/`NOT_YET_RELEASED`/`FINISHED`/
    /// `CANCELLED`/`HIATUS`), stored as-is — no local translation. `None`
    /// for rows synced before this field existed (backfills on the next
    /// sync). Used by `db::random_catalog_anime_in_genre`'s `hide_upcoming`
    /// exclusion.
    #[serde(default)]
    pub status: Option<String>,
    /// AniList's real per-episode duration in minutes, when it has one.
    /// `None` for rows synced before this field existed (backfills on the
    /// next sync) or when AniList itself has no duration for the title —
    /// callers fall back to `db::stats::minutes_per_episode`'s estimate in
    /// that case.
    #[serde(default)]
    pub duration: Option<i64>,
    /// The first `isMain` studio's name, when AniList has one; co-productions
    /// with multiple mains only keep the first — an approximation, not
    /// exhaustive credit data. `None` for rows synced before this field
    /// existed or when AniList reports no credited studio at all.
    #[serde(default)]
    pub studio: Option<String>,
    /// Real AniList premiere date, as a Unix timestamp (midnight UTC) — used
    /// to answer "did this air this season" for airing-site rows that have
    /// no scraped episode data (most unfollowed ones; see
    /// `db::episodes::first_episode_dates`'s doc comment). `None` for rows
    /// synced before this field existed, a fuzzy/partial AniList date
    /// (year-only or year+month, common for older/unannounced titles) that
    /// can't resolve to a real calendar date, or when AniList has no start
    /// date at all.
    #[serde(default)]
    pub start_date: Option<i64>,
    #[serde(default)]
    pub next_airing_at: Option<i64>,
    #[serde(default)]
    pub next_episode: Option<i64>,
    #[serde(default)]
    pub tags: Vec<CatalogTag>,
    #[serde(default)]
    pub relations: Vec<CatalogRelation>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CatalogRelation {
    pub related_id: i64,
    pub relation_type: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CatalogTag {
    pub name: String,
    pub rank: i64,
}

#[derive(Debug, Deserialize)]
struct ResponseData {
    #[serde(rename = "Page")]
    page: PageData,
}

#[derive(Debug, Deserialize)]
struct PageData {
    #[serde(rename = "pageInfo")]
    page_info: PageInfo,
    media: Vec<MediaEntry>,
}

/// `total`/`lastPage` deliberately omitted — verified fake on this API
/// (always report 5000-capped values). `hasNextPage` is the only reliable
/// signal for when to stop paginating a partition.
#[derive(Debug, Deserialize)]
struct PageInfo {
    #[serde(rename = "hasNextPage")]
    has_next_page: bool,
}

#[derive(Debug, Deserialize)]
struct MediaEntry {
    id: i64,
    title: MediaTitle,
    #[serde(default)]
    synonyms: Option<Vec<Option<String>>>,
    #[serde(rename = "coverImage")]
    cover_image: Option<CoverImage>,
    #[serde(rename = "bannerImage", default)]
    banner_image: Option<String>,
    format: Option<String>,
    genres: Vec<String>,
    episodes: Option<i64>,
    #[serde(default)]
    duration: Option<i64>,
    #[serde(rename = "averageScore")]
    average_score: Option<i64>,
    popularity: Option<i64>,
    #[serde(rename = "siteUrl")]
    site_url: String,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    studios: Option<StudioConnection>,
    #[serde(rename = "startDate", default)]
    start_date: Option<FuzzyDate>,
    #[serde(rename = "nextAiringEpisode", default)]
    next_airing_episode: Option<NextAiringEpisode>,
    #[serde(default)]
    tags: Option<Vec<Option<MediaTag>>>,
    #[serde(default)]
    relations: Option<MediaRelations>,
}

#[derive(Debug, Deserialize)]
struct MediaRelations {
    #[serde(default)]
    edges: Option<Vec<Option<MediaRelationEdge>>>,
}

#[derive(Debug, Deserialize)]
struct MediaRelationEdge {
    #[serde(rename = "relationType", default)]
    relation_type: Option<String>,
    #[serde(default)]
    node: Option<MediaRelationNode>,
}

#[derive(Debug, Deserialize)]
struct MediaRelationNode {
    #[serde(default)]
    id: Option<i64>,
    #[serde(rename = "type", default)]
    node_type: Option<String>,
}

#[derive(Debug, Deserialize)]
struct NextAiringEpisode {
    #[serde(rename = "airingAt")]
    airing_at: i64,
    episode: i64,
}

#[derive(Debug, Deserialize)]
struct MediaTag {
    name: String,
    // Both nullable in AniList's schema: one explicit null must not fail the
    // whole 50-row page (that would abort a sync/backfill and retry forever).
    #[serde(default)]
    rank: Option<i64>,
    #[serde(rename = "isMediaSpoiler", default)]
    is_media_spoiler: Option<bool>,
}

/// Tags ranked below this (AniList's 0-100 relevance vote) are noise.
const MIN_TAG_RANK: i64 = 30;

#[derive(Debug, Deserialize)]
struct StudioConnection {
    nodes: Vec<StudioNode>,
}

#[derive(Debug, Deserialize)]
struct StudioNode {
    name: String,
}

/// AniList's `FuzzyDate` output type — any of `year`/`month`/`day` can be
/// `null` (e.g. a title with only a known year). Only a fully-specified
/// date resolves to a real calendar date; see `FuzzyDate::to_timestamp`.
#[derive(Debug, Deserialize)]
struct FuzzyDate {
    year: Option<i32>,
    month: Option<u32>,
    day: Option<u32>,
}

impl FuzzyDate {
    /// Midnight-UTC Unix timestamp for a fully-specified date, `None` when
    /// any component is missing/fuzzy or the combination isn't a real date.
    fn to_timestamp(&self) -> Option<i64> {
        let (year, month, day) = (self.year?, self.month?, self.day?);
        chrono::NaiveDate::from_ymd_opt(year, month, day)
            .and_then(|d| d.and_hms_opt(0, 0, 0))
            .map(|dt| dt.and_utc().timestamp())
    }
}

#[derive(Debug, Clone, Deserialize)]
struct MediaTitle {
    romaji: Option<String>,
    english: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CoverImage {
    large: Option<String>,
}

impl From<MediaEntry> for CatalogAnime {
    fn from(m: MediaEntry) -> Self {
        let title = m.title.english.clone().or_else(|| m.title.romaji.clone()).unwrap_or_default();
        let mut synonyms = Vec::new();
        for s in m.synonyms.into_iter().flatten().flatten() {
            let trimmed = s.trim().to_string();
            if !trimmed.is_empty() && !synonyms.contains(&trimmed) {
                synonyms.push(trimmed);
            }
        }
        let (next_airing_at, next_episode) = match m.next_airing_episode {
            Some(n) => (Some(n.airing_at), Some(n.episode)),
            None => (None, None),
        };
        let mut tags = Vec::new();
        if let Some(media_tags) = m.tags {
            for t in media_tags.into_iter().flatten() {
                let Some(rank) = t.rank else { continue };
                if t.is_media_spoiler == Some(true) || rank < MIN_TAG_RANK {
                    continue;
                }
                let name = t.name.trim().to_string();
                if name.is_empty() {
                    continue;
                }
                // Keep the best rank when a name repeats.
                match tags.iter_mut().find(|x: &&mut CatalogTag| x.name == name) {
                    Some(existing) => existing.rank = existing.rank.max(rank),
                    None => tags.push(CatalogTag { name, rank }),
                }
            }
        }
        let mut relations = Vec::new();
        if let Some(media_rels) = m.relations {
            if let Some(edges) = media_rels.edges {
                for edge in edges.into_iter().flatten() {
                    let Some(rel_type) = edge.relation_type else { continue };
                    if rel_type != "PREQUEL" && rel_type != "SEQUEL" {
                        continue;
                    }
                    let Some(node) = edge.node else { continue };
                    let Some(related_id) = node.id else { continue };
                    if related_id <= 0 {
                        continue;
                    }
                    if let Some(ref t) = node.node_type {
                        if t != "ANIME" {
                            continue;
                        }
                    }
                    if !relations
                        .iter()
                        .any(|r: &CatalogRelation| r.related_id == related_id && r.relation_type == rel_type)
                    {
                        relations.push(CatalogRelation {
                            related_id,
                            relation_type: rel_type,
                        });
                    }
                }
            }
        }
        CatalogAnime {
            id: m.id,
            title,
            title_romaji: m.title.romaji,
            title_english: m.title.english,
            synonyms,
            cover_url: m.cover_image.and_then(|c| c.large),
            banner_url: m.banner_image,
            format: m.format,
            genres: m.genres,
            episodes: m.episodes,
            average_score: m.average_score,
            popularity: m.popularity,
            url: m.site_url,
            status: m.status,
            duration: m.duration,
            studio: m.studios.and_then(|s| s.nodes.into_iter().next()).map(|n| n.name),
            start_date: m.start_date.and_then(|d| d.to_timestamp()),
            next_airing_at,
            next_episode,
            tags,
            relations,
        }
    }
}

/// One slice of the full-catalog crawl. Each partition is guaranteed (by
/// construction) to stay under AniList's 5000-row offset-pagination cap, so
/// pagination within it can just loop on `hasNextPage` to exhaustion.
#[derive(Debug, Clone, PartialEq)]
pub enum PartitionFilter {
    /// FuzzyDateInt bounds (e.g. `19161231` = 1916-12-31). `None` on either
    /// side means unbounded on that side.
    DateRange {
        start_date_greater: Option<i64>,
        start_date_lesser: Option<i64>,
    },
    /// Undated upcoming titles — `startDate` filters can't see these since
    /// they have no `startDate` at all.
    NotYetReleased,
    /// No filter at all — used only for the bounded catch-all pass.
    Unfiltered,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Partition {
    pub label: String,
    pub filter: PartitionFilter,
    /// Hard page cap for partitions that aren't naturally bounded under the
    /// 5000-row cap by their filter (the unfiltered catch-all pass). `None`
    /// means "crawl until `hasNextPage` is false".
    pub max_pages: Option<i64>,
}

/// The full partition list for an initial/from-scratch sync:
///
/// 1. The recent window first: `current_year + 2` down to `current_year - 1`
///    descending, followed by `NOT_YET_RELEASED`. This ensures that on a fresh
///    install, currently-airing and upcoming titles populate the catalog within
///    the first few seconds, allowing "Esta temporada" to display immediately
///    without waiting for the multi-minute crawl of the entire historical back-catalog.
/// 2. Modern years from `current_year - 2` down to 2000 descending.
/// 3. Historical decades from 1990 down to 1940 descending.
/// 4. One pre-modern range (`startDate_lesser: 19400101` — a few hundred
///    titles total).
/// 5. One final unfiltered `sort: ID` pass, capped at the first 100 pages
///    (the 5000-row cap) — catches undated *non*-upcoming leftovers among
///    the oldest ids. Anything past that (undated + not upcoming + id
///    beyond the first 5000) is an accepted, documented gap.
///
/// Duplicates across partitions are harmless — `upsert_catalog_anime` is
/// keyed on AniList's own numeric id.
pub fn build_partitions(current_year: i32) -> Vec<Partition> {
    let mut partitions = Vec::new();

    // 1. Recent years window (newest first: current_year + 2 down to current_year - 1)
    for year in (current_year - 1..=current_year + 2).rev() {
        partitions.push(Partition {
            label: format!("year-{year}"),
            filter: PartitionFilter::DateRange {
                start_date_greater: Some((year as i64 - 1) * 10_000 + 1231),
                start_date_lesser: Some((year as i64 + 1) * 10_000 + 101),
            },
            max_pages: None,
        });
    }

    // 2. Upcoming / undated titles
    partitions.push(Partition {
        label: "not-yet-released".to_string(),
        filter: PartitionFilter::NotYetReleased,
        max_pages: None,
    });

    // 3. Remaining modern years: current_year - 2 down to 2000 descending
    for year in (2000..=(current_year - 2)).rev() {
        partitions.push(Partition {
            label: format!("year-{year}"),
            filter: PartitionFilter::DateRange {
                start_date_greater: Some((year as i64 - 1) * 10_000 + 1231),
                start_date_lesser: Some((year as i64 + 1) * 10_000 + 101),
            },
            max_pages: None,
        });
    }

    // 4. Decades descending from 1990 down to 1940
    // 1940-1999 holds only a few hundred titles per decade, so one partition
    // per decade (one request of ten pages) instead of sixty near-empty year
    // partitions that each cost a whole paced request.
    for decade in (1940..2000).step_by(10).rev() {
        partitions.push(Partition {
            label: format!("decade-{decade}"),
            filter: PartitionFilter::DateRange {
                start_date_greater: Some((decade as i64 - 1) * 10_000 + 1231),
                start_date_lesser: Some((decade as i64 + 10) * 10_000 + 101),
            },
            max_pages: None,
        });
    }

    // 5. Pre-modern range (startDate_lesser: 19400101)
    partitions.push(Partition {
        label: "pre-1940".to_string(),
        filter: PartitionFilter::DateRange {
            start_date_greater: None,
            start_date_lesser: Some(19_400_101),
        },
        max_pages: None,
    });

    // 6. Catch-all pass
    partitions.push(Partition {
        label: "catch-all".to_string(),
        filter: PartitionFilter::Unfiltered,
        max_pages: Some(100),
    });

    partitions
}

/// Partitions for a re-sync *after* a full sync has already completed once:
/// only the window where titles actually change — current year − 1 through
/// current year + 2, plus `NOT_YET_RELEASED` (new/updated titles live there;
/// the pre-modern back-catalog and the catch-all pass are static once
/// synced, so they're skipped entirely).
pub fn incremental_partitions(current_year: i32) -> Vec<Partition> {
    build_partitions(current_year)
        .into_iter()
        .filter(|p| {
            matches!(p.filter, PartitionFilter::NotYetReleased)
                || p
                    .label
                    .strip_prefix("year-")
                    .and_then(|y| y.parse::<i32>().ok())
                    .is_some_and(|y| y >= current_year - 1)
        })
        .collect()
}

/// From the full partition list, the ones not yet marked complete in
/// persisted sync state — lets an interrupted full sync resume instead of
/// restarting from the first partition.
pub fn pending_partitions(all: &[Partition], completed: &[String]) -> Vec<Partition> {
    all.iter().filter(|p| !completed.contains(&p.label)).cloned().collect()
}

/// Persisted (settings table, key `catalog_sync_state`) resume/incremental
/// bookkeeping. `completed` only tracks progress through an in-progress
/// *full* sync — an incremental sync always re-runs its fixed partition set
/// and neither consults nor mutates it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CatalogSyncState {
    #[serde(default)]
    pub completed: Vec<String>,
    #[serde(default)]
    pub full_sync_completed_at: Option<String>,
}

impl CatalogSyncState {
    /// Parses persisted JSON, falling back to the default (empty) state on
    /// missing/invalid input — covers both "never synced before" (no
    /// settings row) and any unexpected corruption without a special case.
    pub fn from_json(s: &str) -> Self {
        serde_json::from_str(s).unwrap_or_default()
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| "{}".to_string())
    }
}

/// One fetched page of one partition, plus the rate-limit headroom AniList
/// reported (`X-Ratelimit-Remaining`) so the caller can decide whether to
/// pace the next request.
pub struct PartitionPage {
    pub items: Vec<CatalogAnime>,
    pub has_next_page: bool,
    pub rate_remaining: Option<i64>,
}

/// Builds the GraphQL `variables` object for one partition/page request.
///
/// Critically, `startDateGreater`/`startDateLesser`/`status` are inserted
/// **only when applicable** — never as an explicit JSON `null`. Verified
/// live against the real API (2026-07-10): sending `startDate_greater:
/// null` (or `_lesser`) causes AniList to reject the whole query with HTTP
/// 400 "Illegal operator and value combination", even though the variable
/// is declared nullable in the query text. Omitting the key from the
/// `variables` object entirely (as opposed to sending it as `null`) is what
/// AniList's resolver treats as "filter not provided" — confirmed by direct
/// curl comparison of both forms against the live endpoint.
fn build_variables(partition: &Partition, page: i64, per_page: i64) -> serde_json::Value {
    let mut vars = serde_json::Map::new();
    vars.insert("page".to_string(), serde_json::json!(page));
    vars.insert("perPage".to_string(), serde_json::json!(per_page));
    match &partition.filter {
        PartitionFilter::DateRange { start_date_greater, start_date_lesser } => {
            if let Some(greater) = start_date_greater {
                vars.insert("startDateGreater".to_string(), serde_json::json!(greater));
            }
            if let Some(lesser) = start_date_lesser {
                vars.insert("startDateLesser".to_string(), serde_json::json!(lesser));
            }
        }
        PartitionFilter::NotYetReleased => {
            vars.insert("status".to_string(), serde_json::json!("NOT_YET_RELEASED"));
        }
        PartitionFilter::Unfiltered => {}
    }
    serde_json::Value::Object(vars)
}

fn search_query() -> String {
    format!(
        r#"
query ($search: String) {{
  Page(page: 1, perPage: 6) {{
    pageInfo {{ hasNextPage }}
    media(type: ANIME, search: $search, sort: SEARCH_MATCH) {{
      {MEDIA_FIELDS}
    }}
  }}
}}
"#
    )
}

pub async fn search_anime(title: &str) -> Result<Vec<CatalogAnime>> {
    let body = serde_json::json!({
        "query": search_query(),
        "variables": { "search": title },
    });
    Ok(execute_query(body, &format!("search '{title}'")).await?.items)
}

/// Query AniList for an anime's banner image by AniList ID or by search title.
/// Returns Ok(Some(banner_url)) if found, Ok(None) if the media has no banner,
/// or Err if the network request fails.
pub async fn fetch_banner_by_id_or_title(
    id: Option<i64>,
    search_title: Option<&str>,
) -> Result<Option<String>> {
    let (query_str, vars) = if let Some(id_val) = id {
        (
            "query ($id: Int) { Media(id: $id, type: ANIME) { id bannerImage } }",
            serde_json::json!({ "id": id_val }),
        )
    } else if let Some(title) = search_title {
        (
            "query ($search: String) { Media(search: $search, type: ANIME) { id bannerImage } }",
            serde_json::json!({ "search": title }),
        )
    } else {
        return Ok(None);
    };

    let body = serde_json::json!({
        "query": query_str,
        "variables": vars,
    });

    #[derive(Deserialize)]
    struct SingleMediaData {
        #[serde(rename = "Media")]
        media: Option<MediaBannerOnly>,
    }
    #[derive(Deserialize)]
    struct MediaBannerOnly {
        #[serde(rename = "bannerImage")]
        banner_image: Option<String>,
    }
    #[derive(Deserialize)]
    struct SingleMediaResponse {
        data: Option<SingleMediaData>,
    }

    let client = reqwest::Client::new();
    let resp = client
        .post(ENDPOINT)
        .json(&body)
        .send()
        .await
        .map_err(|e| anyhow!("AniList banner request failed: {e}"))?;

    if !resp.status().is_success() {
        return Ok(None);
    }

    let parsed: SingleMediaResponse = resp
        .json()
        .await
        .map_err(|e| anyhow!("Failed to parse AniList response: {e}"))?;

    Ok(parsed.data.and_then(|d| d.media).and_then(|m| m.banner_image))
}

/// How many `Page` selections one request carries. AniList's query complexity cap
/// is 500 and a 50-row page of our media fields costs 50, so ten fit (verified live
/// 2026-10-14: 10 pages -> 500 titles in one ~1.7 s response, 12+ is refused with
/// "Max query complexity should be 500"). The rate limit counts REQUESTS (30/min),
/// not rows, so this is a ten-fold speed-up of every bulk crawl.
pub const PAGES_PER_REQUEST: i64 = 10;

/// One multi-page catalog query: `p0..p{n-1}` are consecutive pages of the same
/// partition, starting at `first_page`. Filters stay GraphQL variables, exactly as
/// in the single-page catalog query this replaced.
fn catalog_multi_query(first_page: i64, n_pages: i64) -> String {
    let pages: String = (0..n_pages)
        .map(|i| {
            format!(
                "p{i}: Page(page: {page}, perPage: $perPage) {{ pageInfo {{ hasNextPage }} media(type: ANIME, sort: ID, startDate_greater: $startDateGreater, startDate_lesser: $startDateLesser, status: $status) {{ {MEDIA_FIELDS} }} }}\n",
                page = first_page + i
            )
        })
        .collect();
    format!(
        "query ($perPage: Int, $startDateGreater: FuzzyDateInt, $startDateLesser: FuzzyDateInt, $status: MediaStatus) {{\n{pages}}}\n"
    )
}

/// Several `id_in` lookups in one request: `b0..b{n-1}` each take their own id list
/// variable (`$ids0`..), at most `MAX_IDS_PER_REQUEST` ids each.
fn by_ids_multi_query(n_chunks: usize) -> String {
    let vars: String = (0..n_chunks).map(|i| format!("$ids{i}: [Int], ")).collect();
    let pages: String = (0..n_chunks)
        .map(|i| {
            format!(
                "b{i}: Page(page: 1, perPage: $perPage) {{ pageInfo {{ hasNextPage }} media(type: ANIME, id_in: $ids{i}, sort: ID) {{ {MEDIA_FIELDS} }} }}\n"
            )
        })
        .collect();
    format!("query ({vars}$perPage: Int) {{\n{pages}}}\n")
}

/// Read `prefix0..prefix{n-1}` out of a multi-alias `data` object, in order.
/// Returns the concatenated items and the `hasNextPage` of the LAST alias (an
/// earlier "no more" makes every later alias empty, so the last one is enough).
fn collect_aliased_pages(data: serde_json::Value, prefix: &str, n: usize, context: &str) -> Result<(Vec<CatalogAnime>, bool)> {
    let mut map: std::collections::HashMap<String, PageData> = serde_json::from_value(data)
        .map_err(|e| anyhow!("AniList multi-page response had an unexpected shape ({context}): {e}"))?;
    let mut items = Vec::new();
    let mut has_next = false;
    for i in 0..n {
        let Some(page) = map.remove(&format!("{prefix}{i}")) else {
            return Err(anyhow!("AniList multi-page response is missing {prefix}{i} ({context})"));
        };
        items.extend(page.media.into_iter().map(CatalogAnime::from));
        has_next = page.page_info.has_next_page;
    }
    Ok((items, has_next))
}

/// Fetch `n_pages` consecutive pages of one partition in a SINGLE request.
/// `n_pages` is clamped to `1..=PAGES_PER_REQUEST`. The result's `has_next_page`
/// says whether the page after the last one fetched exists.
pub async fn fetch_partition_pages(partition: &Partition, first_page: i64, n_pages: i64, per_page: i64) -> Result<PartitionPage> {
    let n = n_pages.clamp(1, PAGES_PER_REQUEST);
    let mut variables = build_variables(partition, first_page, per_page);
    if let Some(obj) = variables.as_object_mut() {
        obj.remove("page");
    }
    let body = serde_json::json!({ "query": catalog_multi_query(first_page, n), "variables": variables });
    let context = format!("{} pages {first_page}..{}", partition.label, first_page + n - 1);
    let (data, rate_remaining) = post_graphql(body, &context).await?;
    let (items, has_next_page) = collect_aliased_pages(data, "p", n as usize, &context)?;
    Ok(PartitionPage { items, has_next_page, rate_remaining })
}

/// Look up catalog entries by AniList id, for up to `MAX_IDS_PER_REQUEST * PAGES_PER_REQUEST`
/// (500) ids in one request. Ids AniList no longer serves are simply absent.
pub async fn fetch_by_ids_multi(ids: &[i64]) -> Result<Vec<CatalogAnime>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let cap = MAX_IDS_PER_REQUEST * PAGES_PER_REQUEST as usize;
    if ids.len() > cap {
        return Err(anyhow!("fetch_by_ids_multi got {} ids, over the per-request cap of {cap}", ids.len()));
    }
    let chunks: Vec<&[i64]> = ids.chunks(MAX_IDS_PER_REQUEST).collect();
    let mut variables = serde_json::Map::new();
    variables.insert("perPage".to_string(), serde_json::json!(MAX_IDS_PER_REQUEST));
    for (i, chunk) in chunks.iter().enumerate() {
        variables.insert(format!("ids{i}"), serde_json::json!(chunk));
    }
    let body = serde_json::json!({ "query": by_ids_multi_query(chunks.len()), "variables": variables });
    let (data, _) = post_graphql(body, "by-ids-multi").await?;
    Ok(collect_aliased_pages(data, "b", chunks.len(), "by-ids-multi")?.0)
}

/// POST one GraphQL body with the shared 429/`Retry-After` retry, and return the
/// `data` object plus the rate-limit headroom AniList reported. Every query path in
/// this module goes through here so the rate-limit handling cannot drift.
async fn post_graphql(body: serde_json::Value, context: &str) -> Result<(serde_json::Value, Option<i64>)> {
    const MAX_RETRIES: u32 = 5;

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|e| anyhow!("could not build the AniList HTTP client ({context}): {e}"))?;
    let mut attempt = 0u32;
    loop {
        let resp = client
            .post(ENDPOINT)
            .json(&body)
            .send()
            .await
            .map_err(|e| anyhow!("AniList request failed ({context}): {e}"))?;

        if resp.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
            attempt += 1;
            if attempt > MAX_RETRIES {
                return Err(anyhow!(
                    "AniList rate-limited ({context}) after {MAX_RETRIES} retries"
                ));
            }
            let retry_after_secs = resp
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(2);
            tokio::time::sleep(Duration::from_secs(retry_after_secs)).await;
            continue;
        }

        let rate_remaining = resp
            .headers()
            .get("x-ratelimit-remaining")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<i64>().ok());

        let parsed: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| anyhow!("AniList response was not valid JSON ({context}): {e}"))?;
        if let Some(errors) = parsed.get("errors").and_then(|e| e.as_array()).filter(|e| !e.is_empty()) {
            let msg = errors
                .iter()
                .filter_map(|e| e.get("message").and_then(|m| m.as_str()))
                .collect::<Vec<_>>()
                .join("; ");
            return Err(anyhow!("AniList returned an error ({context}): {msg}"));
        }
        let data = parsed
            .get("data")
            .filter(|d| !d.is_null())
            .cloned()
            .ok_or_else(|| anyhow!("AniList response had no data ({context})"))?;
        return Ok((data, rate_remaining));
    }
}

/// One related title of an anime, as AniList's `relations` edge reports it —
/// every relation type (prequel, side story, spin-off...), unlike the
/// catalog sync, which only keeps PREQUEL/SEQUEL edges for its own logic.
/// Only ANIME nodes are kept: manga/novel sources cannot be opened here.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RelatedMedia {
    pub id: i64,
    pub relation_type: String,
    pub title: String,
    pub cover_url: Option<String>,
    pub format: Option<String>,
    pub year: Option<i64>,
}

/// Detail-view data the catalog sync does not carry: the description (raw
/// AniList markup — `<br>`, `<i>`, `~!spoiler!~` — sanitised by the
/// frontend, which never renders it as HTML) and the full relation list.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MediaExtras {
    pub description: Option<String>,
    pub relations: Vec<RelatedMedia>,
    pub trailer: Option<Trailer>,
}

/// AniList's trailer reference. `site` is lowercased `youtube` or
/// `dailymotion` and `id` only holds `[A-Za-z0-9_-]`: both end up in an
/// iframe URL on the frontend, so `parse_trailer` rejects anything else.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Trailer {
    pub site: String,
    pub id: String,
}

pub(crate) fn parse_trailer(site: Option<String>, id: Option<String>) -> Option<Trailer> {
    let site = site?.trim().to_lowercase();
    let id = id?.trim().to_string();
    if site != "youtube" && site != "dailymotion" {
        return None;
    }
    let valid = !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    valid.then_some(Trailer { site, id })
}

const MEDIA_EXTRAS_QUERY: &str = "query ($id: Int) { Media(id: $id, type: ANIME) { id description(asHtml: false) trailer { id site } relations { edges { relationType node { id type format title { romaji english } coverImage { medium } startDate { year } } } } } }";

/// Read a `{ Media: ... }` response into `MediaExtras`. `Ok(None)` when
/// AniList has no such media (`Media: null`).
pub(crate) fn parse_media_extras(data: serde_json::Value) -> Result<Option<MediaExtras>> {
    #[derive(Deserialize)]
    struct Data {
        #[serde(rename = "Media")]
        media: Option<Media>,
    }
    #[derive(Deserialize)]
    struct Media {
        #[serde(default)]
        description: Option<String>,
        #[serde(default)]
        relations: Option<Rels>,
        #[serde(default)]
        trailer: Option<RawTrailer>,
    }
    #[derive(Deserialize)]
    struct RawTrailer {
        #[serde(default)]
        id: Option<String>,
        #[serde(default)]
        site: Option<String>,
    }
    #[derive(Deserialize)]
    struct Rels {
        #[serde(default)]
        edges: Option<Vec<Option<Edge>>>,
    }
    #[derive(Deserialize)]
    struct Edge {
        #[serde(rename = "relationType", default)]
        relation_type: Option<String>,
        #[serde(default)]
        node: Option<Node>,
    }
    #[derive(Deserialize)]
    struct Node {
        #[serde(default)]
        id: Option<i64>,
        #[serde(rename = "type", default)]
        node_type: Option<String>,
        #[serde(default)]
        format: Option<String>,
        #[serde(default)]
        title: Option<MediaTitle>,
        #[serde(rename = "coverImage", default)]
        cover_image: Option<Medium>,
        #[serde(rename = "startDate", default)]
        start_date: Option<Year>,
    }
    #[derive(Deserialize)]
    struct Medium {
        #[serde(default)]
        medium: Option<String>,
    }
    #[derive(Deserialize)]
    struct Year {
        #[serde(default)]
        year: Option<i64>,
    }

    let data: Data = serde_json::from_value(data).map_err(|e| anyhow!("AniList media extras had an unexpected shape: {e}"))?;
    let Some(media) = data.media else { return Ok(None) };
    let mut relations: Vec<RelatedMedia> = Vec::new();
    for edge in media.relations.and_then(|r| r.edges).into_iter().flatten().flatten() {
        let (Some(relation_type), Some(node)) = (edge.relation_type, edge.node) else { continue };
        let Some(id) = node.id.filter(|id| *id > 0) else { continue };
        if node.node_type.as_deref() != Some("ANIME") {
            continue;
        }
        if relations.iter().any(|r| r.id == id && r.relation_type == relation_type) {
            continue;
        }
        let title = node
            .title
            .and_then(|t| t.english.or(t.romaji))
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
            .unwrap_or_default();
        relations.push(RelatedMedia {
            id,
            relation_type,
            title,
            cover_url: node.cover_image.and_then(|c| c.medium),
            format: node.format,
            year: node.start_date.and_then(|d| d.year),
        });
    }
    let description = media.description.map(|d| d.trim().to_string()).filter(|d| !d.is_empty());
    let trailer = media.trailer.and_then(|t| parse_trailer(t.site, t.id));
    Ok(Some(MediaExtras { description, relations, trailer }))
}

/// On-demand, single-title fetch for the series detail view. One request per
/// opened title (cached by the caller), through the shared 429 handling.
pub async fn fetch_media_extras(id: i64) -> Result<Option<MediaExtras>> {
    let body = serde_json::json!({ "query": MEDIA_EXTRAS_QUERY, "variables": { "id": id } });
    match post_graphql(body, &format!("media extras {id}")).await {
        Ok((data, _)) => parse_media_extras(data),
        // AniList answers an unknown id with a "Not Found." error rather than
        // `Media: null`; that is a definitive "no such title", not a failure.
        Err(e) if e.to_string().contains("Not Found") => Ok(None),
        Err(e) => Err(e),
    }
}

/// Single-`Page` queries (partition page, by-ids, search): `post_graphql` plus the
/// `data.Page` unwrap.
async fn execute_query(body: serde_json::Value, context: &str) -> Result<PartitionPage> {
    let (data, rate_remaining) = post_graphql(body, context).await?;
    let data: ResponseData = serde_json::from_value(data)
        .map_err(|e| anyhow!("AniList response had an unexpected shape ({context}): {e}"))?;
    let items: Vec<CatalogAnime> = data.page.media.into_iter().map(CatalogAnime::from).collect();
    Ok(PartitionPage {
        items,
        has_next_page: data.page.page_info.has_next_page,
        rate_remaining,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fuzzy_date_resolves_a_fully_specified_date() {
        let d = FuzzyDate { year: Some(2026), month: Some(4), day: Some(15) };
        let ts = d.to_timestamp().expect("fully-specified date must resolve");
        assert_eq!(
            chrono::DateTime::from_timestamp(ts, 0).unwrap().format("%Y-%m-%d").to_string(),
            "2026-04-15"
        );
    }

    #[test]
    fn fuzzy_date_year_only_is_none() {
        let d = FuzzyDate { year: Some(2026), month: None, day: None };
        assert_eq!(d.to_timestamp(), None);
    }

    #[test]
    fn fuzzy_date_all_missing_is_none() {
        let d = FuzzyDate { year: None, month: None, day: None };
        assert_eq!(d.to_timestamp(), None);
    }

    #[test]
    fn build_partitions_orders_newest_first_and_ends_with_catch_all() {
        let partitions = build_partitions(2026);
        assert_eq!(partitions[0].label, "year-2028");
        assert_eq!(partitions[1].label, "year-2027");
        assert_eq!(partitions[2].label, "year-2026");
        assert_eq!(partitions[3].label, "year-2025");
        assert_eq!(partitions[4].label, "not-yet-released");
        assert_eq!(partitions[partitions.len() - 2].label, "pre-1940");
        assert_eq!(partitions.last().unwrap().label, "catch-all");

        // Must contain exactly the same set of labels as the historical
        // oldest-first ordering.
        let mut old_labels: Vec<String> = Vec::new();
        old_labels.push("pre-1940".to_string());
        for decade in (1940..2000).step_by(10) {
            old_labels.push(format!("decade-{decade}"));
        }
        for year in 2000..=2028 {
            old_labels.push(format!("year-{year}"));
        }
        old_labels.push("not-yet-released".to_string());
        old_labels.push("catch-all".to_string());
        old_labels.sort();

        let mut new_labels: Vec<String> = partitions.iter().map(|p| p.label.clone()).collect();
        new_labels.sort();
        assert_eq!(new_labels, old_labels);
    }

    #[test]
    fn build_partitions_pre_1940_is_upper_bounded_only() {
        let partitions = build_partitions(2026);
        let pre = partitions.iter().find(|p| p.label == "pre-1940").unwrap();
        assert_eq!(
            pre.filter,
            PartitionFilter::DateRange { start_date_greater: None, start_date_lesser: Some(19_400_101) }
        );
    }

    #[test]
    fn build_partitions_has_no_gap_from_1940_through_current_plus_2() {
        let partitions = build_partitions(2026);
        for decade in (1940..2000).step_by(10) {
            assert!(
                partitions.iter().any(|p| p.label == format!("decade-{decade}")),
                "missing decade-{decade}"
            );
        }
        for year in 2000..=2028 {
            assert!(
                partitions.iter().any(|p| p.label == format!("year-{year}")),
                "missing year-{year}"
            );
        }
        // Per-year partitions are no longer cut before 2000.
        assert!(!partitions.iter().any(|p| p.label == "year-1999"));
        // Nothing beyond current_year + 2.
        assert!(!partitions.iter().any(|p| p.label == "year-2029"));
    }

    #[test]
    fn build_partitions_year_bounds_are_exact_fuzzy_date_ints() {
        let partitions = build_partitions(2026);
        let y2000 = partitions.iter().find(|p| p.label == "year-2000").unwrap();
        assert_eq!(
            y2000.filter,
            PartitionFilter::DateRange {
                start_date_greater: Some(19_991_231),
                start_date_lesser: Some(20_010_101),
            }
        );
    }

    #[test]
    fn build_partitions_decade_bounds_cover_the_whole_decade() {
        let partitions = build_partitions(2026);
        let d1990 = partitions.iter().find(|p| p.label == "decade-1990").unwrap();
        assert_eq!(
            d1990.filter,
            PartitionFilter::DateRange {
                start_date_greater: Some(19_891_231),
                start_date_lesser: Some(20_000_101),
            }
        );
    }

    #[test]
    fn build_partitions_includes_not_yet_released() {
        let partitions = build_partitions(2026);
        assert!(partitions.iter().any(|p| p.filter == PartitionFilter::NotYetReleased));
    }

    #[test]
    fn build_partitions_catch_all_is_unfiltered_and_capped_at_100_pages() {
        let partitions = build_partitions(2026);
        let catch_all = partitions.iter().find(|p| p.label == "catch-all").unwrap();
        assert_eq!(catch_all.filter, PartitionFilter::Unfiltered);
        assert_eq!(catch_all.max_pages, Some(100));
    }

    #[test]
    fn incremental_partitions_excludes_static_back_catalog() {
        let inc = incremental_partitions(2026);
        assert!(!inc.iter().any(|p| p.label == "pre-1940"));
        assert!(!inc.iter().any(|p| p.label == "decade-1990"));
        assert!(!inc.iter().any(|p| p.label == "catch-all"));
    }

    #[test]
    fn incremental_partitions_includes_recent_years_and_not_yet_released() {
        let inc = incremental_partitions(2026);
        assert!(inc.iter().any(|p| p.label == "year-2025"));
        assert!(inc.iter().any(|p| p.label == "year-2026"));
        assert!(inc.iter().any(|p| p.label == "year-2027"));
        assert!(inc.iter().any(|p| p.label == "year-2028"));
        assert!(inc.iter().any(|p| p.filter == PartitionFilter::NotYetReleased));
        assert_eq!(inc.len(), 5);
        let labels: Vec<&str> = inc.iter().map(|p| p.label.as_str()).collect();
        assert_eq!(
            labels,
            vec!["year-2028", "year-2027", "year-2026", "year-2025", "not-yet-released"]
        );
    }

    #[test]
    fn pending_partitions_with_single_completed_omits_it_and_preserves_order() {
        let all = build_partitions(2026);
        let pending = pending_partitions(&all, &["year-2028".to_string()]);
        assert_eq!(pending.len(), all.len() - 1);
        assert!(!pending.iter().any(|p| p.label == "year-2028"));
        let all_except_first: Vec<String> = all[1..].iter().map(|p| p.label.clone()).collect();
        let pending_labels: Vec<String> = pending.iter().map(|p| p.label.clone()).collect();
        assert_eq!(pending_labels, all_except_first);
    }

    #[test]
    fn pending_partitions_skips_completed_labels() {
        let all = build_partitions(2026);
        let completed: Vec<String> = all.iter().take(3).map(|p| p.label.clone()).collect();
        let pending = pending_partitions(&all, &completed);
        assert_eq!(pending.len(), all.len() - 3);
        assert!(pending.iter().all(|p| !completed.contains(&p.label)));
    }

    #[test]
    fn pending_partitions_is_everything_when_nothing_completed() {
        let all = build_partitions(2026);
        let pending = pending_partitions(&all, &[]);
        assert_eq!(pending.len(), all.len());
    }

    #[test]
    fn pending_partitions_is_empty_when_everything_completed() {
        let all = build_partitions(2026);
        let completed: Vec<String> = all.iter().map(|p| p.label.clone()).collect();
        let pending = pending_partitions(&all, &completed);
        assert!(pending.is_empty());
    }

    #[test]
    fn sync_state_json_round_trip() {
        let state = CatalogSyncState {
            completed: vec!["pre-1940".to_string(), "decade-1940".to_string()],
            full_sync_completed_at: Some("2026-07-10T00:00:00Z".to_string()),
        };
        let json = state.to_json();
        let round_tripped = CatalogSyncState::from_json(&json);
        assert_eq!(round_tripped, state);
    }

    #[test]
    fn sync_state_from_missing_or_invalid_json_is_default() {
        assert_eq!(CatalogSyncState::from_json(""), CatalogSyncState::default());
        assert_eq!(CatalogSyncState::from_json("not json"), CatalogSyncState::default());
        assert_eq!(CatalogSyncState::from_json("{}"), CatalogSyncState::default());
    }

    // AniList rejects a GraphQL variable sent as an explicit JSON `null` for
    // startDate_greater/startDate_lesser with HTTP 400 "Illegal operator and
    // value combination" (verified live 2026-07-10 via direct curl against
    // the real API — this broke the very first live sync attempt). The key
    // must be absent from the `variables` object entirely for AniList to
    // treat the filter as "not provided" rather than "provided as null".
    // These tests assert absence, not `null`, for every inapplicable filter.

    #[test]
    fn build_variables_sets_status_only_for_not_yet_released_and_omits_date_keys() {
        let nyr = Partition { label: "not-yet-released".into(), filter: PartitionFilter::NotYetReleased, max_pages: None };
        let vars = build_variables(&nyr, 1, 50);
        assert_eq!(vars["status"], serde_json::json!("NOT_YET_RELEASED"));
        assert!(vars.get("startDateGreater").is_none(), "startDateGreater must be omitted, not null: {vars:?}");
        assert!(vars.get("startDateLesser").is_none(), "startDateLesser must be omitted, not null: {vars:?}");
    }

    #[test]
    fn build_variables_sets_date_bounds_and_omits_status_for_date_range_partition() {
        let range = Partition {
            label: "year-2000".into(),
            filter: PartitionFilter::DateRange { start_date_greater: Some(19_991_231), start_date_lesser: Some(20_010_101) },
            max_pages: None,
        };
        let vars = build_variables(&range, 3, 50);
        assert_eq!(vars["startDateGreater"], serde_json::json!(19_991_231));
        assert_eq!(vars["startDateLesser"], serde_json::json!(20_010_101));
        assert!(vars.get("status").is_none(), "status must be omitted, not null: {vars:?}");
        assert_eq!(vars["page"], serde_json::json!(3));
    }

    #[test]
    fn build_variables_omits_greater_key_when_only_lesser_bound_is_set() {
        // The pre-1940 partition: only an upper bound, no lower bound.
        let pre_1940 = Partition {
            label: "pre-1940".into(),
            filter: PartitionFilter::DateRange { start_date_greater: None, start_date_lesser: Some(19_400_101) },
            max_pages: None,
        };
        let vars = build_variables(&pre_1940, 1, 50);
        assert!(vars.get("startDateGreater").is_none(), "startDateGreater must be omitted, not null: {vars:?}");
        assert_eq!(vars["startDateLesser"], serde_json::json!(19_400_101));
        assert!(vars.get("status").is_none());
    }

    #[test]
    fn build_variables_omits_all_filter_keys_for_unfiltered_partition() {
        let catch_all = Partition { label: "catch-all".into(), filter: PartitionFilter::Unfiltered, max_pages: Some(100) };
        let vars = build_variables(&catch_all, 1, 50);
        assert!(vars.get("startDateGreater").is_none());
        assert!(vars.get("startDateLesser").is_none());
        assert!(vars.get("status").is_none());
        assert_eq!(vars["page"], serde_json::json!(1));
        assert_eq!(vars["perPage"], serde_json::json!(50));
    }

    #[test]
    fn synonyms_deserialize_and_dedup() {
        let json = serde_json::json!({
            "id": 1,
            "title": { "romaji": "Test", "english": null },
            "synonyms": ["Bai Ri Cheng Wang", "  ", "Bai Ri Cheng Wang"],
            "coverImage": { "large": null },
            "format": null,
            "genres": [],
            "episodes": null,
            "duration": null,
            "averageScore": null,
            "popularity": null,
            "siteUrl": "https://url",
            "status": null,
            "studios": null,
            "startDate": null
        });
        let entry: MediaEntry = serde_json::from_value(json).unwrap();
        let mapped = CatalogAnime::from(entry);
        assert_eq!(mapped.synonyms, vec!["Bai Ri Cheng Wang"]);

        let json_empty = serde_json::json!({
            "id": 2,
            "title": { "romaji": "Test", "english": null },
            "coverImage": { "large": null },
            "format": null,
            "genres": [],
            "episodes": null,
            "duration": null,
            "averageScore": null,
            "popularity": null,
            "siteUrl": "https://url",
            "status": null,
            "studios": null,
            "startDate": null
        });
        let entry_empty: MediaEntry = serde_json::from_value(json_empty).unwrap();
        let mapped_empty = CatalogAnime::from(entry_empty);
        assert!(mapped_empty.synonyms.is_empty());
    }

    #[test]
    fn mapping_handles_next_airing_and_tags() {
        let json = serde_json::json!({
            "id": 1,
            "title": { "romaji": "Test", "english": null },
            "coverImage": { "large": null },
            "format": null,
            "genres": [],
            "episodes": null,
            "duration": null,
            "averageScore": null,
            "popularity": null,
            "siteUrl": "https://url",
            "status": null,
            "studios": null,
            "startDate": null,
            "nextAiringEpisode": { "airingAt": 123456789, "episode": 5 },
            "tags": [
                { "name": "Spoiler Tag", "rank": 90, "isMediaSpoiler": true },
                { "name": "Low Rank Tag", "rank": 10, "isMediaSpoiler": false },
                { "name": "Good Tag", "rank": 85, "isMediaSpoiler": false },
                { "name": "Good Tag", "rank": 95, "isMediaSpoiler": false },
                { "name": "Null Rank", "rank": null, "isMediaSpoiler": null },
                { "name": "Null Spoiler Flag", "rank": 60, "isMediaSpoiler": null },
                { "name": "  Trim Tag  ", "rank": 50, "isMediaSpoiler": false },
                null
            ]
        });
        let entry: MediaEntry = serde_json::from_value(json).unwrap();
        let mapped = CatalogAnime::from(entry);
        assert_eq!(mapped.next_airing_at, Some(123456789));
        assert_eq!(mapped.next_episode, Some(5));
        assert_eq!(mapped.tags.len(), 3);
        assert_eq!(mapped.tags[0], CatalogTag { name: "Good Tag".into(), rank: 95 });
        assert_eq!(mapped.tags[1], CatalogTag { name: "Null Spoiler Flag".into(), rank: 60 });
        assert_eq!(mapped.tags[2], CatalogTag { name: "Trim Tag".into(), rank: 50 });

        let json_null = serde_json::json!({
            "id": 2,
            "title": { "romaji": "Test", "english": null },
            "coverImage": { "large": null },
            "format": null,
            "genres": [],
            "episodes": null,
            "duration": null,
            "averageScore": null,
            "popularity": null,
            "siteUrl": "https://url",
            "status": null,
            "studios": null,
            "startDate": null,
            "nextAiringEpisode": null,
            "tags": null
        });
        let entry_null: MediaEntry = serde_json::from_value(json_null).unwrap();
        let mapped_null = CatalogAnime::from(entry_null);
        assert_eq!(mapped_null.next_airing_at, None);
        assert_eq!(mapped_null.next_episode, None);
        assert!(mapped_null.tags.is_empty());

        let json_missing = serde_json::json!({
            "id": 3,
            "title": { "romaji": "Test", "english": null },
            "coverImage": { "large": null },
            "format": null,
            "genres": [],
            "episodes": null,
            "duration": null,
            "averageScore": null,
            "popularity": null,
            "siteUrl": "https://url",
            "status": null,
            "studios": null,
            "startDate": null
        });
        let entry_missing: MediaEntry = serde_json::from_value(json_missing).unwrap();
        let mapped_missing = CatalogAnime::from(entry_missing);
        assert_eq!(mapped_missing.next_airing_at, None);
        assert_eq!(mapped_missing.next_episode, None);
        assert!(mapped_missing.tags.is_empty());
    }

    #[test]
    fn mapping_handles_relations_and_filters_unwanted_edges() {
        // Must-pass example from spec:
        // [SEQUEL 185756 ANIME, SPIN_OFF 9 ANIME, PREQUEL 7 MANGA, SEQUEL null node]
        // yields exactly one relation (185756, "SEQUEL")
        let json = serde_json::json!({
            "id": 1,
            "title": { "romaji": "Test", "english": null },
            "coverImage": { "large": null },
            "format": null,
            "genres": [],
            "episodes": null,
            "duration": null,
            "averageScore": null,
            "popularity": null,
            "siteUrl": "https://url",
            "status": null,
            "studios": null,
            "startDate": null,
            "relations": {
                "edges": [
                    { "relationType": "SEQUEL", "node": { "id": 185756, "type": "ANIME", "format": "TV" } },
                    { "relationType": "SPIN_OFF", "node": { "id": 9, "type": "ANIME", "format": "TV" } },
                    { "relationType": "PREQUEL", "node": { "id": 7, "type": "MANGA", "format": null } },
                    { "relationType": "SEQUEL", "node": null },
                    // Deduplication test: identical relation ignored
                    { "relationType": "SEQUEL", "node": { "id": 185756, "type": "ANIME", "format": "TV" } },
                    // Non-positive id ignored
                    { "relationType": "SEQUEL", "node": { "id": 0, "type": "ANIME", "format": "TV" } },
                    { "relationType": "PREQUEL", "node": { "id": -5, "type": "ANIME", "format": "TV" } },
                    null
                ]
            }
        });
        let entry: MediaEntry = serde_json::from_value(json).unwrap();
        let mapped = CatalogAnime::from(entry);
        assert_eq!(
            mapped.relations,
            vec![CatalogRelation {
                related_id: 185756,
                relation_type: "SEQUEL".into(),
            }]
        );

        // Null and missing relations
        let json_null = serde_json::json!({
            "id": 2,
            "title": { "romaji": "Test", "english": null },
            "coverImage": { "large": null },
            "format": null,
            "genres": [],
            "episodes": null,
            "duration": null,
            "averageScore": null,
            "popularity": null,
            "siteUrl": "https://url",
            "status": null,
            "studios": null,
            "startDate": null,
            "relations": null
        });
        let entry_null: MediaEntry = serde_json::from_value(json_null).unwrap();
        let mapped_null = CatalogAnime::from(entry_null);
        assert!(mapped_null.relations.is_empty());

        let json_missing = serde_json::json!({
            "id": 3,
            "title": { "romaji": "Test", "english": null },
            "coverImage": { "large": null },
            "format": null,
            "genres": [],
            "episodes": null,
            "duration": null,
            "averageScore": null,
            "popularity": null,
            "siteUrl": "https://url",
            "status": null,
            "studios": null,
            "startDate": null
        });
        let entry_missing: MediaEntry = serde_json::from_value(json_missing).unwrap();
        let mapped_missing = CatalogAnime::from(entry_missing);
        assert!(mapped_missing.relations.is_empty());
    }

    #[test]
    fn search_query_builder() {
        let q = search_query();
        assert!(q.contains("query ($search: String)"));
        assert!(q.contains("media(type: ANIME, search: $search, sort: SEARCH_MATCH)"));
        assert!(q.contains(MEDIA_FIELDS));

        let body = serde_json::json!({
            "query": q,
            "variables": { "search": "Naruto" },
        });
        assert_eq!(body["variables"]["search"], "Naruto");
    }
    #[test]
    fn the_multi_page_query_has_one_aliased_page_per_requested_page() {
        let q = catalog_multi_query(7, 3);
        assert!(q.contains("p0: Page(page: 7, perPage: $perPage)"));
        assert!(q.contains("p1: Page(page: 8, perPage: $perPage)"));
        assert!(q.contains("p2: Page(page: 9, perPage: $perPage)"));
        assert!(!q.contains("p3:"));
        assert!(q.contains("$startDateGreater: FuzzyDateInt") && q.contains("$status: MediaStatus"));
        // The same media selection as the single-page query.
        assert!(q.matches("relations { edges").count() == 3);
    }

    #[test]
    fn the_ids_query_declares_one_variable_per_chunk() {
        let q = by_ids_multi_query(3);
        assert!(q.starts_with("query ($ids0: [Int], $ids1: [Int], $ids2: [Int], $perPage: Int)"));
        assert!(q.contains("b2: Page(page: 1, perPage: $perPage)"));
        assert!(q.contains("id_in: $ids1"));
    }

    fn page_json(ids: &[i64], has_next: bool) -> serde_json::Value {
        let media: Vec<serde_json::Value> = ids
            .iter()
            .map(|id| {
                serde_json::json!({
                    "id": id, "title": { "romaji": format!("T{id}"), "english": null }, "synonyms": [],
                    "coverImage": { "large": null }, "format": null, "genres": [], "episodes": null,
                    "duration": null, "averageScore": null, "popularity": null, "siteUrl": format!("https://x/{id}"),
                    "status": null, "studios": null, "startDate": null, "nextAiringEpisode": null,
                    "tags": [], "relations": null
                })
            })
            .collect();
        serde_json::json!({ "pageInfo": { "hasNextPage": has_next }, "media": media })
    }

    #[test]
    fn aliased_pages_are_joined_in_order_and_the_last_has_next_wins() {
        let data = serde_json::json!({
            "p1": page_json(&[3, 4], true),
            "p0": page_json(&[1, 2], true),
            "p2": page_json(&[5], false),
        });
        let (items, has_next) = collect_aliased_pages(data, "p", 3, "t").unwrap();
        assert_eq!(items.iter().map(|a| a.id).collect::<Vec<_>>(), vec![1, 2, 3, 4, 5]);
        assert!(!has_next, "the partition ended inside this batch");

        let data = serde_json::json!({ "p0": page_json(&[1], true), "p1": page_json(&[2], true) });
        assert!(collect_aliased_pages(data, "p", 2, "t").unwrap().1);
    }

    #[test]
    fn a_missing_alias_is_an_error_not_a_silent_short_batch() {
        let data = serde_json::json!({ "p0": page_json(&[1], true) });
        assert!(collect_aliased_pages(data, "p", 2, "t").is_err());
        assert!(collect_aliased_pages(serde_json::json!([1, 2]), "p", 1, "t").is_err());
    }

    #[test]
    fn one_request_carries_at_most_ten_pages_of_fifty() {
        assert_eq!(PAGES_PER_REQUEST, 10);
        assert_eq!(MAX_IDS_PER_REQUEST * PAGES_PER_REQUEST as usize, 500);
    }

    #[test]
    fn media_extras_keep_every_anime_relation_type_and_drop_non_anime_nodes() {
        let data = serde_json::json!({ "Media": {
            "id": 1,
            "description": "  A story.<br><br>~!Twist!~  ",
            "relations": { "edges": [
                { "relationType": "SEQUEL", "node": { "id": 2, "type": "ANIME", "format": "TV",
                  "title": { "romaji": "Dai Ni", "english": "Part Two" },
                  "coverImage": { "medium": "https://img/2" }, "startDate": { "year": 2021 } } },
                { "relationType": "SIDE_STORY", "node": { "id": 3, "type": "ANIME", "format": "OVA",
                  "title": { "romaji": "Gaiden", "english": null }, "coverImage": null, "startDate": { "year": null } } },
                { "relationType": "ADAPTATION", "node": { "id": 4, "type": "MANGA", "title": { "romaji": "Manga" } } },
                { "relationType": "SEQUEL", "node": { "id": 2, "type": "ANIME", "title": { "romaji": "Dup" } } },
                { "relationType": "PREQUEL", "node": null },
                null
            ] }
        }});
        let extras = parse_media_extras(data).unwrap().expect("media present");
        assert_eq!(extras.description.as_deref(), Some("A story.<br><br>~!Twist!~"));
        assert_eq!(
            extras.relations,
            vec![
                RelatedMedia { id: 2, relation_type: "SEQUEL".into(), title: "Part Two".into(),
                    cover_url: Some("https://img/2".into()), format: Some("TV".into()), year: Some(2021) },
                RelatedMedia { id: 3, relation_type: "SIDE_STORY".into(), title: "Gaiden".into(),
                    cover_url: None, format: Some("OVA".into()), year: None },
            ]
        );
    }

    #[test]
    fn media_extras_null_media_is_none_and_blank_description_is_none() {
        assert_eq!(parse_media_extras(serde_json::json!({ "Media": null })).unwrap(), None);
        let e = parse_media_extras(serde_json::json!({ "Media": { "description": "   ", "relations": null } }))
            .unwrap()
            .unwrap();
        assert_eq!(e, MediaExtras { description: None, relations: vec![], trailer: None });
    }

    #[test]
    fn media_extras_keep_a_valid_trailer_and_drop_bad_ones() {
        let parse = |trailer: serde_json::Value| {
            parse_media_extras(serde_json::json!({ "Media": { "trailer": trailer } })).unwrap().unwrap().trailer
        };
        assert_eq!(
            parse(serde_json::json!({ "id": "abc_-123", "site": "YouTube" })),
            Some(Trailer { site: "youtube".into(), id: "abc_-123".into() })
        );
        assert_eq!(
            parse(serde_json::json!({ "id": "x9", "site": "dailymotion" })),
            Some(Trailer { site: "dailymotion".into(), id: "x9".into() })
        );
        assert_eq!(parse(serde_json::json!({ "id": "abc\"><script>", "site": "youtube" })), None);
        assert_eq!(parse(serde_json::json!({ "id": "", "site": "youtube" })), None);
        assert_eq!(parse(serde_json::json!({ "id": "a".repeat(65), "site": "youtube" })), None);
        assert_eq!(parse(serde_json::json!({ "id": "abc", "site": "vimeo" })), None);
        assert_eq!(parse(serde_json::json!(null)), None);
    }
}
