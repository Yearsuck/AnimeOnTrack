//! Short-TTL in-memory HTML cache (optimization C of the scraper-performance
//! design). Consulted by `fetch_html`'s *callers* (`scrape_via_mirrors`), not
//! by `fetch_html` itself — the engine stays dumb. It does nothing for a
//! single refresh (every series is a distinct URL) but makes "refresh, then
//! immediately re-scrape the same page from a discover/link/backfill flow"
//! free within the TTL. Never persisted to disk; bounded to `MAX_ENTRIES`
//! with oldest-inserted eviction.

use std::collections::HashMap;
use std::time::{Duration, Instant};

pub const HTML_CACHE_TTL: Duration = Duration::from_secs(10 * 60);
pub const HTML_CACHE_MAX_ENTRIES: usize = 64;

#[derive(Default)]
pub struct HtmlCache {
    entries: HashMap<String, (Instant, String)>,
}

impl HtmlCache {
    /// The cached HTML for `url` if it was stored less than
    /// `HTML_CACHE_TTL` ago (relative to `now`). Expired entries are
    /// dropped on access rather than by a sweeper.
    pub fn get(&mut self, url: &str, now: Instant) -> Option<String> {
        match self.entries.get(url) {
            Some((stored_at, html))
                if now.saturating_duration_since(*stored_at) < HTML_CACHE_TTL =>
            {
                Some(html.clone())
            }
            Some(_) => {
                self.entries.remove(url);
                None
            }
            None => None,
        }
    }

    /// Store `html` under `url`, evicting the oldest-inserted entry if the
    /// cache is at capacity (simple bound, not LRU — good enough for 64
    /// entries of page HTML).
    pub fn put(&mut self, url: &str, html: String, now: Instant) {
        if !self.entries.contains_key(url) && self.entries.len() >= HTML_CACHE_MAX_ENTRIES {
            if let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, (stored_at, _))| *stored_at)
                .map(|(k, _)| k.clone())
            {
                self.entries.remove(&oldest);
            }
        }
        self.entries.insert(url.to_string(), (now, html));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_returns_fresh_entry_and_misses_unknown_url() {
        let mut c = HtmlCache::default();
        let now = Instant::now();
        c.put("https://a/", "<html>a</html>".into(), now);
        assert_eq!(c.get("https://a/", now).as_deref(), Some("<html>a</html>"));
        assert_eq!(c.get("https://b/", now), None);
    }

    #[test]
    fn entry_expires_after_ttl() {
        let mut c = HtmlCache::default();
        let t0 = Instant::now();
        c.put("https://a/", "x".into(), t0);
        let just_before = t0 + HTML_CACHE_TTL - Duration::from_secs(1);
        assert!(c.get("https://a/", just_before).is_some());
        let just_after = t0 + HTML_CACHE_TTL;
        assert_eq!(c.get("https://a/", just_after), None);
        // And the expired entry was dropped, not kept around.
        assert!(c.entries.is_empty());
    }

    #[test]
    fn capacity_bound_evicts_oldest_inserted() {
        let mut c = HtmlCache::default();
        let t0 = Instant::now();
        for i in 0..HTML_CACHE_MAX_ENTRIES {
            c.put(&format!("https://u{i}/"), "x".into(), t0 + Duration::from_millis(i as u64));
        }
        assert_eq!(c.entries.len(), HTML_CACHE_MAX_ENTRIES);
        // One more insert evicts the oldest (u0), keeps everything else.
        let later = t0 + Duration::from_secs(1);
        c.put("https://new/", "y".into(), later);
        assert_eq!(c.entries.len(), HTML_CACHE_MAX_ENTRIES);
        assert_eq!(c.get("https://u0/", later), None);
        assert!(c.get("https://u1/", later).is_some());
        assert!(c.get("https://new/", later).is_some());
    }

    #[test]
    fn overwriting_an_existing_url_refreshes_it_without_eviction() {
        let mut c = HtmlCache::default();
        let t0 = Instant::now();
        for i in 0..HTML_CACHE_MAX_ENTRIES {
            c.put(&format!("https://u{i}/"), "x".into(), t0 + Duration::from_millis(i as u64));
        }
        // Re-putting an already-present key must not evict anything.
        let later = t0 + Duration::from_secs(1);
        c.put("https://u3/", "fresh".into(), later);
        assert_eq!(c.entries.len(), HTML_CACHE_MAX_ENTRIES);
        assert_eq!(c.get("https://u3/", later).as_deref(), Some("fresh"));
        assert!(c.get("https://u0/", later).is_some(), "u0 must survive an overwrite");
    }
}
