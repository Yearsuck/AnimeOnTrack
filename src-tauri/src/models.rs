use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Series {
    pub id: i64,
    pub slug: String,
    pub title: String,
    pub url: String,
    pub cover_url: Option<String>,
    pub is_airing: bool,
    pub followed: bool,
    /// Unix timestamp of the next episode's release, parsed from the airing
    /// listing's `data-rlsdt` attribute (see `SiteAdapter::parse_airing`).
    /// Only ever fresh for series currently on the airing listing — scan-owned,
    /// like `followed`, and written by `Db::upsert_series`. `None` for series
    /// that have never been seen on the airing listing, or whose card carried
    /// no countdown span.
    #[serde(default)]
    pub next_episode_at: Option<i64>,
    /// The site's own reported episode count for this series (the `.sb` badge
    /// on its airing card), scan-owned like `next_episode_at`. `None` when
    /// the badge is missing or non-numeric (e.g. `"??"`).
    #[serde(default)]
    pub site_episode_count: Option<i64>,
}

/// The lowest-numbered unseen episode of a followed series (`LibraryItem`'s
/// "resume here" affordance) — same ordering `list_series_episodes` uses
/// (`CAST(number AS INTEGER) ASC, id ASC`), so this can never disagree with
/// what `SeriesDetail` shows as "next".
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NextEpisode {
    pub number: String,
    pub title: Option<String>,
    pub url: String,
}

/// A followed series plus its episode counts, for the library view.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LibraryItem {
    pub series: Series,
    pub total_episodes: i64,
    pub seen_episodes: i64,
    /// Insertion timestamp (`datetime('now')`, ISO 8601) of the most recently
    /// scraped episode — used to sort "recently active" first. Not the
    /// episode's air date (that's free-text and not reliably sortable).
    pub last_added: Option<String>,
    /// Lowest-numbered unseen episode, or `None` when every episode is seen
    /// (or there are none). Library view's "▶ Episodio n" affordance.
    #[serde(default)]
    pub next_episode: Option<NextEpisode>,
    /// `MAX(episodes.seen_at)` — when the user last marked an episode of
    /// this series seen (not when it was scraped; see `episodes.seen_at`).
    /// Drives "Viendo"/"Completadas" most-recent-first ordering.
    #[serde(default)]
    pub last_watched_at: Option<String>,
    /// Mirrors `series.watched_externally` — set by a catalog "Ya lo vi"
    /// swipe (`decide_catalog_card`'s Seen decision), which never scrapes
    /// episodes. Lets the frontend classify a zero-episode row as
    /// "Completadas" instead of reading it as an empty/errored followed
    /// series. See docs/superpowers/specs/2026-07-12-ya-lo-vi-library-visibility-design.md.
    #[serde(default)]
    pub watched_externally: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Episode {
    pub id: i64,
    pub series_id: i64,
    pub number: String,
    pub title: Option<String>,
    pub url: String,
    pub released_at: Option<String>,
    pub seen: bool,
}

/// A completed-anime card scraped off a genre-listing page (a `.bsx` card
/// carrying a `.status.Completed` div). Also doubles as the swipe-mode UI's
/// card payload as-is (see `SwipeCard`) — the swipe deck shows exactly what
/// the adapter parses off the listing page, no separate shape needed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FinishedCard {
    pub title: String,
    pub url: String,
    pub poster_url: Option<String>,
    pub kind: String,
    /// Which genre archive this card was found under — only known/set by
    /// `discover_swipe_card` (which is the one place a genre is picked
    /// before scraping), `None` everywhere else `FinishedCard` is built.
    #[serde(default)]
    pub matched_genre: Option<String>,
}

pub type SwipeCard = FinishedCard;

/// Parsed from a series detail page (`/tv/{slug}/`) — the only place a
/// series' *complete* genre set and authoritative type ("Tipo:") are
/// available; listing cards only imply the one genre archive they were
/// found under.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SeriesDetail {
    pub genres: Vec<String>,
    pub kind: Option<String>,
    pub synopsis: Option<String>,
}

/// Aggregate watch counts for the stats dashboard, all scoped to a single
/// `source_id` and to `followed=1` series (except `backlog_want`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WatchSummary {
    pub followed_series: i64,
    /// Distinct animes among followed/watched-externally series, collapsing
    /// seasons of the same show into one (see `db::franchise_key`).
    #[serde(default)]
    pub distinct_anime: i64,
    pub episodes_watched: i64,
    pub episodes_total: i64,
    pub backlog_want: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GenreStat {
    pub genre: String,
    pub count: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TypeStat {
    pub kind: String,
    pub count: i64,
}

/// One genre's affinity score (see `Db::get_genre_affinity`), for surfacing
/// "your favorite genres" in the swipe UI.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GenreAffinity {
    pub genre: String,
    pub score: f64,
}

/// One followed series' graph-relevant fields, for the 3D relationship graph
/// (`get_stats_graph`) — the frontend builds the root/hub/link structure from
/// this flat list itself, no hub-aggregation duplicated here.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SeriesGraphNode {
    pub id: i64,
    pub title: String,
    pub cover_url: Option<String>,
    pub genres: Vec<String>,
    pub kind: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn series_json_roundtrips() {
        let s = Series {
            id: 1,
            slug: "baki-dou".into(),
            title: "Baki-dou".into(),
            url: "https://wwv.animeytx.net/tv/baki-dou/".into(),
            cover_url: Some("https://x/img.jpg".into()),
            is_airing: true,
            followed: false, next_episode_at: None, site_episode_count: None,
        };
        let j = serde_json::to_string(&s).unwrap();
        let back: Series = serde_json::from_str(&j).unwrap();
        assert_eq!(s, back);
    }

    #[test]
    fn episode_json_roundtrips() {
        let e = Episode {
            id: 5,
            series_id: 1,
            number: "1x05".into(),
            title: Some("Ep 5".into()),
            url: "https://wwv.animeytx.net/episodio/baki-dou-5/".into(),
            released_at: None,
            seen: false,
        };
        let j = serde_json::to_string(&e).unwrap();
        let back: Episode = serde_json::from_str(&j).unwrap();
        assert_eq!(e, back);
    }

    #[test]
    fn finished_card_json_roundtrips() {
        let c = FinishedCard {
            title: "Liar Game".into(),
            url: "https://wwv.animeytx.net/tv/liar-game/".into(),
            poster_url: Some("https://x/img.jpg".into()),
            kind: "TV".into(),
            matched_genre: Some("Drama".into()),
        };
        let j = serde_json::to_string(&c).unwrap();
        let back: FinishedCard = serde_json::from_str(&j).unwrap();
        assert_eq!(c, back);
    }

    #[test]
    fn series_graph_node_json_roundtrips() {
        let n = SeriesGraphNode {
            id: 1,
            title: "Baki-dou".into(),
            cover_url: Some("data:image/png;base64,abc".into()),
            genres: vec!["Seinen".into(), "Drama".into()],
            kind: Some("TV".into()),
        };
        let j = serde_json::to_string(&n).unwrap();
        let back: SeriesGraphNode = serde_json::from_str(&j).unwrap();
        assert_eq!(n, back);
    }

    #[test]
    fn series_detail_json_roundtrips() {
        let d = SeriesDetail {
            genres: vec!["Drama".into(), "Seinen".into()],
            kind: Some("TV".into()),
            synopsis: Some("...".into()),
        };
        let j = serde_json::to_string(&d).unwrap();
        let back: SeriesDetail = serde_json::from_str(&j).unwrap();
        assert_eq!(d, back);
    }
}
