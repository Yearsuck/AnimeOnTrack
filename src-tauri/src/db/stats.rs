use super::*;
use std::collections::{HashMap, HashSet};
use chrono::Datelike;

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
    let norm = crate::matching::normalize_title(title);
    let tokens: Vec<&str> = strip_season_markers(norm.split_whitespace().collect(), |t| t.to_string());
    let key = tokens.join(" ");
    if key.is_empty() {
        norm
    } else {
        key
    }
}

/// The franchise key of the text before a colon, when a colon-introduced
/// qualifier is present at all. `None` otherwise, and `None` when stripping it
/// wouldn't change the key.
///
/// This is only ever a *candidate* parent, never applied on its own: the
/// scraped site does split long-runners as "One Piece: Arco de Elbaph", but a
/// colon is also ordinary title punctuation ("Re:Zero kara Hajimeru Isekai
/// Seikatsu", "Code:Breaker", "Re:CREATORS", "Re: Hamatora"), and truncating
/// at it unconditionally collapsed six unrelated shows into a single franchise
/// called "Re". `franchise_rollups` therefore only merges a group into its
/// parent when the parent key is independently present in the user's own data
/// — i.e. they really do have a "One Piece" row — so a colon that is just
/// punctuation never merges anything.
pub(crate) fn franchise_parent_key(title: &str) -> Option<String> {
    let (head, _) = title.split_once(':')?;
    if head.trim().is_empty() {
        return None;
    }
    let parent = franchise_key(head);
    (!parent.is_empty() && parent != franchise_key(title)).then_some(parent)
}

/// Drop trailing season/part markers from `tokens`, testing each token's
/// normalized form via `normalize` (identity for already-normalized input,
/// real normalization for raw display tokens).
///
/// A bare trailing integer is only a season number when a marker word
/// introduces it ("Temporada 2", "Part 2") — stripping any trailing digit
/// unconditionally merged genuinely distinct titles whose name ends in one,
/// e.g. "Steins;Gate 0" (a different work, with its own AniList entry and
/// episode count) into "Steins;Gate".
fn strip_season_markers(
    mut tokens: Vec<&str>,
    normalize: impl Fn(&str) -> String,
) -> Vec<&str> {
    const ROMANS: &[&str] = &["ii", "iii", "iv", "v", "vi", "vii", "viii", "ix", "x"];
    const MARKER_WORDS: &[&str] =
        &["season", "temporada", "part", "parte", "cour", "final", "the"];
    let is_marker_word = |t: &str| MARKER_WORDS.contains(&t);
    let is_int = |t: &str| !t.is_empty() && t.chars().all(|c| c.is_ascii_digit());
    let is_ordinal = |t: &str| {
        // char-boundary safe: use strip_suffix instead of byte indexing.
        // Titles can contain multi-byte Unicode (roman numerals, Cyrillic,
        // superscripts, fullwidth, fractions). Byte-slicing panics on non-char
        // boundaries; in #[tauri::command] paths on the main thread this aborts
        // the whole process (FAST_FAIL_FATAL_APP_EXIT).
        let Some(head) = t
            .strip_suffix("st")
            .or_else(|| t.strip_suffix("nd"))
            .or_else(|| t.strip_suffix("rd"))
            .or_else(|| t.strip_suffix("th"))
        else {
            return false;
        };
        !head.is_empty() && head.chars().all(|c| c.is_ascii_digit())
    };

    while let Some(&last) = tokens.last() {
        let normalized = normalize(last);
        if is_marker_word(&normalized) || is_ordinal(&normalized) || ROMANS.contains(&normalized.as_str())
        {
            tokens.pop();
            continue;
        }
        if is_int(&normalized) {
            let introduced_by_marker = tokens.len() >= 2
                && is_marker_word(&normalize(tokens[tokens.len() - 2]));
            if introduced_by_marker {
                tokens.pop();
                tokens.pop();
                continue;
            }
        }
        break;
    }
    tokens
}

/// A human-facing franchise name derived from one member title, with the
/// original casing and accents preserved — `franchise_key`'s normalized output
/// ("one piece") is a grouping key, never something to show a user.
///
/// Applies the same reduction as `franchise_key` — strip trailing season/part
/// markers — but on the raw tokens, testing each one's normalized form. "One
/// Piece Temporada 2" yields "One Piece". A colon qualifier is deliberately
/// *kept*: this labels whichever group a title ended up in, and dropping the
/// colon tail here would claim a merge that `franchise_rollups` may not have
/// made (see `franchise_parent_key`). When a merge does happen, the parent
/// group supplies the label. Falls back to the trimmed input when stripping
/// would leave nothing.
pub(crate) fn franchise_display_title(title: &str) -> String {
    let tokens = strip_season_markers(
        title.split_whitespace().collect(),
        crate::matching::normalize_title,
    );
    let out = tokens.join(" ").trim().to_string();
    if out.is_empty() {
        title.trim().to_string()
    } else {
        out
    }
}

/// The show name a colon-qualified title hangs off, with casing intact — "One
/// Piece: Arco de Elbaph" gives "One Piece". Unlike `franchise_display_title`
/// this *does* drop the colon tail, so it is a lookup candidate rather than a
/// label: `commands::link_series_to_catalog` tries it against the AniList
/// catalog only after the full title misses, and a wrong guess simply fails to
/// match rather than mislabelling anything.
pub(crate) fn franchise_base_title(title: &str) -> String {
    let head = match title.split_once(':') {
        Some((head, _)) if !head.trim().is_empty() => head,
        _ => title,
    };
    franchise_display_title(head)
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

/// Fold each group whose colon-stripped parent key is *also* a group in this
/// same data into that parent, then return the survivors.
///
/// The corpus check is the whole point: "One Piece: Arco de Elbaph" only joins
/// "One Piece" because the user really has a One Piece row, whereas "Re:Zero
/// kara Hajimeru Isekai Seikatsu" stays put because nobody has a series called
/// "Re". Parents are resolved transitively (a merged group's own parent still
/// applies) with a hard depth cap, so a pathological chain — or a cycle that
/// shouldn't be constructible but must not hang if it were — bottoms out.
fn merge_into_parent_franchises(
    groups: HashMap<String, FranchiseRollup>,
    parent_of: &HashMap<String, String>,
) -> Vec<FranchiseRollup> {
    const MAX_DEPTH: usize = 8;
    let resolve = |start: &str| -> String {
        let mut current = start.to_string();
        for _ in 0..MAX_DEPTH {
            match parent_of.get(&current) {
                Some(parent) if groups.contains_key(parent) && parent != &current => {
                    current = parent.clone();
                }
                _ => break,
            }
        }
        current
    };

    // Seed with the survivors first, so a parent group's own label is already
    // in place before any child folds in — otherwise whichever of the two the
    // HashMap happened to yield first would decide the name.
    let mut merged: HashMap<String, FranchiseRollup> = groups
        .iter()
        .filter(|(key, _)| resolve(key) == **key)
        .map(|(key, rollup)| (key.clone(), rollup.clone()))
        .collect();
    for (key, rollup) in &groups {
        let target = resolve(key);
        if target == *key {
            continue;
        }
        let Some(existing) = merged.get_mut(&target) else { continue };
        existing.real_seen += rollup.real_seen;
        existing.real_minutes += rollup.real_minutes;
        if rollup.external_episodes > existing.external_episodes {
            existing.external_episodes = rollup.external_episodes;
            existing.external_minutes = rollup.external_minutes;
        }
        // The parent's label already won by being seeded above; a child only
        // ever contributes counts.
    }
    merged.into_values().collect()
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
#[derive(Clone)]
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

/// One canonical (cross-site) followed show, as grouped by
/// `Db::group_followed_canonically` — the descriptive-stats counterpart to
/// `FranchiseRollup` (which handles the numeric watch totals).
struct CanonicalFollowed {
    member_ids: Vec<i64>,
    display_title: String,
    kind: Option<String>,
    cover_url: Option<String>,
}

impl Db {
    /// Genre counts across every followed show, canonical across sites: a
    /// show followed on two sites contributes its genre set once (the union
    /// of what each site tagged it with), not twice — see
    /// `group_followed_canonically`.
    pub fn get_genre_stats(&self) -> Result<Vec<crate::models::GenreStat>> {
        let groups = self.group_followed_canonically()?;
        let mut counts: HashMap<String, i64> = HashMap::new();
        for g in &groups {
            let mut genres: HashSet<String> = HashSet::new();
            for &id in &g.member_ids {
                genres.extend(self.list_series_genres(id)?);
            }
            for genre in genres {
                *counts.entry(genre).or_insert(0) += 1;
            }
        }
        let mut rows: Vec<crate::models::GenreStat> = counts
            .into_iter()
            .map(|(genre, count)| crate::models::GenreStat { genre, count })
            .collect();
        rows.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.genre.cmp(&b.genre)));
        Ok(rows)
    }

    /// Followed-show count per `kind` ("TV"/"OVA"/...), descending, canonical
    /// across sites (one show followed on two sites counts once, under
    /// whichever site's `kind` is set first). Shows with no `kind` on any
    /// member are excluded.
    pub fn get_type_stats(&self) -> Result<Vec<crate::models::TypeStat>> {
        let groups = self.group_followed_canonically()?;
        let mut counts: HashMap<String, i64> = HashMap::new();
        for g in &groups {
            if let Some(kind) = g.kind.as_deref().filter(|k| !k.is_empty()) {
                *counts.entry(kind.to_string()).or_insert(0) += 1;
            }
        }
        let mut rows: Vec<crate::models::TypeStat> = counts
            .into_iter()
            .map(|(kind, count)| crate::models::TypeStat { kind, count })
            .collect();
        rows.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.kind.cmp(&b.kind)));
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

    /// Followed shows with their genres (unioned across sites) and kind, for
    /// the 3D relationship graph, canonical across sites — one node per show,
    /// not per site's `series` row (see `group_followed_canonically`). The
    /// node's `id` is one representative member's `series.id`; it only needs
    /// to be a stable per-node key for the graph, not a specific site's row.
    pub fn get_stats_graph_data(&self) -> Result<Vec<crate::models::SeriesGraphNode>> {
        let groups = self.group_followed_canonically()?;
        let mut nodes: Vec<crate::models::SeriesGraphNode> = groups
            .into_iter()
            .map(|g| {
                let mut genres: HashSet<String> = HashSet::new();
                for &id in &g.member_ids {
                    genres.extend(self.list_series_genres(id)?);
                }
                let mut genres: Vec<String> = genres.into_iter().collect();
                genres.sort();
                Ok(crate::models::SeriesGraphNode {
                    id: g.member_ids[0],
                    title: g.display_title,
                    cover_url: g.cover_url,
                    genres,
                    kind: g.kind,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        nodes.sort_by(|a, b| a.title.cmp(&b.title));
        Ok(nodes)
    }

    /// One canonical followed show, grouped from every `series` row (any
    /// site) sharing its `canon_key` — see `super::library::canon_key`. Used
    /// by the descriptive stats (genre/type/graph) that need more than a
    /// count: which `kind`, which cover, which genres. `display_title` and
    /// `kind`/`cover_url` come from the first member encountered per group
    /// (arbitrary but deterministic per query — good enough for display,
    /// unlike watch totals which must never depend on row order).
    fn group_followed_canonically(&self) -> Result<Vec<CanonicalFollowed>> {
        struct Row {
            id: i64,
            anilist_id: Option<i64>,
            title: String,
            kind: Option<String>,
            cover_url: Option<String>,
        }
        let mut stmt = self
            .conn
            .prepare("SELECT id, anilist_id, title, kind, cover_url FROM series WHERE followed=1")?;
        let rows: Vec<Row> = stmt
            .query_map([], |r| {
                Ok(Row {
                    id: r.get(0)?,
                    anilist_id: r.get(1)?,
                    title: r.get(2)?,
                    kind: r.get(3)?,
                    cover_url: r.get(4)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        let mut grouped: HashMap<String, CanonicalFollowed> = HashMap::new();
        for row in rows {
            let key = super::library::canon_key(row.anilist_id, &row.title);
            let entry = grouped.entry(key).or_insert_with(|| CanonicalFollowed {
                member_ids: Vec::new(),
                display_title: row.title.clone(),
                kind: None,
                cover_url: None,
            });
            entry.member_ids.push(row.id);
            if entry.kind.is_none() {
                entry.kind = row.kind.clone();
            }
            if entry.cover_url.is_none() {
                entry.cover_url = row.cover_url.clone();
            }
            if prefer_display_title(&row.title, &entry.display_title) {
                entry.display_title = row.title;
            }
        }
        Ok(grouped.into_values().collect())
    }

    /// Count of distinct `canon_key`s among rows returned by `query` (which
    /// must select `(anilist_id, title)` in that order and take no params) —
    /// the cross-site dedup used by the scalar watch counts in
    /// `get_watch_summary`/`get_watch_insights`. Never reads the `library`
    /// table (see `get_watch_summary`'s doc comment for why); always
    /// recomputed live from `series`.
    fn distinct_canon_count(&self, query: &str) -> Result<i64> {
        let mut stmt = self.conn.prepare(query)?;
        let keys: HashSet<String> = stmt
            .query_map([], |r| {
                let anilist_id: Option<i64> = r.get(0)?;
                let title: String = r.get(1)?;
                Ok(super::library::canon_key(anilist_id, &title))
            })?
            .collect::<rusqlite::Result<HashSet<_>>>()?;
        Ok(keys.len() as i64)
    }

    /// Every franchise with watch evidence, rolled up from its member `series`
    /// rows **across every site** (not just the active one) — a show followed
    /// or watched on any of the 3 sites contributes its evidence here, so
    /// switching the active site never changes the totals. One query for the
    /// whole dashboard — `get_watch_summary` and `get_watch_insights` both
    /// build on this so they can never disagree about what a franchise's
    /// episode/minute totals are.
    ///
    /// Cross-site merging reuses the exact same `franchise_key` grouping that
    /// already collapses one site's per-arc rows ("One Piece", "One Piece:
    /// Wano") into one franchise — normalized-title matching also collapses
    /// the same show scraped under near-identical titles on two different
    /// sites, without needing `anilist_id` as the primary key (which would
    /// regress the arc-collapsing case whenever only *some* arcs are linked).
    pub(crate) fn franchise_rollups(&self) -> Result<Vec<FranchiseRollup>> {
        let mut stmt = self.conn.prepare(
            "SELECT s.title, s.kind, s.watched_externally,
                    (SELECT COUNT(*) FROM episodes e WHERE e.series_id=s.id AND e.seen=1) AS seen_cnt,
                    c.episodes, c.format, c.duration
             FROM series s
             LEFT JOIN anilist_catalog c ON c.id = s.anilist_id
             WHERE s.watched_externally=1
                OR EXISTS (SELECT 1 FROM episodes e WHERE e.series_id=s.id AND e.seen=1)",
        )?;
        let rows: Vec<SeriesWatchRow> = stmt
            .query_map([], |r| {
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
        // Candidate parent per group, recorded while grouping and applied only
        // afterwards, once every key present in this user's data is known.
        let mut parent_of: HashMap<String, String> = HashMap::new();
        for row in rows {
            let key = franchise_key(&row.title);
            let display = franchise_display_title(&row.title);
            if let Some(parent) = franchise_parent_key(&row.title) {
                parent_of.entry(key.clone()).or_insert(parent);
            }
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

            // A "Ya lo vi" row contributes its catalog total regardless of
            // whether that same row also has real marks. Gating this on the
            // row's own seen-count threw the number away exactly when a user
            // both followed a long-runner and swiped "Ya lo vi" on it (49 rows
            // on the reference database, e.g. Boruto: 29 real marks hiding a
            // known 293-episode total). Nothing is double-counted by admitting
            // it — `episodes()` takes the larger of the two signals and
            // `extra_external_episodes()` reports only the remainder.
            if row.watched_externally {
                if let Some(episodes) = row.catalog_episodes {
                    if episodes > entry.external_episodes {
                        entry.external_episodes = episodes;
                        entry.external_minutes = episodes * per_episode;
                    }
                }
            }
        }
        Ok(merge_into_parent_franchises(grouped, &parent_of))
    }

    /// Scalar watch totals for the stats dashboard, **canonical across every
    /// site**: a show followed (or wanted, or pending) on any of the 3 sites
    /// counts once, via `canon_key` dedup — see `distinct_canon_count`. Never
    /// reads the `library` table: it's only resynced at startup and on the
    /// cross-site import flow, not on every follow/seen, so it would show
    /// stale numbers mid-session. This recomputes live from `series` instead,
    /// same as `list_airing` does for the same reason.
    pub fn get_watch_summary(&self) -> Result<crate::models::WatchSummary> {
        let followed_series =
            self.distinct_canon_count("SELECT anilist_id, title FROM series WHERE followed=1")?;
        // Real seen-episode count across ALL series, followed or not — a
        // "Ya lo vi" swipe whose site-link scraped real episodes must count
        // too (see the episode/anime-counts-fix design doc). `episodes_total`
        // stays scoped to series that mean something for a "how far along
        // am I" denominator: followed, or with at least one real seen
        // episode — so discarded/never-touched scraped rows don't inflate it.
        let episodes_watched: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM episodes e WHERE e.seen=1",
            [],
            |r| r.get(0),
        )?;
        let episodes_total: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM episodes e
             JOIN series s ON s.id = e.series_id
             WHERE s.followed=1 OR EXISTS (SELECT 1 FROM episodes x WHERE x.series_id=s.id AND x.seen=1)",
            [],
            |r| r.get(0),
        )?;
        let rollups = self.franchise_rollups()?;
        // Episodes credited on top of `episodes_watched` from "Ya lo vi"
        // catalog estimates. Deliberately the per-franchise *remainder* (see
        // `FranchiseRollup`), not a raw sum: a franchise whose arc rows
        // already account for 151 real seen episodes and whose AniList entry
        // says 1140 contributes 989 here, so the two figures add up to 1140
        // rather than to 1291.
        let episodes_watched_external: i64 =
            rollups.iter().map(|r| r.extra_external_episodes()).sum();
        let backlog_want = self.distinct_canon_count(
            "SELECT anilist_id, title FROM series WHERE backlog_status='want'",
        )?;
        let airing_followed = self.distinct_canon_count(
            "SELECT anilist_id, title FROM series WHERE followed=1 AND is_airing=1",
        )?;
        let pending_to_watch = self.distinct_canon_count(
            "SELECT DISTINCT s.anilist_id, s.title FROM series s
             JOIN episodes e ON e.series_id = s.id
             WHERE s.followed=1 AND e.seen=0",
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
    pub fn get_watch_insights(&self) -> Result<crate::models::WatchInsights> {
        // Both minute figures come off the same franchise roll-up that
        // `get_watch_summary` uses, so the two screens can never disagree.
        // "Tracked" is the minutes behind really-marked episodes; "external"
        // is only the *remainder* a "Ya lo vi" catalog estimate adds on top of
        // those, never a parallel total that would double-count the overlap
        // (see `FranchiseRollup`).
        let rollups = self.franchise_rollups()?;
        let estimated_minutes_tracked: i64 = rollups.iter().map(|r| r.real_minutes).sum();
        let estimated_minutes_external: i64 =
            rollups.iter().map(|r| r.extra_external_minutes()).sum();
        // "Estimated" means a catalog episode count was available for the
        // franchise, not that it happened to exceed the real marks. Counting
        // only the ones with a nonzero remainder made the "X of Y" ratio
        // understate itself: a franchise whose real marks already cover its
        // catalog total is fully accounted for, not missing data.
        let external_titles_estimated =
            rollups.iter().filter(|r| r.external_episodes > 0).count() as i64;

        // Counted as franchises, not raw rows, so it is comparable with
        // `external_titles_estimated` — the site lists one row per season/arc
        // (and now, across sites, one row per site too), which would otherwise
        // make the "X of Y estimated" ratio nonsense.
        let mut stmt =
            self.conn.prepare("SELECT title FROM series WHERE watched_externally=1")?;
        let external_franchises: HashSet<String> = stmt
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<String>>>()?
            .iter()
            .map(|t| franchise_key(t))
            .collect();
        let external_titles_total = external_franchises.len() as i64;

        // Mean episode count (all episodes, not just seen) across followed
        // series rows (every site's row counts separately — this is a rough
        // "how long are the shows I follow" gauge, not a watch total, so
        // cross-site duplicates skewing it slightly is an acceptable
        // approximation). NULL (no followed series at all) reads as 0.0.
        let avg_episodes_per_series: Option<f64> = self.conn.query_row(
            "SELECT AVG(cnt) FROM (
                SELECT s.id, COUNT(e.id) AS cnt
                FROM series s LEFT JOIN episodes e ON e.series_id = s.id
                WHERE s.followed=1
                GROUP BY s.id
             )",
            [],
            |r| r.get(0),
        )?;
        let avg_episodes_per_series = avg_episodes_per_series.unwrap_or(0.0);

        // Canonical (cross-site-deduped) funnel counts — see
        // `get_watch_summary`'s doc comment for why this never reads the
        // `library` table. Kept consistent with `get_watch_summary`'s own
        // `airing_followed`/`backlog_want`/`watched_externally`-shaped counts
        // so the two screens can't disagree with each other either.
        let followed_airing = self.distinct_canon_count(
            "SELECT anilist_id, title FROM series WHERE followed=1 AND is_airing=1",
        )?;
        let followed_finished = self.distinct_canon_count(
            "SELECT anilist_id, title FROM series WHERE followed=1 AND is_airing=0",
        )?;
        let discarded = self.distinct_canon_count(
            "SELECT anilist_id, title FROM series WHERE backlog_status='discarded'",
        )?;
        let want = self.distinct_canon_count(
            "SELECT anilist_id, title FROM series WHERE backlog_status='want'",
        )?;
        let watched_externally = self.distinct_canon_count(
            "SELECT anilist_id, title FROM series WHERE watched_externally=1",
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

        // `seen_at` is stamped with SQLite's `datetime('now')`, i.e. UTC, so
        // every bucket here converts to 'localtime' first. Without it an
        // episode marked at 00:30 in Spain (UTC+1/+2) lands on the previous
        // calendar day — and the late evening is exactly when this app is
        // used, so the misattribution is the common case, not an edge one.
        let mut stmt = self.conn.prepare(
            "SELECT DATE(e.seen_at, 'localtime') AS d, COUNT(*) AS cnt
             FROM episodes e
             WHERE e.seen_at IS NOT NULL
               AND DATE(e.seen_at, 'localtime') >= DATE('now', 'localtime', '-29 days')
             GROUP BY d
             ORDER BY d",
        )?;
        let counted_days: HashMap<String, i64> = stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?
            .collect::<rusqlite::Result<HashMap<_, _>>>()?;

        // Zero-filled 30-day spine. SQL only emits days that have marks, and a
        // consumer plotting that array directly draws inactive days as if they
        // never existed, silently compressing the timeline. Handing back an
        // explicit `count: 0` for every quiet day makes the shape honest and
        // means no caller has to reconstruct the calendar.
        let today = chrono::Local::now().date_naive();
        let marks_by_day: Vec<crate::models::DayCount> = (0..30)
            .rev()
            .filter_map(|offset| today.checked_sub_days(chrono::Days::new(offset)))
            .map(|date| {
                let day = date.format("%Y-%m-%d").to_string();
                let count = counted_days.get(&day).copied().unwrap_or(0);
                crate::models::DayCount { day, count }
            })
            .collect();

        let marks_tracked_since: Option<String> = self.conn.query_row(
            "SELECT MIN(DATE(e.seen_at, 'localtime')) FROM episodes e WHERE e.seen_at IS NOT NULL",
            [],
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

    /// Distinct years (local time) that have at least one seen episode,
    /// descending. Always includes the current year even if it has no marks yet,
    /// so a brand-new user still gets this year's empty grid instead of an empty
    /// year list.
    pub fn get_activity_years(&self) -> Result<Vec<i32>> {
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT CAST(strftime('%Y', DATE(e.seen_at, 'localtime')) AS INTEGER) AS y
             FROM episodes e
             WHERE e.seen_at IS NOT NULL
             ORDER BY y DESC",
        )?;
        let mut years: Vec<i32> = stmt
            .query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<Vec<i32>>>()?;

        // Always include the current year (local time) so the year selector is
        // never empty — mirrors the `marks_tracked_since` grace handling.
        let this_year = chrono::Local::now().date_naive().year();
        if !years.contains(&this_year) {
            years.insert(0, this_year);
        }

        Ok(years)
    }

    /// Full-year activity heatmap: one entry per day of the given calendar year
    /// (Jan 1 to Dec 31), zero-filled like `WatchInsights.marks_by_day` but
    /// across 365/366 days. Uses the same `DATE(e.seen_at, 'localtime')`
    /// conversion as the 30-day spine so an episode marked at 00:30 in Spain
    /// (UTC+1/+2) lands on the correct local calendar day.
    pub fn get_yearly_activity(&self, year: i32) -> Result<crate::models::YearlyActivity> {
        // Determine if the target year is a leap year using chrono (already a
        // dependency) so we generate the correct number of days.
        let is_leap = chrono::NaiveDate::from_ymd_opt(year, 2, 29).is_some();
        let days_in_year = if is_leap { 366 } else { 365 };

        // Count seen episodes per day within the given year (local time).
        let mut stmt = self.conn.prepare(
            "SELECT DATE(e.seen_at, 'localtime') AS d, COUNT(*) AS cnt
             FROM episodes e
             WHERE e.seen_at IS NOT NULL
               AND CAST(strftime('%Y', DATE(e.seen_at, 'localtime')) AS INTEGER) = ?1
             GROUP BY d
             ORDER BY d",
        )?;
        let counted_days: HashMap<String, i64> = stmt
            .query_map([year], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?
            .collect::<rusqlite::Result<HashMap<_, _>>>()?;

        // Zero-filled spine for the entire year.
        let start_date = chrono::NaiveDate::from_ymd_opt(year, 1, 1).unwrap();
        let days: Vec<crate::models::DayCount> = (0..days_in_year)
            .filter_map(|offset| start_date.checked_add_days(chrono::Days::new(offset)))
            .map(|date| {
                let day = date.format("%Y-%m-%d").to_string();
                let count = counted_days.get(&day).copied().unwrap_or(0);
                crate::models::DayCount { day, count }
            })
            .collect();

        Ok(crate::models::YearlyActivity { year, days })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_support::*;

    #[test]
    fn franchise_display_title_strips_season_markers_keeping_case() {
        assert_eq!(franchise_display_title("Kimetsu no Yaiba Temporada 2"), "Kimetsu no Yaiba");
        assert_eq!(franchise_display_title("Overlord IV"), "Overlord");
        assert_eq!(franchise_display_title("86 Eighty-Six Part 2"), "86 Eighty-Six");
        // Accents and internal punctuation survive — this is display text.
        assert_eq!(franchise_display_title("Shingeki no Kyojin: The Final Season"), "Shingeki no Kyojin:");
        // Nothing to strip: returned untouched.
        assert_eq!(franchise_display_title("Bleach"), "Bleach");
    }

    #[test]
    fn franchise_display_title_keeps_a_colon_qualifier() {
        // A label must describe the group the title actually landed in, and a
        // colon title only merges into its base when that base exists in the
        // user's own data (see franchise_parent_key) — which this pure
        // function can't know.
        assert_eq!(franchise_display_title("One Piece: Arco de Elbaph"), "One Piece: Arco de Elbaph");
        assert_eq!(franchise_display_title("Re:Zero kara Hajimeru Isekai Seikatsu"), "Re:Zero kara Hajimeru Isekai Seikatsu");
    }

    #[test]
    fn franchise_display_title_never_returns_empty() {
        assert_eq!(franchise_display_title("Temporada 2"), "Temporada 2");
        assert_eq!(franchise_display_title(": only a subtitle"), ": only a subtitle");
    }

    #[test]
    fn franchise_base_title_drops_the_colon_qualifier_for_catalog_lookup() {
        assert_eq!(franchise_base_title("One Piece: Arco de Elbaph"), "One Piece");
        assert_eq!(franchise_base_title("One Piece: Live Action (2023) Temporada 2"), "One Piece");
        assert_eq!(franchise_base_title("Fate/stay night: Unlimited Blade Works"), "Fate/stay night");
        assert_eq!(franchise_base_title("Bleach"), "Bleach");
    }

    #[test]
    fn franchise_key_does_not_merge_shows_that_merely_share_a_colon_head() {
        // Six unrelated shows previously collapsed into a single franchise
        // called "Re" because every colon was treated as an arc separator.
        let re_titles = [
            "Re:Zero kara Hajimeru Isekai Seikatsu",
            "Re:CREATORS",
            "Re:Monster",
            "Re: Hamatora",
            "Re:Stage! Dream Days",
        ];
        let keys: HashSet<String> = re_titles.iter().map(|t| franchise_key(t)).collect();
        assert_eq!(keys.len(), re_titles.len(), "each Re: show keeps its own key: {keys:?}");
        assert!(!keys.contains("re"), "nothing collapses to the bare colon head");
        assert_ne!(franchise_key("Code:Breaker"), franchise_key("Code: Realize ~Guardian of Rebirth~"));
    }

    #[test]
    fn franchise_key_keeps_a_title_that_merely_ends_in_a_digit_distinct() {
        // "Steins;Gate 0" is a different work with its own AniList entry, not
        // a second season of "Steins;Gate".
        assert_ne!(franchise_key("Steins;Gate"), franchise_key("Steins;Gate 0"));
        // A digit introduced by a season marker still strips.
        assert_eq!(franchise_key("Kimetsu no Yaiba Temporada 2"), franchise_key("Kimetsu no Yaiba"));
        assert_eq!(franchise_key("Overlord IV"), franchise_key("Overlord"));
    }

    /// Regression guard for the char-boundary panic this bug caused in
    /// `strip_season_markers` (a byte-for-byte duplicate of matching.rs'
    /// `is_ordinal`, now fixed identically). Every title comes from the user's
    /// live database and carries a multi-byte Unicode char (Roman numeral Ⅱ,
    /// Cyrillic о, fraction ⅐, superscript ⁿ, fullwidth Ａ) that byte-index
    /// slicing would have split on a non-char boundary and panicked — in a
    /// main-thread `#[tauri::command]` that aborts the whole process
    /// (FAST_FAIL_FATAL_APP_EXIT). `franchise_key`/`franchise_display_title`
    /// feed raw site titles through this path.
    /// The multi-byte char has to be in the **trailing** token: this loop reads
    /// `tokens.last()` and breaks at the first non-marker, so a multi-byte char
    /// in a leading token never reaches `is_ordinal` and never panicked — a
    /// test built on one of those would pass against the buggy code too.
    ///
    /// Second field is the ASCII core that must survive, asserted per title so
    /// one entry can't satisfy the check on behalf of the other five.
    const MULTIBYTE_TITLES: &[(&str, &str)] = &[
        // Real catalog rows: Ⅱ = U+2161 lowercases to ⅱ (U+2171), a trailing
        // token that is a single 3-byte char.
        ("Long Sword Ⅱ", "sword"),
        ("The Morose Mononokean Ⅱ", "mononokean"),
        ("Zhu Dick: Guguai Dao Da Maoxian Ⅰ", "maoxian"),
        // Other scripts from the catalog, moved into trailing position so they
        // exercise the loop: fullwidth Ａ, superscript ⁿ, fraction ⅐.
        ("Galaxy Angel Ａ", "angel"),
        ("Kuusou Episodeⁿ", "kuusou"),
        ("Tom Thumb 00⅐", "thumb"),
    ];

    #[test]
    fn multibyte_titles_do_not_panic_in_franchise_key() {
        for (title, core) in MULTIBYTE_TITLES {
            // Not panicking IS the assertion — the old byte-slicing version
            // aborted the process on every one of these. The multi-byte chars
            // are in none of the ordinal/ROMANS/marker lists, so they survive
            // intact: the fix removes a panic, it invents no stripping rule.
            let key = franchise_key(title);
            assert!(
                key.contains(core),
                "expected {core:?} to survive in {title:?} -> {key:?}"
            );
        }
    }

    #[test]
    fn multibyte_titles_do_not_panic_in_franchise_display_title() {
        for (title, core) in MULTIBYTE_TITLES {
            let display = franchise_display_title(title);
            assert!(
                display.to_lowercase().contains(core),
                "expected {core:?} to survive in {title:?} -> {display:?}"
            );
            // Unlike franchise_key, the display path keeps the original casing.
            assert!(
                display.chars().any(|c| c.is_uppercase()),
                "display title lost its casing: {title:?} -> {display:?}"
            );
        }
    }

    #[test]
    fn franchise_parent_key_only_offers_a_parent_for_a_real_qualifier() {
        assert_eq!(franchise_parent_key("One Piece: Arco de Elbaph").as_deref(), Some("one piece"));
        // No colon at all.
        assert_eq!(franchise_parent_key("Bleach"), None);
        // Colon with an empty head can't yield a parent.
        assert_eq!(franchise_parent_key(": subtitle only"), None);
        // A parent identical to the key itself is not a merge candidate.
        assert_eq!(franchise_parent_key("Overlord: Temporada 2"), None);
    }

    #[test]
    fn arc_rows_only_merge_when_the_base_show_is_in_the_users_own_data() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("A", "a", "animeytx").unwrap();

        // Two colon-qualified titles. The user owns a plain "One Piece" row,
        // so its arc merges; nobody owns a series called "Re", so Re:Zero
        // stays its own franchise.
        for (slug, title) in [
            ("op", "One Piece"),
            ("op-elbaph", "One Piece: Arco de Elbaph"),
            ("rezero", "Re:Zero kara Hajimeru Isekai Seikatsu"),
        ] {
            let id = db.upsert_series(src, &mk_airing(slug, title, None)).unwrap();
            db.set_kind(id, "TV").unwrap();
            db.insert_episode(&crate::models::Episode {
                id: 0, series_id: id, number: "1".into(), title: None,
                url: format!("https://site/anime/{slug}-capitulo-1/"),
                released_at: None, seen: false,
            }).unwrap();
            db.set_seen_cascade(id, "1", true).unwrap();
        }

        let insights = db.get_watch_insights().unwrap();
        let titles: HashSet<&str> = insights.top_series.iter().map(|t| t.title.as_str()).collect();
        assert_eq!(insights.top_series.len(), 2, "One Piece merged, Re:Zero did not: {titles:?}");
        assert!(titles.contains("One Piece"));
        assert!(titles.contains("Re:Zero kara Hajimeru Isekai Seikatsu"));
        let one_piece = insights.top_series.iter().find(|t| t.title == "One Piece").unwrap();
        assert_eq!(one_piece.count, 2, "the arc's episode folded into the base show");
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

        let insights = db.get_watch_insights().unwrap();
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

        let summary = db.get_watch_summary().unwrap();
        assert_eq!(summary.episodes_watched, 30, "real marks stay exactly as marked");
        assert_eq!(
            summary.episodes_watched_external, 70,
            "only the 100-30 remainder is credited on top, so the two add up to 100 not 130"
        );

        let insights = db.get_watch_insights().unwrap();
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

        let summary = db.get_watch_summary().unwrap();
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

        let summary = db.get_watch_summary().unwrap();
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

        let insights = db.get_watch_insights().unwrap();

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
        // A full 30-day spine, zero-filled: only today has marks.
        assert_eq!(insights.marks_by_day.len(), 30, "always a contiguous 30-day window");
        assert_eq!(insights.marks_by_day.last().unwrap().count, 3, "today holds the cascade-marked episodes");
        assert_eq!(
            insights.marks_by_day.iter().map(|d| d.count).sum::<i64>(), 3,
            "every other day is an explicit zero, not a missing entry"
        );
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

        let insights = db.get_watch_insights().unwrap();
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

        let insights = db.get_watch_insights().unwrap();
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

        let insights = db.get_watch_insights().unwrap();
        assert_eq!(insights.estimated_minutes_external, 288, "falls back to 24 min/ep (TV) estimate, unchanged");
    }

    #[test]
    fn get_watch_insights_empty_db_has_no_divide_by_zero_and_empty_collections() {
        let db = Db::open(":memory:").unwrap();
        let insights = db.get_watch_insights().unwrap();
        assert_eq!(insights.estimated_minutes_tracked, 0);
        assert_eq!(insights.estimated_minutes_external, 0);
        assert_eq!(insights.external_titles_estimated, 0);
        assert_eq!(insights.external_titles_total, 0);
        assert_eq!(insights.avg_episodes_per_series, 0.0);
        assert!(insights.top_series.is_empty());
        // The day spine is always present and always 30 long, so a chart never
        // has to distinguish "no data yet" from "a gap in the middle".
        assert_eq!(insights.marks_by_day.len(), 30);
        assert!(insights.marks_by_day.iter().all(|d| d.count == 0));
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

        let insights = db.get_watch_insights().unwrap();
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

        let rows = db.get_stats_graph_data().unwrap();
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

    // ---- Cross-site aggregation (2026-08-13 stats-scoping fix) ----
    //
    // Every test above uses a single `upsert_source`, so it never exercised
    // the bug: every stats query used to filter by `source_id`, silently
    // hiding everything followed/watched/wanted on the app's other sites.
    // These reproduce the real shape — the same show followed on two sites,
    // plus a site-only show — and assert the canonical (deduped) totals.

    #[test]
    fn get_watch_summary_aggregates_and_dedups_a_show_followed_on_two_sites() {
        let db = Db::open(":memory:").unwrap();
        let a = db.upsert_source("AnimeYT", "a", "animeytx").unwrap();
        let b = db.upsert_source("TioAnime", "b", "tioanime").unwrap();

        // Same show, no anilist_id on either — same normalized title is what
        // must collapse them into one canonical entry.
        let shared_a = db.upsert_series(a, &mk_airing("shared", "Shared Show", None)).unwrap();
        db.set_followed(shared_a, true).unwrap();
        insert_eps_seen_up_to(&db, shared_a, 3, 3);
        let shared_b = db.upsert_series(b, &mk_airing("shared", "Shared Show", None)).unwrap();
        db.set_followed(shared_b, true).unwrap();
        insert_eps_seen_up_to(&db, shared_b, 2, 2);

        // Site-B-only show, want-listed — must still surface even though the
        // active site in real use is usually A.
        let want_only = db.upsert_series(b, &mk_airing("wantonly", "Only On B", None)).unwrap();
        db.set_backlog_status(want_only, Some("want")).unwrap();

        let summary = db.get_watch_summary().unwrap();
        assert_eq!(summary.followed_series, 1, "the shared show counts once, not twice");
        assert_eq!(summary.episodes_watched, 5, "real marks sum across both sites' rows");
        assert_eq!(summary.backlog_want, 1, "site-B-only want show is not hidden by site A's scope");
        assert_eq!(summary.distinct_anime, 1);
    }

    #[test]
    fn get_watch_insights_funnel_dedups_a_show_followed_and_airing_on_two_sites() {
        let db = Db::open(":memory:").unwrap();
        let a = db.upsert_source("AnimeYT", "a", "animeytx").unwrap();
        let b = db.upsert_source("TioAnime", "b", "tioanime").unwrap();

        let mut airing_a = mk_airing("dup", "Dup Show", None);
        airing_a.is_airing = true;
        let sid_a = db.upsert_series(a, &airing_a).unwrap();
        db.set_followed(sid_a, true).unwrap();
        let mut airing_b = mk_airing("dup", "Dup Show", None);
        airing_b.is_airing = true;
        let sid_b = db.upsert_series(b, &airing_b).unwrap();
        db.set_followed(sid_b, true).unwrap();

        let discarded_b = db.upsert_series(b, &mk_airing("disc", "Discarded On B", None)).unwrap();
        db.set_backlog_status(discarded_b, Some("discarded")).unwrap();

        let insights = db.get_watch_insights().unwrap();
        assert_eq!(insights.followed_airing, 1, "followed+airing on both sites is still one show");
        assert_eq!(insights.discarded, 1, "a site-B-only discard is not lost");
    }

    #[test]
    fn get_genre_and_type_stats_dedup_a_cross_site_show_and_union_its_genres() {
        let db = Db::open(":memory:").unwrap();
        let a = db.upsert_source("AnimeYT", "a", "animeytx").unwrap();
        let b = db.upsert_source("TioAnime", "b", "tioanime").unwrap();

        // Same show followed on both sites, each site's scrape tagged it with
        // a different genre subset and only site A recorded a `kind`.
        let sid_a = db.upsert_series(a, &mk_airing("dup", "Dup Show", None)).unwrap();
        db.set_followed(sid_a, true).unwrap();
        db.set_kind(sid_a, "TV").unwrap();
        db.insert_series_genres(sid_a, &["Seinen".to_string()]).unwrap();
        let sid_b = db.upsert_series(b, &mk_airing("dup", "Dup Show", None)).unwrap();
        db.set_followed(sid_b, true).unwrap();
        db.insert_series_genres(sid_b, &["Drama".to_string()]).unwrap();

        let genre_stats = db.get_genre_stats().unwrap();
        let genres: HashSet<&str> = genre_stats.iter().map(|g| g.genre.as_str()).collect();
        assert_eq!(genres, HashSet::from(["Seinen", "Drama"]), "genres from both sites' rows are unioned");
        assert!(
            genre_stats.iter().all(|g| g.count == 1),
            "one canonical show contributes 1 to each genre it carries, not 2: {genre_stats:?}"
        );

        let type_stats = db.get_type_stats().unwrap();
        assert_eq!(type_stats.len(), 1);
        assert_eq!(type_stats[0].kind, "TV");
        assert_eq!(type_stats[0].count, 1, "the one canonical show, not one per site row");
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

    // ---- Yearly activity heatmap (2026-08-24) ----

    #[test]
    fn get_yearly_activity_returns_full_year_zero_filled() {
        // Use 2025 (non-leap) to avoid the Feb 29 edge case.
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("A", "a", "animeytx").unwrap();
        let series = db.upsert_series(src, &mk_airing("x", "X", None)).unwrap();

        // Seed three marks on specific dates within 2025, and one in 2024 to
        // prove year-filtering works. Use mid-day times (12:00:00) so the
        // UTC→localtime conversion in the query never crosses a day boundary
        // regardless of the test machine's timezone.
        let eps = [
            ("1", "2025-01-15 12:00:00"),
            ("2", "2025-01-15 12:00:00"),
            ("3", "2025-06-20 12:00:00"),
            ("4", "2024-12-31 12:00:00"), // different year, must not appear
        ];
        for (num, seen_at) in eps {
            let ep = db.insert_episode(&crate::models::Episode {
                id: 0, series_id: series, number: num.into(), title: None,
                url: format!("https://site/x-{num}/"), released_at: None, seen: true,
            }).unwrap();
            // Overwrite seen_at to the exact timestamp we want (local time stored
            // as UTC in the DB; we rely on the query's 'localtime' conversion).
            db.conn
                .execute("UPDATE episodes SET seen_at = ?1 WHERE id = ?2", [seen_at, ep.to_string().as_str()])
                .unwrap();
        }

        let activity = db.get_yearly_activity(2025).unwrap();
        assert_eq!(activity.year, 2025);
        assert_eq!(activity.days.len(), 365, "non-leap year has 365 days");

        // Jan 15 should have 2 marks.
        let jan15 = activity.days.iter().find(|d| d.day == "2025-01-15").unwrap();
        assert_eq!(jan15.count, 2);

        // Jun 20 should have 1 mark.
        let jun20 = activity.days.iter().find(|d| d.day == "2025-06-20").unwrap();
        assert_eq!(jun20.count, 1);

        // Dec 31 2024 must not be counted (year filter).
        let dec31_2024 = activity.days.iter().find(|d| d.day == "2024-12-31");
        assert!(dec31_2024.is_none() || dec31_2024.unwrap().count == 0);

        // All other days must be zero.
        let nonzero = activity.days.iter().filter(|d| d.count > 0).count();
        assert_eq!(nonzero, 2, "only Jan 15 and Jun 20 have marks");
    }

    #[test]
    fn get_activity_years_includes_current_year_even_when_empty() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("A", "a", "animeytx").unwrap();

        // No episodes at all — should still get the current year.
        let years = db.get_activity_years().unwrap();
        let this_year = chrono::Local::now().date_naive().year();
        assert!(years.contains(&this_year), "current year present even with no data: {years:?}");
        assert!(years.iter().all(|&y| y <= this_year), "no future years: {years:?}");
        assert!(years.windows(2).all(|w| w[0] > w[1]), "descending order: {years:?}");

        // Add a mark in a past year — both should appear.
        let series = db.upsert_series(src, &mk_airing("x", "X", None)).unwrap();
        let ep = db.insert_episode(&crate::models::Episode {
            id: 0, series_id: series, number: "1".into(), title: None,
            url: "https://site/x-1/".into(), released_at: None, seen: true,
        }).unwrap();
        let past_year = this_year - 1;
        db.conn
            .execute(
                "UPDATE episodes SET seen_at = ?1 WHERE id = ?2",
                [format!("{past_year}-07-15 12:00:00"), ep.to_string()],
            )
            .unwrap();

        let years = db.get_activity_years().unwrap();
        assert!(years.contains(&this_year), "current year still present: {years:?}");
        assert!(years.contains(&past_year), "past year with data present: {years:?}");
        assert!(years[0] == this_year, "current year first (descending): {years:?}");
    }
}
