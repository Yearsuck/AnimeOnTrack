use super::*;
use std::collections::{HashMap, HashSet};

/// Collapse a series title to a season/part-agnostic "franchise key" so
/// multiple seasons (or arcs) of the same show count as one anime in the
/// stats (`get_watch_summary`'s distinct-anime count, `get_watch_insights`'
/// `top_series`). First drops a colon-introduced arc/subtitle qualifier —
/// the scraped site splits long-runners into one `series` row per arc this
/// way (e.g. "One Piece: Arco de Elbaph", "One Piece: Wano"), which without
/// this step never collapses with the base "One Piece" row and silently
/// undercounts the franchise's real seen-episode total. Then normalizes via
/// `matching::normalize_title` (accents/case/punctuation/noise stripped) and
/// strips trailing season/part markers: "temporada N", "season N", "part N",
/// "parte N", "cour N", "Nth season", "final season", a trailing standalone
/// integer, and trailing Roman numerals (ii..x). It's a heuristic, not a
/// canonical grouping — good enough to keep "X", "X: Arco Y" and "X Temporada
/// 2" together without a metadata source. Never returns "" (falls back to the
/// full normalized title if stripping would otherwise empty it).
pub(crate) fn franchise_key(title: &str) -> String {
    let base = franchise_base_slice(title);
    let norm = crate::matching::normalize_title(base);
    let mut tokens: Vec<&str> = norm.split_whitespace().collect();
    while let Some(&last) = tokens.last() {
        if is_season_marker_token(last) {
            tokens.pop();
        } else {
            break;
        }
    }
    let key = tokens.join(" ");
    if key.is_empty() {
        crate::matching::normalize_title(title)
    } else {
        key
    }
}

/// The slice of `title` before a colon-introduced arc/subtitle qualifier, or
/// the whole title when there is no usable colon. Shared by `franchise_key`
/// (which then normalizes it) and `franchise_display_title` (which keeps the
/// original casing/accents).
fn franchise_base_slice(title: &str) -> &str {
    match title.split_once(':') {
        // Only strip when there's real title text before the colon, so a
        // colon-led title (unlikely, but keep it safe) doesn't collapse to "".
        Some((head, _)) if !head.trim().is_empty() => head,
        _ => title,
    }
}

/// True for a *normalized* trailing token that marks a season/part/arc rather
/// than being part of the show's name: "season"/"temporada"/"part"/"parte"/
/// "cour"/"final"/"the", a bare integer (also catches a stripped "(2024)"
/// year), Roman numerals ii..x, and "1st"/"2nd"/"3rd"/"4th"-style ordinals.
fn is_season_marker_token(token: &str) -> bool {
    const ROMANS: &[&str] = &["ii", "iii", "iv", "v", "vi", "vii", "viii", "ix", "x"];
    const MARKER_WORDS: &[&str] =
        &["season", "temporada", "part", "parte", "cour", "final", "the"];
    let is_int = !token.is_empty() && token.chars().all(|c| c.is_ascii_digit());
    let is_ordinal = token.len() > 2
        && matches!(&token[token.len() - 2..], "st" | "nd" | "rd" | "th")
        && token[..token.len() - 2].chars().all(|c| c.is_ascii_digit());
    is_int || is_ordinal || ROMANS.contains(&token) || MARKER_WORDS.contains(&token)
}

/// A human-facing franchise name derived from one member title, with the
/// original casing and accents preserved — `franchise_key`'s normalized output
/// ("one piece") is a grouping key, never something to show a user.
///
/// Applies the same two reductions as `franchise_key` (drop a colon-introduced
/// qualifier, then strip trailing season/part markers) but operates on the raw
/// tokens, testing each one's normalized form. "One Piece: Arco de Elbaph" and
/// "One Piece: Live Action (2023) Temporada 2" both yield "One Piece", so a
/// franchise row in `top_series` is labelled with the show's name instead of
/// whichever arc happened to have the shortest title. Falls back to the
/// trimmed input when stripping would leave nothing.
pub(crate) fn franchise_display_title(title: &str) -> String {
    let base = franchise_base_slice(title).trim();
    let mut tokens: Vec<&str> = base.split_whitespace().collect();
    while let Some(&last) = tokens.last() {
        let normalized = crate::matching::normalize_title(last);
        if is_season_marker_token(&normalized) {
            tokens.pop();
        } else {
            break;
        }
    }
    let out = tokens.join(" ").trim().to_string();
    if out.is_empty() {
        title.trim().to_string()
    } else {
        out
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

/// Whether `candidate` is a better franchise label than `current`. Members of
/// one franchise usually reduce to the same display title, so this only
/// arbitrates the cases where they don't, and only needs to be *deterministic*
/// — never query-order dependent. Shortest wins first (a shorter reduction
/// means fewer qualifiers survived); among equal-length ones a normally-cased
/// title beats a SHOUTED one (both the site and AniList sometimes list a show
/// in full caps, and "One Piece" reads better than "ONE PIECE"); anything
/// still tied falls back to plain alphabetical order.
fn prefer_display_title(candidate: &str, current: &str) -> bool {
    let (candidate_len, current_len) = (candidate.chars().count(), current.chars().count());
    if candidate_len != current_len {
        return candidate_len < current_len;
    }
    let shouted = |s: &str| s.chars().any(|c| c.is_uppercase()) && !s.chars().any(|c| c.is_lowercase());
    match (shouted(candidate), shouted(current)) {
        (false, true) => true,
        (true, false) => false,
        _ => candidate < current,
    }
}

/// One `series` row's raw watch evidence, straight out of SQL — the input to
/// the franchise roll-up, never exposed outside this module.
struct SeriesWatchRow {
    title: String,
    kind: Option<String>,
    watched_externally: bool,
    seen_count: i64,
    catalog_episodes: Option<i64>,
    catalog_format: Option<String>,
    catalog_duration: Option<i64>,
}

/// One franchise's watch contribution, rolled up across every `series` row
/// sharing its `franchise_key`.
///
/// The two signals are deliberately kept apart because they measure the same
/// thing two different ways and must never be added together. The scraped site
/// splits long-runners into one row per arc, so a user who has watched all of
/// One Piece typically has (a) a few hundred episodes really marked seen on
/// whichever arc rows the site currently lists, and (b) a "Ya lo vi" swipe on
/// the single AniList entry for the whole 1000+ episode show. Summing those
/// counts the overlapping episodes twice; taking the larger of the two credits
/// the franchise with the best evidence available. See `episodes`/`minutes`.
pub(crate) struct FranchiseRollup {
    pub display_title: String,
    /// Episodes actually marked seen, summed across member rows.
    pub real_seen: i64,
    /// Minutes for exactly those episodes.
    pub real_minutes: i64,
    /// Largest catalog episode count among members marked "Ya lo vi" that have
    /// no real seen episodes of their own. 0 when there is no such member.
    pub external_episodes: i64,
    pub external_minutes: i64,
}

impl FranchiseRollup {
    /// Episodes credited to this franchise: the larger of the two signals,
    /// never their sum (see the struct's doc comment).
    pub fn episodes(&self) -> i64 {
        self.real_seen.max(self.external_episodes)
    }

    /// Episodes credited *beyond* what was really marked seen — the additive
    /// remainder, so `real_seen + extra_external_episodes` is the total with
    /// no double counting.
    pub fn extra_external_episodes(&self) -> i64 {
        (self.external_episodes - self.real_seen).max(0)
    }

    /// Minutes for `extra_external_episodes`, on the same additive basis.
    pub fn extra_external_minutes(&self) -> i64 {
        if self.external_episodes > self.real_seen {
            (self.external_minutes - self.real_minutes).max(0)
        } else {
            0
        }
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

    /// Every franchise with watch evidence, rolled up from its member `series`
    /// rows. One query for the whole dashboard — `get_watch_summary` and
    /// `get_watch_insights` both build on this so they can never disagree
    /// about what a franchise's episode/minute totals are.
    pub(crate) fn franchise_rollups(&self, source_id: i64) -> Result<Vec<FranchiseRollup>> {
        let mut stmt = self.conn.prepare(
            "SELECT s.title, s.kind, s.watched_externally,
                    (SELECT COUNT(*) FROM episodes e WHERE e.series_id=s.id AND e.seen=1) AS seen_cnt,
                    c.episodes, c.format, c.duration
             FROM series s
             LEFT JOIN anilist_catalog c ON c.id = s.anilist_id
             WHERE s.source_id=?1
               AND (s.watched_externally=1
                    OR EXISTS (SELECT 1 FROM episodes e WHERE e.series_id=s.id AND e.seen=1))",
        )?;
        let rows: Vec<SeriesWatchRow> = stmt
            .query_map([source_id], |r| {
                Ok(SeriesWatchRow {
                    title: r.get(0)?,
                    kind: r.get(1)?,
                    watched_externally: r.get::<_, i64>(2)? != 0,
                    seen_count: r.get(3)?,
                    catalog_episodes: r.get(4)?,
                    catalog_format: r.get(5)?,
                    catalog_duration: r.get(6)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        let mut grouped: HashMap<String, FranchiseRollup> = HashMap::new();
        for row in rows {
            let key = franchise_key(&row.title);
            let display = franchise_display_title(&row.title);
            let entry = grouped.entry(key).or_insert_with(|| FranchiseRollup {
                display_title: display.clone(),
                real_seen: 0,
                real_minutes: 0,
                external_episodes: 0,
                external_minutes: 0,
            });
            if prefer_display_title(&display, &entry.display_title) {
                entry.display_title = display;
            }

            // AniList's real per-episode duration wins whenever the series is
            // linked to a synced catalog row that has one; otherwise fall back
            // to the format/kind estimate, preferring the site's own `kind`
            // (always present for scraped rows) over the catalog `format`.
            let per_episode = row.catalog_duration.unwrap_or_else(|| {
                minutes_per_episode(row.kind.as_deref().or(row.catalog_format.as_deref()))
            });
            entry.real_seen += row.seen_count;
            entry.real_minutes += row.seen_count * per_episode;

            // A "Ya lo vi" row only contributes a catalog-based estimate when
            // it has no real seen episodes of its own — otherwise the real
            // data already describes it better.
            if row.watched_externally && row.seen_count == 0 {
                if let Some(episodes) = row.catalog_episodes {
                    if episodes > entry.external_episodes {
                        entry.external_episodes = episodes;
                        entry.external_minutes = episodes * per_episode;
                    }
                }
            }
        }
        Ok(grouped.into_values().collect())
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
        let rollups = self.franchise_rollups(source_id)?;
        // Episodes credited on top of `episodes_watched` from "Ya lo vi"
        // catalog estimates. Deliberately the per-franchise *remainder* (see
        // `FranchiseRollup`), not a raw sum: a franchise whose arc rows
        // already account for 151 real seen episodes and whose AniList entry
        // says 1140 contributes 989 here, so the two figures add up to 1140
        // rather than to 1291.
        let episodes_watched_external: i64 =
            rollups.iter().map(|r| r.extra_external_episodes()).sum();
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
        // Distinct animes = distinct franchises with actual watch evidence —
        // watched-externally (a "Ya lo vi" swipe) OR at least one real seen
        // episode — so seasons/arcs of the same show count once. A followed
        // series with zero seen episodes has no evidence of having been
        // watched and is excluded (it's "in my library", not "in my watched
        // anime"); `franchise_rollups`' WHERE clause enforces exactly that.
        let distinct_anime = rollups.len() as i64;
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
        // Both minute figures come off the same franchise roll-up that
        // `get_watch_summary` uses, so the two screens can never disagree.
        // "Tracked" is the minutes behind really-marked episodes; "external"
        // is only the *remainder* a "Ya lo vi" catalog estimate adds on top of
        // those, never a parallel total that would double-count the overlap
        // (see `FranchiseRollup`).
        let rollups = self.franchise_rollups(source_id)?;
        let estimated_minutes_tracked: i64 = rollups.iter().map(|r| r.real_minutes).sum();
        let estimated_minutes_external: i64 =
            rollups.iter().map(|r| r.extra_external_minutes()).sum();
        let external_titles_estimated =
            rollups.iter().filter(|r| r.extra_external_episodes() > 0).count() as i64;

        // Counted as franchises, not raw rows, so it is comparable with
        // `external_titles_estimated` — the site lists one row per season/arc,
        // which would otherwise make the "X of Y estimated" ratio nonsense.
        let mut stmt = self.conn.prepare(
            "SELECT title FROM series WHERE source_id=?1 AND watched_externally=1",
        )?;
        let external_franchises: HashSet<String> = stmt
            .query_map([source_id], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<String>>>()?
            .iter()
            .map(|t| franchise_key(t))
            .collect();
        let external_titles_total = external_franchises.len() as i64;

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

        // One row per franchise, not per `series` row: the scraped site models
        // long-runners like One Piece as several distinct `series` rows (one
        // per arc/saga), so a plain per-series ranking reported one arc's own
        // seen-count (e.g. 114) under that arc's name ("One Piece: Arco de
        // Elbaph") as if it were the whole show. The roll-up credits the
        // franchise with `episodes()` — real marks, or the bigger "Ya lo vi"
        // catalog total when there is one — and labels it with
        // `franchise_display_title` ("One Piece").
        let mut top_series: Vec<crate::models::TitleCount> = rollups
            .iter()
            .filter(|r| r.episodes() > 0)
            .map(|r| crate::models::TitleCount {
                title: r.display_title.clone(),
                count: r.episodes(),
            })
            .collect();
        top_series.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.title.cmp(&b.title)));
        top_series.truncate(8);

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
    fn franchise_display_title_strips_arc_and_season_qualifiers_keeping_case() {
        assert_eq!(franchise_display_title("One Piece: Arco de Elbaph"), "One Piece");
        assert_eq!(franchise_display_title("One Piece: Live Action (2023) Temporada 2"), "One Piece");
        assert_eq!(franchise_display_title("Kimetsu no Yaiba Temporada 2"), "Kimetsu no Yaiba");
        assert_eq!(franchise_display_title("Overlord IV"), "Overlord");
        // Accents and internal punctuation survive — this is display text.
        assert_eq!(franchise_display_title("Fate/stay night: Unlimited Blade Works"), "Fate/stay night");
        // Nothing to strip: returned untouched.
        assert_eq!(franchise_display_title("Bleach"), "Bleach");
    }

    #[test]
    fn franchise_display_title_never_returns_empty() {
        assert_eq!(franchise_display_title("Temporada 2"), "Temporada 2");
        assert_eq!(franchise_display_title(": only a subtitle"), ": only a subtitle");
    }

    #[test]
    fn franchise_display_title_agrees_with_franchise_key_grouping() {
        // Any two titles that group together must also display the same name,
        // otherwise a franchise row's label depends on query order.
        for (a, b) in [
            ("One Piece: Arco de Elbaph", "One Piece: Wano"),
            ("Overlord", "Overlord IV"),
            ("Kimetsu no Yaiba", "Kimetsu no Yaiba Temporada 2"),
        ] {
            assert_eq!(franchise_key(a), franchise_key(b), "{a} / {b} must group");
            assert_eq!(franchise_display_title(a), franchise_display_title(b), "{a} / {b} must display alike");
        }
    }

    #[test]
    fn top_series_reports_the_franchise_name_and_the_whole_franchise_count() {
        // The real-world One Piece shape: the site lists per-arc rows, each
        // with its own scraped episodes, and the user separately swiped "Ya lo
        // vi" on the AniList entry for the whole show.
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("A", "a", "animeytx").unwrap();

        let arc = db.upsert_series(src, &mk_airing("op-elbaph", "One Piece: Arco de Elbaph", None)).unwrap();
        db.set_followed(arc, true).unwrap();
        db.set_kind(arc, "TV").unwrap();
        for n in 1..=20 {
            db.insert_episode(&crate::models::Episode {
                id: 0, series_id: arc, number: n.to_string(), title: None,
                url: format!("https://site/anime/op-elbaph-capitulo-{n}/"),
                released_at: None, seen: false,
            }).unwrap();
        }
        db.set_seen_cascade(arc, "20", true).unwrap();

        let season2 = db.upsert_series(src, &mk_airing("op-t2", "One Piece Temporada 2", None)).unwrap();
        db.set_kind(season2, "TV").unwrap();
        for n in 1..=5 {
            db.insert_episode(&crate::models::Episode {
                id: 0, series_id: season2, number: n.to_string(), title: None,
                url: format!("https://site/anime/op-t2-capitulo-{n}/"),
                released_at: None, seen: false,
            }).unwrap();
        }
        db.set_seen_cascade(season2, "5", true).unwrap();

        db.upsert_catalog_anime(
            &crate::anilist::CatalogAnime {
                id: 21, title: "ONE PIECE".into(), title_romaji: None, title_english: None,
                cover_url: None, format: Some("TV".into()), genres: vec![],
                episodes: Some(1140), average_score: None, popularity: None,
                url: "https://anilist.co/anime/21".into(), status: None, duration: Some(24),
                studio: None, start_date: None,
            },
            0,
        ).unwrap();
        let whole = db.upsert_series(src, &mk_airing("one-piece", "ONE PIECE", None)).unwrap();
        db.set_watched_externally(whole, true).unwrap();
        db.set_anilist_id(whole, 21).unwrap();

        let insights = db.get_watch_insights(src).unwrap();
        let top = insights.top_series.first().expect("one franchise expected");
        assert_eq!(top.title, "One Piece", "labelled with the franchise, not the arc");
        assert_eq!(top.count, 1140, "credited with the whole show, not one arc's 20 episodes");
        assert_eq!(insights.top_series.len(), 1, "all three rows are one franchise");
    }

    #[test]
    fn franchise_external_estimate_never_double_counts_real_marks() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("A", "a", "animeytx").unwrap();

        db.upsert_catalog_anime(
            &crate::anilist::CatalogAnime {
                id: 21, title: "ONE PIECE".into(), title_romaji: None, title_english: None,
                cover_url: None, format: Some("TV".into()), genres: vec![],
                episodes: Some(100), average_score: None, popularity: None,
                url: "https://anilist.co/anime/21".into(), status: None, duration: Some(24),
                studio: None, start_date: None,
            },
            0,
        ).unwrap();

        let arc = db.upsert_series(src, &mk_airing("op-arc", "One Piece: Arco", None)).unwrap();
        db.set_kind(arc, "TV").unwrap();
        for n in 1..=30 {
            db.insert_episode(&crate::models::Episode {
                id: 0, series_id: arc, number: n.to_string(), title: None,
                url: format!("https://site/anime/op-arc-capitulo-{n}/"),
                released_at: None, seen: false,
            }).unwrap();
        }
        db.set_seen_cascade(arc, "30", true).unwrap();

        let whole = db.upsert_series(src, &mk_airing("one-piece", "One Piece", None)).unwrap();
        db.set_watched_externally(whole, true).unwrap();
        db.set_anilist_id(whole, 21).unwrap();

        let summary = db.get_watch_summary(src).unwrap();
        assert_eq!(summary.episodes_watched, 30, "real marks stay exactly as marked");
        assert_eq!(
            summary.episodes_watched_external, 70,
            "only the 100-30 remainder is credited on top, so the two add up to 100 not 130"
        );

        let insights = db.get_watch_insights(src).unwrap();
        assert_eq!(insights.estimated_minutes_tracked, 30 * 24);
        assert_eq!(insights.estimated_minutes_external, 70 * 24, "remainder minutes only");
        assert_eq!(insights.external_titles_estimated, 1);
        assert_eq!(insights.external_titles_total, 1, "counted per franchise, not per row");
    }

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
                studio: None, start_date: None,
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
                crate::models::TitleCount { title: "External Show".into(), count: 12 },
                crate::models::TitleCount { title: "Airing Show".into(), count: 3 },
                crate::models::TitleCount { title: "Finished Show".into(), count: 2 },
            ],
            "ordered by credited episodes descending; a 'Ya lo vi' title is credited \
             with its catalog episode count, which is the only evidence it has"
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
                studio: None, start_date: None,
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
                studio: None, start_date: None,
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
                studio: None, start_date: None,
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
    fn top_series_sums_watched_episodes_across_a_franchises_split_arc_rows() {
        // Reproduces the reported bug: the site models a long-runner as
        // several `series` rows by arc, so a naive per-row GROUP BY reported
        // one arc's own count (114) instead of the show's real total.
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("A", "a", "animeytx").unwrap();

        let arcs = [
            ("op1", "One Piece", 500),
            ("op2", "One Piece: Wano", 400),
            ("op3", "One Piece: Arco de Elbaph", 114),
        ];
        for (slug, title, seen_count) in arcs {
            let id = db.upsert_series(src, &mk_airing(slug, title, None)).unwrap();
            db.set_followed(id, true).unwrap();
            insert_eps_seen_up_to(&db, id, seen_count, seen_count);
        }
        // An unrelated series must stay in its own bucket, not get folded in.
        let n = db.upsert_series(src, &mk_airing("naruto", "Naruto", None)).unwrap();
        db.set_followed(n, true).unwrap();
        insert_eps_seen_up_to(&db, n, 50, 50);

        let insights = db.get_watch_insights(src).unwrap();
        let one_piece = insights.top_series.iter().find(|t| t.title == "One Piece").expect("collapsed One Piece entry");
        assert_eq!(one_piece.count, 1014, "500 + 400 + 114 summed across all arc rows");
        assert!(
            !insights.top_series.iter().any(|t| t.title.contains("Wano") || t.title.contains("Elbaph")),
            "arc-specific rows must not surface as separate top-series entries: {:?}",
            insights.top_series
        );
        let naruto = insights.top_series.iter().find(|t| t.title == "Naruto").expect("Naruto entry");
        assert_eq!(naruto.count, 50);
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
