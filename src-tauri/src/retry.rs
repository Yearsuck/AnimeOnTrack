//! Shared retry/backoff policy: exponential delay with a cap and optional
//! jitter, plus an async driver that also honors a server-provided wait
//! (e.g. HTTP `Retry-After`).

use std::future::Future;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RetryPolicy {
    /// Total attempts, including the first one.
    pub max_attempts: u32,
    /// Delay after the first failed attempt.
    pub base: Duration,
    /// Multiplier applied per further failure (1.0 = constant delay).
    pub factor: f64,
    /// Upper bound for any single wait, server-provided ones included.
    pub max_delay: Duration,
    /// Fraction in `0.0..=1.0`: the delay is scaled by `1 - jitter * r`, r in `[0,1)`.
    pub jitter: f64,
}

impl RetryPolicy {
    #[allow(dead_code)]
    /// Deterministic delay after the `attempt`-th failure (1-based), capped.
    pub fn delay_for(&self, attempt: u32) -> Duration {
        self.jittered(attempt, 0.0)
    }

    /// Like `delay_for` but shortened by up to `jitter` of itself; `r` in `[0,1)`.
    pub fn jittered(&self, attempt: u32, r: f64) -> Duration {
        let exp = attempt.saturating_sub(1).min(32) as i32;
        let secs = self.base.as_secs_f64() * self.factor.max(0.0).powi(exp);
        let capped = secs.min(self.max_delay.as_secs_f64());
        let scale = 1.0 - self.jitter.clamp(0.0, 1.0) * r.clamp(0.0, 1.0);
        Duration::from_secs_f64((capped * scale).max(0.0))
    }
}

fn unit_random() -> f64 {
    let mut b = [0u8; 4];
    if getrandom::fill(&mut b).is_err() {
        return 0.0;
    }
    f64::from(u32::from_le_bytes(b)) / (f64::from(u32::MAX) + 1.0)
}

/// Run `op` until it succeeds, `is_retryable` rejects the error, or
/// `max_attempts` is reached (the last error is returned).
#[allow(dead_code)]
pub async fn retry_with_backoff<T, E, F, Fut>(
    policy: RetryPolicy,
    op: F,
    is_retryable: impl Fn(&E) -> bool,
) -> Result<T, E>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, E>>,
{
    retry_with_hint(policy, op, is_retryable, |_| None).await
}

/// Same, but `retry_after(&err)` may supply a server-requested wait that
/// replaces the computed delay (still bounded by `policy.max_delay`).
pub async fn retry_with_hint<T, E, F, Fut>(
    policy: RetryPolicy,
    mut op: F,
    is_retryable: impl Fn(&E) -> bool,
    retry_after: impl Fn(&E) -> Option<Duration>,
) -> Result<T, E>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, E>>,
{
    let mut attempt = 1u32;
    loop {
        match op().await {
            Ok(v) => return Ok(v),
            Err(e) => {
                if attempt >= policy.max_attempts || !is_retryable(&e) {
                    return Err(e);
                }
                let wait = match retry_after(&e) {
                    Some(d) => d.min(policy.max_delay),
                    None => policy.jittered(attempt, unit_random()),
                };
                tokio::time::sleep(wait).await;
                attempt += 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn pol(max_attempts: u32) -> RetryPolicy {
        RetryPolicy {
            max_attempts,
            base: Duration::from_millis(1),
            factor: 2.0,
            max_delay: Duration::from_millis(5),
            jitter: 0.0,
        }
    }

    #[test]
    fn delay_sequence_and_cap() {
        let p = RetryPolicy { max_delay: Duration::from_millis(10), ..pol(9) };
        let ms: Vec<u128> = (1..=6).map(|a| p.delay_for(a).as_millis()).collect();
        assert_eq!(ms, vec![1, 2, 4, 8, 10, 10]);
    }

    #[test]
    fn jitter_stays_in_bounds() {
        let p = RetryPolicy {
            base: Duration::from_millis(100),
            jitter: 0.5,
            max_delay: Duration::from_secs(1),
            ..pol(3)
        };
        assert_eq!(p.jittered(1, 0.0), Duration::from_millis(100));
        for i in 0..=10 {
            let d = p.jittered(1, f64::from(i) / 10.0);
            assert!(d <= Duration::from_millis(100) && d >= Duration::from_millis(50));
        }
    }

    #[tokio::test]
    async fn succeeds_after_transient_errors() {
        let n = AtomicU32::new(0);
        let r: Result<u32, &str> = retry_with_backoff(
            pol(5),
            || async {
                if n.fetch_add(1, Ordering::SeqCst) < 2 {
                    Err("busy")
                } else {
                    Ok(7)
                }
            },
            |_| true,
        )
        .await;
        assert_eq!(r, Ok(7));
        assert_eq!(n.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn stops_on_non_retryable() {
        let n = AtomicU32::new(0);
        let r: Result<(), &str> = retry_with_backoff(
            pol(5),
            || async {
                n.fetch_add(1, Ordering::SeqCst);
                Err("fatal")
            },
            |_| false,
        )
        .await;
        assert_eq!(r, Err("fatal"));
        assert_eq!(n.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn stops_at_max_attempts() {
        let n = AtomicU32::new(0);
        let r: Result<(), &str> = retry_with_backoff(
            pol(4),
            || async {
                n.fetch_add(1, Ordering::SeqCst);
                Err("busy")
            },
            |_| true,
        )
        .await;
        assert_eq!(r, Err("busy"));
        assert_eq!(n.load(Ordering::SeqCst), 4);
    }

    #[tokio::test]
    async fn server_wait_replaces_delay_and_is_capped() {
        let n = AtomicU32::new(0);
        let t = std::time::Instant::now();
        let r: Result<u32, u64> = retry_with_hint(
            pol(3),
            || async {
                if n.fetch_add(1, Ordering::SeqCst) < 1 {
                    Err(3600)
                } else {
                    Ok(1)
                }
            },
            |_| true,
            |e| Some(Duration::from_secs(*e)),
        )
        .await;
        assert_eq!(r, Ok(1));
        assert!(t.elapsed() < Duration::from_secs(1));
    }
}
