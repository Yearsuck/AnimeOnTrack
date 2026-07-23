use crate::models::FinishedCard;
use std::collections::HashSet;

/// Cards whose url doesn't already have a `series` row (i.e. hasn't been
/// swiped/decided yet).
pub fn undecided_cards(cards: Vec<FinishedCard>, known_urls: &HashSet<String>) -> Vec<FinishedCard> {
    cards.into_iter().filter(|c| !known_urls.contains(&c.url)).collect()
}

/// Pseudo-random index in `0..len`, or `None` if `len` is 0. Uses the current
/// time as its source of randomness — same low-effort approach
/// `scraper_engine::uuid_like` already takes to avoid pulling in a `rand`
/// dependency for something this small; the swipe deck's shuffle order has
/// no correctness requirement beyond "not always the same order".
pub fn pick_index(len: usize) -> Option<usize> {
    if len == 0 {
        return None;
    }
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    Some((nanos as usize) % len)
}

/// Weighted random index, biased toward higher `weights` — powers the swipe
/// deck's taste-weighted genre pick. Falls back to a uniform `pick_index`
/// over the same length whenever every weight is <= 0 (cold start: nothing
/// followed/decided yet, or every candidate genre nets non-positive), so
/// discovery never goes silent on a genre just because it has no signal —
/// only decisions actively push it up or down.
pub fn weighted_pick_index(weights: &[f64]) -> Option<usize> {
    let total: f64 = weights.iter().filter(|w| **w > 0.0).sum();
    if total <= 0.0 {
        return pick_index(weights.len());
    }
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    let r = (nanos % 1_000_000) as f64 / 1_000_000.0 * total;
    let mut acc = 0.0;
    for (i, w) in weights.iter().enumerate() {
        if *w > 0.0 {
            acc += w;
            if r < acc {
                return Some(i);
            }
        }
    }
    weights.iter().rposition(|w| *w > 0.0)
}

/// Fisher-Yates shuffle driven by `pick_index`.
pub fn shuffle<T>(items: &mut [T]) {
    for i in (1..items.len()).rev() {
        if let Some(j) = pick_index(i + 1) {
            items.swap(i, j);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card(url: &str) -> FinishedCard {
        FinishedCard {
            title: url.into(),
            url: url.into(),
            poster_url: None,
            kind: "TV".into(),
            matched_genre: None,
        }
    }

    #[test]
    fn undecided_cards_excludes_known_urls() {
        let cards = vec![card("a"), card("b"), card("c")];
        let known: HashSet<String> = ["a".to_string()].into_iter().collect();
        let out = undecided_cards(cards, &known);
        assert_eq!(out.len(), 2);
        assert!(out.iter().all(|c| c.url != "a"));
    }

    #[test]
    fn undecided_cards_empty_when_all_known() {
        let cards = vec![card("a")];
        let known: HashSet<String> = ["a".to_string()].into_iter().collect();
        assert!(undecided_cards(cards, &known).is_empty());
    }

    #[test]
    fn undecided_cards_all_kept_when_nothing_known() {
        let cards = vec![card("a"), card("b")];
        let known: HashSet<String> = HashSet::new();
        assert_eq!(undecided_cards(cards, &known).len(), 2);
    }

    #[test]
    fn shuffle_preserves_all_elements() {
        let mut items = vec![1, 2, 3, 4, 5];
        shuffle(&mut items);
        items.sort();
        assert_eq!(items, vec![1, 2, 3, 4, 5]);
    }

    #[test]
    fn pick_index_is_in_bounds_and_none_for_empty() {
        for _ in 0..20 {
            let i = pick_index(7).unwrap();
            assert!(i < 7);
        }
        assert_eq!(pick_index(0), None);
    }

    #[test]
    fn weighted_pick_index_only_ever_picks_the_sole_positive_weight() {
        for _ in 0..20 {
            assert_eq!(weighted_pick_index(&[0.0, 0.0, 5.0]), Some(2));
        }
    }

    #[test]
    fn weighted_pick_index_falls_back_to_uniform_when_all_non_positive() {
        for _ in 0..20 {
            let i = weighted_pick_index(&[0.0, -1.0, 0.0]).unwrap();
            assert!(i < 3);
        }
    }

    #[test]
    fn weighted_pick_index_none_for_empty() {
        assert_eq!(weighted_pick_index(&[]), None);
    }
}
