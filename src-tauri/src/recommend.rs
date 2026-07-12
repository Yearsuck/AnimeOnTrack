//! Pure taste-scoring for the Descubrir catalog deck's inner (per-genre)
//! pick. `commands::discover_catalog_card` already does the *outer* pick —
//! which genre to browse — via `get_genre_affinity` + `swipe::weighted_pick_index`.
//! This module scores the `survivors` batch `db::random_catalog_anime_in_genre`
//! gathers for that chosen genre (already filtered for exclusions/bans/quality
//! floor — see its doc comment) so the *inner* pick favors what the user
//! actually watches instead of being uniformly random.
//!
//! Deliberately DB/State-free: every fn here takes plain values (a
//! `CatalogAnime` slice, `HashMap<String, f64>` affinity maps) so the scoring
//! logic is unit-testable without a `rusqlite::Connection` or
//! `tauri::State`. See
//! docs/superpowers/specs/2026-07-12-discover-recommendation-engine-design.md.

use crate::anilist::CatalogAnime;
use std::collections::HashMap;

/// Exponent applied to a raw (positive) genre-affinity score before it's used
/// as an outer-pick weight. An exponent in (0,1) is sub-linear: it compresses
/// large leads while preserving relative order (score 20 vs 4 becomes
/// 20^0.6≈6.03 vs 4^0.6≈2.30 — 4x becomes ~2.6x), so one heavily-followed
/// genre no longer swamps every other candidate genre in
/// `weighted_pick_index`. 0.6 is a middle ground: closer to 1.0 would barely
/// dampen; closer to 0 would flatten taste signal into near-uniform. Chosen
/// so the "no single-genre monopoly" property (see
/// `dampened_share_never_monopolizes` below) holds for realistic follow
/// distributions while a strong genre still wins more often than not.
pub const GENRE_WEIGHT_EXPONENT: f64 = 0.6;

/// Per-candidate score weights (see `score_candidate`). Genre overlap leads
/// (the strongest taste signal); format is a meaningful secondary nudge
/// toward the shape of show this user actually finishes; quality is a mild
/// tiebreak only — `average_score` is capped to `[0,1]` after the `/100.0`
/// normalization, so at `W_QUALITY=0.25` it can never outweigh even a single
/// dampened genre-affinity point, only break ties between otherwise-similar
/// candidates.
pub const W_GENRE: f64 = 1.0;
pub const W_FORMAT: f64 = 0.6;
pub const W_QUALITY: f64 = 0.25;

/// How many top-scored survivors `pick_recommended` samples from. Kept small
/// so the deck stays taste-biased, kept >1 so the same top candidate isn't
/// shown every single time until decided (the deck would feel repetitive/dead
/// otherwise — see spec section 4).
pub const RECOMMEND_TOP_K: usize = 8;

/// Small positive floor added to every top-k weight before
/// `swipe::weighted_pick_index` — without it, a 0-scored candidate (cold
/// start, or a candidate with literally no signal) would never be sampled
/// even though it's in the top-k (weight <= 0 is treated as ineligible by
/// `weighted_pick_index`). Negligible next to any real score.
const TOP_K_EPSILON: f64 = 0.01;

/// Sub-linear dampening for one genre's raw affinity score (see
/// `GENRE_WEIGHT_EXPONENT`). Non-positive scores (never followed/wanted, or a
/// net-discarded genre) stay exactly 0 — dampening only ever compresses a
/// *positive* lead, it never turns a disliked genre into a fake positive
/// weight, so `weighted_pick_index`'s cold-start/all-non-positive uniform
/// fallback is unaffected.
pub fn dampen_genre_weight(score: f64) -> f64 {
    if score <= 0.0 {
        0.0
    } else {
        score.powf(GENRE_WEIGHT_EXPONENT)
    }
}

/// Build a `[0,1]`-normalized format-affinity map from `db::get_type_stats`'s
/// output (followed-series count per `kind`, descending). The most-watched
/// format gets weight 1.0, others scale proportionally; a brand-new user with
/// no follows yields an empty map (every `unwrap_or(0.0)` lookup in
/// `score_candidate` then contributes exactly 0 — pure genre/quality
/// behavior, matching the spec's cold-start requirement).
pub fn format_affinity_from_type_stats(stats: &[crate::models::TypeStat]) -> HashMap<String, f64> {
    let max_count = stats.iter().map(|s| s.count).max().unwrap_or(0);
    if max_count <= 0 {
        return HashMap::new();
    }
    stats.iter().map(|s| (s.kind.clone(), s.count as f64 / max_count as f64)).collect()
}

/// Per-candidate taste score. Pure — no DB/State, so it's directly
/// unit-testable. `chosen_genre` (the outer weighted pick that produced this
/// `survivors` batch) is excluded from the genre-overlap sum so the score
/// rewards *secondary* overlap with the user's other tastes rather than just
/// re-crediting the genre already used to find this candidate.
pub fn score_candidate(
    cand: &CatalogAnime,
    genre_affinity: &HashMap<String, f64>,
    format_affinity: &HashMap<String, f64>,
    chosen_genre: &str,
) -> f64 {
    let genre_term: f64 = cand
        .genres
        .iter()
        .filter(|g| !g.eq_ignore_ascii_case(chosen_genre))
        .map(|g| dampen_genre_weight(*genre_affinity.get(g.as_str()).unwrap_or(&0.0)))
        .sum();
    let format_term = cand
        .format
        .as_deref()
        .and_then(|f| format_affinity.get(f))
        .copied()
        .unwrap_or(0.0);
    let quality_term = cand.average_score.unwrap_or(0) as f64 / 100.0;
    W_GENRE * genre_term + W_FORMAT * format_term + W_QUALITY * quality_term
}

/// Rank `survivors` (already exclusion/ban/quality-filtered by the caller —
/// see `db::random_catalog_anime_in_genre`'s doc comment) by
/// `score_candidate` descending, keep the top `RECOMMEND_TOP_K`, and pick
/// among *those* with `swipe::weighted_pick_index` biased by score. Never
/// invents a candidate: the result, when `Some`, is always one of the values
/// passed in `survivors` — so any exclusion/ban already applied upstream
/// (engaged titles, banned genres/formats) can never resurface here no matter
/// how it would have scored. Cold start (both affinity maps empty, every
/// score reduces to the small quality term or 0) degrades to a
/// near-uniform weighted pick over the whole batch (or all of it, if
/// `survivors.len() <= RECOMMEND_TOP_K`) — never panics on an empty score
/// distribution because of `TOP_K_EPSILON`.
pub fn pick_recommended(
    survivors: Vec<CatalogAnime>,
    genre_affinity: &HashMap<String, f64>,
    format_affinity: &HashMap<String, f64>,
    chosen_genre: &str,
) -> Option<CatalogAnime> {
    if survivors.is_empty() {
        return None;
    }
    let mut scored: Vec<(f64, CatalogAnime)> = survivors
        .into_iter()
        .map(|c| (score_candidate(&c, genre_affinity, format_affinity, chosen_genre), c))
        .collect();
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    scored.truncate(RECOMMEND_TOP_K);

    let weights: Vec<f64> = scored.iter().map(|(s, _)| s + TOP_K_EPSILON).collect();
    let idx = crate::swipe::weighted_pick_index(&weights)?;
    scored.into_iter().nth(idx).map(|(_, c)| c)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn anime(id: i64, title: &str, genres: &[&str], format: &str, average_score: Option<i64>) -> CatalogAnime {
        CatalogAnime {
            id,
            title: title.into(),
            title_romaji: None,
            title_english: None,
            cover_url: None,
            format: Some(format.into()),
            genres: genres.iter().map(|g| g.to_string()).collect(),
            episodes: Some(12),
            average_score,
            popularity: Some(1000),
            url: format!("https://anilist.co/anime/{id}"),
        }
    }

    // --- dampen_genre_weight -------------------------------------------

    #[test]
    fn dampen_genre_weight_zeroes_non_positive_scores() {
        assert_eq!(dampen_genre_weight(0.0), 0.0);
        assert_eq!(dampen_genre_weight(-3.0), 0.0);
    }

    #[test]
    fn dampen_genre_weight_compresses_but_preserves_order() {
        let small = dampen_genre_weight(4.0);
        let big = dampen_genre_weight(20.0);
        // Order preserved.
        assert!(big > small);
        // But compressed: raw ratio is 5x, dampened ratio must be less.
        let raw_ratio = 20.0 / 4.0;
        let dampened_ratio = big / small;
        assert!(dampened_ratio < raw_ratio);
    }

    #[test]
    fn dampened_share_never_monopolizes() {
        // One dominant genre (score 20) vs three moderate ones (score 4 each).
        // Raw weights: the dominant genre's share of the total is 20/32 = 0.625.
        // That alone isn't over a naive 0.7 "monopoly" bar, so make the
        // imbalance starker to actually exercise the property meaningfully:
        // dominant=50, moderates=4,4,4 -> raw share = 50/62 = 0.806 (collapse).
        let raw = [50.0, 4.0, 4.0, 4.0];
        let raw_total: f64 = raw.iter().sum();
        let raw_share = raw[0] / raw_total;
        assert!(raw_share >= 0.7, "test setup should actually collapse under raw weights");

        let dampened: Vec<f64> = raw.iter().map(|w| dampen_genre_weight(*w)).collect();
        let dampened_total: f64 = dampened.iter().sum();
        let dampened_share = dampened[0] / dampened_total;
        assert!(
            dampened_share < 0.7,
            "dampened share {dampened_share} should drop below the collapse threshold (was {raw_share} raw)"
        );
        // Still the largest share (order preserved, not flattened to uniform).
        assert!(dampened_share > 1.0 / raw.len() as f64);
    }

    // --- format_affinity_from_type_stats ---------------------------------

    #[test]
    fn format_affinity_normalizes_to_max_one() {
        let stats = vec![
            crate::models::TypeStat { kind: "TV".into(), count: 10 },
            crate::models::TypeStat { kind: "MOVIE".into(), count: 5 },
        ];
        let aff = format_affinity_from_type_stats(&stats);
        assert_eq!(aff.get("TV"), Some(&1.0));
        assert_eq!(aff.get("MOVIE"), Some(&0.5));
    }

    #[test]
    fn format_affinity_empty_for_no_follows() {
        let aff = format_affinity_from_type_stats(&[]);
        assert!(aff.is_empty());
    }

    // --- score_candidate ---------------------------------------------------

    #[test]
    fn taste_matching_genres_outscore_no_signal_genres() {
        let mut genre_affinity = HashMap::new();
        genre_affinity.insert("Fantasy".to_string(), 4.0);
        let format_affinity = HashMap::new();

        // Both candidates are in the chosen genre "Action" (excluded from
        // scoring); "Loved" additionally carries "Fantasy" (liked), "Cold"
        // carries an unrelated genre with no affinity signal.
        let loved = anime(1, "Loved", &["Action", "Fantasy"], "TV", Some(50));
        let cold = anime(2, "Cold", &["Action", "Slice of Life"], "TV", Some(50));

        let loved_score = score_candidate(&loved, &genre_affinity, &format_affinity, "Action");
        let cold_score = score_candidate(&cold, &genre_affinity, &format_affinity, "Action");
        assert!(loved_score > cold_score, "loved={loved_score} cold={cold_score}");
    }

    #[test]
    fn chosen_genre_itself_does_not_count_toward_its_own_score() {
        // A candidate whose ONLY genre is the already-chosen outer genre gets
        // zero genre_term, even if that genre has a huge affinity — overlap
        // must be with something ADDITIONAL to be rewarded.
        let mut genre_affinity = HashMap::new();
        genre_affinity.insert("Action".to_string(), 100.0);
        let format_affinity = HashMap::new();
        let cand = anime(1, "OnlyAction", &["Action"], "TV", Some(0));
        assert_eq!(score_candidate(&cand, &genre_affinity, &format_affinity, "Action"), 0.0);
    }

    #[test]
    fn format_affinity_changes_ranking_between_equal_genre_candidates() {
        let genre_affinity = HashMap::new(); // no genre signal at all
        let mut format_affinity = HashMap::new();
        format_affinity.insert("TV".to_string(), 1.0);
        format_affinity.insert("MOVIE".to_string(), 0.1);

        let tv = anime(1, "TVShow", &["Action"], "TV", Some(50));
        let movie = anime(2, "MovieShow", &["Action"], "MOVIE", Some(50));

        let tv_score = score_candidate(&tv, &genre_affinity, &format_affinity, "Action");
        let movie_score = score_candidate(&movie, &genre_affinity, &format_affinity, "Action");
        assert!(tv_score > movie_score, "tv={tv_score} movie={movie_score}");
    }

    #[test]
    fn quality_term_is_a_mild_tiebreak_not_a_dominant_signal() {
        // A candidate with strong secondary-genre overlap but low score must
        // still beat a candidate with zero overlap and a perfect score,
        // proving quality can't override genre/format taste signal.
        let mut genre_affinity = HashMap::new();
        genre_affinity.insert("Fantasy".to_string(), 4.0);
        let format_affinity = HashMap::new();

        let tasteful_low_quality = anime(1, "TastefulButMediocre", &["Action", "Fantasy"], "TV", Some(1));
        let tasteless_perfect = anime(2, "PerfectButUnrelated", &["Action", "Horror"], "TV", Some(100));

        let a = score_candidate(&tasteful_low_quality, &genre_affinity, &format_affinity, "Action");
        let b = score_candidate(&tasteless_perfect, &genre_affinity, &format_affinity, "Action");
        assert!(a > b, "tasteful={a} tasteless={b}");
    }

    // --- pick_recommended ---------------------------------------------------

    #[test]
    fn pick_recommended_none_for_empty_survivors() {
        let genre_affinity = HashMap::new();
        let format_affinity = HashMap::new();
        assert!(pick_recommended(vec![], &genre_affinity, &format_affinity, "Action").is_none());
    }

    #[test]
    fn pick_recommended_never_returns_a_title_outside_survivors() {
        // Exclusions/bans are applied by the caller BEFORE building the
        // survivors vec (db::random_catalog_anime_in_genre already filters
        // engaged/banned titles out) — this asserts pick_recommended itself
        // can never surface anything beyond what it was handed, so an
        // excluded/banned title that was correctly kept OUT of `survivors`
        // can never come back via scoring.
        let genre_affinity = HashMap::new();
        let format_affinity = HashMap::new();
        let allowed_titles: std::collections::HashSet<&str> = ["A", "B", "C"].into_iter().collect();
        let survivors = vec![
            anime(1, "A", &["Action"], "TV", Some(10)),
            anime(2, "B", &["Action"], "TV", Some(90)),
            anime(3, "C", &["Action"], "TV", Some(50)),
        ];
        for _ in 0..30 {
            let picked = pick_recommended(survivors.clone(), &genre_affinity, &format_affinity, "Action").unwrap();
            assert!(allowed_titles.contains(picked.title.as_str()));
        }
    }

    #[test]
    fn pick_recommended_cold_start_does_not_panic_and_returns_a_survivor() {
        // Brand-new user: both affinity maps empty (nothing followed/decided
        // yet). Must degrade gracefully to a non-panicking pick rather than
        // erroring or always returning the same index out of bounds.
        let genre_affinity = HashMap::new();
        let format_affinity = HashMap::new();
        let survivors = vec![
            anime(1, "A", &["Action"], "TV", None),
            anime(2, "B", &["Drama"], "MOVIE", None),
        ];
        for _ in 0..30 {
            let picked =
                pick_recommended(survivors.clone(), &genre_affinity, &format_affinity, "Action").unwrap();
            assert!(picked.title == "A" || picked.title == "B");
        }
    }

    #[test]
    fn pick_recommended_caps_sampling_pool_to_top_k() {
        // Build more than RECOMMEND_TOP_K survivors with strictly descending
        // scores via distinct average_score; the lowest-scored tail (beyond
        // top-k) must never be picked.
        let genre_affinity = HashMap::new();
        let format_affinity = HashMap::new();
        let n = RECOMMEND_TOP_K + 5;
        let survivors: Vec<CatalogAnime> = (0..n)
            .map(|i| anime(i as i64, &format!("T{i}"), &["Action"], "TV", Some((n - i) as i64)))
            .collect();
        let excluded_tail: std::collections::HashSet<String> =
            (RECOMMEND_TOP_K..n).map(|i| format!("T{i}")).collect();
        for _ in 0..50 {
            let picked = pick_recommended(survivors.clone(), &genre_affinity, &format_affinity, "Action").unwrap();
            assert!(!excluded_tail.contains(&picked.title), "picked out-of-top-k title {}", picked.title);
        }
    }
}
