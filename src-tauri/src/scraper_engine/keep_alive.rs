//! Ref-counted scan keep-alive guard.
//!
//! Keeps Android from stopping background work and off-screen WebViews during long scans
//! by holding a foreground service with a partial wake lock while any scan runs.
//! On other platforms it is a no-op returning `None`.

use std::sync::{Arc, Mutex};

/// Ref-counted "a scan is running" switch. The first holder calls `start`, the last one to drop
/// calls `stop`, so overlapping scans (refresh + airing scan) share one service.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub struct KeepAlive {
    count: Mutex<usize>,
    start: Box<dyn Fn() + Send + Sync>,
    stop: Box<dyn Fn() + Send + Sync>,
}

#[cfg_attr(not(target_os = "android"), allow(dead_code))]
impl KeepAlive {
    pub fn new(
        start: impl Fn() + Send + Sync + 'static,
        stop: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        Self {
            count: Mutex::new(0),
            start: Box::new(start),
            stop: Box::new(stop),
        }
    }

    pub fn hold(self: &Arc<Self>) -> KeepAliveGuard {
        // start runs under the lock: otherwise a concurrent last drop could send its stop
        // after this start and leave the service stopped mid-scan. start/stop must not block.
        let mut count = self.count.lock().unwrap_or_else(|e| e.into_inner());
        *count += 1;
        if *count == 1 {
            (self.start)();
        }
        drop(count);
        KeepAliveGuard(self.clone())
    }
}

/// RAII guard representing an in-flight scan holding the keep-alive service.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub struct KeepAliveGuard(Arc<KeepAlive>);

impl Drop for KeepAliveGuard {
    fn drop(&mut self) {
        // stop runs under the lock so it is ordered against a concurrent hold's start.
        let mut count = self.0.count.lock().unwrap_or_else(|e| e.into_inner());
        if *count > 0 {
            *count -= 1;
            if *count == 0 {
                (self.0.stop)();
            }
        }
    }
}

#[cfg(target_os = "android")]
static KEEP_ALIVE: std::sync::OnceLock<Arc<KeepAlive>> = std::sync::OnceLock::new();

/// Process-wide scan keep-alive helper.
///
/// On Android, starts the foreground service on the first active scan and stops it when the
/// last guard drops. On other platforms, returns `None`.
#[cfg(target_os = "android")]
pub fn scan_keep_alive(app: &tauri::AppHandle) -> Option<KeepAliveGuard> {
    use tauri::Manager;
    use tauri_plugin_webfetch::WebFetch;

    let keep_alive = KEEP_ALIVE.get_or_init(|| {
        // One worker thread applies start/stop in order: two independent blocking
        // tasks could run a quick scan's stop before its start and leave the service
        // (and its notification) running. Plugin calls block on the main thread, so
        // they never run on the async executor.
        let (tx, rx) = std::sync::mpsc::channel::<bool>();
        let app = app.clone();
        std::thread::spawn(move || {
            let mut started_at: Option<std::time::Instant> = None;
            for on in rx {
                let Some(webfetch) = app.try_state::<WebFetch<tauri::Wry>>() else { continue };
                if on {
                    match webfetch.start_scan_service() {
                        Ok(()) => started_at = Some(std::time::Instant::now()),
                        Err(e) => eprintln!("[scan-service] start failed: {e}"),
                    }
                } else {
                    // Stopping a service before it reached startForeground() can crash the
                    // app on some Android versions; give it a moment after a fast scan.
                    if let Some(t) = started_at.take() {
                        let min = std::time::Duration::from_secs(3);
                        if let Some(rest) = min.checked_sub(t.elapsed()) {
                            std::thread::sleep(rest);
                        }
                    }
                    if let Err(e) = webfetch.stop_scan_service() {
                        eprintln!("[scan-service] stop failed: {e}");
                    }
                }
            }
        });
        let tx_stop = tx.clone();
        Arc::new(KeepAlive::new(
            move || {
                let _ = tx.send(true);
            },
            move || {
                let _ = tx_stop.send(false);
            },
        ))
    });
    Some(keep_alive.hold())
}

#[cfg(not(target_os = "android"))]
pub fn scan_keep_alive(_app: &tauri::AppHandle) -> Option<KeepAliveGuard> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn one_hold_starts_and_stops_once() {
        let starts = Arc::new(AtomicUsize::new(0));
        let stops = Arc::new(AtomicUsize::new(0));
        let s = starts.clone();
        let p = stops.clone();
        let keep_alive = Arc::new(KeepAlive::new(
            move || {
                s.fetch_add(1, Ordering::SeqCst);
            },
            move || {
                p.fetch_add(1, Ordering::SeqCst);
            },
        ));

        assert_eq!(starts.load(Ordering::SeqCst), 0);
        assert_eq!(stops.load(Ordering::SeqCst), 0);

        let guard = keep_alive.hold();
        assert_eq!(starts.load(Ordering::SeqCst), 1);
        assert_eq!(stops.load(Ordering::SeqCst), 0);

        drop(guard);
        assert_eq!(starts.load(Ordering::SeqCst), 1);
        assert_eq!(stops.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn overlapping_holds_start_once_and_stop_once_after_both_drop() {
        let starts = Arc::new(AtomicUsize::new(0));
        let stops = Arc::new(AtomicUsize::new(0));
        let s = starts.clone();
        let p = stops.clone();
        let keep_alive = Arc::new(KeepAlive::new(
            move || {
                s.fetch_add(1, Ordering::SeqCst);
            },
            move || {
                p.fetch_add(1, Ordering::SeqCst);
            },
        ));

        let guard1 = keep_alive.hold();
        assert_eq!(starts.load(Ordering::SeqCst), 1);
        assert_eq!(stops.load(Ordering::SeqCst), 0);

        let guard2 = keep_alive.hold();
        assert_eq!(starts.load(Ordering::SeqCst), 1);
        assert_eq!(stops.load(Ordering::SeqCst), 0);

        drop(guard1);
        assert_eq!(starts.load(Ordering::SeqCst), 1);
        assert_eq!(stops.load(Ordering::SeqCst), 0);

        drop(guard2);
        assert_eq!(starts.load(Ordering::SeqCst), 1);
        assert_eq!(stops.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn hold_drop_hold_starts_twice_and_stops_twice() {
        let starts = Arc::new(AtomicUsize::new(0));
        let stops = Arc::new(AtomicUsize::new(0));
        let s = starts.clone();
        let p = stops.clone();
        let keep_alive = Arc::new(KeepAlive::new(
            move || {
                s.fetch_add(1, Ordering::SeqCst);
            },
            move || {
                p.fetch_add(1, Ordering::SeqCst);
            },
        ));

        let guard1 = keep_alive.hold();
        assert_eq!(starts.load(Ordering::SeqCst), 1);
        assert_eq!(stops.load(Ordering::SeqCst), 0);
        drop(guard1);
        assert_eq!(starts.load(Ordering::SeqCst), 1);
        assert_eq!(stops.load(Ordering::SeqCst), 1);

        let guard2 = keep_alive.hold();
        assert_eq!(starts.load(Ordering::SeqCst), 2);
        assert_eq!(stops.load(Ordering::SeqCst), 1);
        drop(guard2);
        assert_eq!(starts.load(Ordering::SeqCst), 2);
        assert_eq!(stops.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn guard_dropped_during_panic_unwinding_calls_stop() {
        let starts = Arc::new(AtomicUsize::new(0));
        let stops = Arc::new(AtomicUsize::new(0));
        let s = starts.clone();
        let p = stops.clone();
        let keep_alive = Arc::new(KeepAlive::new(
            move || {
                s.fetch_add(1, Ordering::SeqCst);
            },
            move || {
                p.fetch_add(1, Ordering::SeqCst);
            },
        ));

        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = keep_alive.hold();
            assert_eq!(starts.load(Ordering::SeqCst), 1);
            panic!("deliberate panic to test unwinding");
        }));

        assert_eq!(starts.load(Ordering::SeqCst), 1);
        assert_eq!(stops.load(Ordering::SeqCst), 1);
    }
}
