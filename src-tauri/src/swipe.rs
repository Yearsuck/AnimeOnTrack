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

/// Fisher-Yates shuffle driven by `pick_index`.
pub fn shuffle<T>(items: &mut Vec<T>) {
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
        FinishedCard { title: url.into(), url: url.into(), poster_url: None, kind: "TV".into() }
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
}
