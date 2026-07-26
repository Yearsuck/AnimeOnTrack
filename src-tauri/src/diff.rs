use crate::models::Episode;
use std::collections::HashSet;

/// Return the subset of `scraped` whose episode *number* is not already known.
///
/// Keyed on number, not URL: the sites move between domains and change URL
/// path shapes, so a URL-keyed diff treats every episode as new after a domain
/// change and re-inserts the whole list as unseen duplicates (the
/// episode-duplication bug). The number is an episode's stable identity — it's
/// what `set_seen_cascade`, the pending queue, and `max_episode_number` all key
/// on — so a re-scan on a new domain now correctly finds nothing "new".
pub fn new_episodes(scraped: &[Episode], known_numbers: &HashSet<String>) -> Vec<Episode> {
    scraped
        .iter()
        .filter(|e| !known_numbers.contains(&e.number))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ep(number: &str, url: &str) -> Episode {
        Episode {
            id: 0, series_id: 1, number: number.into(), title: None,
            url: url.into(), released_at: None, seen: false,
        }
    }

    #[test]
    fn returns_only_unknown_numbers() {
        let scraped = vec![ep("1", "u1"), ep("2", "u2"), ep("3", "u3")];
        let known: HashSet<String> = ["1".to_string(), "2".to_string()].into_iter().collect();
        let out = new_episodes(&scraped, &known);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].number, "3");
    }

    #[test]
    fn empty_when_all_numbers_known() {
        let scraped = vec![ep("1", "u1")];
        let known: HashSet<String> = ["1".to_string()].into_iter().collect();
        assert!(new_episodes(&scraped, &known).is_empty());
    }

    #[test]
    fn all_new_when_nothing_known() {
        let scraped = vec![ep("1", "u1"), ep("2", "u2")];
        let known: HashSet<String> = HashSet::new();
        assert_eq!(new_episodes(&scraped, &known).len(), 2);
    }

    // The bug this diff exists to prevent: the site moved domains, so every
    // scraped URL is different from what's stored, but the episode *numbers*
    // are identical. Keyed on number, none of these read as new — no
    // duplicate, unseen copies get inserted.
    #[test]
    fn domain_change_is_not_new_episodes() {
        let scraped = vec![
            ep("1", "https://animeyt.cc/999/anime/x-capitulo-1/"),
            ep("2", "https://animeyt.cc/998/anime/x-capitulo-2/"),
        ];
        let known: HashSet<String> = ["1".to_string(), "2".to_string()].into_iter().collect();
        assert!(
            new_episodes(&scraped, &known).is_empty(),
            "same numbers on a new domain must not count as new episodes"
        );
    }
}
