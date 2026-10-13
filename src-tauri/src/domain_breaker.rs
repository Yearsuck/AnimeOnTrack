//! In-memory per-host failure cool-down ("circuit breaker") for scraper mirrors.
//!
//! When scraping pirate anime sites through WebView2, dead domains or Cloudflare-blocked
//! mirrors cost a 20-30 second timeout on every refresh or fallback cycle.
//! This module tracks consecutive fetch failures per host so dead hosts can be demoted
//! or skipped when alternatives exist.
//!
//! ### Important distinction: Fetch failures vs. Parse failures
//! Only **FETCH** failures (WebView2 timeouts, Cloudflare interstitial challenge blocks,
//! network drops) count as host failures and trigger this circuit breaker.
//!
//! A "page loaded but did not parse" result (e.g., empty search results or unmatched page layout)
//! indicates that the mirror serves content in a different structure or does not carry
//! that specific title—it is an incompatible/wrong-site signal, **not** a dead host.
//! Empty parses must **never** record a domain failure.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use url::Url;

/// Cooldown duration during which an open host is demoted or skipped (5 minutes).
pub const COOLDOWN: Duration = Duration::from_secs(5 * 60);

/// Failure and cool-down tracking state for a single host.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct State {
    pub consecutive_failures: u32,
    pub open_until: Option<Instant>,
}

/// Host-keyed circuit breaker tracking failures and cool-down periods.
#[derive(Debug, Default)]
pub struct Breaker {
    hosts: HashMap<String, State>,
}

impl Breaker {
    /// Create a new, empty breaker instance.
    pub fn new() -> Self {
        Self {
            hosts: HashMap::new(),
        }
    }

    /// Clear all host states (test helper).
    #[cfg(test)]
    pub fn clear(&mut self) {
        self.hosts.clear();
    }

    /// Record a fetch failure at an explicit timestamp `now`.
    ///
    /// Increments `consecutive_failures`. At 2 or more consecutive failures,
    /// `open_until` is set to `now + COOLDOWN`.
    pub fn record_failure_at(&mut self, url: &str, now: Instant) {
        let host = host_key(url);
        let state = self.hosts.entry(host).or_default();
        state.consecutive_failures = state.consecutive_failures.saturating_add(1);
        if state.consecutive_failures >= 2 {
            state.open_until = Some(now + COOLDOWN);
        }
    }

    /// Record a successful fetch, resetting and removing the host's failure state.
    pub fn record_success(&mut self, url: &str) {
        let host = host_key(url);
        self.hosts.remove(&host);
    }

    /// Record a successful fetch at an explicit timestamp `_now` (alias for `record_success`).
    pub fn record_success_at(&mut self, url: &str, _now: Instant) {
        self.record_success(url);
    }

    /// Returns `true` if the host is currently in cool-down (`now < open_until`).
    pub fn is_open_at(&self, url: &str, now: Instant) -> bool {
        let host = host_key(url);
        if let Some(state) = self.hosts.get(&host) {
            if let Some(until) = state.open_until {
                return now < until;
            }
        }
        false
    }

    /// Reorders candidate URLs so that cooling ones are moved to the end.
    ///
    /// Preserves stable ordering within both active and cooling partitions.
    /// Never drops any URL (if all URLs are cooling, they are tried in their original order).
    pub fn order_for_attempt_at(&self, urls: &[String], now: Instant) -> Vec<String> {
        let mut active = Vec::with_capacity(urls.len());
        let mut cooling = Vec::new();
        for u in urls {
            if self.is_open_at(u, now) {
                cooling.push(u.clone());
            } else {
                active.push(u.clone());
            }
        }
        active.extend(cooling);
        active
    }
}

/// Extract a lowercase host key from a URL or raw string.
///
/// If `url::Url::parse` succeeds and extracts a host, it is lowercased.
/// Otherwise, the input string itself is trimmed and lowercased.
pub fn host_key(url_or_host: &str) -> String {
    let trimmed = url_or_host.trim();
    if let Ok(parsed) = Url::parse(trimmed) {
        if let Some(host) = parsed.host_str() {
            return host.to_ascii_lowercase();
        }
    }
    trimmed.to_ascii_lowercase()
}

static GLOBAL_BREAKER: OnceLock<Mutex<Breaker>> = OnceLock::new();

fn global() -> &'static Mutex<Breaker> {
    GLOBAL_BREAKER.get_or_init(|| Mutex::new(Breaker::new()))
}

/// Record a fetch failure for the host of `url` at `Instant::now()`.
pub fn record_failure(url: &str) {
    record_failure_at(url, Instant::now());
}

/// Record a fetch failure for the host of `url` at an explicit timestamp `now`.
pub fn record_failure_at(url: &str, now: Instant) {
    global().lock().unwrap().record_failure_at(url, now);
}

/// Record a successful fetch for the host of `url`, removing any stored failure state.
pub fn record_success(url: &str) {
    record_success_at(url, Instant::now());
}

/// Record a successful fetch for the host of `url` at an explicit timestamp `now`.
pub fn record_success_at(url: &str, now: Instant) {
    global().lock().unwrap().record_success_at(url, now);
}

/// Check if the host of `url` is currently cooling at `Instant::now()`.
pub fn is_open(url: &str) -> bool {
    is_open_at(url, Instant::now())
}

/// Check if the host of `url` is currently cooling at an explicit timestamp `now`.
pub fn is_open_at(url: &str, now: Instant) -> bool {
    global().lock().unwrap().is_open_at(url, now)
}

/// Reorder mirror URLs moving cooling ones to the end at `Instant::now()`.
pub fn order_for_attempt(urls: &[String]) -> Vec<String> {
    order_for_attempt_at(urls, Instant::now())
}

/// Reorder mirror URLs moving cooling ones to the end at an explicit timestamp `now`.
pub fn order_for_attempt_at(urls: &[String], now: Instant) -> Vec<String> {
    global().lock().unwrap().order_for_attempt_at(urls, now)
}

/// Reset global breaker state (for testing).
#[cfg(test)]
pub fn reset_for_tests() {
    global().lock().unwrap().clear();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_failure_threshold_and_cooldown_expiration() {
        let mut breaker = Breaker::new();
        let base = Instant::now();
        let url = "https://broken.example/path";

        // One failure does not open the host
        breaker.record_failure_at(url, base);
        assert!(!breaker.is_open_at(url, base));

        // Second failure opens it for 5 minutes
        let t1 = base + Duration::from_secs(1);
        breaker.record_failure_at(url, t1);
        assert!(breaker.is_open_at(url, t1));
        assert!(breaker.is_open_at(url, t1 + Duration::from_secs(299)));

        // is_open is false again at now + 5 min
        assert!(!breaker.is_open_at(url, t1 + COOLDOWN));
        assert!(!breaker.is_open_at(url, t1 + COOLDOWN + Duration::from_secs(1)));
    }

    #[test]
    fn test_record_success_resets_failures() {
        let mut breaker = Breaker::new();
        let base = Instant::now();
        let url = "https://flaky.example";

        // failure, success, failure leaves the host closed
        breaker.record_failure_at(url, base);
        assert!(!breaker.is_open_at(url, base));

        breaker.record_success(url);

        breaker.record_failure_at(url, base + Duration::from_secs(2));
        assert!(!breaker.is_open_at(url, base + Duration::from_secs(2)));

        // Even after being opened, success resets it immediately
        breaker.record_failure_at(url, base + Duration::from_secs(3));
        assert!(breaker.is_open_at(url, base + Duration::from_secs(3)));

        breaker.record_success_at(url, base + Duration::from_secs(4));
        assert!(!breaker.is_open_at(url, base + Duration::from_secs(4)));
    }

    #[test]
    fn test_order_for_attempt_demotes_cooling_and_preserves_order() {
        let mut breaker = Breaker::new();
        let base = Instant::now();
        let a = "https://a.example".to_string();
        let b = "https://b.example".to_string();
        let urls = vec![a.clone(), b.clone()];

        // With none open: unchanged
        assert_eq!(breaker.order_for_attempt_at(&urls, base), vec![a.clone(), b.clone()]);

        // With a open: returns [b, a]
        breaker.record_failure_at(&a, base);
        breaker.record_failure_at(&a, base);
        assert!(breaker.is_open_at(&a, base));
        assert_eq!(breaker.order_for_attempt_at(&urls, base), vec![b.clone(), a.clone()]);

        // With both open: returns [a, b] unchanged
        breaker.record_failure_at(&b, base);
        breaker.record_failure_at(&b, base);
        assert!(breaker.is_open_at(&b, base));
        assert_eq!(breaker.order_for_attempt_at(&urls, base), vec![a.clone(), b.clone()]);
    }

    #[test]
    fn test_host_extraction_shares_state() {
        let mut breaker = Breaker::new();
        let base = Instant::now();

        // https://A.Example/path?x=1 and https://a.example share state
        breaker.record_failure_at("https://A.Example/path?x=1", base);
        breaker.record_failure_at("https://a.example", base);

        assert!(breaker.is_open_at("https://A.Example/path?x=1", base));
        assert!(breaker.is_open_at("https://a.example", base));
        assert!(breaker.is_open_at("https://a.example/other?foo=bar", base));

        breaker.record_success("https://A.Example/reset");
        assert!(!breaker.is_open_at("https://a.example", base));
    }

    #[test]
    fn test_never_drops_any_url() {
        let mut breaker = Breaker::new();
        let base = Instant::now();

        let urls = vec![
            "https://1.example".to_string(),
            "https://2.example".to_string(),
            "https://3.example".to_string(),
            "https://4.example".to_string(),
        ];

        // Open 1 and 3
        breaker.record_failure_at(&urls[0], base);
        breaker.record_failure_at(&urls[0], base);
        breaker.record_failure_at(&urls[2], base);
        breaker.record_failure_at(&urls[2], base);

        let reordered = breaker.order_for_attempt_at(&urls, base);
        assert_eq!(reordered.len(), urls.len());
        assert_eq!(
            reordered,
            vec![
                "https://2.example".to_string(),
                "https://4.example".to_string(),
                "https://1.example".to_string(),
                "https://3.example".to_string(),
            ]
        );
    }

    #[test]
    fn test_raw_string_without_url_scheme() {
        let mut breaker = Breaker::new();
        let base = Instant::now();

        assert_eq!(host_key("RAW-HOST.EXAMPLE"), "raw-host.example");
        breaker.record_failure_at("RAW-HOST.EXAMPLE", base);
        breaker.record_failure_at("raw-host.example", base);
        assert!(breaker.is_open_at("raw-host.example", base));
        assert!(breaker.is_open_at("RAW-HOST.EXAMPLE", base));
    }

    #[test]
    fn test_global_breaker_and_reset_for_tests() {
        reset_for_tests();
        let url = "https://global.example";

        record_failure(url);
        record_failure(url);
        assert!(is_open(url));

        let urls = vec!["https://other.example".to_string(), url.to_string()];
        let ordered = order_for_attempt(&urls);
        assert_eq!(ordered, vec!["https://other.example".to_string(), url.to_string()]);

        record_success(url);
        assert!(!is_open(url));

        record_failure(url);
        record_failure(url);
        assert!(is_open(url));
        reset_for_tests();
        assert!(!is_open(url));
    }

    #[test]
    fn test_empty_parse_flow_does_not_record_failure() {
        let mut breaker = Breaker::new();
        let base = Instant::now();
        let url = "https://empty-parse.example";

        // Successful fetch followed by empty parse records success, NOT failure
        breaker.record_success(url);
        assert!(!breaker.is_open_at(url, base));

        // Even after previous failures, successful fetch clears the state
        breaker.record_failure_at(url, base);
        breaker.record_success(url);
        assert!(!breaker.is_open_at(url, base));
    }
}
