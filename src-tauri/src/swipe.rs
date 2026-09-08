use crate::models::FinishedCard;
use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;

/// Cards whose url doesn't already have a `series` row (i.e. hasn't been
/// swiped/decided yet).
pub fn undecided_cards(cards: Vec<FinishedCard>, known_urls: &HashSet<String>) -> Vec<FinishedCard> {
    cards.into_iter().filter(|c| !known_urls.contains(&c.url)).collect()
}

/// Process-wide PRNG state, seeded **once** from the OS entropy source.
///
/// This used to be `SystemTime::now().as_nanos() % len`, reseeded from the
/// wall clock on every single call. That is not random on Windows: the system
/// clock ticks in units of 100ns, so `as_nanos()` is always a multiple of 100
/// and `nanos % len` is a near-constant for every `len` that divides 100
/// (2, 4, 5, 10, 20, 25, 50) — exactly the pool sizes the deck hits in
/// practice. The genre/page pick was therefore effectively deterministic and
/// the deck could sit cycling the same genre forever.
///
/// One seed from `getrandom` (already a direct dependency — see
/// `backup::oauth`) plus a counter-based generator fixes that without
/// re-reading any clock: the value returned no longer depends on *when* the
/// call happens at all.
fn rng_state() -> &'static AtomicU64 {
    static STATE: OnceLock<AtomicU64> = OnceLock::new();
    STATE.get_or_init(|| {
        let mut seed_bytes = [0u8; 8];
        let seed = if getrandom::fill(&mut seed_bytes).is_ok() {
            u64::from_ne_bytes(seed_bytes)
        } else {
            // Deck shuffling is not security-sensitive, so a failed OS RNG
            // must degrade rather than panic (unlike `oauth`'s nonce). Mixing
            // a stack address in gives ASLR entropy that the clock alone
            // can't, and this runs at most once per process, not per call.
            use std::time::{SystemTime, UNIX_EPOCH};
            let nanos =
                SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(0);
            let addr = std::ptr::addr_of!(seed_bytes) as u64;
            nanos ^ addr.rotate_left(32) ^ 0x9E37_79B9_7F4A_7C15
        };
        AtomicU64::new(seed)
    })
}

/// SplitMix64 over the shared counter — lock-free (one `fetch_add`), so
/// concurrent deck fills never hand out the same value, and well-distributed
/// in every bit (which is what the old `% len` had no right to assume).
fn next_u64() -> u64 {
    const GAMMA: u64 = 0x9E37_79B9_7F4A_7C15;
    let z = rng_state().fetch_add(GAMMA, Ordering::Relaxed).wrapping_add(GAMMA);
    let z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    let z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Pseudo-random index in `0..len`, or `None` if `len` is 0.
///
/// Scales into range by multiply-and-shift (Lemire) rather than `%`: it is
/// always in bounds, and unlike a modulo it doesn't concentrate the result on
/// a few residues when the generator's low bits are the poorly-distributed
/// ones. Randomness comes from `next_u64`, not the clock — see `rng_state`.
pub fn pick_index(len: usize) -> Option<usize> {
    if len == 0 {
        return None;
    }
    Some(((next_u64() as u128 * len as u128) >> 64) as usize)
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
    // Uniform in [0, total). Same clock-independence fix as `pick_index`: the
    // old `(nanos % 1_000_000) as f64 / 1_000_000.0` inherited the 100ns tick,
    // so on Windows only every hundredth value in that range was reachable.
    let r = next_u64() as f64 / (u64::MAX as f64 + 1.0) * total;
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

    /// Regression: the old `SystemTime::now().as_nanos() % len` was a
    /// near-constant on Windows for every `len` dividing 100 (the system
    /// clock's 100ns tick), which is why the existing bounds tests above use
    /// 3 and 7 and never noticed. 10 is the smallest such length the deck
    /// actually hits; a degenerate generator returns one index here.
    #[test]
    fn pick_index_is_not_degenerate_for_a_length_that_divides_100() {
        for len in [2usize, 4, 5, 10, 20, 25, 50] {
            let seen: HashSet<usize> = (0..400).map(|_| pick_index(len).unwrap()).collect();
            assert!(seen.iter().all(|&i| i < len), "out of bounds for len={len}");
            assert!(
                seen.len() > 1,
                "pick_index({len}) returned only {:?} over 400 samples — the generator is degenerate",
                seen
            );
        }
    }

    /// Every index of a divides-100 pool must actually be reachable, not just
    /// "more than one of them" — a generator stuck alternating two values
    /// would pass the test above while still starving most of the pool.
    #[test]
    fn pick_index_reaches_every_index_of_a_ten_wide_pool() {
        let seen: HashSet<usize> = (0..2000).map(|_| pick_index(10).unwrap()).collect();
        assert_eq!(seen.len(), 10, "expected all 10 indices over 2000 samples, got {:?}", seen);
    }

    /// The weighted path lost the same clock precision. With a 1:9 split the
    /// heavier weight must dominate, and both must be reachable.
    #[test]
    fn weighted_pick_index_spreads_across_weights_and_favors_the_heavier() {
        let mut counts = [0usize; 2];
        for _ in 0..2000 {
            counts[weighted_pick_index(&[1.0, 9.0]).unwrap()] += 1;
        }
        assert!(counts[0] > 0, "the light weight is never picked: {counts:?}");
        assert!(counts[1] > counts[0], "the heavy weight must dominate: {counts:?}");
    }

    /// A shuffle over a divides-100 length must actually reorder — this is
    /// the deck's own card order, and it was effectively fixed before.
    #[test]
    fn shuffle_actually_reorders_a_ten_element_slice() {
        let original: Vec<usize> = (0..10).collect();
        let reordered = (0..20).any(|_| {
            let mut items = original.clone();
            shuffle(&mut items);
            items != original
        });
        assert!(reordered, "20 shuffles of 10 elements never changed the order");
    }
}
