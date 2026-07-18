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

const CATALOG_QUERY: &str = r#"
query ($page: Int, $perPage: Int, $startDateGreater: FuzzyDateInt, $startDateLesser: FuzzyDateInt, $status: MediaStatus) {
  Page(page: $page, perPage: $perPage) {
    pageInfo { hasNextPage }
    media(
      type: ANIME
      sort: ID
      startDate_greater: $startDateGreater
      startDate_lesser: $startDateLesser
      status: $status
    ) {
      id
      title { romaji english }
      coverImage { large }
      format
      genres
      episodes
      averageScore
      popularity
      siteUrl
      status
    }
  }
}
"#;

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
    pub cover_url: Option<String>,
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
}

#[derive(Debug, Deserialize)]
struct GraphQlResponse {
    data: Option<ResponseData>,
    errors: Option<Vec<GraphQlError>>,
}

#[derive(Debug, Deserialize)]
struct GraphQlError {
    message: String,
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
    #[serde(rename = "coverImage")]
    cover_image: Option<CoverImage>,
    format: Option<String>,
    genres: Vec<String>,
    episodes: Option<i64>,
    #[serde(rename = "averageScore")]
    average_score: Option<i64>,
    popularity: Option<i64>,
    #[serde(rename = "siteUrl")]
    site_url: String,
    #[serde(default)]
    status: Option<String>,
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
        CatalogAnime {
            id: m.id,
            title,
            title_romaji: m.title.romaji,
            title_english: m.title.english,
            cover_url: m.cover_image.and_then(|c| c.large),
            format: m.format,
            genres: m.genres,
            episodes: m.episodes,
            average_score: m.average_score,
            popularity: m.popularity,
            url: m.site_url,
            status: m.status,
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
/// 1. One pre-modern range (`startDate_lesser: 19400101` — a few hundred
///    titles total).
/// 2. One partition per year, 1940 through `current_year + 2` — no single
///    year approaches the 5000-row cap.
/// 3. One `NOT_YET_RELEASED` partition (undated upcoming titles).
/// 4. One final unfiltered `sort: ID` pass, capped at the first 100 pages
///    (the 5000-row cap) — catches undated *non*-upcoming leftovers among
///    the oldest ids. Anything past that (undated + not upcoming + id
///    beyond the first 5000) is an accepted, documented gap.
///
/// Duplicates across partitions are harmless — `upsert_catalog_anime` is
/// keyed on AniList's own numeric id.
pub fn build_partitions(current_year: i32) -> Vec<Partition> {
    let mut partitions = Vec::new();
    partitions.push(Partition {
        label: "pre-1940".to_string(),
        filter: PartitionFilter::DateRange {
            start_date_greater: None,
            start_date_lesser: Some(19_400_101),
        },
        max_pages: None,
    });
    for year in 1940..=(current_year + 2) {
        partitions.push(Partition {
            label: format!("year-{year}"),
            filter: PartitionFilter::DateRange {
                start_date_greater: Some((year as i64 - 1) * 10_000 + 1231),
                start_date_lesser: Some((year as i64 + 1) * 10_000 + 101),
            },
            max_pages: None,
        });
    }
    partitions.push(Partition {
        label: "not-yet-released".to_string(),
        filter: PartitionFilter::NotYetReleased,
        max_pages: None,
    });
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

/// Fetch one page of one partition. Retries on HTTP 429 honoring the
/// `Retry-After` header (seconds), capped at 5 attempts — failing loudly
/// after the cap beats retrying forever indistinguishably from a real hang.
pub async fn fetch_partition_page(partition: &Partition, page: i64, per_page: i64) -> Result<PartitionPage> {
    const MAX_RETRIES: u32 = 5;

    let client = reqwest::Client::new();
    let variables = build_variables(partition, page, per_page);
    let body = serde_json::json!({ "query": CATALOG_QUERY, "variables": variables });

    let mut attempt = 0u32;
    loop {
        let resp = client
            .post(ENDPOINT)
            .json(&body)
            .send()
            .await
            .map_err(|e| anyhow!("AniList request failed ({} page {page}): {e}", partition.label))?;

        if resp.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
            attempt += 1;
            if attempt > MAX_RETRIES {
                return Err(anyhow!(
                    "AniList rate-limited partition '{}' page {page} after {MAX_RETRIES} retries",
                    partition.label
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

        let parsed: GraphQlResponse = resp.json().await.map_err(|e| {
            anyhow!("AniList response was not valid JSON ({} page {page}): {e}", partition.label)
        })?;
        if let Some(errors) = parsed.errors {
            let msg = errors.into_iter().map(|e| e.message).collect::<Vec<_>>().join("; ");
            return Err(anyhow!("AniList returned an error ({} page {page}): {msg}", partition.label));
        }
        let data = parsed
            .data
            .ok_or_else(|| anyhow!("AniList response had no data ({} page {page})", partition.label))?;
        let items: Vec<CatalogAnime> = data.page.media.into_iter().map(CatalogAnime::from).collect();
        return Ok(PartitionPage {
            items,
            has_next_page: data.page.page_info.has_next_page,
            rate_remaining,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_partitions_starts_with_pre_1940_and_ends_with_catch_all() {
        let partitions = build_partitions(2026);
        assert_eq!(partitions.first().unwrap().label, "pre-1940");
        assert_eq!(partitions.last().unwrap().label, "catch-all");
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
    fn build_partitions_has_no_year_gap_from_1940_through_current_plus_2() {
        let partitions = build_partitions(2026);
        for year in 1940..=2028 {
            assert!(
                partitions.iter().any(|p| p.label == format!("year-{year}")),
                "missing year-{year}"
            );
        }
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
        assert!(!inc.iter().any(|p| p.label == "year-1999"));
        assert!(!inc.iter().any(|p| p.label == "catch-all"));
    }

    #[test]
    fn incremental_partitions_includes_recent_years_and_not_yet_released() {
        let inc = incremental_partitions(2026);
        assert!(inc.iter().any(|p| p.label == "year-2025"));
        assert!(inc.iter().any(|p| p.label == "year-2026"));
        assert!(inc.iter().any(|p| p.label == "year-2028"));
        assert!(inc.iter().any(|p| p.filter == PartitionFilter::NotYetReleased));
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
            completed: vec!["pre-1940".to_string(), "year-1940".to_string()],
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
}
