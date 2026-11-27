use super::*;
use std::collections::{HashMap, HashSet};
use chrono::Datelike;

/// The change-fingerprint `backup::signature_string` renders and
/// `backup::is_auto_backup_due` compares. Every field is one thing a user can
/// change that ought to make an auto-backup worth taking — see
/// `Db::signature_counts` for why this is far more than the episode counts it
/// started as. Adding a field here is the way to teach the auto-backup about a
/// new kind of mutation; `signature_string` renders whatever is here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignatureCounts {
    pub series: i64,
    pub episodes: i64,
    pub max_episode_id: i64,
    /// Newest `episodes.seen_at`, so re-watching (un-mark then re-mark, no
    /// count change) still moves the fingerprint.
    pub max_seen_at: Option<String>,
    pub seen_episodes: i64,
    pub followed: i64,
    pub watched_externally: i64,
    pub backlog_want: i64,
    pub backlog_discarded: i64,
    pub anilist_linked: i64,
    /// FNV-1a over the `settings` table minus backup bookkeeping — see
    /// `Db::settings_hash`.
    pub settings_hash: u64,
}

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
/// 2" together without a metadata source. Never returns "" — falls back to
/// the full normalized title if stripping would otherwise empty it, and (a
/// title that is itself all punctuation/noise, so even `normalize_title`
/// reduces it to "") to the raw trimmed+lowercased title, so two different
/// garbage titles don't silently collapse into one shared empty-keyed
/// franchise.
pub(crate) fn franchise_key(title: &str) -> String {
    let norm = crate::matching::normalize_title(title);
    let tokens: Vec<&str> =
        crate::matching::strip_season_markers(norm.split_whitespace().collect(), |t| t.to_string());
    let key = tokens.join(" ");
    if !key.is_empty() {
        key
    } else if !norm.is_empty() {
        norm
    } else {
        title.trim().to_lowercase()
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
    let tokens = crate::matching::strip_season_markers(
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

/// The type label of a series: the AniList catalog format when linked, else the
/// site's free-text kind. Both are put in the vocabulary the Library filter
/// already uses (`Library.tsx` `normalizeKind`: TV, MOVIE, OVA, ONA, SPECIAL
/// upper-case, "Pelicula" -> MOVIE) so the type breakdown does not split
/// "Movie"/"MOVIE"/"Pelicula" into separate buckets. TV_SHORT folds into TV
/// (the short-episode minutes estimate reads the raw format separately).
/// Site values outside that vocabulary are kept as they are.
pub(crate) fn effective_kind(catalog_format: Option<&str>, site_kind: Option<&str>) -> Option<String> {
    let normalize = |raw: &str| -> Option<String> {
        let upper = raw.trim().to_uppercase();
        match upper.as_str() {
            "TV" | "TV_SHORT" | "TV SHOW" | "ANIME" => Some("TV".into()),
            "MOVIE" | "PELICULA" | "PELÍCULA" => Some("MOVIE".into()),
            "OVA" | "ONA" | "SPECIAL" | "MUSIC" => Some(upper),
            _ => None,
        }
    };
    if let Some(label) = catalog_format.and_then(normalize) {
        return Some(label);
    }
    site_kind.map(|k| normalize(k).unwrap_or_else(|| k.to_string()))
}

/// AniList tags below this rank are noise ("mentioned once"); same floor as
/// the `series_effective_genres` view.
const STATS_TAG_MIN_RANK: i64 = 50;

/// Map one stats label onto AniList's genre vocabulary. AniList's own genres
/// are kept even if `canonical_genre` doesn't know them (its list is closed
/// but may grow); a scraped site label survives only when it maps onto a real
/// genre ("Acción" -> "Action"), so site-only labels ("Shounen", "Isekai",
/// "Escolar") never become stats "genres".
fn stats_genre_label(raw: String, from_anilist: bool) -> Option<String> {
    match crate::genres::canonical_genre(&raw) {
        Some(canon) => Some(canon.to_string()),
        None if from_anilist => Some(raw),
        None => None,
    }
}

/// How many cover thumbnails a ranked genre/tag card carries.
const CARD_COVERS: usize = 3;

/// One show's contribution to a ranked card: display title, cover, weight.
type CardShow = (String, Option<String>, i64);

/// Turn per-label show lists into ranked cards (by watched episodes, then
/// name), truncated to `limit`. Covers are picked so the biggest show does not
/// open every card: walking the cards in rank order, a show that already
/// *leads* (first cover of) a higher-ranked card is skipped in later cards, so
/// each card shows the shows most characteristic of it rather than the same
/// One Piece / Pokémon on every card. If every show of a card leads elsewhere
/// the card falls back to its top show, so a card is never cover-less.
fn rank_cards(
    by_label: HashMap<String, Vec<CardShow>>,
    corpus_episodes: i64,
    limit: Option<usize>,
) -> Vec<crate::models::GenreCard> {
    let mut ranked: Vec<(String, i64, Vec<CardShow>)> = by_label
        .into_iter()
        .map(|(label, mut shows)| {
            shows.sort_by(|a, b| b.2.cmp(&a.2).then_with(|| a.0.cmp(&b.0)));
            let count: i64 = shows.iter().map(|s| s.2).sum();
            (label, count, shows)
        })
        .collect();
    ranked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    if let Some(limit) = limit {
        ranked.truncate(limit);
    }
    let mut leads: HashSet<String> = HashSet::new();
    ranked
        .into_iter()
        .map(|(genre, count, shows)| {
            let show_count = shows.len() as i64;
            let mut picked: Vec<&CardShow> =
                shows.iter().filter(|s| !leads.contains(&s.0)).take(CARD_COVERS).collect();
            if picked.is_empty() {
                picked.extend(shows.first());
            }
            if let Some(lead) = picked.first() {
                leads.insert(lead.0.clone());
            }
            let top_series = picked
                .into_iter()
                .map(|(title, cover_url, _)| crate::models::GenreCardSeries {
                    title: title.clone(),
                    cover_url: cover_url.clone(),
                })
                .collect();
            let share = if corpus_episodes > 0 { count as f64 / corpus_episodes as f64 } else { 0.0 };
            crate::models::GenreCard { genre, count, show_count, share, top_series }
        })
        .collect()
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

/// Record `parent` as the candidate parent key of `key`, deterministically.
///
/// Two member rows of one `franchise_key` can propose *different* parents —
/// differing colon-heads that only collapse to the same key after season
/// stripping ("Fate: Zero" and "Fate/stay night: UBW" both living under one
/// key, say). This used to be `parent_of.entry(key).or_insert(parent)`, i.e.
/// whichever row SQLite happened to return first won, so the same database
/// could roll up differently between two runs. Every other tie-break in this
/// file is explicit (see `prefer_display_title`), and this one now reuses those
/// same rules — shortest key wins, ties broken alphabetically — so the winner
/// depends only on the data, never on row order.
fn record_parent_candidate(parent_of: &mut HashMap<String, String>, key: &str, parent: String) {
    match parent_of.get(key) {
        Some(current) if !prefer_display_title(&parent, current) => {}
        _ => {
            parent_of.insert(key.to_string(), parent);
        }
    }
}

/// Alias map folding the title-derived `franchise_key`s of rows AniList says
/// are the same show onto one shared key.
///
/// `franchise_key` is pure title text, so the same show scraped as "Shingeki no
/// Kyojin" on one site and "Attack on Titan" on another produced two franchises
/// — inflating `distinct_anime`/`top_series` and defeating the per-site
/// "best single site, never the sum" rule, which buckets by this key and so
/// never saw the two sites as one show. Whenever two keys carry the same
/// `anilist_id` they are folded together here, *before* any grouping runs.
///
/// Title matching stays the primary rule (a franchise's arcs are usually only
/// partly linked, and keying off `anilist_id` alone would split them again);
/// this only ever *merges* keys, never splits one. Rows with no `anilist_id`
/// are untouched and simply keep their own key.
///
/// The chosen representative is the lexicographically smallest key of each
/// connected component, so the result depends only on the set of keys, not on
/// query order. Parent pointers only ever move to a smaller key, so the
/// resolution walk cannot cycle.
fn franchise_key_aliases(rows: &[(Option<i64>, String)]) -> HashMap<String, String> {
    use std::collections::{BTreeMap, BTreeSet};

    let mut by_anilist_id: BTreeMap<i64, BTreeSet<String>> = BTreeMap::new();
    for (anilist_id, key) in rows {
        if let Some(id) = anilist_id {
            by_anilist_id.entry(*id).or_default().insert(key.clone());
        }
    }

    fn root(parent: &BTreeMap<String, String>, key: &str) -> String {
        // Bounded purely defensively — pointers strictly decrease, so a chain
        // is finite by construction.
        const MAX_DEPTH: usize = 64;
        let mut current = key.to_string();
        for _ in 0..MAX_DEPTH {
            match parent.get(&current) {
                Some(next) if next != &current => current = next.clone(),
                _ => break,
            }
        }
        current
    }

    let mut parent: BTreeMap<String, String> = BTreeMap::new();
    for keys in by_anilist_id.values() {
        if keys.len() < 2 {
            continue;
        }
        for key in keys {
            parent.entry(key.clone()).or_insert_with(|| key.clone());
        }
        let mut members = keys.iter();
        let anchor = members.next().expect("len >= 2");
        for key in members {
            let (a, b) = (root(&parent, anchor), root(&parent, key));
            if a != b {
                let (winner, loser) = if a < b { (a, b) } else { (b, a) };
                parent.insert(loser, winner);
            }
        }
    }

    parent
        .keys()
        .map(|key| (key.clone(), root(&parent, key)))
        .filter(|(key, target)| key != target)
        .collect()
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
///
/// Generic over the per-group payload so every franchise stat in this file can
/// share one parent-merge instead of each inventing its own grouping: the
/// caller supplies `fold_child(&mut parent, &child)` with its own merge
/// semantics. The parent's own payload is seeded *before* any child folds in,
/// so identity-ish fields (labels) are already decided and a child only ever
/// contributes counts.
fn merge_into_parent_groups<T: Clone>(
    groups: HashMap<String, T>,
    parent_of: &HashMap<String, String>,
    mut fold_child: impl FnMut(&mut T, &T),
) -> Vec<T> {
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
    let mut merged: HashMap<String, T> = groups
        .iter()
        .filter(|(key, _)| resolve(key) == **key)
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    for (key, value) in &groups {
        let target = resolve(key);
        if target == *key {
            continue;
        }
        let Some(existing) = merged.get_mut(&target) else { continue };
        fold_child(existing, value);
    }
    merged.into_values().collect()
}

/// `merge_into_parent_groups` with `FranchiseRollup`'s own merge rules: real
/// marks add up (they're different episodes of one show), the "Ya lo vi"
/// catalog estimate takes the largest single claim rather than a sum, and the
/// external *flag* ORs across members so the "X of Y estimated" ratio counts
/// both of its sides on this same post-merge grouping.
fn merge_into_parent_franchises(
    groups: HashMap<String, FranchiseRollup>,
    parent_of: &HashMap<String, String>,
) -> Vec<FranchiseRollup> {
    merge_into_parent_groups(groups, parent_of, |existing, rollup| {
        existing.real_seen += rollup.real_seen;
        existing.real_minutes += rollup.real_minutes;
        existing.real_minutes_estimated += rollup.real_minutes_estimated;
        existing.has_external |= rollup.has_external;
        if rollup.external_episodes > existing.external_episodes {
            existing.external_episodes = rollup.external_episodes;
            // The rate travels with the count it belongs to — see
            // `FranchiseRollup::external_per_episode`.
            existing.external_per_episode = rollup.external_per_episode;
        }
        // The parent's label already won by being seeded above; a child only
        // ever contributes counts.
    })
}

/// One `series` row's raw watch evidence, straight out of SQL — the input to
/// the franchise roll-up, never exposed outside this module.
struct SeriesWatchRow {
    /// `series.id`, so a caller grouping by something other than the
    /// franchise key (the taste metrics group by `canon_key`) can find a row.
    id: i64,
    title: String,
    /// The linked AniList id, when the series has been resolved to a catalog
    /// row — the identity signal `franchise_key_aliases` folds titles by.
    anilist_id: Option<i64>,
    kind: Option<String>,
    watched_externally: bool,
    source_id: i64,
    seen_count: i64,
    catalog_next_episode: Option<i64>,
    catalog_status: Option<String>,
    catalog_episodes: Option<i64>,
    catalog_format: Option<String>,
    catalog_duration: Option<i64>,
    /// The row's `canonical_group_keys` identity: twin rows of one show on
    /// several sites share it, different seasons (different AniList entries
    /// or titles) don't.
    canon_group: String,
    /// `canonical_episode_label` of every episode of this row marked seen,
    /// with or without a `seen_at` — an undated mark is still a real one.
    seen_episodes: Vec<String>,
}

/// AniList's per-episode `duration`, when it is usable: a non-positive value
/// is missing data (AniList reports 0 for some entries), not a zero-minute
/// episode, so callers fall back to their estimate instead of pricing every
/// episode at nothing.
fn known_duration(duration: Option<i64>) -> Option<i64> {
    duration.filter(|&minutes| minutes > 0)
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
    /// Real minutes that used fallback/default duration rather than known AniList duration.
    pub real_minutes_estimated: i64,
    /// Largest catalog episode count among members marked "Ya lo vi" that have
    /// no real seen episodes of their own. 0 when there is no such member.
    pub external_episodes: i64,
    /// Minutes per episode behind `external_episodes` — the per-episode rate of
    /// whichever member row supplied that count (AniList's `duration`, else the
    /// format/kind estimate). Deliberately stored as a *rate*, not as a
    /// precomputed total: `extra_external_minutes` must price exactly the
    /// episodes `extra_external_episodes` reports, and a total invited the two
    /// to be computed on different bases. It used to hold
    /// `external_episodes * rate` and the minute remainder was
    /// `external_minutes - real_minutes` — but `real_minutes` accumulates each
    /// member row's OWN rate (a "Pelicula" arc row prices its episodes at 100
    /// min each), so a franchise with a movie-format arc plus a normal-length
    /// "Ya lo vi" entry produced a negative difference, clamped to 0, and the
    /// same stats page showed positive extra episodes worth zero extra minutes.
    pub external_per_episode: i64,
    /// Whether any member row carries the "Ya lo vi" flag *at all*, whether or
    /// not a catalog episode count was available for it. `external_episodes`
    /// only answers "did we have data"; this answers "did the user claim to
    /// have watched it". The two are shown against each other as the "X of Y
    /// estimated" ratio, so they must be counted off the same (post-merge)
    /// franchise grouping — counting the denominator from raw `franchise_key`s
    /// before the parent merge reported "1 of 2" for two arcs of one franchise
    /// that both had data.
    pub has_external: bool,
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

    /// Minutes for `extra_external_episodes`, on the same additive basis — and
    /// on the same *basis* as the episode figure it prices: exactly those extra
    /// episodes at the estimate's own per-episode rate. Episodes and minutes
    /// therefore agree by construction (one is 0 iff the other is), instead of
    /// being two independently computed differences that could disagree — see
    /// `external_per_episode`.
    pub fn extra_external_minutes(&self) -> i64 {
        self.extra_external_episodes() * self.external_per_episode
    }
}

/// Folds `SeriesWatchRow`s into one `FranchiseRollup` with the roll-up's
/// rules, whatever the caller groups by, and the largest "Ya lo vi" catalog
/// claim kept apart from the real marks (see `FranchiseRollup`).
///
/// Real marks are credited with the larger of two readings, each of which
/// can only undercount:
/// - the best single site, that site's rows summed. Two sites scraping the
///   same show are mirrors of the same episodes — a user following One Piece
///   on two sites had ~1170 real marks on each, and summing them reported
///   ~2600 real episodes for a ~1170-episode show — while one site's own rows
///   (arcs, seasons) are genuinely different episodes, even when arc rows
///   linked to one AniList entry restart their numbering.
/// - the distinct canonical episodes per canonical show
///   (`SeriesWatchRow::canon_group`, the dedupe `deduplicated_seen_episodes`
///   applies), summed across shows, so S1 marked on one site and S2 on
///   another add up to 24 as in the activity charts. Each show is capped at
///   its AniList episode count when that is known: a site that numbers the
///   whole franchise 1-25 straight through on a row linked to the 12-episode
///   S1 entry otherwise credits S1 with 25 *and* S2 with its own 12 (High
///   Card read 37 for a 25-episode franchise). Unlinked shows, or shows with
///   no catalog total, are uncapped.
struct RollupBuilder {
    rollup: FranchiseRollup,
    /// source_id -> (seen episodes, their minutes), every member row summed
    per_site: HashMap<i64, (i64, i64, i64)>,
    /// canon_group -> canonical episode -> its minutes (the longest rate
    /// among the rows marking it, so row order never decides it)
    distinct: HashMap<String, HashMap<String, (i64, bool)>>,
    /// canon_group -> its AniList episode count, when known (> 0)
    catalog_total: HashMap<String, i64>,
}

impl RollupBuilder {
    fn new(display_title: String) -> Self {
        Self {
            rollup: FranchiseRollup {
                display_title,
                real_seen: 0,
                real_minutes: 0,
                real_minutes_estimated: 0,
                external_episodes: 0,
                external_per_episode: 0,
                has_external: false,
            },
            per_site: HashMap::new(),
            distinct: HashMap::new(),
            catalog_total: HashMap::new(),
        }
    }

    fn add(&mut self, row: &SeriesWatchRow) {
        // AniList's real per-episode duration wins whenever the series is
        // linked to a synced catalog row that has a usable one; otherwise
        // estimate from the format, AniList's first (raw, so TV_SHORT keeps
        // its short-episode estimate) and the site's own kind as the fallback.
        let duration_opt = known_duration(row.catalog_duration);
        let is_default = duration_opt.is_none();
        let per_episode = duration_opt.unwrap_or_else(|| {
            minutes_per_episode(row.catalog_format.as_deref().or(row.kind.as_deref()))
        });
        let bucket = self.per_site.entry(row.source_id).or_insert((0, 0, 0));
        bucket.0 += row.seen_count;
        bucket.1 += row.seen_count * per_episode;
        if is_default {
            bucket.2 += row.seen_count * per_episode;
        }
        if let Some(total) = row.catalog_episodes.filter(|&n| n > 0) {
            let cap = self.catalog_total.entry(row.canon_group.clone()).or_insert(total);
            *cap = (*cap).max(total);
        }
        let episodes = self.distinct.entry(row.canon_group.clone()).or_default();
        for label in &row.seen_episodes {
            let entry = episodes.entry(label.clone()).or_insert((per_episode, is_default));
            if per_episode > entry.0 || (per_episode == entry.0 && !is_default && entry.1) {
                *entry = (per_episode, is_default);
            }
        }

        // A "Ya lo vi" row contributes its catalog total regardless of
        // whether that same row also has real marks. Gating this on the
        // row's own seen-count threw the number away exactly when a user
        // both followed a long-runner and swiped "Ya lo vi" on it (49 rows
        // on the reference database, e.g. Boruto: 29 real marks hiding a
        // known 293-episode total). Nothing is double-counted by admitting
        // it — `episodes()` takes the larger of the two signals and
        // `extra_external_episodes()` reports only the remainder.
        if row.watched_externally {
            self.rollup.has_external = true;
            let episodes = row.catalog_episodes.or_else(|| {
                if row.catalog_status.as_deref() == Some("RELEASING") {
                    row.catalog_next_episode.filter(|&n| n > 1).map(|n| n - 1)
                } else {
                    None
                }
            });
            if let Some(episodes) = episodes {
                if episodes > self.rollup.external_episodes {
                    self.rollup.external_episodes = episodes;
                    self.rollup.external_per_episode = per_episode;
                }
            }
        }
    }

    /// Credit the group with the larger of its best single site and its
    /// capped distinct canonical episodes (see the struct's doc comment) —
    /// never the sum of what several sites separately claim about the same
    /// underlying episodes. Minutes follow whichever reading wins; a capped
    /// show's episodes are priced at its longest per-episode rate.
    fn finish(mut self) -> FranchiseRollup {
        let (site_seen, site_minutes, site_estimated) =
            self.per_site.values().copied().max_by_key(|&(seen, _, _)| seen).unwrap_or((0, 0, 0));
        let (mut distinct_seen, mut distinct_minutes, mut distinct_estimated) = (0, 0, 0);
        for (group, episodes) in &self.distinct {
            let count = episodes.len() as i64;
            match self.catalog_total.get(group) {
                Some(&cap) if count > cap => {
                    distinct_seen += cap;
                    let max_entry = episodes.values().copied().max_by_key(|&(m, is_def)| (m, !is_def));
                    if let Some((max_m, is_def)) = max_entry {
                        distinct_minutes += cap * max_m;
                        if is_def {
                            distinct_estimated += cap * max_m;
                        }
                    }
                }
                _ => {
                    distinct_seen += count;
                    for &(m, is_def) in episodes.values() {
                        distinct_minutes += m;
                        if is_def {
                            distinct_estimated += m;
                        }
                    }
                }
            }
        }
        if distinct_seen > site_seen {
            self.rollup.real_seen = distinct_seen;
            self.rollup.real_minutes = distinct_minutes;
            self.rollup.real_minutes_estimated = distinct_estimated;
        } else {
            self.rollup.real_seen = site_seen;
            self.rollup.real_minutes = site_minutes;
            self.rollup.real_minutes_estimated = site_estimated;
        }
        self.rollup
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

#[cfg(test)]
thread_local! {
    /// Test-only count of `real_genres_batch` statements, to pin the stats
    /// genre lookups to O(chunks) instead of O(series).
    pub(super) static REAL_GENRES_BATCH_CALLS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

impl Db {
    /// Genre counts across every show the user follows or marked "Ya lo vi",
    /// canonical across sites: a show followed on two sites (or followed on
    /// one and "Ya lo vi" on another, or both on one row) contributes its
    /// genre set once (the union of what each row was tagged with), not
    /// twice — see `group_taste_canonically`.
    pub fn get_genre_stats(&self) -> Result<Vec<crate::models::GenreStat>> {
        let groups = self.group_taste_canonically()?;
        let genres_by_id = self.stats_genres_for_groups(groups.iter().map(|g| &g.member_ids))?;
        let mut counts: HashMap<String, i64> = HashMap::new();
        for g in &groups {
            let mut genres: HashSet<String> = HashSet::new();
            for id in &g.member_ids {
                genres.extend(genres_by_id.get(id).into_iter().flatten().cloned());
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

    /// Genre cards: per-genre stats plus top 4 representative series with
    /// cover_url, for the ranked "genre cards" UI in Estadísticas.
    /// Same corpus and canonical grouping as `get_genre_stats` (followed or
    /// "Ya lo vi", one show once, genres unioned). Each show is weighted by
    /// `FranchiseRollup::episodes` computed by the shared `RollupBuilder`
    /// over its member rows: real marks (mirrors counted once), or the "Ya lo vi"
    /// catalog estimate when larger — never their sum.
    pub fn get_genre_cards(&self) -> Result<Vec<crate::models::GenreCard>> {
        self.taste_cards(
            |db, groups| db.stats_genres_for_groups(groups.iter().map(|g| &g.member_ids)),
            None,
        )
    }

    /// The tag counterpart of `get_genre_cards`: AniList tags with rank >= 50
    /// ("Male Protagonist", "Time Skip"...) over the same taste corpus,
    /// canonical grouping and episode weighting, top `limit` by episodes.
    /// Kept apart from genres so the graph and genre cards stay readable.
    /// Unlinked shows have no tags (the sites only publish genres).
    pub fn get_tag_cards(&self, limit: usize) -> Result<Vec<crate::models::GenreCard>> {
        self.taste_cards(
            |db, groups| {
                let mut map = HashMap::new();
                for id in groups.iter().flat_map(|g| g.member_ids.iter().copied()) {
                    map.insert(id, db.stats_tags(id)?);
                }
                Ok(map)
            },
            Some(limit),
        )
    }

    /// Shared body of the genre and tag cards. Each canonical show is weighted
    /// by `FranchiseRollup::episodes` from the shared `RollupBuilder` over its
    /// member rows (real marks, or the "Ya lo vi" estimate when larger — never
    /// their sum) and contributes once to each label any member carries.
    fn taste_cards(
        &self,
        labels_of: impl Fn(&Self, &[CanonicalFollowed]) -> Result<HashMap<i64, Vec<String>>>,
        limit: Option<usize>,
    ) -> Result<Vec<crate::models::GenreCard>> {
        let groups = self.group_taste_canonically()?;
        let labels_by_id = labels_of(self, &groups)?;
        let watch_rows: HashMap<i64, SeriesWatchRow> = self
            .series_watch_rows("s.followed=1 OR s.watched_externally=1")?
            .into_iter()
            .map(|row| (row.id, row))
            .collect();

        let mut by_label: HashMap<String, Vec<CardShow>> = HashMap::new();
        let mut corpus_episodes: i64 = 0;
        for g in &groups {
            let mut labels: HashSet<String> = HashSet::new();
            let mut rollup = RollupBuilder::new(g.display_title.clone());
            for &id in &g.member_ids {
                labels.extend(labels_by_id.get(&id).into_iter().flatten().cloned());
                if let Some(row) = watch_rows.get(&id) {
                    rollup.add(row);
                }
            }
            let watched_episodes = rollup.finish().episodes();
            corpus_episodes += watched_episodes;
            for label in labels {
                by_label.entry(label).or_default().push((
                    g.display_title.clone(),
                    g.cover_url.clone(),
                    watched_episodes,
                ));
            }
        }
        Ok(rank_cards(by_label, corpus_episodes, limit))
    }

    /// Real genres of one series row for the descriptive stats (graph, genre
    /// cards, genre bars): AniList's genres when the row is linked to a
    /// catalog entry that has any, otherwise its scraped site genres mapped
    /// through `stats_genre_label` (unmapped site labels dropped). Same
    /// precedence as the `series_effective_genres` view minus its AniList
    /// tags, which stay in that view for Descubrir's affinity but read as a
    /// hairball of 15+ "genres" per show here (see `stats_tags`).
    /// Genres of every member of every group, fetched in chunked batches
    /// (the statement repeats the placeholder list, so stay well under
    /// SQLite's variable limit) instead of one statement per series row.
    fn stats_genres_for_groups<'a>(
        &self,
        member_ids: impl Iterator<Item = &'a Vec<i64>>,
    ) -> Result<HashMap<i64, Vec<String>>> {
        let all_ids: Vec<i64> = member_ids.flatten().copied().collect();
        let mut map = HashMap::with_capacity(all_ids.len());
        for chunk in all_ids.chunks(450) {
            map.extend(self.real_genres_batch(chunk)?);
        }
        Ok(map)
    }

    /// `stats_genres` for many series rows in one statement: id -> sorted,
    /// de-duplicated real genres (every requested id present, possibly empty).
    /// Also backs the Biblioteca genre filter, so the library and the stats
    /// offer the same genre vocabulary.
    pub(crate) fn real_genres_batch(&self, series_ids: &[i64]) -> Result<HashMap<i64, Vec<String>>> {
        let mut map: HashMap<i64, Vec<String>> = series_ids.iter().map(|&id| (id, Vec::new())).collect();
        if series_ids.is_empty() {
            return Ok(map);
        }
        #[cfg(test)]
        REAL_GENRES_BATCH_CALLS.with(|c| c.set(c.get() + 1));
        let placeholders = (1..=series_ids.len()).map(|i| format!("?{i}")).collect::<Vec<_>>().join(",");
        let sql = format!(
            "SELECT s.id, cg.genre, 1
             FROM series s
             JOIN anilist_catalog_genres cg ON cg.anilist_id = s.anilist_id
             WHERE s.id IN ({placeholders})
             UNION
             SELECT sg.series_id, sg.genre, 0
             FROM series_genres sg
             JOIN series s ON s.id = sg.series_id
             WHERE sg.series_id IN ({placeholders})
               AND (s.anilist_id IS NULL
                    OR NOT EXISTS (SELECT 1 FROM anilist_catalog_genres WHERE anilist_id = s.anilist_id))"
        );
        let mut stmt = self.conn.prepare_cached(&sql)?;
        let rows = stmt
            .query_map(rusqlite::params_from_iter(series_ids.iter()), |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)? == 1))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for (id, raw, from_anilist) in rows {
            if let Some(label) = stats_genre_label(raw, from_anilist) {
                let out = map.entry(id).or_default();
                if !out.contains(&label) {
                    out.push(label);
                }
            }
        }
        for v in map.values_mut() {
            v.sort();
        }
        Ok(map)
    }

    /// AniList tags (rank >= `STATS_TAG_MIN_RANK`) of one linked series row.
    fn stats_tags(&self, series_id: i64) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT DISTINCT ct.tag
             FROM series s
             JOIN anilist_catalog_tags ct ON ct.anilist_id = s.anilist_id
             WHERE s.id = ?1 AND ct.rank >= ?2
             ORDER BY ct.tag",
        )?;
        let rows = stmt
            .query_map((series_id, STATS_TAG_MIN_RANK), |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Show count per `kind` ("TV"/"OVA"/...) over followed and "Ya lo vi"
    /// shows, descending, canonical across sites (one show counts once, under
    /// whichever member's `kind` is set first). Shows with no `kind` on any
    /// member are excluded. Also feeds Descubrir's format affinity.
    pub fn get_type_stats(&self) -> Result<Vec<crate::models::TypeStat>> {
        let groups = self.group_taste_canonically()?;
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
             FROM series_effective_genres sg
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

    /// The same taste weighting as `get_genre_affinity` (+2 followed, +1
    /// "want", -1.5 discarded, folded onto AniList's canonical genre names),
    /// but computed over the **whole library** instead of one `source_id`, and
    /// deduped across sites: a show followed on two sites is one show with the
    /// union of both sites' genre tags, contributing its +2 once. A show
    /// marked "Ya lo vi" (already watched elsewhere) is the same +2 tier as
    /// a followed one, and a show that is both still counts +2 once.
    ///
    /// `get_genre_affinity` is deliberately per-site — it feeds the swipe
    /// deck's genre pick, which browses one site's genre pages, so only that
    /// site's rows are relevant. "Tus géneros favoritos" is a summary of the
    /// user's taste, not of one site, and reusing the deck's query for it hid
    /// every genre whose evidence lived on a non-active site (and double-counted
    /// the ones present on two). Every other stats figure in this file is
    /// site-agnostic; this one now is too.
    pub fn get_genre_affinity_across_sites(&self) -> Result<HashMap<String, f64>> {
        let mut stmt = self.conn.prepare(
            "SELECT s.anilist_id, s.title, (s.followed=1 OR s.watched_externally=1),
                    s.backlog_status, sg.genre
             FROM series_effective_genres sg
             JOIN series s ON s.id = sg.series_id",
        )?;
        struct GenreRow {
            canon_key: String,
            followed: bool,
            backlog_status: Option<String>,
            genre: String,
        }
        let rows: Vec<GenreRow> = stmt
            .query_map([], |r| {
                let anilist_id: Option<i64> = r.get(0)?;
                let title: String = r.get(1)?;
                Ok(GenreRow {
                    canon_key: super::library::canon_key(anilist_id, &title),
                    followed: r.get::<_, i64>(2)? != 0,
                    backlog_status: r.get(3)?,
                    genre: r.get(4)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        // One verdict per canonical show, with its genres unioned across sites
        // — the same shape `group_followed_canonically` gives the descriptive
        // stats. Status precedence matches `library::merge_backlog`: an actual
        // follow outranks any backlog tag, and "want" outranks "discarded".
        #[derive(Default)]
        struct CanonicalShow {
            followed: bool,
            want: bool,
            discarded: bool,
            genres: HashSet<String>,
        }
        let mut shows: HashMap<String, CanonicalShow> = HashMap::new();
        for row in rows {
            let entry = shows.entry(row.canon_key).or_default();
            entry.followed |= row.followed;
            match row.backlog_status.as_deref() {
                Some("want") => entry.want = true,
                Some("discarded") => entry.discarded = true,
                _ => {}
            }
            entry.genres.insert(
                crate::genres::canonical_genre(&row.genre)
                    .map(|s| s.to_string())
                    .unwrap_or(row.genre),
            );
        }

        let mut scores: HashMap<String, f64> = HashMap::new();
        for show in shows.values() {
            let delta = if show.followed {
                2.0
            } else if show.want {
                1.0
            } else if show.discarded {
                -1.5
            } else {
                0.0
            };
            for genre in &show.genres {
                *scores.entry(genre.clone()).or_insert(0.0) += delta;
            }
        }
        Ok(scores)
    }

    /// Taste signals over the AniList catalog tags and studios across the **whole library**,
    /// deduped across sites per canonical show (`super::library::canon_key`).
    ///
    /// Follows the same per-canonical-show verdict (+2 followed, +1 "want", -1.5 discarded,
    /// with followed outranking want outranking discarded) as `get_genre_affinity_across_sites`.
    /// Tags are gathered from linked series (`series.anilist_id` joined to `anilist_catalog_tags`),
    /// ignoring tags with rank < 50. Studios are gathered from `anilist_catalog.studio`
    /// (non-NULL and non-empty).
    /// Unlinked rows (`anilist_id IS NULL`) contribute nothing.
    ///
    /// Both tag and studio affinity scores are normalized to `[0, 1]` by their maximum
    /// positive raw value; entries with raw score <= 0 are dropped. If no positive signals
    /// exist, returns empty maps (cold start).
    pub fn get_taste_signals_across_sites(&self) -> Result<crate::recommend::TasteSignals> {
        let mut tag_stmt = self.conn.prepare(
            // Deliberately ignores "Ya lo vi" for now (not yet aligned with the genre/type
            // affinity, which scores "Ya lo vi" like a follow).
            "SELECT s.anilist_id, s.title, s.followed, s.backlog_status, act.tag
             FROM series s
             JOIN anilist_catalog_tags act ON act.anilist_id = s.anilist_id
             WHERE s.anilist_id IS NOT NULL AND act.rank >= 50",
        )?;
        struct TagRow {
            canon_key: String,
            followed: bool,
            backlog_status: Option<String>,
            tag: String,
        }
        let tag_rows: Vec<TagRow> = tag_stmt
            .query_map([], |r| {
                let anilist_id: i64 = r.get(0)?;
                let title: String = r.get(1)?;
                Ok(TagRow {
                    canon_key: super::library::canon_key(Some(anilist_id), &title),
                    followed: r.get::<_, i64>(2)? != 0,
                    backlog_status: r.get(3)?,
                    tag: r.get(4)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        #[derive(Default)]
        struct CanonicalTagShow {
            followed: bool,
            want: bool,
            discarded: bool,
            tags: HashSet<String>,
        }
        let mut tag_shows: HashMap<String, CanonicalTagShow> = HashMap::new();
        for row in tag_rows {
            let entry = tag_shows.entry(row.canon_key).or_default();
            entry.followed |= row.followed;
            match row.backlog_status.as_deref() {
                Some("want") => entry.want = true,
                Some("discarded") => entry.discarded = true,
                _ => {}
            }
            entry.tags.insert(row.tag);
        }

        let mut raw_tags: HashMap<String, f64> = HashMap::new();
        for show in tag_shows.values() {
            let delta = if show.followed {
                2.0
            } else if show.want {
                1.0
            } else if show.discarded {
                -1.5
            } else {
                0.0
            };
            for tag in &show.tags {
                *raw_tags.entry(tag.clone()).or_insert(0.0) += delta;
            }
        }

        let mut studio_stmt = self.conn.prepare(
            // Deliberately ignores "Ya lo vi" for now, like the tag query above.
            "SELECT s.anilist_id, s.title, s.followed, s.backlog_status, c.studio
             FROM series s
             JOIN anilist_catalog c ON c.id = s.anilist_id
             WHERE s.anilist_id IS NOT NULL AND c.studio IS NOT NULL AND c.studio != ''",
        )?;
        struct StudioRow {
            canon_key: String,
            followed: bool,
            backlog_status: Option<String>,
            studio: String,
        }
        let studio_rows: Vec<StudioRow> = studio_stmt
            .query_map([], |r| {
                let anilist_id: i64 = r.get(0)?;
                let title: String = r.get(1)?;
                Ok(StudioRow {
                    canon_key: super::library::canon_key(Some(anilist_id), &title),
                    followed: r.get::<_, i64>(2)? != 0,
                    backlog_status: r.get(3)?,
                    studio: r.get(4)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        #[derive(Default)]
        struct CanonicalStudioShow {
            followed: bool,
            want: bool,
            discarded: bool,
            studios: HashSet<String>,
        }
        let mut studio_shows: HashMap<String, CanonicalStudioShow> = HashMap::new();
        for row in studio_rows {
            let entry = studio_shows.entry(row.canon_key).or_default();
            entry.followed |= row.followed;
            match row.backlog_status.as_deref() {
                Some("want") => entry.want = true,
                Some("discarded") => entry.discarded = true,
                _ => {}
            }
            let trimmed = row.studio.trim();
            if !trimmed.is_empty() {
                entry.studios.insert(trimmed.to_string());
            }
        }

        let mut raw_studios: HashMap<String, f64> = HashMap::new();
        for show in studio_shows.values() {
            let delta = if show.followed {
                2.0
            } else if show.want {
                1.0
            } else if show.discarded {
                -1.5
            } else {
                0.0
            };
            for studio in &show.studios {
                *raw_studios.entry(studio.clone()).or_insert(0.0) += delta;
            }
        }

        Ok(crate::recommend::TasteSignals::from_raw(raw_tags, raw_studios))
    }

    /// This user's top `limit` genres by cross-site affinity, positive scores
    /// only, strongest first — the "Tus géneros favoritos" chip row. Ties break
    /// alphabetically: the scores come out of a `HashMap`, so without a
    /// secondary key equal-scored genres would reorder between calls.
    pub fn get_favorite_genres(&self, limit: usize) -> Result<Vec<crate::models::GenreAffinity>> {
        let mut scored: Vec<(String, f64)> = self
            .get_genre_affinity_across_sites()?
            .into_iter()
            .filter(|(_, score)| *score > 0.0)
            .collect();
        scored.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.0.cmp(&b.0))
        });
        scored.truncate(limit);
        Ok(scored
            .into_iter()
            .map(|(genre, score)| crate::models::GenreAffinity { genre, score })
            .collect())
    }

    /// Followed shows with their genres (unioned across sites) and kind, for
    /// the 3D relationship graph, canonical across sites — one node per show,
    /// not per site's `series` row (see `group_followed_canonically`). The
    /// node's `id` is one representative member's `series.id`; it only needs
    /// to be a stable per-node key for the graph, not a specific site's row.
    pub fn get_stats_graph_data(&self) -> Result<Vec<crate::models::SeriesGraphNode>> {
        let groups = self.group_followed_canonically()?;
        let genres_by_id = self.stats_genres_for_groups(groups.iter().map(|g| &g.member_ids))?;
        let mut nodes: Vec<crate::models::SeriesGraphNode> = groups
            .into_iter()
            .map(|g| {
                let mut genres: HashSet<String> = HashSet::new();
                for id in &g.member_ids {
                    genres.extend(genres_by_id.get(id).into_iter().flatten().cloned());
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
        self.group_canonically("s.followed=1")
    }

    /// `group_followed_canonically` over the taste corpus: followed OR "Ya lo
    /// vi" rows. One row carrying both flags, or one show followed on one site
    /// and "Ya lo vi" on another, is still one group (`canon_key`).
    fn group_taste_canonically(&self) -> Result<Vec<CanonicalFollowed>> {
        self.group_canonically("s.followed=1 OR s.watched_externally=1")
    }

    fn group_canonically(&self, scope: &str) -> Result<Vec<CanonicalFollowed>> {
        struct Row {
            id: i64,
            anilist_id: Option<i64>,
            title: String,
            kind: Option<String>,
            cover_url: Option<String>,
            catalog_format: Option<String>,
        }
        let mut stmt = self.conn.prepare(&format!(
            "SELECT s.id, s.anilist_id, s.title, s.kind,
                    COALESCE(c.cover_url, s.cover_url), c.format
             FROM series s
             LEFT JOIN anilist_catalog c ON c.id = s.anilist_id
             WHERE {scope}"
        ))?;
        let rows: Vec<Row> = stmt
            .query_map([], |r| {
                Ok(Row {
                    id: r.get(0)?,
                    anilist_id: r.get(1)?,
                    title: r.get(2)?,
                    kind: r.get(3)?,
                    cover_url: r.get(4)?,
                    catalog_format: r.get(5)?,
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
                entry.kind = effective_kind(row.catalog_format.as_deref(), row.kind.as_deref());
            }
            // A scraped site thumbnail is blocked by the CSP and renders as
            // initials, so any member with a displayable cover (AniList's, a
            // cached file, a data URI) replaces one that is not.
            let displayable = |c: &Option<String>| crate::db::is_csp_displayable_cover(c.as_deref());
            if row.cover_url.is_some()
                && (entry.cover_url.is_none() || (!displayable(&entry.cover_url) && displayable(&row.cover_url)))
            {
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

    /// The canonical followed shows split into `(still airing, finished)` — an
    /// exact partition of `get_watch_summary`'s `followed_series`, so the
    /// funnel's two bars always add up to the follow count printed beside them.
    ///
    /// `series.is_airing` is per-site and only refreshed by that site's own
    /// scan, so the two rows of one cross-site show routinely disagree: the
    /// site rescanned last night says airing, the one last scanned in March
    /// still says finished. Counting each bucket with its own independent
    /// `distinct_canon_count` over `is_airing=1` / `is_airing=0` therefore put
    /// such a show in **both**, and `followed_airing + followed_finished` could
    /// exceed the canonical follow count on the same page. One pass, one
    /// verdict per canonical show: airing when ANY member row still says so —
    /// the same "any site says it's airing" convention `airing_followed` has
    /// always had, now applied to the complement too.
    fn followed_airing_split(&self) -> Result<(i64, i64)> {
        let mut stmt = self
            .conn
            .prepare("SELECT anilist_id, title, is_airing FROM series WHERE followed=1")?;
        let rows: Vec<(String, bool)> = stmt
            .query_map([], |r| {
                let anilist_id: Option<i64> = r.get(0)?;
                let title: String = r.get(1)?;
                Ok((
                    super::library::canon_key(anilist_id, &title),
                    r.get::<_, i64>(2)? != 0,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut any_airing: HashMap<String, bool> = HashMap::new();
        for (key, is_airing) in rows {
            *any_airing.entry(key).or_insert(false) |= is_airing;
        }
        let airing = any_airing.values().filter(|&&a| a).count() as i64;
        Ok((airing, any_airing.len() as i64 - airing))
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
    /// `anilist_id` is applied on top of it as a *merge* signal only, via
    /// `franchise_key_aliases`: titles that share a link are one franchise even
    /// when the text doesn't match at all ("Shingeki no Kyojin" / "Attack on
    /// Titan"), while unlinked rows keep the title-derived key they always had.
    pub(crate) fn franchise_rollups(&self) -> Result<Vec<FranchiseRollup>> {
        let rows = self.series_watch_rows(
            "s.watched_externally=1
             OR EXISTS (SELECT 1 FROM episodes e WHERE e.series_id=s.id AND e.seen=1)",
        )?;

        // Two rows the user's own AniList links say are the same show share one
        // franchise even when their titles don't look alike at all ("Shingeki
        // no Kyojin" / "Attack on Titan") — see `franchise_key_aliases`.
        let aliases = franchise_key_aliases(
            &rows
                .iter()
                .map(|row| (row.anilist_id, franchise_key(&row.title)))
                .collect::<Vec<_>>(),
        );
        let canonical = |key: String| aliases.get(&key).cloned().unwrap_or(key);

        let mut grouped: HashMap<String, RollupBuilder> = HashMap::new();
        // Candidate parent per group, recorded while grouping and applied only
        // afterwards, once every key present in this user's data is known.
        let mut parent_of: HashMap<String, String> = HashMap::new();
        for row in &rows {
            let key = canonical(franchise_key(&row.title));
            let display = franchise_display_title(&row.title);
            if let Some(parent) = franchise_parent_key(&row.title).map(canonical) {
                if parent != key {
                    record_parent_candidate(&mut parent_of, &key, parent);
                }
            }
            let entry = grouped
                .entry(key)
                .or_insert_with(|| RollupBuilder::new(display.clone()));
            if prefer_display_title(&display, &entry.rollup.display_title) {
                entry.rollup.display_title = display;
            }
            entry.add(row);
        }
        let grouped: HashMap<String, FranchiseRollup> =
            grouped.into_iter().map(|(key, b)| (key, b.finish())).collect();
        Ok(merge_into_parent_franchises(grouped, &parent_of))
    }

    /// Every `series` row matching `scope` (a SQL predicate over alias `s`)
    /// with its raw watch evidence — the one query behind every roll-up, so
    /// the franchise totals and the per-show taste weights read the same
    /// columns the same way.
    fn series_watch_rows(&self, scope: &str) -> Result<Vec<SeriesWatchRow>> {
        let sql = format!(
            "SELECT s.id, s.title, s.anilist_id, s.kind, s.watched_externally, s.source_id,
                    (SELECT COUNT(*) FROM episodes e WHERE e.series_id=s.id AND e.seen=1) AS seen_cnt,
                    c.episodes, c.format, c.duration, c.next_episode, c.status
             FROM series s
             LEFT JOIN anilist_catalog c ON c.id = s.anilist_id
             WHERE {scope}"
        );
        let mut group_keys = self.canonical_group_keys()?;
        let mut seen_by_series = self.seen_canonical_episodes()?;
        let mut stmt = self.conn.prepare(&sql)?;
        let mut rows = stmt
            .query_map([], |r| {
                Ok(SeriesWatchRow {
                    id: r.get(0)?,
                    title: r.get(1)?,
                    anilist_id: r.get(2)?,
                    kind: r.get(3)?,
                    watched_externally: r.get::<_, i64>(4)? != 0,
                    source_id: r.get(5)?,
                    seen_count: r.get(6)?,
                    catalog_next_episode: r.get(10)?,
                    catalog_status: r.get(11)?,
                    catalog_episodes: r.get(7)?,
                    catalog_format: r.get(8)?,
                    catalog_duration: r.get(9)?,
                    canon_group: String::new(),
                    seen_episodes: Vec::new(),
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for row in &mut rows {
            row.canon_group =
                group_keys.remove(&row.id).unwrap_or_else(|| format!("s:{}", row.id));
            row.seen_episodes = seen_by_series.remove(&row.id).unwrap_or_default();
        }
        Ok(rows)
    }

    /// series_id -> `canonical_episode_label` of each of its seen episodes,
    /// dated or not (unlike `deduplicated_seen_episodes`, which needs a day to
    /// bucket into). Only series with at least one seen episode are present.
    fn seen_canonical_episodes(&self) -> Result<HashMap<i64, Vec<String>>> {
        let mut stmt = self
            .conn
            .prepare("SELECT series_id, number FROM episodes WHERE seen = 1")?;
        let raw = stmt
            .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        let now = chrono::Utc::now().timestamp();
        let mut aligns: HashMap<i64, super::episode_align::RowAlign> = HashMap::new();
        let mut out: HashMap<i64, Vec<String>> = HashMap::new();
        for (series_id, number) in raw {
            let align = match aligns.entry(series_id) {
                std::collections::hash_map::Entry::Occupied(slot) => *slot.get(),
                std::collections::hash_map::Entry::Vacant(slot) => {
                    *slot.insert(self.row_align(series_id, now)?)
                }
            };
            out.entry(series_id)
                .or_default()
                .push(canonical_episode_label(align, &number));
        }
        Ok(out)
    }

    /// How many episodes each franchise has, one entry per franchise, with the
    /// show followed on several sites counted once (what the "Completado %"
    /// denominator and the average episodes per series need).
    ///
    /// A franchise's total is the larger of (a) the best single site's episode
    /// rows (arcs of one site are summed, sites are not) and (b) the sum of
    /// AniList's `episodes` over the distinct linked entries: AniList is the
    /// authority for a finished show, the site count covers a long-runner
    /// AniList has no total for. Consequences worth knowing: an airing show
    /// counts its full announced season length, and a followed show with no
    /// marks counts all of its episodes.
    ///
    /// Groups with the same key/alias/parent rules as `franchise_rollups`, but
    /// over its own corpus (`followed_only`: followed series; otherwise followed
    /// OR at least one seen episode OR "Ya lo vi"), so in odd data (an unseen
    /// followed parent row) the grouping can differ from the watched numerator:
    /// callers clamp.
    ///
    /// A "Ya lo vi" row that is neither followed nor really seen contributes
    /// only its catalog count, never its scraped episode rows: the numerator
    /// (`FranchiseRollup::episodes`) credits such a row with its catalog count
    /// alone, so counting its site rows here would put episodes in the
    /// denominator that can never reach the numerator.
    ///
    /// For the same reason, in the completion corpus (`!followed_only`) the
    /// AniList entries reached *only* through "Ya lo vi" rows count by the
    /// largest one, not their sum: the numerator credits a franchise's "Ya lo
    /// vi" claims with the largest single catalog count (`RollupBuilder`), so
    /// three "Ya lo vi" seasons of 25 + 12 + 22 episodes are 25 on both sides
    /// (100 %), not 25 of 59. Entries also reached through a followed or seen
    /// row still add up as before.
    pub(crate) fn canonical_franchise_episode_totals(&self, followed_only: bool) -> Result<Vec<i64>> {
        let scope = if followed_only {
            "s.followed=1"
        } else {
            "(s.followed=1 OR s.watched_externally=1
              OR EXISTS (SELECT 1 FROM episodes x WHERE x.series_id=s.id AND x.seen=1))"
        };
        let sql = format!(
            "SELECT s.title, s.anilist_id, s.source_id,
                    CASE WHEN s.followed=1 OR SUM(e.seen)>0 THEN COUNT(e.id) ELSE 0 END,
                    c.episodes, s.watched_externally, c.next_episode, c.status
             FROM series s
             LEFT JOIN episodes e ON e.series_id = s.id
             LEFT JOIN anilist_catalog c ON c.id = s.anilist_id
             WHERE {scope}
             GROUP BY s.id"
        );
        let mut stmt = self.conn.prepare(&sql)?;

        struct Row {
            title: String,
            anilist_id: Option<i64>,
            source_id: i64,
            episode_rows: i64,
            catalog_next_episode: Option<i64>,
            catalog_status: Option<String>,
            catalog_episodes: Option<i64>,
            watched_externally: bool,


        }
        let rows: Vec<Row> = stmt.query_map([], |r| {
            Ok(Row {
                title: r.get(0)?,
                anilist_id: r.get(1)?,
                source_id: r.get(2)?,
                episode_rows: r.get(3)?,
                catalog_next_episode: r.get(6)?,
                catalog_status: r.get(7)?,
                catalog_episodes: r.get(4)?,
                watched_externally: r.get::<_, i64>(5)? != 0,
            })
        })?.collect::<rusqlite::Result<Vec<_>>>()?;

        let aliases = franchise_key_aliases(
            &rows
                .iter()
                .map(|row| (row.anilist_id, franchise_key(&row.title)))
                .collect::<Vec<_>>(),
        );
        let canonical = |key: String| aliases.get(&key).cloned().unwrap_or(key);

        #[derive(Clone)]
        struct FranchiseEps {
            per_site: HashMap<i64, i64>,
            /// anilist_id -> (catalog episodes, reached only through "Ya lo vi" rows)
            catalog_by_anilist: HashMap<i64, (i64, bool)>,
        }

        let mut grouped: HashMap<String, FranchiseEps> = HashMap::new();
        let mut parent_of: HashMap<String, String> = HashMap::new();

        for row in rows {
            let key = canonical(franchise_key(&row.title));
            if let Some(parent) = franchise_parent_key(&row.title).map(canonical) {
                if parent != key {
                    record_parent_candidate(&mut parent_of, &key, parent);
                }
            }
            let entry = grouped.entry(key).or_insert_with(|| FranchiseEps {
                per_site: HashMap::new(),
                catalog_by_anilist: HashMap::new(),
            });

            *entry.per_site.entry(row.source_id).or_insert(0) += row.episode_rows;

            if let Some(aid) = row.anilist_id {
                let catalog_count = row.catalog_episodes.or_else(|| {
                    if row.catalog_status.as_deref() == Some("RELEASING") {
                        row.catalog_next_episode.filter(|&n| n > 1).map(|n| n - 1)
                    } else {
                        None
                    }
                });
                if let Some(cep) = catalog_count {
                    let only_external = !followed_only && row.watched_externally;
                    entry
                        .catalog_by_anilist
                        .entry(aid)
                        .and_modify(|(eps, ext)| {
                            *eps = cep;
                            *ext &= only_external;
                        })
                        .or_insert((cep, only_external));
                }
            }
        }

        let franchises = merge_into_parent_groups(grouped, &parent_of, |existing, child| {
            for (source_id, rows) in &child.per_site {
                *existing.per_site.entry(*source_id).or_insert(0) += rows;
            }
            for (aid, &(cep, only_external)) in &child.catalog_by_anilist {
                existing
                    .catalog_by_anilist
                    .entry(*aid)
                    .and_modify(|(eps, ext)| {
                        *eps = cep;
                        *ext &= only_external;
                    })
                    .or_insert((cep, only_external));
            }
        });

        Ok(franchises.into_iter().map(|f| {
            let site_max = f.per_site.values().copied().max().unwrap_or(0);
            let shared: i64 = f
                .catalog_by_anilist
                .values()
                .filter(|(_, only_external)| !only_external)
                .map(|(eps, _)| eps)
                .sum();
            let external_claim: i64 = f
                .catalog_by_anilist
                .values()
                .filter(|(_, only_external)| *only_external)
                .map(|(eps, _)| *eps)
                .max()
                .unwrap_or(0);
            site_max.max(shared + external_claim)
        }).collect())
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
        let rollups = self.franchise_rollups()?;
        // Real seen-episode count across ALL series, followed or not — a
        // "Ya lo vi" swipe whose site-link scraped real episodes must count
        // too (see the episode/anime-counts-fix design doc).
        //
        // Taken off the same franchise roll-up `get_watch_insights`'
        // `estimated_minutes_tracked` is built from, never a raw
        // `COUNT(*) FROM episodes WHERE seen=1`: that count summed every
        // site's rows, so a show followed on two sites with the same progress
        // marked on both was counted twice (One Piece marked to 600 on two
        // sites read as 1200 real episodes for an 1140-episode show) — and it
        // then disagreed with every figure that *was* collapsed, breaking the
        // invariant this file's doc comments claim. `real_seen` already
        // counts mirrored episodes once (see `RollupBuilder`).
        let episodes_watched: i64 = rollups.iter().map(|r| r.real_seen).sum();
        // `episodes_total` stays scoped to series that mean something for a
        // "how far along am I" denominator: followed, or with at least one
        // real seen episode — so discarded/never-touched scraped rows don't
        // inflate it.
        //
        // Collapsed per franchise (same parent merge as the numerator) so a show
        // followed on two sites doesn't double-count its denominator.
        // Never below what was watched: the two come from separately built
        // groupings (see `canonical_franchise_episode_totals`), so in odd data
        // the ratio must still stay a sane 0-100 %.
        //
        // "Ya lo vi" franchises are in the denominator too, on the same base
        // the numerator credits them with (`episodes_watched +
        // episodes_watched_external`, i.e. `FranchiseRollup::episodes`).
        // Leaving them out of one side made the completion ratio meaningless:
        // a followed show also swiped "Ya lo vi" put its catalog total in the
        // denominator while only its real marks reached the numerator.
        //
        // Episodes credited on top of `episodes_watched` from "Ya lo vi"
        // catalog estimates. Deliberately the per-franchise *remainder* (see
        // `FranchiseRollup`), not a raw sum: a franchise whose arc rows
        // already account for 151 real seen episodes and whose AniList entry
        // says 1140 contributes 989 here, so the two figures add up to 1140
        // rather than to 1291.
        let episodes_watched_external: i64 =
            rollups.iter().map(|r| r.extra_external_episodes()).sum();
        let episodes_total: i64 = self
            .canonical_franchise_episode_totals(false)?
            .iter()
            .sum::<i64>()
            .max(episodes_watched + episodes_watched_external);
        // "Ya lo vi" franchises the roll-up can credit with nothing: no
        // catalog episode count and no real marks. They add 0 episodes and 0
        // minutes, which the UI states instead of hiding.
        let titles_without_episode_data = rollups
            .iter()
            .filter(|r| r.has_external && r.episodes() == 0)
            .count() as i64;
        let backlog_want = self.distinct_canon_count(
            "SELECT anilist_id, title FROM series WHERE backlog_status='want'",
        )?;
        // One verdict per canonical show, shared with `get_watch_insights`'
        // airing/finished funnel so the two screens can't disagree — see
        // `followed_airing_split`.
        let (airing_followed, _) = self.followed_airing_split()?;
        // A "Ya lo vi" row is claimed as watched, so its unseen scraped
        // episodes are not pending — the same exclusion `list_pending`
        // applies, or this counter reports shows the Pendientes screen hides.
        let pending_to_watch = self.distinct_canon_count(
            "SELECT DISTINCT s.anilist_id, s.title FROM series s
             JOIN episodes e ON e.series_id = s.id
             WHERE s.followed=1 AND s.watched_externally=0 AND e.seen=0",
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
            titles_without_episode_data,
            airing_followed,
            pending_to_watch,
            backlog_want,
        })
    }

}

/// One episode's number in its canonical (cross-site) space, as the dedupe
/// key both `deduplicated_seen_episodes` and the roll-up's distinct-episode
/// count use: mapped through the row's `RowAlign`, the raw number when the
/// mapping has no answer, the raw text when it isn't numeric.
fn canonical_episode_label(align: super::episode_align::RowAlign, number: &str) -> String {
    match crate::db::episodes::parse_ep_number(number) {
        Some(n) => match align.to_canonical(n) {
            Some(c) => crate::db::episode_align::format_ep_number(c),
            None => crate::db::episode_align::format_ep_number(n),
        },
        None => number.to_string(),
    }
}

#[derive(Debug, Clone)]
pub(crate) struct DeduplicatedSeenEpisode {
    pub title: String,
    pub seen_at: String,
    pub local_day: String,
    pub local_hour: i64,
    pub local_year: i32,
}

impl Db {
    /// Returns seen episodes deduplicated across twin site rows of the same show.
    ///
    /// For each canonical episode of a canonical show group (bridged by `canonical_group_keys`
    /// and canonicalized by `RowAlign::to_canonical`), keeps only the earliest mark
    /// (minimum `seen_at`). Each entry carries the timestamp converted to local calendar
    /// day, hour (0-23), and year.
    pub(crate) fn deduplicated_seen_episodes(&self) -> Result<Vec<DeduplicatedSeenEpisode>> {
        let group_keys = self.canonical_group_keys()?;
        let now = chrono::Utc::now().timestamp();

        let mut stmt = self.conn.prepare(
            "SELECT e.series_id, s.title, e.number, e.seen_at,
                    DATE(e.seen_at, 'localtime') AS local_day,
                    CAST(strftime('%H', e.seen_at, 'localtime') AS INTEGER) AS local_hour,
                    CAST(strftime('%Y', DATE(e.seen_at, 'localtime')) AS INTEGER) AS local_year
             FROM episodes e
             JOIN series s ON s.id = e.series_id
             WHERE e.seen = 1 AND e.seen_at IS NOT NULL
             ORDER BY e.seen_at ASC, e.id ASC",
        )?;

        struct RawSeen {
            series_id: i64,
            title: String,
            number: String,
            seen_at: String,
            local_day: String,
            local_hour: i64,
            local_year: i32,
        }

        let raw_rows = stmt
            .query_map([], |r| {
                Ok(RawSeen {
                    series_id: r.get(0)?,
                    title: r.get(1)?,
                    number: r.get(2)?,
                    seen_at: r.get(3)?,
                    local_day: r.get(4)?,
                    local_hour: r.get(5)?,
                    local_year: r.get(6)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        let mut aligns: HashMap<i64, super::episode_align::RowAlign> = HashMap::new();
        for r in &raw_rows {
            if let std::collections::hash_map::Entry::Vacant(slot) = aligns.entry(r.series_id) {
                slot.insert(self.row_align(r.series_id, now)?);
            }
        }

        let mut deduped: HashMap<(String, String), DeduplicatedSeenEpisode> = HashMap::new();
        for r in raw_rows {
            let group = group_keys
                .get(&r.series_id)
                .cloned()
                .unwrap_or_else(|| format!("s:{}", r.series_id));
            let align = aligns.get(&r.series_id).copied().unwrap_or(super::episode_align::RowAlign {
                offset: 0,
                cumulative: false,
            });

            let canon_ep = canonical_episode_label(align, &r.number);

            let item = DeduplicatedSeenEpisode {
                title: r.title,
                seen_at: r.seen_at,
                local_day: r.local_day,
                local_hour: r.local_hour,
                local_year: r.local_year,
            };

            deduped
                .entry((group, canon_ep))
                .and_modify(|existing| {
                    if item.seen_at < existing.seen_at {
                        *existing = item.clone();
                    }
                })
                .or_insert(item);
        }

        Ok(deduped.into_values().collect())
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
        let estimated_tracked_default: i64 = rollups.iter().map(|r| r.real_minutes_estimated).sum();
        let estimated_minutes_share = if estimated_minutes_tracked > 0 {
            estimated_tracked_default as f64 / estimated_minutes_tracked as f64
        } else {
            0.0
        };
        // "Estimated" means a catalog episode count was available for the
        // franchise, not that it happened to exceed the real marks. Counting
        // only the ones with a nonzero remainder made the "X of Y" ratio
        // understate itself: a franchise whose real marks already cover its
        // catalog total is fully accounted for, not missing data.
        let external_titles_estimated =
            rollups.iter().filter(|r| r.external_episodes > 0).count() as i64;

        // Counted off the *same* roll-up as the numerator, post parent-merge.
        // It used to be a separate pass that grouped the raw
        // `watched_externally` rows by bare `franchise_key`, i.e. before
        // `merge_into_parent_franchises` ran — so two colon-qualified arcs of
        // one franchise ("One Piece: Wano", "One Piece: Arco de Elbaph"), both
        // swiped "Ya lo vi" and both carrying catalog data, merged into one
        // estimated franchise while still counting as two totals: "1 de 2 con
        // datos" when the real answer is 1 of 1.
        let external_titles_total = rollups.iter().filter(|r| r.has_external).count() as i64;

        // Mean episode count (all episodes, not just seen) across followed
        // franchises (canonical across sites). NULL/empty reads as 0.0.
        let followed_eps = self.canonical_franchise_episode_totals(true)?;
        let avg_episodes_per_series = if followed_eps.is_empty() {
            0.0
        } else {
            followed_eps.iter().sum::<i64>() as f64 / followed_eps.len() as f64
        };

        // Canonical (cross-site-deduped) funnel counts — see
        // `get_watch_summary`'s doc comment for why this never reads the
        // `library` table. Kept consistent with `get_watch_summary`'s own
        // `airing_followed`/`backlog_want`/`watched_externally`-shaped counts
        // so the two screens can't disagree with each other either.
        // One canonical pass, not two independent counts: `series.is_airing` is
        // per-site and the rows of one cross-site show disagree whenever only
        // one of the sites has been rescanned, which put that show in both
        // buckets at once. See `followed_airing_split`.
        let (followed_airing, followed_finished) = self.followed_airing_split()?;
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
        //
        // `seen=1 AND seen_at IS NOT NULL` is the file-wide predicate for
        // "really watched, on a known day". `set_seen`/`set_seen_cascade`
        // write the two together today, so the flag alone and the timestamp
        // alone happen to agree — but they are two columns, and the day
        // buckets used to trust only the timestamp while the hourly and dusty
        // queries required both. One write that clears `seen` without clearing
        // `seen_at` and the same screen would contradict itself.
        //
        // Both ends of the window are bounded, exactly like `get_yearly_activity`
        // constrains its query to the year its spine covers. The lower bound
        // alone let a future-dated `seen_at` — clock skew, or a backup restored
        // from a machine whose clock ran ahead — be counted by this query and
        // then land in no bucket at all, since the spine only spans
        // `today-29..=today`: the chart's own total silently disagreed with a
        // raw count of the rows it claimed to be counting.
        let deduped = self.deduplicated_seen_episodes()?;
        let today = chrono::Local::now().date_naive();
        let min_day = today
            .checked_sub_days(chrono::Days::new(29))
            .unwrap()
            .format("%Y-%m-%d")
            .to_string();
        let max_day = today.format("%Y-%m-%d").to_string();

        let mut counted_days: HashMap<String, i64> = HashMap::new();
        for ep in &deduped {
            if ep.local_day >= min_day && ep.local_day <= max_day {
                *counted_days.entry(ep.local_day.clone()).or_insert(0) += 1;
            }
        }

        // Zero-filled 30-day spine. SQL only emits days that have marks, and a
        // consumer plotting that array directly draws inactive days as if they
        // never existed, silently compressing the timeline. Handing back an
        // explicit `count: 0` for every quiet day makes the shape honest and
        // means no caller has to reconstruct the calendar.
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
            "SELECT MIN(DATE(e.seen_at, 'localtime')) FROM episodes e
             WHERE e.seen = 1 AND e.seen_at IS NOT NULL",
            [],
            |r| r.get(0),
        )?;

        // --- T1 (Backlog), T2 (Studios), T3 (Era) ---
        let mut backlog_episodes = 0;
        let mut backlog_minutes = 0;
        let top_studios: Vec<crate::models::TitleCount>;
        let era_distribution: Vec<crate::models::EraCount>;

        {
            use std::collections::HashSet;
            let mut stmt = self.conn.prepare(
                "SELECT s.title, s.anilist_id, s.source_id, s.watched_externally, s.kind,
                        (SELECT COUNT(*) FROM episodes e WHERE e.series_id=s.id) AS available,
                        (SELECT COUNT(*) FROM episodes e WHERE e.series_id=s.id AND e.seen=1) AS seen_cnt,
                        c.format, c.duration, c.studio, c.start_date, s.followed
                 FROM series s
                 LEFT JOIN anilist_catalog c ON c.id = s.anilist_id
                 WHERE s.followed = 1 OR EXISTS (SELECT 1 FROM episodes e WHERE e.series_id=s.id AND e.seen=1) OR s.watched_externally=1"
            )?;
            struct EngagedRow {
                title: String,
                anilist_id: Option<i64>,
                source_id: i64,
                watched_externally: bool,


                kind: Option<String>,
                available: i64,
                seen_cnt: i64,
                catalog_format: Option<String>,
                catalog_duration: Option<i64>,


                studio: Option<String>,
                start_date: Option<i64>,
                followed: bool,
            }
            let rows: Vec<EngagedRow> = stmt.query_map([], |r| {
                Ok(EngagedRow {
                    title: r.get(0)?,
                    anilist_id: r.get(1)?,
                    source_id: r.get(2)?,
                    watched_externally: r.get::<_, i64>(3)? != 0,
                    kind: r.get(4)?,
                    available: r.get(5)?,
                    seen_cnt: r.get(6)?,
                    catalog_format: r.get(7)?,
                    catalog_duration: r.get(8)?,
                    studio: r.get(9)?,
                    start_date: r.get(10)?,
                    followed: r.get::<_, i64>(11)? != 0,
                })
            })?.collect::<rusqlite::Result<Vec<_>>>()?;

            let aliases = franchise_key_aliases(
                &rows.iter().map(|row| (row.anilist_id, franchise_key(&row.title))).collect::<Vec<_>>()
            );
            let canonical = |key: String| aliases.get(&key).cloned().unwrap_or(key);

            #[derive(Clone)]
            struct EngagedFranchise {
                per_site: HashMap<i64, (i64, i64)>, // source_id -> (available, seen)
                // Minutes per episode: the longest AniList duration among its rows,
                // else the longest format estimate (deterministic, row order must
                // not decide the price of a franchise's remaining episodes).
                catalog_minutes: Option<i64>,
                non_movie_catalog_minutes: Option<i64>,
                estimate_minutes: i64,
                non_movie_estimate_minutes: Option<i64>,
                any_followed: bool,
                any_external: bool,
                studios: HashSet<String>,
                start_dates: Vec<i64>,
            }
            let mut grouped: HashMap<String, EngagedFranchise> = HashMap::new();
            let mut parent_of: HashMap<String, String> = HashMap::new();

            for row in rows {
                let key = canonical(franchise_key(&row.title));
                if let Some(parent) = franchise_parent_key(&row.title).map(canonical) {
                    if parent != key {
                        record_parent_candidate(&mut parent_of, &key, parent);
                    }
                }
                let fmt = row.catalog_format.as_deref().or(row.kind.as_deref());
                let estimate = minutes_per_episode(fmt);
                let is_movie_or_music = matches!(
                    fmt.map(|f| f.trim().to_uppercase()).as_deref(),
                    Some("MOVIE") | Some("PELICULA") | Some("PELÍCULA") | Some("MUSIC")
                );

                let entry = grouped.entry(key).or_insert_with(|| EngagedFranchise {
                    per_site: HashMap::new(),
                    catalog_minutes: None,
                    non_movie_catalog_minutes: None,
                    estimate_minutes: 0,
                    non_movie_estimate_minutes: None,
                    any_followed: false,
                    any_external: false,
                    studios: HashSet::new(),
                    start_dates: Vec::new(),
                });

                if row.followed {
                    entry.any_followed = true;
                }
                if row.watched_externally {
                    entry.any_external = true;
                }
                if let Some(studio) = row.studio {
                    if !studio.trim().is_empty() {
                        entry.studios.insert(studio);
                    }
                }
                if let Some(start_date) = row.start_date {
                    entry.start_dates.push(start_date);
                }

                let bucket = entry.per_site.entry(row.source_id).or_insert((0, 0));
                bucket.0 += row.available;
                bucket.1 += row.seen_cnt;

                // A non-positive AniList duration is missing data, not a
                // zero-minute episode (see `known_duration`).
                let catalog_duration = known_duration(row.catalog_duration);
                entry.catalog_minutes = entry.catalog_minutes.max(catalog_duration);
                if !is_movie_or_music {
                    entry.non_movie_catalog_minutes =
                        entry.non_movie_catalog_minutes.max(catalog_duration);
                    entry.non_movie_estimate_minutes = entry.non_movie_estimate_minutes.max(Some(estimate));
                }
                entry.estimate_minutes = entry.estimate_minutes.max(estimate);
            }

            let merged = merge_into_parent_groups(grouped, &parent_of, |existing, child| {
                for (source_id, &(avail, seen)) in &child.per_site {
                    let bucket = existing.per_site.entry(*source_id).or_insert((0, 0));
                    bucket.0 += avail;
                    bucket.1 += seen;
                }
                existing.any_followed |= child.any_followed;
                existing.any_external |= child.any_external;
                existing.catalog_minutes = existing.catalog_minutes.max(child.catalog_minutes);
                existing.non_movie_catalog_minutes =
                    existing.non_movie_catalog_minutes.max(child.non_movie_catalog_minutes);
                existing.non_movie_estimate_minutes =
                    existing.non_movie_estimate_minutes.max(child.non_movie_estimate_minutes);
                existing.estimate_minutes = existing.estimate_minutes.max(child.estimate_minutes);
                for s in &child.studios {
                    existing.studios.insert(s.clone());
                }
                existing.start_dates.extend(&child.start_dates);
            });

            let mut studio_counts: HashMap<String, i64> = HashMap::new();
            let mut era_counts: HashMap<i32, i64> = HashMap::new();

            for franchise in merged {
                // T1 Backlog: sites are mirrors of the same episodes, so the most
                // any site has minus the most any site has been marked (the
                // roll-up's `real_seen` convention). A "Ya lo vi" franchise is
                // claimed as watched: no backlog even if its scraped rows are unseen.
                if franchise.any_followed && !franchise.any_external {
                    let most_available = franchise.per_site.values().map(|&(avail, _)| avail).max().unwrap_or(0);
                    let most_seen = franchise.per_site.values().map(|&(_, seen)| seen).max().unwrap_or(0);
                    let remaining = (most_available - most_seen).max(0);
                    // A TV member without a stored duration is priced at the TV estimate,
                    // never at a movie member's runtime.
                    let per_episode = match franchise.non_movie_estimate_minutes {
                        Some(tv_estimate) => franchise.non_movie_catalog_minutes.unwrap_or(tv_estimate),
                        None => franchise.catalog_minutes.unwrap_or(franchise.estimate_minutes),
                    };
                    backlog_episodes += remaining;
                    backlog_minutes += remaining * per_episode;
                }

                // T2 Studios
                for studio in &franchise.studios {
                    *studio_counts.entry(studio.clone()).or_insert(0) += 1;
                }

                // T3 Era
                if let Some(&min_date) = franchise.start_dates.iter().min() {
                    use chrono::{TimeZone, Utc};
                    if let chrono::LocalResult::Single(dt) = Utc.timestamp_opt(min_date, 0) {
                        let year = dt.year();
                        let decade = (year / 10) * 10;
                        *era_counts.entry(decade).or_insert(0) += 1;
                    }
                }
            }

            let mut studios: Vec<crate::models::TitleCount> = studio_counts
                .into_iter()
                .map(|(title, count)| crate::models::TitleCount { title, count })
                .collect();
            studios.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.title.cmp(&b.title)));
            studios.truncate(8);
            top_studios = studios;

            let mut eras: Vec<crate::models::EraCount> = era_counts
                .into_iter()
                .map(|(decade, count)| crate::models::EraCount { decade, count })
                .collect();
            eras.sort_by_key(|e| e.decade);
            era_distribution = eras;
        }

        Ok(crate::models::WatchInsights {
            estimated_minutes_tracked,
            estimated_minutes_external,
            estimated_minutes_share,
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
            backlog_episodes,
            backlog_minutes,
            top_studios,
            era_distribution,
        })
    }

    /// Followed franchises that have gone quiet — no episode marked seen in
    /// 90+ days while episodes are still waiting. Grouped per franchise with
    /// the same key/alias/parent rules as `franchise_rollups`, so the same show
    /// followed on two sites appears once and S1 isn't "dusty" while S2 of the
    /// same franchise is being watched. "Never started" (no seen_at at all) is
    /// excluded — dusty means it was watched, then stopped — and so is a
    /// franchise with nothing left to watch in a member the user started:
    /// finished is not abandoned, and an unstarted sequel is backlog. Returns
    /// at most 12 entries, sorted by most stale first (oldest last_seen_at).
    pub fn get_dusty_watchlist(&self) -> Result<Vec<crate::models::DustyEntry>> {
        // Every followed row with its latest dated seen mark (NULL when it has
        // none) and whether it still has episodes to watch. A "Ya lo vi" row is
        // claimed as watched, so its unseen scraped episodes don't count as
        // waiting — the rule `list_pending` and `pending_to_watch` apply. Only
        // a row the user has started (at least one seen mark) can be the one
        // left waiting: S1 finished long ago next to a never-started S2 is a
        // backlog entry, not an abandoned show.
        // IMPORTANT: the 90-day filter runs AFTER grouping, not before. If we
        // filtered before grouping, a show followed on two sites (or a
        // franchise with two seasons) where one row is recent (<90 days) and
        // another is stale (>90 days) would wrongly keep the stale one. By
        // reducing to MAX per franchise in Rust, then filtering, we correctly
        // answer "has the user engaged with this franchise recently on ANY
        // site, in ANY season".
        let mut stmt = self.conn.prepare(
            "SELECT s.anilist_id, s.title,
                    MAX(CASE WHEN e.seen=1 AND e.seen_at IS NOT NULL
                             THEN DATE(e.seen_at, 'localtime') END) AS last_seen,
                    MAX(CASE WHEN e.seen=0 AND s.watched_externally=0 THEN 1 ELSE 0 END)
                      * MAX(e.seen) AS has_unseen
             FROM series s
             JOIN episodes e ON e.series_id = s.id
             WHERE s.followed=1
             GROUP BY s.id",
        )?;
        struct Row {
            anilist_id: Option<i64>,
            title: String,
            last_seen: Option<String>,
            has_unseen: bool,
        }
        let rows: Vec<Row> = stmt
            .query_map([], |r| {
                Ok(Row {
                    anilist_id: r.get(0)?,
                    title: r.get(1)?,
                    last_seen: r.get(2)?,
                    has_unseen: r.get::<_, i64>(3)? != 0,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        let aliases = franchise_key_aliases(
            &rows.iter().map(|row| (row.anilist_id, franchise_key(&row.title))).collect::<Vec<_>>(),
        );
        let canonical = |key: String| aliases.get(&key).cloned().unwrap_or(key);

        #[derive(Clone)]
        struct DustyGroup {
            display_title: String,
            /// The MOST RECENT (max) dated seen mark across every member row.
            last_seen: Option<String>,
            has_unseen: bool,
        }
        let mut grouped: HashMap<String, DustyGroup> = HashMap::new();
        let mut parent_of: HashMap<String, String> = HashMap::new();
        for row in rows {
            let key = canonical(franchise_key(&row.title));
            if let Some(parent) = franchise_parent_key(&row.title).map(canonical) {
                if parent != key {
                    record_parent_candidate(&mut parent_of, &key, parent);
                }
            }
            let display = franchise_display_title(&row.title);
            let entry = grouped.entry(key).or_insert_with(|| DustyGroup {
                display_title: display.clone(),
                last_seen: None,
                has_unseen: false,
            });
            if prefer_display_title(&display, &entry.display_title) {
                entry.display_title = display;
            }
            entry.last_seen = entry.last_seen.take().max(row.last_seen);
            entry.has_unseen |= row.has_unseen;
        }
        let merged = merge_into_parent_groups(grouped, &parent_of, |existing, child| {
            existing.last_seen = existing.last_seen.take().max(child.last_seen.clone());
            existing.has_unseen |= child.has_unseen;
        });

        // Now filter: only keep groups with something left to watch whose most
        // recent engagement is >90 days old.
        let cutoff = chrono::Local::now().date_naive() - chrono::Days::new(90);
        let cutoff_str = cutoff.format("%Y-%m-%d").to_string();

        let mut entries: Vec<crate::models::DustyEntry> = merged
            .into_iter()
            .filter(|g| g.has_unseen)
            .filter_map(|g| {
                let last_seen_at = g.last_seen?;
                (last_seen_at < cutoff_str).then_some(crate::models::DustyEntry {
                    title: g.display_title,
                    last_seen_at,
                })
            })
            .collect();

        // Sort by most stale first (oldest last_seen_at), title as a stable
        // tie-break since the groups come out of a HashMap.
        entries.sort_by(|a, b| a.last_seen_at.cmp(&b.last_seen_at).then_with(|| a.title.cmp(&b.title)));
        // Cap at 12
        entries.truncate(12);
        Ok(entries)
    }

    /// Binge record: the single calendar day with the most episodes marked
    /// seen, and that count. `seen_at` is stamped UTC (SQLite `datetime('now')`),
    /// so — exactly like `get_watch_insights`'s day buckets — every row is
    /// converted to 'localtime' before grouping; otherwise an episode marked
    /// at 00:30 in Spain lands on the previous calendar day. Returns
    /// `count: 0, day: None` when nothing has ever been marked seen, rather
    /// than erroring. Filters on `seen=1 AND seen_at IS NOT NULL`, the same
    /// pair every other watch-history query in this file uses.
    pub fn get_binge_record(&self) -> Result<crate::models::BingeRecord> {
        let deduped = self.deduplicated_seen_episodes()?;
        if deduped.is_empty() {
            return Ok(crate::models::BingeRecord {
                day: None,
                count: 0,
                top_series: Vec::new(),
            });
        }

        let mut day_counts: HashMap<String, i64> = HashMap::new();
        for ep in &deduped {
            *day_counts.entry(ep.local_day.clone()).or_insert(0) += 1;
        }

        // Find day with max count. Tie-breaker: most recent day (cnt DESC, d DESC).
        let mut sorted_days: Vec<(String, i64)> = day_counts.into_iter().collect();
        sorted_days.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| b.0.cmp(&a.0)));

        let (top_day, count) = sorted_days.into_iter().next().unwrap();

        let mut groups: HashMap<String, i64> = HashMap::new();
        for ep in &deduped {
            if ep.local_day == top_day {
                let label = franchise_display_title(&ep.title);
                *groups.entry(label).or_insert(0) += 1;
            }
        }

        let mut top_list: Vec<(String, i64)> = groups.into_iter().collect();
        top_list.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        top_list.truncate(3);
        let top_series: Vec<String> = top_list.into_iter().map(|(t, _)| t).collect();

        Ok(crate::models::BingeRecord {
            day: Some(top_day),
            count,
            top_series,
        })
    }

    /// 24-hour distribution of when the user marks episodes as seen — one entry
    /// per hour (0-23), zero-filled. Mirrors `get_watch_insights`'s 30-day spine
    /// logic: `seen_at` is stamped with SQLite's `datetime('now')` (UTC), so
    /// every bucket converts to 'localtime' first. Without it an episode marked
    /// at 00:30 in Spain (UTC+1/+2) lands in the previous hour — and the late
    /// evening is exactly when this app is used, so the misattribution is the
    /// common case, not an edge one.
    pub fn get_hourly_distribution(&self) -> Result<Vec<crate::models::HourCount>> {
        let deduped = self.deduplicated_seen_episodes()?;
        let mut counted_hours: HashMap<i64, i64> = HashMap::new();
        for ep in &deduped {
            *counted_hours.entry(ep.local_hour).or_insert(0) += 1;
        }

        // Zero-fill all 24 hours — the frontend must get exactly 24 entries,
        // hour 0 through 23, with count 0 for hours with no activity.
        let mut distribution: Vec<crate::models::HourCount> = Vec::with_capacity(24);
        for hour in 0..24 {
            let count = counted_hours.get(&hour).copied().unwrap_or(0);
            distribution.push(crate::models::HourCount { hour, count });
        }
        Ok(distribution)
    }

    /// Average days a franchise took **the user** to finish, across every
    /// franchise they actually finished. Returns `None` when none qualifies
    /// (nothing completed, no timestamps) — a meaningful distinction from
    /// `0.0`, which would imply instant completion.
    ///
    /// "Finished" needs both halves, and the second one is the fix for a real
    /// bug: the show itself has stopped broadcasting (no member row still says
    /// `is_airing=1`) **and** the user's own progress covers everything known
    /// about it. Filtering on the broadcast status alone counted a 100-episode
    /// finished show the user sampled a single episode of as a completed
    /// franchise with a 0-day span (one `seen_at`, so first == last), dragging
    /// the whole average toward zero — it measured "the show stopped airing",
    /// not "the user finished watching it".
    ///
    /// The known total is the largest of: the episode rows the app holds for
    /// the best site, that site's own advertised `site_episode_count`, and the
    /// linked AniList row's `episodes`. Progress still uses the old
    /// best-single-site rule (summed within one site, the best site across
    /// sites) — not `franchise_rollups`' distinct-episode count — so a show
    /// marked on two sites never looks like twice its own length, but seasons
    /// marked on different sites don't add up here either. Only franchises with at least one real
    /// `episodes.seen_at` count (the first/last date is what the span is made
    /// of), and grouping runs through `franchise_key` **plus** the same parent
    /// merge every other franchise stat here uses: this one function used to
    /// group by bare `franchise_key`, so "One Piece: Wano" was a separate
    /// franchise contributing its own separate span.
    pub fn get_avg_completion_days(&self) -> Result<Option<f64>> {
        // franchise_key() is a Rust function (see its definition above), not
        // a SQLite one — every other place in this file that groups by
        // franchise (franchise_rollups, get_dusty_watchlist) fetches flat
        // per-series rows via plain SQL and does the franchise_key grouping
        // in Rust; this follows the same pattern rather than trying to call
        // franchise_key() from inside a CTE.
        let mut stmt = self.conn.prepare(
            "SELECT s.title, s.anilist_id, s.source_id, s.is_airing, s.site_episode_count, c.episodes,
                    (SELECT COUNT(*) FROM episodes t WHERE t.series_id=s.id) AS total_eps,
                    (SELECT COUNT(*) FROM episodes t WHERE t.series_id=s.id AND t.seen=1) AS seen_eps,
                    MIN(DATE(e.seen_at, 'localtime')) AS first_seen,
                    MAX(DATE(e.seen_at, 'localtime')) AS last_seen
             FROM series s
             JOIN episodes e ON e.series_id = s.id
             LEFT JOIN anilist_catalog c ON c.id = s.anilist_id
             WHERE e.seen = 1
               AND e.seen_at IS NOT NULL
             GROUP BY s.id",
        )?;
        struct SeriesSpan {
            title: String,
            anilist_id: Option<i64>,
            source_id: i64,
            is_airing: bool,
            site_episode_count: Option<i64>,
            catalog_episodes: Option<i64>,
            total_eps: i64,
            seen_eps: i64,
            first_seen: String,
            last_seen: String,
        }
        let rows: Vec<SeriesSpan> = stmt
            .query_map([], |r| {
                Ok(SeriesSpan {
                    title: r.get(0)?,
                    anilist_id: r.get(1)?,
                    source_id: r.get(2)?,
                    is_airing: r.get::<_, i64>(3)? != 0,
                    site_episode_count: r.get(4)?,
                    catalog_episodes: r.get(5)?,
                    total_eps: r.get(6)?,
                    seen_eps: r.get(7)?,
                    first_seen: r.get(8)?,
                    last_seen: r.get(9)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        // Same identity folding as `franchise_rollups` — two sites' rows for
        // one show that the user's AniList links tie together are one
        // franchise, whatever their titles look like.
        let aliases = franchise_key_aliases(
            &rows
                .iter()
                .map(|row| (row.anilist_id, franchise_key(&row.title)))
                .collect::<Vec<_>>(),
        );
        let canonical = |key: String| aliases.get(&key).cloned().unwrap_or(key);

        // Roll up to franchise level: any airing member disqualifies the
        // whole franchise (not "finished" yet), and the span is the earliest
        // first_seen to the latest last_seen across every member series —
        // same reasoning as franchise_rollups, a long-runner's arcs are one
        // show, not several.
        #[derive(Clone)]
        struct FranchiseSpan {
            any_airing: bool,
            first_seen: String,
            last_seen: String,
            /// `(episodes marked seen, episode rows held)` per `source_id`.
            per_site: HashMap<i64, (i64, i64)>,
            /// Best externally-advertised episode total across member rows —
            /// AniList's `episodes` or the site's own count, whichever is
            /// larger. 0 when nothing declares one.
            declared_total: i64,
        }
        let mut grouped: HashMap<String, FranchiseSpan> = HashMap::new();
        // Candidate parent per group, applied only once every key present in
        // this user's data is known — same two-phase shape as
        // `franchise_rollups`.
        let mut parent_of: HashMap<String, String> = HashMap::new();
        for row in rows {
            let key = canonical(franchise_key(&row.title));
            if let Some(parent) = franchise_parent_key(&row.title).map(canonical) {
                if parent != key {
                    record_parent_candidate(&mut parent_of, &key, parent);
                }
            }
            let entry = grouped.entry(key).or_insert_with(|| FranchiseSpan {
                any_airing: false,
                first_seen: row.first_seen.clone(),
                last_seen: row.last_seen.clone(),
                per_site: HashMap::new(),
                declared_total: 0,
            });
            entry.any_airing |= row.is_airing;
            if row.first_seen < entry.first_seen {
                entry.first_seen = row.first_seen;
            }
            if row.last_seen > entry.last_seen {
                entry.last_seen = row.last_seen;
            }
            let bucket = entry.per_site.entry(row.source_id).or_insert((0, 0));
            bucket.0 += row.seen_eps;
            bucket.1 += row.total_eps;
            entry.declared_total = entry
                .declared_total
                .max(row.site_episode_count.unwrap_or(0))
                .max(row.catalog_episodes.unwrap_or(0));
        }

        let franchises = merge_into_parent_groups(grouped, &parent_of, |existing, child| {
            existing.any_airing |= child.any_airing;
            if child.first_seen < existing.first_seen {
                existing.first_seen = child.first_seen.clone();
            }
            if child.last_seen > existing.last_seen {
                existing.last_seen = child.last_seen.clone();
            }
            for (source_id, (seen, total)) in &child.per_site {
                let bucket = existing.per_site.entry(*source_id).or_insert((0, 0));
                bucket.0 += seen;
                bucket.1 += total;
            }
            existing.declared_total = existing.declared_total.max(child.declared_total);
        });

        let spans_days: Vec<f64> = franchises
            .iter()
            .filter(|f| !f.any_airing)
            .filter(|f| {
                // The site with the most marks speaks for the franchise, and
                // its own episode-row count is the floor for "how long is
                // this" — raised to whatever AniList or the site itself
                // declares when that is larger.
                let (seen, rows_held) = f
                    .per_site
                    .values()
                    .copied()
                    .max_by_key(|&(seen, _)| seen)
                    .unwrap_or((0, 0));
                let total = rows_held.max(f.declared_total);
                total > 0 && seen >= total
            })
            .filter_map(|f| {
                let first = chrono::NaiveDate::parse_from_str(&f.first_seen, "%Y-%m-%d").ok()?;
                let last = chrono::NaiveDate::parse_from_str(&f.last_seen, "%Y-%m-%d").ok()?;
                let days = (last - first).num_days();
                // Everything marked on one calendar day is a bulk "mark as
                // seen" (or a one-day marathon): not a completion time, and
                // counted as 0 days it would drag the average toward nothing.
                if days == 0 {
                    None
                } else {
                    Some(days as f64)
                }
            })
            .collect();

        if spans_days.is_empty() {
            return Ok(None);
        }
        Ok(Some(spans_days.iter().sum::<f64>() / spans_days.len() as f64))
    }

    /// A cheap fingerprint of the user's data, used to skip a redundant
    /// auto-backup when nothing changed since the last one.
    ///
    /// This deliberately reaches past the episode table. It used to be only
    /// `(COUNT(series), COUNT(episodes), MAX(episodes.id), MAX(seen_at))`,
    /// which is blind to almost everything the app actually lets you change:
    /// follow/unfollow, "Ya lo vi", backlog moves, every swipe decision on a
    /// catalog card (those write a `series` row but never an episode), AniList
    /// links, and every setting — the mirror list, the active site, the deck's
    /// genre/format bans and date bounds. An evening spent on any of that left
    /// the fingerprint byte-identical, so `is_auto_backup_due` kept answering
    /// "no" and the backup silently never ran.
    ///
    /// Cheap enough to run on every startup and after every refresh: eight
    /// aggregates over indexed-or-tiny tables plus a scan of `settings` (a few
    /// dozen rows).
    pub fn signature_counts(&self) -> Result<SignatureCounts> {
        let one = |sql: &str| -> Result<i64> { Ok(self.conn.query_row(sql, [], |r| r.get(0))?) };
        let series = one("SELECT COUNT(*) FROM series")?;
        let episodes = one("SELECT COUNT(*) FROM episodes")?;
        let max_episode_id = one("SELECT COALESCE(MAX(id),0) FROM episodes")?;
        let max_seen_at: Option<String> = self
            .conn
            .query_row("SELECT MAX(seen_at) FROM episodes", [], |r| r.get(0))
            .optional()?
            .flatten();
        // Counts, not just "did a row appear": these all flip back and forth
        // on rows that already exist, which no COUNT(*) over the table sees.
        let seen_episodes = one("SELECT COUNT(*) FROM episodes WHERE seen=1")?;
        let followed = one("SELECT COUNT(*) FROM series WHERE followed=1")?;
        let watched_externally = one("SELECT COUNT(*) FROM series WHERE watched_externally=1")?;
        let backlog_want = one("SELECT COUNT(*) FROM series WHERE backlog_status='want'")?;
        let backlog_discarded = one("SELECT COUNT(*) FROM series WHERE backlog_status='discarded'")?;
        let anilist_linked = one("SELECT COUNT(*) FROM series WHERE anilist_id IS NOT NULL")?;
        let settings_hash = self.settings_hash()?;
        Ok(SignatureCounts {
            series,
            episodes,
            max_episode_id,
            max_seen_at,
            seen_episodes,
            followed,
            watched_externally,
            backlog_want,
            backlog_discarded,
            anilist_linked,
            settings_hash,
        })
    }

    /// Order-independent hash of the `settings` table, so a *changed value*
    /// (not just an added key) moves the backup signature — the deck's ban
    /// lists, the mirror list and the active site all live there as values
    /// under a fixed set of keys, so a COUNT would never notice them.
    ///
    /// Backup bookkeeping is excluded on purpose: `backup_*`/`gdrive_*` are
    /// written *by* a backup, so including them would make every backup change
    /// the signature and the "has anything changed?" half of
    /// `is_auto_backup_due` would degenerate into "always yes". Hashed rather
    /// than concatenated so nothing from the settings table (which holds the
    /// Google client secret) can end up stored in `backup_signature`.
    fn settings_hash(&self) -> Result<u64> {
        let mut stmt = self.conn.prepare(
            "SELECT key, value FROM settings
             WHERE key NOT LIKE 'backup\\_%' ESCAPE '\\'
               AND key NOT LIKE 'gdrive\\_%' ESCAPE '\\'
             ORDER BY key",
        )?;
        let rows: Vec<(String, String)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        // FNV-1a over "key=value\n" pairs in key order. Not cryptographic and
        // doesn't need to be — the only requirement is that it changes when
        // the settings do, and stays identical when they don't.
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        let mut write = |bytes: &[u8]| {
            for b in bytes {
                hash ^= *b as u64;
                hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
            }
        };
        for (key, value) in &rows {
            write(key.as_bytes());
            write(b"=");
            write(value.as_bytes());
            write(b"\n");
        }
        Ok(hash)
    }

    /// Mainstream vs underground taste score — average AniList popularity of
    /// followed or "Ya lo vi" series linked to the catalog, normalized to
    /// 0-10. Cross-site canonical dedup via `canon_key` so the same show
    /// (followed on two sites, or followed and "Ya lo vi") counts once.
    /// Returns `None` for both fields when fewer than 3 such linked shows
    /// exist (not enough data for a meaningful score).
    pub fn get_popularity_bias(&self) -> Result<crate::models::PopularityBias> {
        // Get all followed series with their anilist_id and title, then group by canon_key
        // and pick one representative per canonical show. Then join with anilist_catalog
        // to get popularity.
        let mut stmt = self.conn.prepare(
            "SELECT s.anilist_id, s.title, c.popularity
             FROM series s
             LEFT JOIN anilist_catalog c ON c.id = s.anilist_id
             WHERE (s.followed=1 OR s.watched_externally=1)
               AND s.anilist_id IS NOT NULL AND c.popularity IS NOT NULL",
        )?;
        let rows: Vec<(Option<i64>, String, i64)> = stmt
            .query_map([], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        // Group by canon_key to dedup cross-site
        let mut grouped: HashMap<String, i64> = HashMap::new();
        for (anilist_id, title, popularity) in rows {
            let key = super::library::canon_key(anilist_id, &title);
            // Keep the first popularity per canon_key (any is fine since they
            // represent the same show and should have the same AniList ID)
            grouped.entry(key).or_insert(popularity);
        }

        let count = grouped.len();
        if count < 3 {
            return Ok(crate::models::PopularityBias {
                average_popularity: None,
                normalized_score: None,
                percentile: None,
            });
        }

        let sum: i64 = grouped.values().sum();
        let average = sum as f64 / count as f64;
        // 70,000 per point is an approximation, not an AniList API contract
        // (see this task's own spec) — a handful of the most popular shows
        // on AniList individually clear 700,000, so a followed list skewed
        // toward those would otherwise push the normalized score past 10 and
        // break the 0-10 slider. Clamp to the scale's own bounds.
        let normalized = (average / 70_000.0).min(10.0);

        let (count_below, total_catalog): (i64, i64) = self.conn.query_row(
            "SELECT
                (SELECT COUNT(*) FROM anilist_catalog WHERE popularity < ?1),
                (SELECT COUNT(*) FROM anilist_catalog WHERE popularity IS NOT NULL)",
            [average],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;

        let percentile = if total_catalog > 0 {
            (count_below as f64 / total_catalog as f64) * 100.0
        } else {
            0.0
        };

        Ok(crate::models::PopularityBias {
            average_popularity: Some(average),
            normalized_score: Some(normalized),
            percentile: Some(percentile),
        })
    }

    /// Distinct years (local time) that have at least one seen episode,
    /// descending. Always includes the current year even if it has no marks yet,
    /// so a brand-new user still gets this year's empty grid instead of an empty
    /// year list.
    pub fn get_activity_years(&self) -> Result<Vec<i32>> {
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT CAST(strftime('%Y', DATE(e.seen_at, 'localtime')) AS INTEGER) AS y
             FROM episodes e
             WHERE e.seen = 1
               AND e.seen_at IS NOT NULL
             ORDER BY y DESC",
        )?;
        let mut years: Vec<i32> = stmt
            .query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<Vec<i32>>>()?;

        // Always include the current year (local time) so the year selector is
        // never empty — mirrors the `marks_tracked_since` grace handling.
        // Sorted after inserting it: a future-dated mark (clock skew, a backup
        // from a machine whose clock ran ahead) is a later year than this one.
        let this_year = chrono::Local::now().date_naive().year();
        if !years.contains(&this_year) {
            years.push(this_year);
            years.sort_unstable_by(|a, b| b.cmp(a));
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

        let deduped = self.deduplicated_seen_episodes()?;
        let mut counted_days: HashMap<String, i64> = HashMap::new();
        for ep in &deduped {
            if ep.local_year == year {
                *counted_days.entry(ep.local_day.clone()).or_insert(0) += 1;
            }
        }

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
mod tests;
