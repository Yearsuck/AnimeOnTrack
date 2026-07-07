use crate::models::Episode;
use std::collections::HashSet;

/// Return the subset of `scraped` whose url is not already known.
pub fn new_episodes(scraped: &[Episode], known_urls: &HashSet<String>) -> Vec<Episode> {
    scraped
        .iter()
        .filter(|e| !known_urls.contains(&e.url))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ep(url: &str) -> Episode {
        Episode {
            id: 0, series_id: 1, number: "1".into(), title: None,
            url: url.into(), released_at: None, seen: false,
        }
    }

    #[test]
    fn returns_only_unknown() {
        let scraped = vec![ep("a"), ep("b"), ep("c")];
        let known: HashSet<String> = ["a".to_string(), "b".to_string()].into_iter().collect();
        let out = new_episodes(&scraped, &known);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].url, "c");
    }

    #[test]
    fn empty_when_all_known() {
        let scraped = vec![ep("a")];
        let known: HashSet<String> = ["a".to_string()].into_iter().collect();
        assert!(new_episodes(&scraped, &known).is_empty());
    }

    #[test]
    fn all_new_when_nothing_known() {
        let scraped = vec![ep("a"), ep("b")];
        let known: HashSet<String> = HashSet::new();
        assert_eq!(new_episodes(&scraped, &known).len(), 2);
    }
}
