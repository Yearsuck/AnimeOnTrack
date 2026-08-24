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

/// One airing series plus its parsed first-episode date, for the "Esta
/// temporada" filter (`list_airing_season`). Kept as its own struct rather
/// than adding a field to `Series` — `Series` is the scan-owned shared model
/// with many callers/fixtures, and this timestamp is derived read-side from
/// `episodes.released_at`, not scan-owned. See
/// docs/superpowers/specs/2026-07-13-airing-this-season-design.md.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AiringItem {
    pub series: Series,
    /// Unix timestamp (UTC midnight) of the series' lowest-numbered episode's
    /// release date, parsed via `dates::parse_spanish_date`. `None` when the
    /// series has no scraped episodes yet or its first episode's
    /// `released_at` didn't parse — most airing series fall in this bucket
    /// (episodes are only scraped on-demand, see [[project-scraping-scope]]),
    /// so `None` here means "unknown", not "old".
    #[serde(default)]
    pub first_episode_at: Option<i64>,
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
    /// `series.kind` ("TV"/"MOVIE"/"Pelicula"/"OVA"/"ONA"/"SPECIAL"/site
    /// quality tags like "4K"/"Blu-Ray"/`None`). Raw/unnormalized — the
    /// frontend buckets the messy values (see the library-filters design
    /// doc), this is just a passthrough of the column.
    #[serde(default)]
    pub kind: Option<String>,
    /// This series' rows from `series_genres`, sorted by genre. Empty when
    /// genres haven't been backfilled yet for this series.
    #[serde(default)]
    pub genres: Vec<String>,
    /// Only populated for series linked to an AniList catalog row
    /// (`anilist_id` set) — scraped-only followed series have no native
    /// studio data and this stays `None`.
    #[serde(default)]
    pub studio: Option<String>,
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
/// card payload as-is — the swipe deck shows exactly what the adapter parses
/// off the listing page, no separate shape needed (the frontend calls this
/// same payload a `SwipeCard`).
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
    /// Kept for compatibility (still used by the graph/other callers); no
    /// longer drives a "en seguimiento" tile — see `airing_followed`.
    pub followed_series: i64,
    /// Distinct animes among followed/watched-externally series, collapsing
    /// seasons of the same show into one (see `db::franchise_key`).
    #[serde(default)]
    pub distinct_anime: i64,
    pub episodes_watched: i64,
    pub episodes_total: i64,
    /// Episodes attributed to "Ya vistas" (`watched_externally=1`) series via
    /// the catalog's episode count — only for series with NO real seen
    /// episode data (real data always wins; see `Db::get_watch_summary`).
    #[serde(default)]
    pub episodes_watched_external: i64,
    /// Followed series still airing (`followed=1 AND is_airing=1`) — the
    /// "Siguiendo en emisión" tile.
    #[serde(default)]
    pub airing_followed: i64,
    /// Followed series with at least one unseen episode — the real
    /// "Pendientes de ver" backlog.
    #[serde(default)]
    pub pending_to_watch: i64,
    /// The `Quiero ver` wishlist from Descubrir (`backlog_status='want'`),
    /// unrelated to episodes actually pending. Surfaced under its own
    /// unambiguous label.
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

/// One calendar day's episode-marking count, e.g. `{"2026-07-13", 160}` — see
/// `Db::marks_by_day`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DayCount {
    pub day: String,
    pub count: i64,
}

/// One series' watched-episode count, e.g. `{"One Piece: Arco de Elbaph",
/// 114}` — see `Db::top_series_by_episodes_watched`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TitleCount {
    pub title: String,
    pub count: i64,
}

/// A followed series that has gone quiet — no episode marked seen in 90+ days.
/// Returned by `Db::get_dusty_watchlist`. Canonical across sites (deduped via
/// `canon_key`), so the same show followed on two sites appears once.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DustyEntry {
    pub title: String,
    pub last_seen_at: String,
}

/// One day's binge record: the calendar day with the highest episode-marking
/// count and that count. Mirrors the day-bucket query in `get_watch_insights`
/// (localtime conversion, see that function's comment for why).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BingeRecord {
    pub day: Option<String>,
    pub count: i64,
}

/// One hour's episode-marking count, e.g. `{ hour: 14, count: 42 }` — see
/// `Db::get_hourly_distribution`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HourCount {
    pub hour: i64,
    pub count: i64,
}

/// Local-only watch metrics for the Estadísticas "Resumen" block — see the
/// design doc `docs/superpowers/specs/2026-07-13-stats-new-metrics-design.md`.
/// Computed entirely from SQLite (no network) via `Db::get_watch_insights`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WatchInsights {
    /// Estimated minutes watched across followed series' seen episodes
    /// (`series.kind` -> `db::minutes_per_episode`).
    pub estimated_minutes_tracked: i64,
    /// Estimated minutes watched across "Ya vistas" (`watched_externally=1`)
    /// series that are linked to a catalog row with a known episode count.
    pub estimated_minutes_external: i64,
    /// How many "Ya vistas" rows actually contributed to
    /// `estimated_minutes_external` (linked + catalog `episodes` non-NULL).
    pub external_titles_estimated: i64,
    /// Total "Ya vistas" rows, for an honest "{estimated} of {total}" caption.
    pub external_titles_total: i64,
    /// Mean episode count (all episodes, not just seen) across followed
    /// series.
    pub avg_episodes_per_series: f64,
    pub followed_airing: i64,
    pub followed_finished: i64,
    pub discarded: i64,
    pub want: i64,
    pub watched_externally: i64,
    /// Top series by watched-episode count, descending, capped at 8.
    pub top_series: Vec<TitleCount>,
    /// Days (within the last 30) that had at least one episode marked seen,
    /// chronological. Empty if `episodes.seen_at` has no data yet.
    pub marks_by_day: Vec<DayCount>,
    /// `MIN(DATE(seen_at))` across all episodes — earliest date the
    /// "marked seen" data goes back to, for the UI's honesty disclaimer.
    /// `None` when no episode has ever been marked seen.
    pub marks_tracked_since: Option<String>,
}

/// Snapshot of cloud-backup state shown in Settings — whether Google
/// credentials were compiled in, whether the user connected their Drive,
/// and the last backup's timestamp/size.
#[derive(serde::Serialize)]
pub struct BackupStatus {
    pub configured: bool,
    pub connected: bool,
    pub last_at: Option<String>,
    pub size_bytes: Option<i64>,
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
    fn airing_item_json_roundtrips() {
        let item = AiringItem {
            series: Series {
                id: 1,
                slug: "baki-dou".into(),
                title: "Baki-dou".into(),
                url: "https://wwv.animeytx.net/tv/baki-dou/".into(),
                cover_url: None,
                is_airing: true,
                followed: false,
                next_episode_at: None,
                site_episode_count: None,
            },
            first_episode_at: Some(1_780_000_000),
        };
        let j = serde_json::to_string(&item).unwrap();
        let back: AiringItem = serde_json::from_str(&j).unwrap();
        assert_eq!(item, back);
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

/// Result of importing the canonical library onto the active site: how many
/// stranded entries were searched, how many actually linked, and how many
/// couldn't be resolved (no anilist id, or no confident match on this site).
#[derive(Debug, Clone, Serialize)]
pub struct LibraryImportResult {
    pub total: usize,
    pub linked: usize,
    pub skipped: usize,
}

/// Per-entry progress for the library import (Tauri event
/// `library-import-progress`), so the UI can show "Trayendo X (i/total)".
#[derive(Debug, Clone, Serialize)]
pub struct LibraryImportProgress {
    pub done: usize,
    pub total: usize,
    pub title: String,
}
