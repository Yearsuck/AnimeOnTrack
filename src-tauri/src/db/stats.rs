use super::*;
use std::collections::{HashMap, HashSet};

/// Collapse a series title to a season/part-agnostic "franchise key" so
/// multiple seasons of the same show count as one anime in the stats
/// (`get_watch_summary`'s distinct-anime count). Normalizes via
/// `matching::normalize_title` (accents/case/punctuation/noise stripped), then
/// strips trailing season/part markers: "temporada N", "season N", "part N",
/// "parte N", "cour N", "Nth season", "final season", a trailing standalone
/// integer, and trailing Roman numerals (ii..x). It's a heuristic, not a
/// canonical grouping — good enough to keep "X" and "X Temporada 2" together
/// without a metadata source. Never returns "" (an all-marker title keeps its
/// normalized form).
pub(crate) fn franchise_key(title: &str) -> String {
    const ROMANS: &[&str] = &["ii", "iii", "iv", "v", "vi", "vii", "viii", "ix", "x"];
    const MARKER_WORDS: &[&str] =
        &["season", "temporada", "part", "parte", "cour", "final", "the"];
    let norm = crate::matching::normalize_title(title);
    let mut tokens: Vec<&str> = norm.split_whitespace().collect();
    while let Some(&last) = tokens.last() {
        let is_int = !last.is_empty() && last.chars().all(|c| c.is_ascii_digit());
        let is_roman = ROMANS.contains(&last);
        let is_marker = MARKER_WORDS.contains(&last);
        // "2nd"/"3rd"/"1st"/"4th" ordinal (digits + st/nd/rd/th).
        let is_ordinal = last.len() > 2
            && matches!(&last[last.len() - 2..], "st" | "nd" | "rd" | "th")
            && last[..last.len() - 2].chars().all(|c| c.is_ascii_digit());
        if is_int || is_roman || is_marker || is_ordinal {
            tokens.pop();
        } else {
            break;
        }
    }
    let key = tokens.join(" ");
    if key.is_empty() {
        norm
    } else {
        key
    }
}

/// Minutes estimated per episode by format/kind, case-insensitively. This is
/// the *fallback* estimate — `anilist_catalog.duration` (AniList's real
/// per-episode minutes, synced per `anilist::CatalogAnime::duration`) wins
/// over this whenever a series is linked to a catalog row that has one; see
/// `get_watch_insights`. This function only ever runs for unlinked series, or
/// catalog rows synced before `duration` existed / with no duration on
/// AniList. The scraped site's own `series.kind` is free-text vocabulary, not
/// a validated enum (real values seen live: `TV`, `MOVIE`, `4K`, `Pelicula`,
/// `OVA`, `ONA`, `Sin Censura`, `SPECIAL`, `Blu-Ray`, `Resubido`, `Yaoi`...),
/// so this remains an explicit, documented *estimate*, not a fact — callers
/// must present it as one. Anything not recognized (including all the site's
/// noise and `None`) falls back to the plain-TV estimate of 24 minutes.
pub(crate) fn minutes_per_episode(format: Option<&str>) -> i64 {
    match format.map(|f| f.trim().to_uppercase()).as_deref() {
        Some("MOVIE") | Some("PELICULA") | Some("PELÍCULA") => 100,
        Some("MUSIC") => 5,
        Some("TV_SHORT") => 8,
        Some("OVA") | Some("SPECIAL") => 26,
        _ => 24,
    }
}

impl Db {
    pub fn get_genre_stats(&self, source_id: i64) -> Result<Vec<crate::models::GenreStat>> {
        let mut stmt = self.conn.prepare(
            "SELECT g.genre, COUNT(DISTINCT g.series_id) AS cnt
             FROM series_genres g
             JOIN series s ON s.id = g.series_id
             WHERE s.source_id=?1 AND s.followed=1
             GROUP BY g.genre
             ORDER BY cnt DESC, g.genre",
        )?;
        let rows = stmt
            .query_map([source_id], |r| {
                Ok(crate::models::GenreStat { genre: r.get(0)?, count: r.get(1)? })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Followed-series count per `kind` ("TV"/"OVA"/...), descending. Series
    /// with no `kind` set are excluded.
    pub fn get_type_stats(&self, source_id: i64) -> Result<Vec<crate::models::TypeStat>> {
        let mut stmt = self.conn.prepare(
            "SELECT s.kind, COUNT(*) AS cnt
             FROM series s
             WHERE s.source_id=?1 AND s.followed=1 AND s.kind IS NOT NULL AND s.kind <> ''
             GROUP BY s.kind
             ORDER BY cnt DESC, s.kind",
        )?;
        let rows = stmt
            .query_map([source_id], |r| {
                Ok(crate::models::TypeStat { kind: r.get(0)?, count: r.get(1)? })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Per-genre affinity score, used to weight the swipe deck's genre pick
    /// toward what this user actually likes instead of picking uniformly at
    /// random. +2 per followed series in that genre (actively watching or
    /// already watched — the strongest signal), +1 per 'want' backlog series
    /// (interested, not started yet), -1.5 per 'discarded' series (explicit
    /// pass). Genres with no signal simply don't appear in the map — the
    /// caller treats a missing entry as 0, and `weighted_pick_index` falls
    /// back to uniform whenever every candidate nets <= 0, so a new user (or
    /// a genre nobody's decided on yet) never gets a silently empty deck.
    pub fn get_genre_affinity(&self, source_id: i64) -> Result<HashMap<String, f64>> {
        let mut stmt = self.conn.prepare(
            "SELECT sg.genre, s.followed, s.backlog_status
             FROM series_genres sg
             JOIN series s ON s.id = sg.series_id
             WHERE s.source_id=?1",
        )?;
        let rows: Vec<(String, bool, Option<String>)> = stmt
            .query_map([source_id], |r| {
                Ok((r.get(0)?, r.get::<_, i64>(1)? != 0, r.get(2)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        let mut scores: HashMap<String, f64> = HashMap::new();
        for (genre, followed, backlog_status) in rows {
            let delta = if followed {
                2.0
            } else {
                match backlog_status.as_deref() {
                    Some("want") => 1.0,
                    Some("discarded") => -1.5,
                    _ => 0.0,
                }
            };
            // Fold onto AniList's canonical genre name when this raw tag has
            // one (e.g. site's "Acción" and catalog's "Action" both become
            // "Action"), so the same taste signal from either source lands
            // in one bucket instead of two unrelated ones. Tags with no
            // AniList equivalent (e.g. "Isekai") keep their raw name — they
            // still carry real signal for the site path's genre pages.
            let key = crate::genres::canonical_genre(&genre)
                .map(|s| s.to_string())
                .unwrap_or(genre);
            *scores.entry(key).or_insert(0.0) += delta;
        }
        Ok(scores)
    }

    /// Followed series with their genres (aggregated) and kind, for the 3D
    /// relationship graph. One follow-up `list_series_genres` query per
    /// series — simplest option given that helper already exists, and the
    /// followed-series count here is small (never bulk-scraped).
    pub fn get_stats_graph_data(&self, source_id: i64) -> Result<Vec<crate::models::SeriesGraphNode>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, title, cover_url, kind FROM series
             WHERE source_id=?1 AND followed=1 ORDER BY title",
        )?;
        let rows: Vec<(i64, String, Option<String>, Option<String>)> = stmt
            .query_map([source_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows.into_iter()
            .map(|(id, title, cover_url, kind)| {
                Ok(crate::models::SeriesGraphNode {
                    id,
                    title,
                    cover_url,
                    genres: self.list_series_genres(id)?,
                    kind,
                })
            })
            .collect()
    }

    /// Scalar watch totals for the stats dashboard.
    pub fn get_watch_summary(&self, source_id: i64) -> Result<crate::models::WatchSummary> {
        let followed_series: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM series WHERE source_id=?1 AND followed=1",
            [source_id],
            |r| r.get(0),
        )?;
        // Real seen-episode count across ALL series, followed or not — a
        // "Ya lo vi" swipe whose site-link scraped real episodes must count
        // too (see the episode/anime-counts-fix design doc). `episodes_total`
        // stays scoped to series that mean something for a "how far along
        // am I" denominator: followed, or with at least one real seen
        // episode — so discarded/never-touched scraped rows don't inflate it.
        let episodes_watched: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM episodes e
             JOIN series s ON s.id = e.series_id
             WHERE s.source_id=?1 AND e.seen=1",
            [source_id],
            |r| r.get(0),
        )?;
        let episodes_total: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM episodes e
             JOIN series s ON s.id = e.series_id
             WHERE s.source_id=?1
               AND (s.followed=1 OR EXISTS (SELECT 1 FROM episodes x WHERE x.series_id=s.id AND x.seen=1))",
            [source_id],
            |r| r.get(0),
        )?;
        // Episodes attributed to "Ya vistas" via the catalog estimate — only
        // for series with NO real seen-episode data, so real data always
        // wins and nothing is double-counted against `episodes_watched`.
        let episodes_watched_external: i64 = self.conn.query_row(
            "SELECT COALESCE(SUM(c.episodes), 0)
             FROM series s JOIN anilist_catalog c ON c.id = s.anilist_id
             WHERE s.source_id=?1 AND s.watched_externally=1 AND c.episodes IS NOT NULL
               AND NOT EXISTS (SELECT 1 FROM episodes e WHERE e.series_id=s.id AND e.seen=1)",
            [source_id],
            |r| r.get(0),
        )?;
        let backlog_want: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM series WHERE source_id=?1 AND backlog_status='want'",
            [source_id],
            |r| r.get(0),
        )?;
        let airing_followed: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM series WHERE source_id=?1 AND followed=1 AND is_airing=1",
            [source_id],
            |r| r.get(0),
        )?;
        let pending_to_watch: i64 = self.conn.query_row(
            "SELECT COUNT(DISTINCT s.id) FROM series s
             JOIN episodes e ON e.series_id = s.id
             WHERE s.source_id=?1 AND s.followed=1 AND e.seen=0",
            [source_id],
            |r| r.get(0),
        )?;
        // Distinct animes = distinct franchise keys among series with actual
        // watch evidence — watched-externally (a "Ya lo vi" swipe) OR at
        // least one real seen episode — so seasons of the same show count
        // once. A followed series with zero seen episodes has no evidence of
        // having been watched and is excluded (it's "in my library", not "in
        // my watched anime"). Grouping is done in Rust (franchise_key)
        // because SQLite can't run the normalization/marker-stripping.
        let mut stmt = self.conn.prepare(
            "SELECT s.title FROM series s
             WHERE s.source_id=?1
               AND (s.watched_externally=1
                    OR EXISTS (SELECT 1 FROM episodes e WHERE e.series_id=s.id AND e.seen=1))",
        )?;
        let franchises: HashSet<String> = stmt
            .query_map([source_id], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<String>>>()?
            .into_iter()
            .map(|t| franchise_key(&t))
            .collect();
        let distinct_anime = franchises.len() as i64;
        Ok(crate::models::WatchSummary {
            followed_series,
            distinct_anime,
            episodes_watched,
            episodes_total,
            episodes_watched_external,
            airing_followed,
            pending_to_watch,
            backlog_want,
        })
    }

    /// Local-only watch metrics for the Estadísticas "Resumen" block — see
    /// `docs/superpowers/specs/2026-07-13-stats-new-metrics-design.md`. Pure
    /// SQL against the local DB; never touches the network. Time uses
    /// AniList's real per-episode `duration` when a series is linked to a
    /// synced catalog row that has one, falling back to `minutes_per_episode`'s
    /// format/kind-based estimate otherwise (unsynced/unlinked rows, or rows
    /// synced before `duration` existed).
    pub fn get_watch_insights(&self, source_id: i64) -> Result<crate::models::WatchInsights> {
        // Minutes tracked: seen-episode counts grouped by series (and its
        // `kind`), summed in Rust since SQLite can't run the format->minutes
        // map itself. All series with real seen episodes count, followed or
        // not — a "Ya lo vi" swipe whose site-link scraped real episodes
        // must contribute its real minutes too (see the episode/anime-
        // counts-fix design doc).
        // LEFT JOINed so series with `anilist_id IS NULL` (the common case —
        // most followed/site-scraped series have no catalog link at all)
        // still come through with `duration = NULL`, falling back to the
        // exact same `kind`-based estimate as before the join was added.
        let mut stmt = self.conn.prepare(
            "SELECT s.kind, COUNT(*) AS cnt, c.duration
             FROM episodes e JOIN series s ON s.id = e.series_id
             LEFT JOIN anilist_catalog c ON c.id = s.anilist_id
             WHERE e.seen=1 AND s.source_id=?1
             GROUP BY s.id",
        )?;
        let tracked_rows: Vec<(Option<String>, i64, Option<i64>)> = stmt
            .query_map([source_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let estimated_minutes_tracked: i64 = tracked_rows
            .iter()
            .map(|(kind, cnt, duration)| {
                duration.unwrap_or_else(|| minutes_per_episode(kind.as_deref())) * cnt
            })
            .sum();

        // Minutes from "Ya vistas" linked to a catalog row with a known
        // episode count — only those rows can contribute an estimate, AND
        // only when the series has no real seen-episode data already
        // counted above (otherwise it double-counts: real minutes above,
        // catalog-estimated minutes here, for the same episodes). Real
        // AniList `duration` wins over the format-based estimate when synced.
        let mut stmt = self.conn.prepare(
            "SELECT c.episodes, c.format, c.duration
             FROM series s JOIN anilist_catalog c ON c.id = s.anilist_id
             WHERE s.source_id=?1 AND s.watched_externally=1 AND c.episodes IS NOT NULL
               AND NOT EXISTS (SELECT 1 FROM episodes e WHERE e.series_id=s.id AND e.seen=1)",
        )?;
        let external_rows: Vec<(i64, Option<String>, Option<i64>)> = stmt
            .query_map([source_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let estimated_minutes_external: i64 = external_rows
            .iter()
            .map(|(episodes, format, duration)| {
                duration.unwrap_or_else(|| minutes_per_episode(format.as_deref())) * episodes
            })
            .sum();
        let external_titles_estimated = external_rows.len() as i64;

        let external_titles_total: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM series WHERE source_id=?1 AND watched_externally=1",
            [source_id],
            |r| r.get(0),
        )?;

        // Mean episode count (all episodes, not just seen) across followed
        // series. NULL (no followed series at all) reads as 0.0, not a crash.
        let avg_episodes_per_series: Option<f64> = self.conn.query_row(
            "SELECT AVG(cnt) FROM (
                SELECT s.id, COUNT(e.id) AS cnt
                FROM series s LEFT JOIN episodes e ON e.series_id = s.id
                WHERE s.source_id=?1 AND s.followed=1
                GROUP BY s.id
             )",
            [source_id],
            |r| r.get(0),
        )?;
        let avg_episodes_per_series = avg_episodes_per_series.unwrap_or(0.0);

        let followed_airing: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM series WHERE source_id=?1 AND followed=1 AND is_airing=1",
            [source_id],
            |r| r.get(0),
        )?;
        let followed_finished: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM series WHERE source_id=?1 AND followed=1 AND is_airing=0",
            [source_id],
            |r| r.get(0),
        )?;
        let discarded: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM series WHERE source_id=?1 AND backlog_status='discarded'",
            [source_id],
            |r| r.get(0),
        )?;
        let want: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM series WHERE source_id=?1 AND backlog_status='want'",
            [source_id],
            |r| r.get(0),
        )?;
        let watched_externally: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM series WHERE source_id=?1 AND watched_externally=1",
            [source_id],
            |r| r.get(0),
        )?;

        let mut stmt = self.conn.prepare(
            "SELECT s.title, COUNT(*) AS cnt
             FROM episodes e JOIN series s ON s.id = e.series_id
             WHERE e.seen=1 AND s.source_id=?1
             GROUP BY s.id
             ORDER BY cnt DESC, s.title
             LIMIT 8",
        )?;
        let top_series = stmt
            .query_map([source_id], |r| {
                Ok(crate::models::TitleCount { title: r.get(0)?, count: r.get(1)? })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        let mut stmt = self.conn.prepare(
            "SELECT DATE(e.seen_at) AS d, COUNT(*) AS cnt
             FROM episodes e JOIN series s ON s.id = e.series_id
             WHERE e.seen_at IS NOT NULL AND s.source_id=?1
               AND DATE(e.seen_at) >= DATE('now', '-30 days')
             GROUP BY d
             ORDER BY d",
        )?;
        let marks_by_day = stmt
            .query_map([source_id], |r| {
                Ok(crate::models::DayCount { day: r.get(0)?, count: r.get(1)? })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        let marks_tracked_since: Option<String> = self.conn.query_row(
            "SELECT MIN(DATE(e.seen_at)) FROM episodes e JOIN series s ON s.id = e.series_id
             WHERE e.seen_at IS NOT NULL AND s.source_id=?1",
            [source_id],
            |r| r.get(0),
        )?;

        Ok(crate::models::WatchInsights {
            estimated_minutes_tracked,
            estimated_minutes_external,
            external_titles_estimated,
            external_titles_total,
            avg_episodes_per_series,
            followed_airing,
            followed_finished,
            discarded,
            want,
            watched_externally,
            top_series,
            marks_by_day,
            marks_tracked_since,
        })
    }

    /// A cheap fingerprint of the data, used to skip a redundant auto-backup
    /// when nothing changed since the last one.
    pub fn signature_counts(&self) -> Result<(i64, i64, i64, Option<String>)> {
        let series: i64 = self.conn.query_row("SELECT COUNT(*) FROM series", [], |r| r.get(0))?;
        let eps: i64 = self.conn.query_row("SELECT COUNT(*) FROM episodes", [], |r| r.get(0))?;
        let max_ep: i64 = self.conn
            .query_row("SELECT COALESCE(MAX(id),0) FROM episodes", [], |r| r.get(0))?;
        let max_seen: Option<String> = self.conn
            .query_row("SELECT MAX(seen_at) FROM episodes", [], |r| r.get(0))
            .optional()?
            .flatten();
        Ok((series, eps, max_ep, max_seen))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_support::*;

    #[test]
    fn franchise_key_keeps_distinct_shows_distinct_and_never_empty() {
        assert_ne!(franchise_key("Naruto"), franchise_key("Bleach"));
        // A title that is *only* a season marker must not collapse to "".
        assert!(!franchise_key("Season 2").is_empty());
        assert!(!franchise_key("IV").is_empty());
    }

    #[test]
    fn get_watch_summary_counts_distinct_animes_collapsing_seasons() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("A", "a", "animeytx").unwrap();

        // Three followed rows that are the same franchise (S1/S2/S3), each
        // with at least one real seen episode — distinct_anime now requires
        // watch evidence (see the episode/anime-counts-fix design doc), not
        // just `followed=1`.
        for (slug, title) in [
            ("slime1", "Tensei shitara Slime Datta Ken"),
            ("slime2", "Tensei shitara Slime Datta Ken Temporada 2"),
            ("slime3", "Tensei shitara Slime Datta Ken Temporada 3"),
        ] {
            let id = db.upsert_series(src, &mk_airing(slug, title, None)).unwrap();
            db.set_followed(id, true).unwrap();
            insert_eps_seen_up_to(&db, id, 1, 1);
        }
        // A different followed franchise, also with a seen episode.
        let n = db.upsert_series(src, &mk_airing("naruto", "Naruto", None)).unwrap();
        db.set_followed(n, true).unwrap();
        insert_eps_seen_up_to(&db, n, 1, 1);
        // A watched-externally-only franchise (counts too, even if not followed).
        let w = db.upsert_series(src, &mk_airing("frieren", "Frieren", None)).unwrap();
        db.set_watched_externally(w, true).unwrap();

        let summary = db.get_watch_summary(src).unwrap();
        assert_eq!(summary.followed_series, 4, "4 followed rows");
        assert_eq!(summary.distinct_anime, 3, "Slime(x3 seasons)=1, Naruto=1, Frieren=1");
    }

    // ---- Stats stale refresh + metric concepts (2026-07-13) ----

    #[test]
    fn get_watch_summary_computes_airing_followed_and_pending_to_watch() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("A", "a", "animeytx").unwrap();

        // Followed + still airing + one unseen episode: counts toward both
        // airing_followed and pending_to_watch.
        let mut airing = mk_airing("airing1", "Airing Show", None);
        airing.is_airing = true;
        let airing_id = db.upsert_series(src, &airing).unwrap();
        db.set_followed(airing_id, true).unwrap();
        db.insert_episode(&crate::models::Episode {
            id: 0,
            series_id: airing_id,
            number: "1".into(),
            title: None,
            url: "https://site/anime/airing1-capitulo-1/".into(),
            released_at: None,
            seen: false,
        })
        .unwrap();

        // Followed + finished + fully seen: counts toward neither.
        let mut finished = mk_airing("finished1", "Finished Show", None);
        finished.is_airing = false;
        let finished_id = db.upsert_series(src, &finished).unwrap();
        db.set_followed(finished_id, true).unwrap();
        db.insert_episode(&crate::models::Episode {
            id: 0,
            series_id: finished_id,
            number: "1".into(),
            title: None,
            url: "https://site/anime/finished1-capitulo-1/".into(),
            released_at: None,
            seen: true,
        })
        .unwrap();

        // Want-only (never followed): counts toward backlog_want only.
        let want = mk_airing("want1", "Want Show", None);
        let want_id = db.upsert_series(src, &want).unwrap();
        db.set_backlog_status(want_id, Some("want")).unwrap();

        let summary = db.get_watch_summary(src).unwrap();
        assert_eq!(summary.airing_followed, 1, "only the airing+followed row counts");
        assert_eq!(summary.pending_to_watch, 1, "only the followed row with an unseen episode counts");
        assert_eq!(summary.backlog_want, 1, "only the want-only row counts");
    }

    // ---- Stats new metrics: minutes_per_episode + get_watch_insights (2026-07-13) ----

    #[test]
    fn minutes_per_episode_maps_each_format_branch() {
        assert_eq!(minutes_per_episode(Some("MOVIE")), 100);
        assert_eq!(minutes_per_episode(Some("movie")), 100, "case-insensitive");
        assert_eq!(minutes_per_episode(Some("Pelicula")), 100);
        assert_eq!(minutes_per_episode(Some("Película")), 100, "accented site vocabulary");
        assert_eq!(minutes_per_episode(Some("MUSIC")), 5);
        assert_eq!(minutes_per_episode(Some("TV_SHORT")), 8);
        assert_eq!(minutes_per_episode(Some("OVA")), 26);
        assert_eq!(minutes_per_episode(Some("SPECIAL")), 26);
        assert_eq!(minutes_per_episode(Some("TV")), 24);
        assert_eq!(minutes_per_episode(Some("ONA")), 24);
    }

    #[test]
    fn minutes_per_episode_defaults_to_24_for_real_site_noise_and_none() {
        // Real dirty `series.kind` vocabulary observed live on 2026-07-13
        // (see the design doc) must not crash or misclassify — it all falls
        // back to the plain-TV estimate.
        for noisy in ["4K", "Blu-Ray", "Resubido", "Sin Censura", "Yaoi"] {
            assert_eq!(minutes_per_episode(Some(noisy)), 24, "noisy kind: {noisy}");
        }
        assert_eq!(minutes_per_episode(None), 24, "no kind at all");
    }

    #[test]
    fn get_watch_insights_estimates_minutes_from_seeded_db() {
        // Mirrors the design doc's acceptance test: a followed series with 3
        // seen episodes (kind=TV -> 24 min/ep = 72) plus a linked "Ya vista"
        // whose catalog row has 12 episodes at format TV (24 min/ep = 288).
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("A", "a", "animeytx").unwrap();

        // Followed, airing, TV, 4 episodes numbered 1..4, cascade-mark 1..3 seen.
        let mut airing = mk_airing("airing1", "Airing Show", None);
        airing.is_airing = true;
        let airing_id = db.upsert_series(src, &airing).unwrap();
        db.set_followed(airing_id, true).unwrap();
        db.set_kind(airing_id, "TV").unwrap();
        for n in 1..=4 {
            db.insert_episode(&crate::models::Episode {
                id: 0, series_id: airing_id, number: n.to_string(), title: None,
                url: format!("https://site/anime/airing1-capitulo-{n}/"),
                released_at: None, seen: false,
            }).unwrap();
        }
        db.set_seen_cascade(airing_id, "3", true).unwrap();

        // Followed, finished, MOVIE, 2 episodes already seen (no seen_at —
        // simulates pre-existing data from before the column existed).
        let mut finished = mk_airing("finished1", "Finished Show", None);
        finished.is_airing = false;
        let finished_id = db.upsert_series(src, &finished).unwrap();
        db.set_followed(finished_id, true).unwrap();
        db.set_kind(finished_id, "MOVIE").unwrap();
        for n in 1..=2 {
            db.insert_episode(&crate::models::Episode {
                id: 0, series_id: finished_id, number: n.to_string(), title: None,
                url: format!("https://site/anime/finished1-capitulo-{n}/"),
                released_at: None, seen: true,
            }).unwrap();
        }

        // Want-only.
        let want_id = db.upsert_series(src, &mk_airing("want1", "Want Show", None)).unwrap();
        db.set_backlog_status(want_id, Some("want")).unwrap();

        // Discarded-only.
        let disc_id = db.upsert_series(src, &mk_airing("disc1", "Discarded Show", None)).unwrap();
        db.set_backlog_status(disc_id, Some("discarded")).unwrap();

        // Watched-externally, linked to a catalog row with 12 TV episodes.
        db.upsert_catalog_anime(
            &crate::anilist::CatalogAnime {
                id: 100, title: "External Show".into(), title_romaji: None, title_english: None,
                cover_url: None, format: Some("TV".into()), genres: vec![],
                episodes: Some(12), average_score: None, popularity: None,
                url: "https://anilist.co/anime/100".into(), status: None, duration: None,
                studio: None,
            },
            0,
        ).unwrap();
        let ext_id = db.upsert_series(src, &mk_airing("ext1", "External Show", None)).unwrap();
        db.set_watched_externally(ext_id, true).unwrap();
        db.set_anilist_id(ext_id, 100).unwrap();

        // A second watched-externally row, unlinked (no catalog data) — must
        // count toward the total but not toward the estimated minutes.
        let ext2_id = db.upsert_series(src, &mk_airing("ext2", "Unlinked External", None)).unwrap();
        db.set_watched_externally(ext2_id, true).unwrap();

        let insights = db.get_watch_insights(src).unwrap();

        // Both series are followed (one airing, one finished) — tracked
        // minutes sum across ALL followed series' seen episodes, not just
        // airing ones: 3 TV eps * 24 min + 2 MOVIE eps * 100 min.
        assert_eq!(insights.estimated_minutes_tracked, 272, "3*24 (TV) + 2*100 (MOVIE)");
        assert_eq!(insights.estimated_minutes_external, 288, "12 eps * 24 min (TV)");
        assert_eq!(insights.external_titles_estimated, 1, "only the linked row counted");
        assert_eq!(insights.external_titles_total, 2, "both watched-externally rows");
        assert_eq!(insights.avg_episodes_per_series, 3.0, "(4 + 2) / 2 followed series");
        assert_eq!(insights.followed_airing, 1);
        assert_eq!(insights.followed_finished, 1);
        assert_eq!(insights.want, 1);
        assert_eq!(insights.discarded, 1);
        assert_eq!(insights.watched_externally, 2);
        assert_eq!(
            insights.top_series,
            vec![
                crate::models::TitleCount { title: "Airing Show".into(), count: 3 },
                crate::models::TitleCount { title: "Finished Show".into(), count: 2 },
            ],
            "ordered by seen-episode count, descending"
        );
        assert_eq!(insights.marks_by_day.len(), 1, "only the cascade-marked episodes have seen_at");
        assert_eq!(insights.marks_by_day[0].count, 3);
        assert!(insights.marks_tracked_since.is_some());
    }

    #[test]
    fn get_watch_insights_matches_design_doc_acceptance_example() {
        // Literal acceptance scenario from
        // docs/superpowers/specs/2026-07-13-stats-new-metrics-design.md:
        // one followed series with 3 seen episodes, plus one linked
        // watched-externally series whose catalog row has 12 TV episodes.
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("A", "a", "animeytx").unwrap();

        let sid = db.upsert_series(src, &mk_airing("s1", "S1", None)).unwrap();
        db.set_followed(sid, true).unwrap();
        db.set_kind(sid, "TV").unwrap();
        for n in 1..=3 {
            db.insert_episode(&crate::models::Episode {
                id: 0, series_id: sid, number: n.to_string(), title: None,
                url: format!("https://site/anime/s1-capitulo-{n}/"),
                released_at: None, seen: true,
            }).unwrap();
        }

        db.upsert_catalog_anime(
            &crate::anilist::CatalogAnime {
                id: 200, title: "S2".into(), title_romaji: None, title_english: None,
                cover_url: None, format: Some("TV".into()), genres: vec![],
                episodes: Some(12), average_score: None, popularity: None,
                url: "https://anilist.co/anime/200".into(), status: None, duration: None,
                studio: None,
            },
            0,
        ).unwrap();
        let ext_id = db.upsert_series(src, &mk_airing("s2", "S2", None)).unwrap();
        db.set_watched_externally(ext_id, true).unwrap();
        db.set_anilist_id(ext_id, 200).unwrap();

        let insights = db.get_watch_insights(src).unwrap();
        assert_eq!(insights.estimated_minutes_tracked, 72);
        assert_eq!(insights.estimated_minutes_external, 288);
    }

    #[test]
    fn get_watch_insights_uses_real_duration_when_synced() {
        // Same shape as the design-doc acceptance test, but the linked
        // catalog row has a real synced `duration` of 23 min/ep — that must
        // win over the format-based estimate (TV -> 24 min/ep, which would
        // give 288). 23 * 12 = 276.
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("A", "a", "animeytx").unwrap();

        db.upsert_catalog_anime(
            &crate::anilist::CatalogAnime {
                id: 201, title: "S3".into(), title_romaji: None, title_english: None,
                cover_url: None, format: Some("TV".into()), genres: vec![],
                episodes: Some(12), average_score: None, popularity: None,
                url: "https://anilist.co/anime/201".into(), status: None, duration: Some(23),
                studio: None,
            },
            0,
        ).unwrap();
        let ext_id = db.upsert_series(src, &mk_airing("s3", "S3", None)).unwrap();
        db.set_watched_externally(ext_id, true).unwrap();
        db.set_anilist_id(ext_id, 201).unwrap();

        let insights = db.get_watch_insights(src).unwrap();
        assert_eq!(insights.estimated_minutes_external, 276, "23 min/ep * 12 eps beats the 24 min/ep TV estimate");
    }

    #[test]
    fn get_watch_insights_falls_back_to_estimate_when_duration_is_null() {
        // Same shape, but `duration: None` (unsynced/no-duration catalog row)
        // — must fall back to today's format-based estimate: 24 * 12 = 288.
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("A", "a", "animeytx").unwrap();

        db.upsert_catalog_anime(
            &crate::anilist::CatalogAnime {
                id: 202, title: "S4".into(), title_romaji: None, title_english: None,
                cover_url: None, format: Some("TV".into()), genres: vec![],
                episodes: Some(12), average_score: None, popularity: None,
                url: "https://anilist.co/anime/202".into(), status: None, duration: None,
                studio: None,
            },
            0,
        ).unwrap();
        let ext_id = db.upsert_series(src, &mk_airing("s4", "S4", None)).unwrap();
        db.set_watched_externally(ext_id, true).unwrap();
        db.set_anilist_id(ext_id, 202).unwrap();

        let insights = db.get_watch_insights(src).unwrap();
        assert_eq!(insights.estimated_minutes_external, 288, "falls back to 24 min/ep (TV) estimate, unchanged");
    }

    #[test]
    fn get_watch_insights_empty_db_has_no_divide_by_zero_and_empty_collections() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("A", "a", "animeytx").unwrap();
        let insights = db.get_watch_insights(src).unwrap();
        assert_eq!(insights.estimated_minutes_tracked, 0);
        assert_eq!(insights.estimated_minutes_external, 0);
        assert_eq!(insights.external_titles_estimated, 0);
        assert_eq!(insights.external_titles_total, 0);
        assert_eq!(insights.avg_episodes_per_series, 0.0);
        assert!(insights.top_series.is_empty());
        assert!(insights.marks_by_day.is_empty());
        assert_eq!(insights.marks_tracked_since, None);
    }

    #[test]
    fn get_genre_affinity_weighs_followed_want_discarded_correctly() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();

        let followed = crate::models::Series {
            id: 0, slug: "f".into(), title: "F".into(),
            url: "u1".into(), cover_url: None, is_airing: false, followed: true, next_episode_at: None, site_episode_count: None,
        };
        let sid_f = db.upsert_series(src, &followed).unwrap();
        db.set_followed(sid_f, true).unwrap();
        db.insert_series_genres(sid_f, &["Seinen".to_string(), "Shared".to_string()]).unwrap();

        let want = crate::models::Series {
            id: 0, slug: "w".into(), title: "W".into(),
            url: "u2".into(), cover_url: None, is_airing: false, followed: false, next_episode_at: None, site_episode_count: None,
        };
        let sid_w = db.upsert_series(src, &want).unwrap();
        db.set_backlog_status(sid_w, Some("want")).unwrap();
        db.insert_series_genres(sid_w, &["Romance".to_string(), "Shared".to_string()]).unwrap();

        let discarded = crate::models::Series {
            id: 0, slug: "d".into(), title: "D".into(),
            url: "u3".into(), cover_url: None, is_airing: false, followed: false, next_episode_at: None, site_episode_count: None,
        };
        let sid_d = db.upsert_series(src, &discarded).unwrap();
        db.set_backlog_status(sid_d, Some("discarded")).unwrap();
        db.insert_series_genres(sid_d, &["Horror".to_string()]).unwrap();

        let scores = db.get_genre_affinity(src).unwrap();
        assert_eq!(scores.get("Seinen"), Some(&2.0));
        assert_eq!(scores.get("Romance"), Some(&1.0));
        assert_eq!(scores.get("Horror"), Some(&-1.5));
        assert_eq!(scores.get("Shared"), Some(&3.0)); // 2.0 (followed) + 1.0 (want)
        assert_eq!(scores.get("Unseen"), None);
    }

    #[test]
    fn get_stats_graph_data_returns_genres_kind_and_cover_for_followed_series() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();

        let full = crate::models::Series {
            id: 0, slug: "full".into(), title: "Full".into(),
            url: "u1".into(), cover_url: Some("data:image/png;base64,x".into()),
            is_airing: false, followed: true, next_episode_at: None, site_episode_count: None,
        };
        let sid_full = db.upsert_series(src, &full).unwrap();
        db.set_followed(sid_full, true).unwrap();
        db.insert_series_genres(sid_full, &["Seinen".to_string(), "Drama".to_string()]).unwrap();
        db.set_kind(sid_full, "TV").unwrap();

        let bare = crate::models::Series {
            id: 0, slug: "bare".into(), title: "Bare".into(),
            url: "u2".into(), cover_url: None, is_airing: false, followed: true, next_episode_at: None, site_episode_count: None,
        };
        let sid_bare = db.upsert_series(src, &bare).unwrap();
        db.set_followed(sid_bare, true).unwrap();

        // not followed => excluded
        let other = crate::models::Series {
            id: 0, slug: "other".into(), title: "Other".into(),
            url: "u3".into(), cover_url: None, is_airing: false, followed: false, next_episode_at: None, site_episode_count: None,
        };
        db.upsert_series(src, &other).unwrap();

        let rows = db.get_stats_graph_data(src).unwrap();
        assert_eq!(rows.len(), 2);

        let full_row = rows.iter().find(|r| r.id == sid_full).unwrap();
        assert_eq!(full_row.genres, vec!["Drama".to_string(), "Seinen".to_string()]);
        assert_eq!(full_row.kind.as_deref(), Some("TV"));
        assert_eq!(full_row.cover_url.as_deref(), Some("data:image/png;base64,x"));

        let bare_row = rows.iter().find(|r| r.id == sid_bare).unwrap();
        assert!(bare_row.genres.is_empty());
        assert_eq!(bare_row.kind, None);
        assert_eq!(bare_row.cover_url, None);
    }

    #[test]
    fn get_genre_affinity_folds_spanish_and_english_genre_into_one_canonical_key() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();

        // A followed series tagged with the site's Spanish genre "Acción".
        let followed = crate::models::Series {
            id: 0, slug: "f".into(), title: "F".into(),
            url: "u1".into(), cover_url: None, is_airing: false, followed: true, next_episode_at: None, site_episode_count: None,
        };
        let sid_f = db.upsert_series(src, &followed).unwrap();
        db.set_followed(sid_f, true).unwrap();
        db.insert_series_genres(sid_f, &["Acción".to_string(), "Isekai".to_string()]).unwrap();

        // A 'want' series tagged with the catalog's English genre "Action".
        let want = crate::models::Series {
            id: 0, slug: "w".into(), title: "W".into(),
            url: "u2".into(), cover_url: None, is_airing: false, followed: false, next_episode_at: None, site_episode_count: None,
        };
        let sid_w = db.upsert_series(src, &want).unwrap();
        db.set_backlog_status(sid_w, Some("want")).unwrap();
        db.insert_series_genres(sid_w, &["Action".to_string()]).unwrap();

        let scores = db.get_genre_affinity(src).unwrap();
        // 2.0 (followed, "Acción" normalized to "Action") + 1.0 (want, "Action") = 3.0
        assert_eq!(scores.get("Action"), Some(&3.0));
        assert_eq!(scores.get("Acción"), None);
        // Site-only tag with no AniList equivalent keeps its raw name.
        assert_eq!(scores.get("Isekai"), Some(&2.0));
    }

    #[test]
    fn signature_changes_when_episode_marked_seen() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
        let series = db.upsert_series(src, &mk_airing("x", "X", None)).unwrap();
        db.insert_episode(&crate::models::Episode {
            id: 0, series_id: series, number: "1".into(), title: None,
            url: "https://site/x-1".into(), released_at: None, seen: false,
        }).unwrap();
        let before = db.signature_counts().unwrap();
        db.set_seen_cascade(series, "1", true).unwrap();
        let after = db.signature_counts().unwrap();
        assert_ne!(before, after);
    }
}
