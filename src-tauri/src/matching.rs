//! Fuzzy title matching between AniList catalog titles (romaji/english) and
//! site search-result titles (Spanish/romaji, with noise like "Sub Español",
//! season suffixes, punctuation). Pure, no I/O — see
//! `commands::link_catalog_series` for the caller that feeds it real search
//! results.

use std::collections::{HashMap, HashSet};

/// One candidate title scraped off a search-results page.
pub struct TitleCandidate<'a> {
    pub title: &'a str,
    /// The candidate's source URL. `best_match` returns the winning index, so
    /// callers read the URL from their own parallel list rather than from
    /// here; the field is carried for callers that want a self-contained
    /// candidate. `#[allow]` since nothing reads it through this struct today.
    #[allow(dead_code)]
    pub url: &'a str,
}

/// The winning candidate's index into the slice passed to `best_match`, plus
/// its score (for logging/debugging — callers don't need to re-derive it).
#[derive(Debug, Clone, PartialEq)]
pub struct MatchResult {
    pub index: usize,
    pub score: f64,
}

/// Below this score, `best_match` reports no match at all rather than a weak
/// guess — a wrong automatic link (real site URL, real episodes) is worse
/// than leaving a title unlinked, since `NoMatch` has a manual retry escape
/// hatch and a wrong link doesn't. Tuned against the fixture-driven test
/// cases in this module: decoys (different season, different show, unrelated
/// title with one shared word) score well below this, and true matches
/// (identical after normalization, or same title plus a stripped-off noise
/// suffix) always land above it.
pub const MATCH_THRESHOLD: f64 = 0.72;

/// Lowercase, strip accents, collapse punctuation/whitespace to single
/// spaces, then strip a short trailing noise-token set the site appends to
/// otherwise-clean titles ("Sub Español", "Latino", "Castellano", "HD",
/// "Online"). Not a general slugifier — just enough to make "Attack on
/// Titan" and "Attack on Titan Sub Español" compare equal.
///
/// Public (not just an internal `best_match` helper) because `db.rs` reuses
/// it to compare site-series titles against catalog titles for the
/// Descubrir-deck engaged-title exclusion (see
/// `db::engaged_series_titles`/`db::random_catalog_anime_in_genre`) — those
/// callers want the exact same normalization `best_match` uses internally,
/// not a second slightly-different implementation.
pub fn normalize_title(s: &str) -> String {
    let lower = crate::genres::strip_accents(s).to_lowercase();
    let mut collapsed = String::with_capacity(lower.len());
    let mut last_was_space = true; // true at start so leading punctuation doesn't emit a leading space
    for c in lower.chars() {
        if c.is_alphanumeric() {
            collapsed.push(c);
            last_was_space = false;
        } else if !last_was_space {
            collapsed.push(' ');
            last_was_space = true;
        }
    }
    let trimmed = collapsed.trim_end().to_string();
    strip_noise_suffixes(&trimmed)
}

/// Short, explicit noise-suffix list (see `normalize`'s doc comment). Order
/// doesn't matter — every suffix is retried after each successful strip, so
/// "... Sub Español HD" strips both in either order.
const NOISE_SUFFIXES: &[&str] = &["sub espanol", "latino", "castellano", "hd", "online"];

fn strip_noise_suffixes(s: &str) -> String {
    let mut current = s.to_string();
    loop {
        let mut stripped_any = false;
        for suffix in NOISE_SUFFIXES {
            if let Some(rest) = current.strip_suffix(suffix) {
                // Only a real word-boundary strip: either nothing precedes it,
                // or it's preceded by the space our own normalization always
                // inserts between words. Prevents e.g. stripping "hd" out of
                // a hypothetical title that happens to end in "...ahd" (no
                // boundary) — can't happen after our alnum-only collapse
                // anyway, but keep the check explicit rather than relying on
                // that invariant silently.
                if rest.is_empty() || rest.ends_with(' ') {
                    current = rest.trim_end().to_string();
                    stripped_any = true;
                    break;
                }
            }
        }
        if !stripped_any {
            break;
        }
    }
    current
}

/// One `anilist_catalog` row reduced to what local (no-network) linking needs.
pub struct CatalogTitleRow {
    pub id: i64,
    /// AniList's display title plus its romaji/english variants when stored.
    /// All are indexed, so a site row matches whichever spelling it uses.
    pub titles: Vec<String>,
    /// Tiebreaker when several catalog rows normalize to the same title —
    /// remakes, shorts and specials routinely share a show's exact name, and
    /// the popular one is the entry a user means. Missing popularity reads
    /// as 0.
    pub popularity: i64,
}

/// Normalized-title -> AniList id lookup table, built once per linking run.
///
/// Deliberately exact-match only, no fuzzy scoring: this runs unattended over
/// every unlinked series at once, and `MATCH_THRESHOLD`'s "a wrong automatic
/// link is worse than no link" reasoning applies far harder here than it does
/// to `best_match`, which scores a handful of candidates the user just
/// searched for. Coverage comes instead from trying several reduced forms of
/// the site title (see `lookup`), each of which is still an exact match.
pub struct CatalogIndex {
    by_normalized_title: HashMap<String, (i64, i64)>,
    /// Parallel arrays for the fuzzy fallback: every catalog title's normalized
    /// form with its `(id, popularity)`. Indexed by position.
    fuzzy_titles: Vec<(String, i64, i64)>,
    /// Inverted token -> positions in `fuzzy_titles`, so a fuzzy lookup only
    /// scores catalog titles that share a real word with the query instead of
    /// all ~22k of them.
    token_index: HashMap<String, Vec<u32>>,
}

/// Minimum score for the fuzzy catalog fallback to *auto-link* a series.
/// Stricter than `MATCH_THRESHOLD` (which scores a handful of candidates the
/// user just searched): here it runs unattended over every unlinked row, and a
/// wrong automatic link is worse than none. 0.84 clears the 0.9 season-suffix
/// bonus ("…2nd Season" vs "…Temporada 2") and near-exact romaji/English
/// variants, while rejecting loose partial overlaps.
const FUZZY_LINK_THRESHOLD: f64 = 0.84;

/// Tokens shorter than this are skipped in the token index — anime romaji is
/// full of particles ("no", "wa", "de", "ni") that would bloat postings and
/// match everything. Distinctive words are longer.
const MIN_INDEX_TOKEN_LEN: usize = 4;

impl CatalogIndex {
    pub fn build(rows: &[CatalogTitleRow]) -> Self {
        let mut by_normalized_title: HashMap<String, (i64, i64)> = HashMap::new();
        let mut fuzzy_titles: Vec<(String, i64, i64)> = Vec::new();
        let mut token_index: HashMap<String, Vec<u32>> = HashMap::new();
        for row in rows {
            for title in &row.titles {
                let key = normalize_title(title);
                if key.is_empty() {
                    continue;
                }
                by_normalized_title
                    .entry(key.clone())
                    .and_modify(|existing| {
                        // Higher popularity wins; equal popularity resolves to
                        // the lower id so a run is reproducible regardless of
                        // the order rows came out of SQLite.
                        if row.popularity > existing.1
                            || (row.popularity == existing.1 && row.id < existing.0)
                        {
                            *existing = (row.id, row.popularity);
                        }
                    })
                    .or_insert((row.id, row.popularity));

                let pos = fuzzy_titles.len() as u32;
                for tok in key.split_whitespace().filter(|t| t.len() >= MIN_INDEX_TOKEN_LEN) {
                    let postings = token_index.entry(tok.to_string()).or_default();
                    if postings.last() != Some(&pos) {
                        postings.push(pos);
                    }
                }
                fuzzy_titles.push((key, row.id, row.popularity));
            }
        }
        Self { by_normalized_title, fuzzy_titles, token_index }
    }

    /// Best fuzzy catalog id for any of `queries`, or `None` when nothing clears
    /// `FUZZY_LINK_THRESHOLD`. Only catalog titles sharing a ≥4-char word with
    /// the query are scored (via `token_index`), so this stays cheap even over
    /// the whole catalog. Ties break toward the more popular row — remakes and
    /// shorts reuse a show's words, and the popular entry is the one meant.
    pub fn fuzzy_lookup(&self, queries: &[&str]) -> Option<i64> {
        let mut best: Option<(f64, i64, i64)> = None; // (score, popularity, id)
        for q in queries {
            let qn = normalize_title(q);
            if qn.is_empty() {
                continue;
            }
            // Candidate positions: union of postings for the query's real words.
            let mut seen: HashSet<u32> = HashSet::new();
            for tok in qn.split_whitespace().filter(|t| t.len() >= MIN_INDEX_TOKEN_LEN) {
                if let Some(postings) = self.token_index.get(tok) {
                    seen.extend(postings.iter().copied());
                }
            }
            for pos in seen {
                let (cand_norm, id, pop) = &self.fuzzy_titles[pos as usize];
                let s = score(&qn, cand_norm);
                if s >= FUZZY_LINK_THRESHOLD
                    && best.as_ref().is_none_or(|(bs, bpop, _)| s > *bs || (s == *bs && pop > bpop))
                {
                    best = Some((s, *pop, *id));
                }
            }
        }
        best.map(|(_, _, id)| id)
    }

    /// First `candidates` entry with an exact normalized hit, so callers pass
    /// their forms most-specific first (the raw site title, then progressively
    /// reduced ones like the franchise base name).
    pub fn lookup(&self, candidates: &[&str]) -> Option<i64> {
        candidates.iter().find_map(|candidate| {
            let key = normalize_title(candidate);
            if key.is_empty() {
                return None;
            }
            self.by_normalized_title.get(&key).map(|(id, _)| *id)
        })
    }

    /// Whether the index holds no titles at all — every catalog title
    /// normalized to nothing, or the catalog was empty. Part of the type's
    /// public surface (asserted by tests); `#[allow]` because the production
    /// caller happens to check emptiness a different way.
    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.by_normalized_title.is_empty()
    }
}

fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let (n, m) = (a.len(), b.len());
    if n == 0 {
        return m;
    }
    if m == 0 {
        return n;
    }
    let mut dp = vec![0usize; m + 1];
    for (j, cell) in dp.iter_mut().enumerate() {
        *cell = j;
    }
    for i in 1..=n {
        let mut prev_diag = dp[0];
        dp[0] = i;
        for j in 1..=m {
            let cost = if a[i - 1] == b[j - 1] { 0 } else { 1 };
            let deletion = dp[j] + 1;
            let insertion = dp[j - 1] + 1;
            let substitution = prev_diag + cost;
            prev_diag = dp[j];
            dp[j] = deletion.min(insertion).min(substitution);
        }
    }
    dp[m]
}

/// 1.0 for identical strings, trending toward 0.0 as edit distance grows
/// relative to the longer string's length.
fn levenshtein_ratio(a: &str, b: &str) -> f64 {
    let max_len = a.chars().count().max(b.chars().count());
    if max_len == 0 {
        return 1.0;
    }
    1.0 - (levenshtein(a, b) as f64 / max_len as f64)
}

/// Jaccard similarity over whitespace-split token sets.
fn jaccard(a: &str, b: &str) -> f64 {
    let sa: HashSet<&str> = a.split_whitespace().collect();
    let sb: HashSet<&str> = b.split_whitespace().collect();
    if sa.is_empty() && sb.is_empty() {
        return 1.0;
    }
    let intersection = sa.intersection(&sb).count();
    let union = sa.union(&sb).count();
    if union == 0 {
        0.0
    } else {
        intersection as f64 / union as f64
    }
}

/// Season/part marker words: tokens that denote a later season/part of the
/// *same* show rather than a distinct work. Deliberately excludes "movie",
/// "ova", "special", "arc"/"saga" — those name different content, and treating
/// them as season markers would import the wrong episodes.
const SEASON_MARKERS: &[&str] =
    &["season", "temporada", "part", "parte", "cour", "final", "the"];
const ROMAN_NUMERALS: &[&str] = &["ii", "iii", "iv", "v", "vi", "vii", "viii", "ix", "x"];

fn is_ascii_int(t: &str) -> bool {
    !t.is_empty() && t.chars().all(|c| c.is_ascii_digit())
}

/// The "franchise base" of a normalized title: the same title with any
/// trailing season/part markers stripped, in order. Drops trailing marker
/// words ("season", "temporada", "part", "final", "the", …), ordinals ("2nd"),
/// Roman numerals ("ii".."x"), and a bare integer **only when a marker word
/// immediately precedes it** ("... part 2" → the 2 goes; "steins gate 0" → the
/// 0 stays, because no marker precedes it, so it's part of the title of a
/// distinct work). Order-aware on purpose — a token *set* can't tell "Part 2"
/// (same show, later part) from "Steins;Gate 0" (a different work).
fn franchise_base(norm: &str) -> String {
    const MARKERS: &[&str] = SEASON_MARKERS;
    const ROMANS: &[&str] = ROMAN_NUMERALS;
    let is_marker = |t: &str| MARKERS.contains(&t);
    let is_int = is_ascii_int;
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
    // A trailing 4-digit year (1900–2099) is a re-release tag ("Fruits Basket
    // (2019)", "Hunter x Hunter 2011"), not a distinct work — strip it so year
    // variants reduce to the same base.
    let is_year = |t: &str| {
        // `starts_with` rather than the byte slice `&t[..2]` this used to do:
        // the `is_int` guard already made that slice safe, but there is no
        // reason to leave a second byte-index into a title token in the file.
        t.len() == 4 && is_int(t) && (t.starts_with("19") || t.starts_with("20"))
    };
    let mut tokens: Vec<&str> = norm.split_whitespace().collect();
    while let Some(&last) = tokens.last() {
        if is_marker(last) || is_ordinal(last) || ROMANS.contains(&last) || is_year(last) {
            tokens.pop();
        } else if is_int(last) && tokens.len() >= 2 && is_marker(tokens[tokens.len() - 2]) {
            tokens.pop(); // the number
            tokens.pop(); // its preceding marker word
        } else {
            break;
        }
    }
    tokens.join(" ")
}

/// Do two normalized titles reduce to the same franchise base — i.e. are they
/// the same show differing only by a season/part/year suffix? Guarded so the
/// shared base is substantial: either **≥2 tokens** ("attack on titan") or a
/// single token of **≥4 chars** ("bleach", "overlord"). This still admits the
/// many one-word franchises with numbered seasons while rejecting a bare short
/// word ("one", "the", a lone roman numeral) that could collapse unrelated
/// shows. An empty base (two pure season markers) never matches.
fn same_franchise(a: &str, b: &str) -> bool {
    let ba = franchise_base(a);
    let tokens = ba.split_whitespace().count();
    if ba.is_empty() || (tokens < 2 && ba.chars().count() < 4) {
        return false;
    }
    ba == franchise_base(b)
}

/// A key for deduping *unlinked* (no `anilist_id`) rows of the same show across
/// sites in the airing/library unions. Normalizes, strips trailing season/part
/// markers (so "…2nd Season" and "…Temporada 2" collapse), then removes all
/// whitespace (so romanization spacing differences — "Yarikomi-zuki" vs
/// "Yarikomizuki", "Dai Dai Dai" vs "Daidaidai" — collapse too). Two rows with
/// the same key are treated as the same show. Empty only for a title that
/// normalizes to nothing.
pub fn franchise_dedup_key(title: &str) -> String {
    franchise_base(&normalize_title(title)).split_whitespace().collect()
}

/// One title is a strict token-prefix of the other, and the extra tail is a
/// **distinguishing** subtitle/sequel rather than a season/part/year suffix —
/// the pattern of a *different* work that reuses the base name: "Steins;Gate"
/// vs "Steins;Gate 0", "Digimon Adventure" vs "Digimon Adventure 02", "Mobile
/// Suit Gundam" vs "Mobile Suit Gundam Wing". We block these outright, because
/// the raw blend scores a one-word extension high (≈0.76) and would silently
/// import the wrong episodes into a followed show — whereas a miss is harmless
/// (the user re-follows). The test is order-aware: the tail is "distinguishing"
/// exactly when stripping season/part/year markers from the longer title does
/// **not** collapse it back to the shorter one, so a genuine season suffix
/// ("…Final Season", "…Part 2", "…2011") is *not* caught and still matches.
fn distinct_extension(a: &str, b: &str) -> bool {
    let ta: Vec<&str> = a.split_whitespace().collect();
    let tb: Vec<&str> = b.split_whitespace().collect();
    let (short, long, long_norm) = if ta.len() <= tb.len() { (&ta, &tb, b) } else { (&tb, &ta, a) };
    if short.is_empty() || long.len() <= short.len() {
        return false; // not a strict extension
    }
    if short[..] != long[..short.len()] {
        return false; // the shorter title is not a leading token-prefix
    }
    // Distinguishing unless the extra tail is a pure season/part/year suffix.
    franchise_base(long_norm) != short.join(" ")
}

/// Combined score for one (normalized query, normalized candidate) pair.
/// Exact normalized equality short-circuits to 1.0. Otherwise it's the better
/// of a **base** blend (token-Jaccard + Levenshtein, for typos/word-order/
/// partial overlaps) and a **0.9 season-suffix bonus** when the two titles
/// share a franchise base — the same show differing only by a season/part
/// suffix (`same_franchise`), which is the dominant reason library shows
/// failed to match across sites. The bonus is order-aware, so it lifts
/// "Attack on Titan" ↔ "…Final Season" without ever collapsing distinct works
/// like "Steins;Gate" ↔ "Steins;Gate 0" or "…Gundam" ↔ "…Gundam Wing" — which
/// a `distinct_extension` guard actively holds below the threshold even though
/// the raw blend would otherwise pass them.
fn score(query_norm: &str, candidate_norm: &str) -> f64 {
    if query_norm == candidate_norm {
        return 1.0;
    }
    // Distinct name-reusing work ("…Gate" vs "…Gate 0", "…Gundam" vs
    // "…Gundam Wing") → never a match, no matter how high the blend scores. A
    // wrong library merge is silent data corruption; a miss is harmless.
    if distinct_extension(query_norm, candidate_norm) {
        return 0.0;
    }
    let base = jaccard(query_norm, candidate_norm) * 0.6 + levenshtein_ratio(query_norm, candidate_norm) * 0.4;
    // Same show, different season/part suffix → a strong (but sub-exact) score,
    // the fix for cross-site/library matches that a full-set overlap missed.
    if same_franchise(query_norm, candidate_norm) {
        base.max(0.9)
    } else {
        base
    }
}

/// Best candidate for any of `queries` (try romaji, then english — callers
/// pass whichever titles they have), or `None` if nothing clears
/// `MATCH_THRESHOLD`. Each candidate is scored against every query and the
/// max is taken, so a candidate need only match one of the queries well.
pub fn best_match(queries: &[&str], candidates: &[TitleCandidate]) -> Option<MatchResult> {
    let normalized_queries: Vec<String> = queries.iter().map(|q| normalize_title(q)).collect();
    let mut best: Option<MatchResult> = None;
    for (index, candidate) in candidates.iter().enumerate() {
        let candidate_norm = normalize_title(candidate.title);
        let candidate_score = normalized_queries
            .iter()
            .map(|q| score(q, &candidate_norm))
            .fold(0.0_f64, f64::max);
        if candidate_score >= MATCH_THRESHOLD
            && best.as_ref().is_none_or(|b| candidate_score > b.score)
        {
            best = Some(MatchResult { index, score: candidate_score });
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_title_is_public_and_pins_a_known_normalization() {
        // Task 5 (discover-exclude-followed) needs this normalizer outside
        // this module (db.rs, comparing site-series titles against catalog
        // titles) — pin that it's callable as `normalize_title` and that
        // running it twice on the same input is idempotent/stable.
        assert_eq!(normalize_title("Overlord IV"), normalize_title("Overlord IV"));
        assert_eq!(normalize_title("Overlord IV"), "overlord iv");
    }

    #[test]
    fn normalize_strips_accents_case_and_punctuation() {
        assert_eq!(normalize_title("Shingeki no Kyojin"), "shingeki no kyojin");
        assert_eq!(normalize_title("ATTACK ON TITAN"), "attack on titan");
        assert_eq!(normalize_title("Shingeki no Kyojin: The Final Season"), "shingeki no kyojin the final season");
        assert_eq!(normalize_title("Ataque a los Titanes"), "ataque a los titanes");
    }

    #[test]
    fn normalize_strips_trailing_noise_tokens() {
        assert_eq!(normalize_title("Shingeki no Kyojin Sub Español"), "shingeki no kyojin");
        assert_eq!(normalize_title("Baki-dou Latino"), "baki dou");
        assert_eq!(normalize_title("One Piece HD"), "one piece");
        assert_eq!(normalize_title("Naruto Online"), "naruto");
        // Multiple stacked suffixes strip in one normalize_title() call.
        assert_eq!(normalize_title("Bleach Sub Español HD"), "bleach");
    }

    #[test]
    fn normalize_does_not_strip_noise_words_that_are_not_trailing() {
        // "hd" only strips as a genuine trailing token; a title that merely
        // contains "hd"-like substrings mid-string must survive untouched.
        assert_eq!(normalize_title("High and Low"), "high and low");
    }

    #[test]
    fn best_match_picks_the_exact_title_among_decoys() {
        let candidates = vec![
            TitleCandidate { title: "Shingeki no Kyojin Sub Español", url: "u1" },
            TitleCandidate { title: "Shingeki no Kyojin: The Final Season", url: "u2" },
            TitleCandidate { title: "Shingeki no Bahamut", url: "u3" },
        ];
        let result = best_match(&["Shingeki no Kyojin"], &candidates).expect("expected a match");
        assert_eq!(result.index, 0);
        assert_eq!(result.score, 1.0);
    }

    #[test]
    fn best_match_returns_none_below_threshold() {
        let candidates = vec![
            TitleCandidate { title: "Monster Musume", url: "u1" },
            TitleCandidate { title: "Kaiju No. 8", url: "u2" },
        ];
        assert_eq!(best_match(&["Monster"], &candidates), None);
    }

    #[test]
    fn best_match_falls_back_to_english_query_when_romaji_does_not_match() {
        let candidates = vec![
            TitleCandidate { title: "Attack on Titan", url: "u1" },
            TitleCandidate { title: "Totally Unrelated Show", url: "u2" },
        ];
        let result =
            best_match(&["Shingeki no Kyojin", "Attack on Titan"], &candidates).expect("expected a match");
        assert_eq!(result.index, 0);
        assert_eq!(result.score, 1.0);
    }

    #[test]
    fn best_match_prefers_the_higher_scoring_candidate() {
        let candidates = vec![
            TitleCandidate { title: "Kaiju No. 8", url: "u1" },
            TitleCandidate { title: "Baki-dou Sub Español", url: "u2" },
        ];
        let result = best_match(&["Baki-dou"], &candidates).expect("expected a match");
        assert_eq!(result.index, 1);
    }

    #[test]
    fn best_match_empty_candidates_is_none() {
        assert_eq!(best_match(&["Anything"], &[]), None);
    }

    fn catalog_row(id: i64, popularity: i64, titles: &[&str]) -> CatalogTitleRow {
        CatalogTitleRow {
            id,
            titles: titles.iter().map(|t| t.to_string()).collect(),
            popularity,
        }
    }

    #[test]
    fn catalog_index_matches_any_stored_title_variant() {
        let rows = vec![catalog_row(16498, 700, &["Attack on Titan", "Shingeki no Kyojin"])];
        let index = CatalogIndex::build(&rows);
        assert_eq!(index.lookup(&["Shingeki no Kyojin"]), Some(16498));
        assert_eq!(index.lookup(&["ATTACK ON TITAN"]), Some(16498), "normalization is case-insensitive");
        assert_eq!(index.lookup(&["Attack on Titan Sub Español"]), Some(16498), "site noise suffix stripped");
    }

    #[test]
    fn catalog_index_prefers_the_popular_row_when_titles_collide() {
        // Remakes/shorts routinely reuse a show's exact name; the popular
        // entry is the one a user means.
        let rows = vec![
            catalog_row(999, 12, &["Hunter x Hunter"]),
            catalog_row(11061, 500_000, &["Hunter x Hunter"]),
        ];
        assert_eq!(CatalogIndex::build(&rows).lookup(&["Hunter x Hunter"]), Some(11061));
    }

    #[test]
    fn catalog_index_collision_is_order_independent() {
        let a = catalog_row(999, 12, &["Hunter x Hunter"]);
        let b = catalog_row(11061, 500_000, &["Hunter x Hunter"]);
        let forward = CatalogIndex::build(&[
            catalog_row(a.id, a.popularity, &["Hunter x Hunter"]),
            catalog_row(b.id, b.popularity, &["Hunter x Hunter"]),
        ]);
        let reversed = CatalogIndex::build(&[b, a]);
        assert_eq!(forward.lookup(&["Hunter x Hunter"]), reversed.lookup(&["Hunter x Hunter"]));
    }

    #[test]
    fn catalog_index_tries_candidates_most_specific_first() {
        let rows = vec![
            catalog_row(21, 600_000, &["ONE PIECE"]),
            catalog_row(459, 1000, &["One Piece Film: Red"]),
        ];
        let index = CatalogIndex::build(&rows);
        // The exact title wins over the more general fallback form.
        assert_eq!(index.lookup(&["One Piece Film: Red", "One Piece"]), Some(459));
        // A site arc row has no exact entry and falls through to the base name.
        assert_eq!(index.lookup(&["One Piece: Arco de Elbaph", "One Piece"]), Some(21));
    }

    #[test]
    fn catalog_index_reports_no_match_rather_than_a_guess() {
        let rows = vec![catalog_row(21, 600_000, &["ONE PIECE"])];
        let index = CatalogIndex::build(&rows);
        assert_eq!(index.lookup(&["Totally Different Show"]), None);
        // A near-miss is still a miss — this index never fuzzy-matches.
        assert_eq!(index.lookup(&["One Piece Film"]), None);
    }

    #[test]
    fn fuzzy_lookup_links_cross_language_and_season_variants() {
        let rows = vec![
            catalog_row(209983, 5000, &["Hell Mode: Yarikomizuki no Gamer wa Hai Settei no Isekai de Musou suru"]),
            catalog_row(21, 900000, &["One Piece"]),
        ];
        let index = CatalogIndex::build(&rows);
        // Season-suffix variant a site adds ("Temporada 2") still resolves.
        assert_eq!(
            index.fuzzy_lookup(&["Hell Mode: Yarikomizuki no Gamer wa Hai Settei no Isekai de Musou suru Temporada 2"]),
            Some(209983)
        );
        // A completely different show is not force-linked.
        assert_eq!(index.fuzzy_lookup(&["Totally Unrelated Mecha Show"]), None);
    }

    #[test]
    fn franchise_dedup_key_collapses_season_and_spacing_variants() {
        // Season marker in two languages → same key.
        assert_eq!(
            franchise_dedup_key("Hell Mode: Yarikomizuki no Gamer 2nd Season"),
            franchise_dedup_key("Hell Mode: Yarikomizuki no Gamer Temporada 2"),
        );
        // Romanization spacing differences → same key.
        assert_eq!(
            franchise_dedup_key("Kimi no Koto ga Dai Dai Daisuki"),
            franchise_dedup_key("Kimi no Koto ga Daidaidaisuki"),
        );
        // A distinguishing sequel word is NOT a season marker → distinct keys.
        assert_ne!(franchise_dedup_key("Naruto"), franchise_dedup_key("Naruto Shippuden"));
    }

    #[test]
    fn fuzzy_lookup_does_not_link_a_mere_word_overlap() {
        let rows = vec![catalog_row(21, 900000, &["One Piece"])];
        let index = CatalogIndex::build(&rows);
        // Shares the word "piece" but is a different work — must stay unlinked.
        assert_eq!(index.fuzzy_lookup(&["A Piece of Cake at the Bakery"]), None);
    }

    #[test]
    fn catalog_index_ignores_titles_that_normalize_to_nothing() {
        let rows = vec![catalog_row(1, 0, &["", "   ", "!!!"])];
        let index = CatalogIndex::build(&rows);
        assert!(index.is_empty());
        assert_eq!(index.lookup(&[""]), None);
    }

    // ---- Corpus: does one title match the other, as the library importer sees
    // it? `best_match` with a single candidate returns Some iff the pair clears
    // MATCH_THRESHOLD, so this is exactly the "are these the same show?"
    // decision the importer makes.
    fn same_show(a: &str, b: &str) -> bool {
        best_match(&[a], &[TitleCandidate { title: b, url: "u" }]).is_some()
    }

    /// Recall corpus: pairs that ARE the same show and MUST match. These are
    /// the real-world variants a cross-site library import trips over — season
    /// and part suffixes (both worded and Roman), punctuation, romaji↔spelling
    /// noise, and year re-releases.
    const SHOULD_MATCH: &[(&str, &str)] = &[
        // Worded season / part / final-season suffixes.
        ("Attack on Titan", "Attack on Titan Final Season"),
        ("Attack on Titan", "Attack on Titan The Final Season"),
        ("Shingeki no Kyojin", "Shingeki no Kyojin The Final Season"),
        ("Jujutsu Kaisen", "Jujutsu Kaisen 2nd Season"),
        ("Spy x Family", "Spy x Family Part 2"),
        ("Spy x Family", "Spy x Family Season 2"),
        ("Demon Slayer", "Demon Slayer Season 3"),
        ("Kimetsu no Yaiba", "Kimetsu no Yaiba Season 2"),
        ("The Rising of the Shield Hero", "The Rising of the Shield Hero Season 2"),
        ("Kaguya-sama: Love is War", "Kaguya-sama Love is War Season 3"),
        ("My Hero Academia", "My Hero Academia Season 6"),
        ("Black Clover", "Black Clover Season 4"),
        ("Fire Force", "Fire Force Season 2"),
        ("The Promised Neverland", "The Promised Neverland Season 2"),
        ("Dr. Stone", "Dr. Stone Season 3"),
        ("Made in Abyss", "Made in Abyss Season 2"),
        ("Re:Zero", "Re:Zero Season 2"),
        ("Vinland Saga", "Vinland Saga Season 2"),
        ("Tokyo Revengers", "Tokyo Revengers Season 2"),
        ("Chainsaw Man", "Chainsaw Man Part 2"),
        // (Sword Art Online is intentionally absent: "online" is a site
        // noise-suffix that normalize_title strips, a separate normalizer quirk
        // outside this metric's scope.)
        ("The Eminence in Shadow", "The Eminence in Shadow 2nd Season"),
        ("Classroom of the Elite", "Classroom of the Elite Season 3"),
        ("Dr. Stone", "Dr. Stone Final Season"),
        ("Bocchi the Rock", "Bocchi the Rock Season 2"),
        // Roman-numeral season markers.
        ("Overlord", "Overlord IV"),
        ("Mob Psycho 100", "Mob Psycho 100 II"),
        ("Saekano", "Saekano II"),
        ("Konosuba", "Konosuba III"),
        ("Golden Kamuy", "Golden Kamuy IV"),
        // Cour split with a marker word in front of the number.
        ("Chainsaw Man", "Chainsaw Man Cour 2"),
        // Year re-release (4-digit → never treated as a distinct sequel).
        ("Hunter x Hunter", "Hunter x Hunter (2011)"),
        ("Fruits Basket", "Fruits Basket (2019)"),
        ("Bleach", "Bleach 2004"),
        // Punctuation / casing / spacing only.
        ("Fullmetal Alchemist: Brotherhood", "Fullmetal Alchemist Brotherhood"),
        ("Kaguya-sama wa Kokurasetai", "Kaguya sama wa Kokurasetai"),
        ("Kono Subarashii Sekai ni Shukufuku wo!", "Kono Subarashii Sekai ni Shukufuku wo"),
        ("That Time I Got Reincarnated as a Slime", "That Time I Got Reincarnated as a Slime"),
        ("JOJO'S BIZARRE ADVENTURE", "jojo's bizarre adventure"),
        // Site noise suffixes (stripped by normalize_title before scoring).
        ("One Piece", "One Piece Sub Español"),
        ("Naruto Shippuden", "Naruto Shippuden HD"),
        ("Boruto", "Boruto Latino"),
        // Worded season on a longer base.
        ("Rurouni Kenshin", "Rurouni Kenshin Season 2"),
        ("Dragon Ball Super", "Dragon Ball Super Season 2"),
        ("One Punch Man", "One Punch Man Season 2"),
        ("Haikyuu", "Haikyuu Season 4"),
        ("Mushoku Tensei", "Mushoku Tensei Part 2"),
        ("Mushoku Tensei", "Mushoku Tensei Season 2"),
        ("Oshi no Ko", "Oshi no Ko 2nd Season"),
        ("Frieren Beyond Journey's End", "Frieren Beyond Journey's End Season 2"),
        ("The Apothecary Diaries", "The Apothecary Diaries Season 2"),
        ("Solo Leveling", "Solo Leveling Season 2"),
        ("Blue Lock", "Blue Lock Season 2"),
        ("Dandadan", "Dandadan Season 2"),
        ("Wind Breaker", "Wind Breaker Season 2"),
        ("Tower of God", "Tower of God Season 2"),
        ("Kaiju No. 8", "Kaiju No. 8 Season 2"),
        ("Delicious in Dungeon", "Delicious in Dungeon Season 2"),
        ("My Hero Academia", "My Hero Academia Final Season"),
        ("Jujutsu Kaisen", "Jujutsu Kaisen Season 2"),
        ("Attack on Titan", "Attack on Titan Season 2"),
    ];

    /// Safety corpus: pairs that share tokens but are DIFFERENT works, and must
    /// NOT match — a false merge silently imports the wrong episodes. The nasty
    /// ones are name-reusing sequels ("Steins;Gate 0"), same-prefix distinct
    /// shows ("Fate/Zero" vs "Fate/stay night"), and short titles that overlap
    /// on one common word.
    const SHOULD_NOT_MATCH: &[(&str, &str)] = &[
        // Name-reusing distinct sequels — the bare-number trap.
        ("Steins;Gate", "Steins;Gate 0"),
        ("Digimon Adventure", "Digimon Adventure 02"),
        ("Ghost in the Shell", "Ghost in the Shell 2"),
        ("Psycho-Pass", "Psycho-Pass 2"),
        ("Initial D", "Initial D 2"),
        // Same franchise prefix, genuinely different works.
        ("Fate/Zero", "Fate/stay night"),
        ("Fate/stay night", "Fate/Apocrypha"),
        ("Fate/Zero", "Fate/Grand Order"),
        ("Code Geass", "Code:Breaker"),
        ("Mobile Suit Gundam", "Mobile Suit Gundam Wing"),
        ("Gundam Seed", "Gundam Wing"),
        ("Persona 4", "Persona 5"),
        ("Digimon Adventure", "Digimon Tamers"),
        ("Digimon Adventure", "Digimon Frontier"),
        ("Precure", "Heartcatch Precure"),
        // Share exactly one common word — must never carry a match.
        ("Attack on Titan", "Attack No. 1"),
        ("One Piece", "One Punch Man"),
        ("One Piece", "One Room"),
        ("Black Clover", "Black Lagoon"),
        ("Black Butler", "Black Clover"),
        ("Fire Force", "Fairy Tail"),
        ("Blue Lock", "Blue Exorcist"),
        ("Blue Period", "Blue Lock"),
        ("Dragon Ball", "Dragon Quest"),
        ("Sword Art Online", "Sword of the Stranger"),
        ("Tokyo Ghoul", "Tokyo Revengers"),
        ("Tokyo Ghoul", "Tokyo Mew Mew"),
        ("Monster", "Monster Musume"),
        ("Bakemonogatari", "Nisemonogatari"),
        ("Golden Kamuy", "Golden Boy"),
        ("Hell's Paradise", "Hell Girl"),
        ("The Seven Deadly Sins", "The Ancient Magus' Bride"),
        // Completely unrelated titles.
        ("Naruto", "Bleach"),
        ("Death Note", "Code Geass"),
        ("Cowboy Bebop", "Samurai Champloo"),
        ("Spy x Family", "Sky Wizards Academy"),
        ("Chainsaw Man", "Chained Soldier"),
        ("Jujutsu Kaisen", "Jubei-chan"),
        ("Demon Slayer", "Demon King Daimao"),
        ("Made in Abyss", "Maid Sama"),
        ("High School DxD", "High School of the Dead"),
        ("Boku no Hero Academia", "Boku no Pico"),
        ("Gundam Seed", "Gundam Seed Destiny"),
    ];

    #[test]
    fn corpus_recall_same_show_variants_match() {
        let misses: Vec<_> = SHOULD_MATCH
            .iter()
            .filter(|(a, b)| !same_show(a, b))
            .collect();
        assert!(
            misses.is_empty(),
            "these same-show pairs failed to match (recall gap): {misses:#?}"
        );
    }

    #[test]
    fn corpus_safety_distinct_works_do_not_match() {
        let false_merges: Vec<_> = SHOULD_NOT_MATCH
            .iter()
            .filter(|(a, b)| same_show(a, b))
            .collect();
        assert!(
            false_merges.is_empty(),
            "these DISTINCT works were wrongly merged (unsafe false positives): {false_merges:#?}"
        );
    }

    #[test]
    fn corpus_is_large_and_symmetric() {
        // The user asked for a 100+ title check; keep it honest.
        assert!(
            SHOULD_MATCH.len() + SHOULD_NOT_MATCH.len() >= 100,
            "corpus shrank below 100 pairs"
        );
        // Matching must not depend on argument order.
        for (a, b) in SHOULD_MATCH.iter().chain(SHOULD_NOT_MATCH) {
            assert_eq!(same_show(a, b), same_show(b, a), "asymmetric verdict for ({a:?}, {b:?})");
        }
    }

    /// Regression guard for the char-boundary panic that aborted the whole
    /// process. Each title is copied verbatim out of the live `anilist_catalog`
    /// / `series` tables, and each normalizes to a token whose `len() - 2` byte
    /// index falls **inside** a multi-byte character — exactly what the old
    /// `is_ordinal` (`&t[t.len() - 2..]`) sliced on, panicking. Because
    /// `franchise_base` is reached from synchronous `#[tauri::command]`s, which
    /// Tauri runs on the main thread from a COM/FFI callback, that panic could
    /// not unwind and aborted the process (Windows 0xC0000409,
    /// FAST_FAIL_FATAL_APP_EXIT) — the "app closes when the sync finishes" bug.
    ///
    /// The multi-byte char has to be in the **trailing** token: the strip loop
    /// inspects `tokens.last()` and breaks at the first token that is not a
    /// marker, so a title like "Episodeⁿ | Kuusou" (multi-byte in a leading
    /// token) never reached `is_ordinal` and never panicked. A regression test
    /// built on those would pass just as happily against the buggy code.
    ///
    /// The second field is the ASCII core that must survive the reduction, so
    /// the assertion is specific per title rather than an any-of-these chain
    /// that a single title could satisfy on behalf of all six.
    const MULTIBYTE_TITLES: &[(&str, &str)] = &[
        // Real catalog rows. Ⅱ = U+2161, lowercasing to ⅱ (U+2171): the whole
        // trailing token is one 3-byte char, so the old `&t[t.len() - 2..]`
        // sliced straight through the middle of it.
        ("Long Sword Ⅱ", "longsword"),
        ("The Morose Mononokean Ⅱ", "mononokean"),
        ("Zhu Dick: Guguai Dao Da Maoxian Ⅰ", "maoxian"),
        // Same shape, other scripts present in the catalog, moved into the
        // trailing position so they actually exercise the loop: fullwidth Ａ
        // (U+FF21), superscript ⁿ (U+207F), vulgar fraction ⅐ (U+2150).
        ("Galaxy Angel Ａ", "angel"),
        ("Kuusou Episodeⁿ", "kuusou"),
        ("Tom Thumb 00⅐", "thumb"),
    ];

    #[test]
    fn multibyte_titles_do_not_panic_in_franchise_dedup_key() {
        for (title, core) in MULTIBYTE_TITLES {
            // Not panicking IS the assertion here — the old byte-slicing
            // version aborted the process on every one of these.
            let key = franchise_dedup_key(title);
            assert!(
                key.contains(core),
                "expected {core:?} to survive in {title:?} -> {key:?}"
            );
        }
    }

    #[test]
    fn multibyte_titles_do_not_panic_in_normalize_title() {
        for (title, _) in MULTIBYTE_TITLES {
            // normalize_title itself is char-safe, but pin it over these exact
            // inputs so a regression there (or in anything it calls) is caught.
            let norm = normalize_title(title);
            assert!(!norm.is_empty(), "normalize_title reduced {title:?} to empty");
        }
    }

    #[test]
    fn distinct_extension_guard_is_targeted() {
        // Fires: a distinguishing tail (bare number or a subtitle word).
        assert!(distinct_extension("steins gate", "steins gate 0"));
        assert!(distinct_extension("digimon adventure", "digimon adventure 02"));
        assert!(distinct_extension("mobile suit gundam", "mobile suit gundam wing"));
        // Does not fire: a pure season index (number after a marker)…
        assert!(!distinct_extension("spy x family", "spy x family part 2"));
        assert!(!distinct_extension("spy x family", "spy x family season 2"));
        // …a 4-digit year…
        assert!(!distinct_extension("hunter x hunter", "hunter x hunter 2011"));
        // …a Roman-numeral / worded season…
        assert!(!distinct_extension("overlord", "overlord iv"));
        assert!(!distinct_extension("attack on titan", "attack on titan final season"));
        // …and not when neither is a prefix of the other.
        assert!(!distinct_extension("fate zero", "fate stay night"));
    }
}
