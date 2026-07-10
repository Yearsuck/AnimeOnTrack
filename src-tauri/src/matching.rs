//! Fuzzy title matching between AniList catalog titles (romaji/english) and
//! site search-result titles (Spanish/romaji, with noise like "Sub Español",
//! season suffixes, punctuation). Pure, no I/O — see
//! `commands::link_catalog_series` for the caller that feeds it real search
//! results.

use std::collections::HashSet;

/// One candidate title scraped off a search-results page.
pub struct TitleCandidate<'a> {
    pub title: &'a str,
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
fn normalize(s: &str) -> String {
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

/// Combined score for one (normalized query, normalized candidate) pair.
/// Exact normalized equality always short-circuits to 1.0 (handles e.g. a
/// query matching a candidate only after the candidate's noise suffix is
/// stripped, which token-Jaccard/Levenshtein alone would still score high
/// but not perfectly).
fn score(query_norm: &str, candidate_norm: &str) -> f64 {
    if query_norm == candidate_norm {
        return 1.0;
    }
    jaccard(query_norm, candidate_norm) * 0.6 + levenshtein_ratio(query_norm, candidate_norm) * 0.4
}

/// Best candidate for any of `queries` (try romaji, then english — callers
/// pass whichever titles they have), or `None` if nothing clears
/// `MATCH_THRESHOLD`. Each candidate is scored against every query and the
/// max is taken, so a candidate need only match one of the queries well.
pub fn best_match(queries: &[&str], candidates: &[TitleCandidate]) -> Option<MatchResult> {
    let normalized_queries: Vec<String> = queries.iter().map(|q| normalize(q)).collect();
    let mut best: Option<MatchResult> = None;
    for (index, candidate) in candidates.iter().enumerate() {
        let candidate_norm = normalize(candidate.title);
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
    fn normalize_strips_accents_case_and_punctuation() {
        assert_eq!(normalize("Shingeki no Kyojin"), "shingeki no kyojin");
        assert_eq!(normalize("ATTACK ON TITAN"), "attack on titan");
        assert_eq!(normalize("Shingeki no Kyojin: The Final Season"), "shingeki no kyojin the final season");
        assert_eq!(normalize("Ataque a los Titanes"), "ataque a los titanes");
    }

    #[test]
    fn normalize_strips_trailing_noise_tokens() {
        assert_eq!(normalize("Shingeki no Kyojin Sub Español"), "shingeki no kyojin");
        assert_eq!(normalize("Baki-dou Latino"), "baki dou");
        assert_eq!(normalize("One Piece HD"), "one piece");
        assert_eq!(normalize("Naruto Online"), "naruto");
        // Multiple stacked suffixes strip in one normalize() call.
        assert_eq!(normalize("Bleach Sub Español HD"), "bleach");
    }

    #[test]
    fn normalize_does_not_strip_noise_words_that_are_not_trailing() {
        // "hd" only strips as a genuine trailing token; a title that merely
        // contains "hd"-like substrings mid-string must survive untouched.
        assert_eq!(normalize("High and Low"), "high and low");
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
}
