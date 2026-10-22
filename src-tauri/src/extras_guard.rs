//! Throttle for the on-demand AniList "extras" request (`get_anime_extras`).
//!
//! Without it, every detail-view open while AniList is down or rate-limited
//! starts a fresh request chain (each one may sleep through several 429
//! `Retry-After`s). This remembers recent failures per id for a while and lets
//! only one request per id be in flight. In-memory only; never touches the db.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

/// How long a failed fetch suppresses new requests for the same id.
pub const FAILURE_TTL_SECS: i64 = 10 * 60;

#[derive(Default)]
pub struct ExtrasGuard {
    failed_at: Mutex<HashMap<i64, i64>>,
    in_flight: Mutex<HashSet<i64>>,
}

impl ExtrasGuard {
    /// `true` when the caller may start a request for `id`: no failure within
    /// `FAILURE_TTL_SECS` and none already in flight. On `true` the id is marked
    /// in flight; the caller must call `finish` afterwards.
    pub fn try_begin(&self, id: i64, now: i64) -> bool {
        if let Some(&t) = self.failed_at.lock().unwrap().get(&id) {
            if now - t < FAILURE_TTL_SECS {
                return false;
            }
        }
        self.in_flight.lock().unwrap().insert(id)
    }

    /// Release the in-flight slot and, on failure, remember it.
    pub fn finish(&self, id: i64, now: i64, failed: bool) {
        self.in_flight.lock().unwrap().remove(&id);
        let mut f = self.failed_at.lock().unwrap();
        if failed {
            f.insert(id, now);
        } else {
            f.remove(&id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_request_for_same_id_is_deduped_until_finished() {
        let g = ExtrasGuard::default();
        assert!(g.try_begin(1, 0));
        assert!(!g.try_begin(1, 0));
        assert!(g.try_begin(2, 0), "other ids are independent");
        g.finish(1, 0, false);
        assert!(g.try_begin(1, 1));
    }

    #[test]
    fn failure_blocks_until_ttl_then_allows_again() {
        let g = ExtrasGuard::default();
        assert!(g.try_begin(7, 100));
        g.finish(7, 100, true);
        assert!(!g.try_begin(7, 100 + FAILURE_TTL_SECS - 1));
        assert!(g.try_begin(7, 100 + FAILURE_TTL_SECS));
    }

    #[test]
    fn success_clears_a_previous_failure() {
        let g = ExtrasGuard::default();
        assert!(g.try_begin(3, 0));
        g.finish(3, 0, true);
        assert!(g.try_begin(3, FAILURE_TTL_SECS));
        g.finish(3, FAILURE_TTL_SECS, false);
        assert!(g.try_begin(3, FAILURE_TTL_SECS + 1));
    }
}
