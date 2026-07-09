//! Thin client for AniList's public GraphQL API — the source for the
//! "Catálogo" tab's full anime catalog (independent of what's scraped off
//! the tracked site) and for feeding catalog cards into Descubrir.
//!
//! Chosen over MyAnimeList/Jikan, Kitsu and AniDB: no API key or OAuth
//! registration needed for public read queries, a single GraphQL request
//! returns exactly the fields we want (title/cover/genres/format/episodes)
//! for a whole page instead of N+1 REST round-trips, and cover images are
//! served reliably off AniList's own CDN in multiple sizes. Rate limit is
//! 30 req/min (unauthenticated) — fine for paginated browsing, not for
//! bulk-downloading the ~5000+ title catalog in one shot, so this fetches
//! one page at a time on demand rather than pretending "the whole catalog"
//! can be pulled in a single request.

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};

const ENDPOINT: &str = "https://graphql.anilist.co";

const CATALOG_QUERY: &str = r#"
query ($page: Int, $perPage: Int) {
  Page(page: $page, perPage: $perPage) {
    pageInfo { hasNextPage lastPage }
    media(type: ANIME, sort: POPULARITY_DESC) {
      id
      title { romaji english }
      coverImage { large }
      format
      genres
      episodes
      averageScore
      siteUrl
    }
  }
}
"#;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CatalogAnime {
    pub id: i64,
    pub title: String,
    pub cover_url: Option<String>,
    pub format: Option<String>,
    pub genres: Vec<String>,
    pub episodes: Option<i64>,
    pub average_score: Option<i64>,
    pub url: String,
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

#[derive(Debug, Deserialize)]
struct PageInfo {
    #[serde(rename = "hasNextPage")]
    has_next_page: bool,
    #[serde(rename = "lastPage")]
    last_page: i64,
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
    #[serde(rename = "siteUrl")]
    site_url: String,
}

#[derive(Debug, Deserialize)]
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
        let title = m.title.english.or(m.title.romaji).unwrap_or_default();
        CatalogAnime {
            id: m.id,
            title,
            cover_url: m.cover_image.and_then(|c| c.large),
            format: m.format,
            genres: m.genres,
            episodes: m.episodes,
            average_score: m.average_score,
            url: m.site_url,
        }
    }
}

/// One page of the full AniList anime catalog, sorted by popularity so the
/// most recognizable titles surface first. `last_page` lets the frontend
/// show/clamp page navigation without a separate count query.
pub async fn fetch_catalog_page(page: i64, per_page: i64) -> Result<(Vec<CatalogAnime>, bool, i64)> {
    let client = reqwest::Client::new();
    let body = serde_json::json!({
        "query": CATALOG_QUERY,
        "variables": { "page": page, "perPage": per_page }
    });
    let resp = client
        .post(ENDPOINT)
        .json(&body)
        .send()
        .await
        .map_err(|e| anyhow!("AniList request failed: {e}"))?;
    let parsed: GraphQlResponse = resp
        .json()
        .await
        .map_err(|e| anyhow!("AniList response was not valid JSON: {e}"))?;
    if let Some(errors) = parsed.errors {
        let msg = errors.into_iter().map(|e| e.message).collect::<Vec<_>>().join("; ");
        return Err(anyhow!("AniList returned an error: {msg}"));
    }
    let data = parsed.data.ok_or_else(|| anyhow!("AniList response had no data"))?;
    let items: Vec<CatalogAnime> = data.page.media.into_iter().map(CatalogAnime::from).collect();
    Ok((items, data.page.page_info.has_next_page, data.page.page_info.last_page))
}
