//! UI-hang watchdog: detects a frozen main thread and, within tight safety
//! bounds, restarts the process to give the user a working window again.
//!
//! # Why this exists
//!
//! `WebviewWindowBuilder::build()` runs synchronously on the GUI main thread
//! via `run_on_main_thread`. It has been live-observed to hang indefinitely
//! (zero CPU, 10+ minutes) right after a batch of concurrent scraper windows
//! close — a WebView2-runtime hiccup under rapid window churn is the leading
//! suspect. The `recv_timeout` guard in `build_webview_window_with_timeout`
//! unblocks the *async* caller, but it leaves the main-thread closure queued
//! and hanging forever, so the whole GUI event loop is stuck: the window
//! stops repainting, input events go unprocessed, and Windows eventually logs
//! a "Application Hang" event (Event ID 1002) with no Rust panic anywhere.
//!
//! This watchdog measures main-thread liveness by posting a tiny closure via
//! `run_on_main_thread` every two seconds and recording when the closure
//! actually executed.
//!
//! # What happens on a stall
//!
//! 1. After 25 s without an acknowledgement the stall and the recent window
//!    activity are appended to `hang.log` in the app-data directory — evidence
//!    for the next investigation. Nothing else happens yet: a synchronous
//!    command that is merely busy (they run on the main thread) must not be
//!    killed.
//! 2. If the main thread is *still* silent after 60 s, the app relaunches
//!    itself: a fresh copy of the process is spawned (detached, no waiting) and
//!    the current process exits. The relaunch is bounded by a persisted
//!    history (`hang-relaunch.marker`, one unix timestamp per line): at least
//!    120 s apart and at most 2 in 10 minutes, so a stall that happens at
//!    every start can never turn into an endless restart loop — later stalls
//!    are only logged.
//! 3. A stall that ends by itself (the main thread acknowledges again) is
//!    logged once and never relaunched.
//!
//! # Not a false alarm
//!
//! The loop compares its own wall-clock readings between iterations, so a
//! laptop suspend/resume or a clock jump is forgiven (see
//! `resumed_from_suspend`).
//!
//! # Opt-out
//!
//! Setting the environment variable `AOT_NO_WATCHDOG` (to any value) disables
//! the watchdog entirely. This lets a debugger or integration test attach
//! without the watchdog racing to restart the process when a breakpoint pause
//! looks like a stall.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::AppHandle;

// ── Pure, testable helpers ────────────────────────────────────────────────────

/// Returns `true` when the main thread has not acknowledged a watchdog ping
/// for longer than `limit_ms` milliseconds.
///
/// The comparison is strictly greater-than so that `now_ms - last_ack_ms ==
/// limit_ms` is NOT considered stalled — a thread that just barely made the
/// deadline should not trigger a restart. Saturating subtraction means a clock
/// that hasn't moved yet (last_ack_ms > now_ms) produces zero, which is never
/// stalled.
pub(crate) fn is_stalled(now_ms: u64, last_ack_ms: u64, limit_ms: u64) -> bool {
    now_ms.saturating_sub(last_ack_ms) > limit_ms
}

/// Decide whether a relaunch is allowed, given how many seconds ago each
/// previous relaunch happened (`recent_ages_secs`, in any order).
///
/// A relaunch is refused when:
/// - the most recent one was less than `min_gap_secs` ago (the fresh process
///   is probably still starting, or is itself stuck), OR
/// - `max_in_window` or more relaunches happened within the last
///   `window_secs` (a stall that recurs at every start must not become an
///   endless restart loop; after that the user keeps the frozen window and
///   `hang.log` keeps the evidence).
pub(crate) fn should_relaunch(
    recent_ages_secs: &[u64],
    min_gap_secs: u64,
    max_in_window: usize,
    window_secs: u64,
) -> bool {
    if recent_ages_secs.iter().any(|&age| age < min_gap_secs) {
        return false;
    }
    recent_ages_secs.iter().filter(|&&age| age < window_secs).count() < max_in_window
}

/// Returns `true` when the watchdog thread itself was not scheduled for far
/// longer than its own sleep, i.e. the machine was suspended or hibernated (or
/// the clock jumped). The wall clock keeps counting across a suspend, so on
/// resume `now - last_ack` looks like a multi-hour main-thread stall and the
/// app would "recover" from a hang that never happened, relaunching itself
/// after every wake-up. The loop therefore compares successive wall-clock
/// readings of its OWN iterations: a healthy loop advances by about
/// `PING_INTERVAL`, a resumed one by minutes or hours.
pub(crate) fn resumed_from_suspend(now_ms: u64, prev_loop_ms: u64, gap_ms: u64) -> bool {
    now_ms.saturating_sub(prev_loop_ms) > gap_ms
}

/// Format one line for `hang.log`.
///
/// Includes:
/// - Unix timestamp (seconds) so log lines are sortable without a real logger.
/// - Stall duration in whole seconds (derived from `stalled_ms`), rounding
///   down, so a 25 100 ms stall is reported as "25s".
/// - The most-recent window-activity summary from `scraper_engine`.
/// - What the watchdog did about it (`action`).
pub(crate) fn format_hang_line(unix_secs: u64, stalled_ms: u64, activity: &str, action: &str) -> String {
    format!(
        "[{unix_secs}] main thread stalled {stalled_secs}s; activity: {activity}; action: {action}\n",
        stalled_secs = stalled_ms / 1000,
    )
}

/// Parse the relaunch history file (one unix timestamp in seconds per line;
/// anything unparsable is skipped) into "seconds ago" values relative to
/// `now_secs`. Timestamps in the future (clock moved back) count as age 0, the
/// conservative reading: a relaunch is then treated as very recent.
pub(crate) fn parse_relaunch_ages(contents: &str, now_secs: u64) -> Vec<u64> {
    contents
        .lines()
        .filter_map(|l| l.trim().parse::<u64>().ok())
        .map(|t| now_secs.saturating_sub(t))
        .collect()
}

// ── Watchdog thread ───────────────────────────────────────────────────────────

/// Start the watchdog background thread.
///
/// The thread is named `ui-watchdog` and runs for the lifetime of the process.
/// It is a plain `std::thread` (not a tokio task) so that it keeps running
/// even if the main thread is blocked — a tokio task running on a blocked
/// thread cannot detect that the thread is blocked.
///
/// `data_dir` must be the app-data directory (the same `dir` used in
/// `lib.rs`'s setup closure) so that `hang.log` and `hang-relaunch.marker`
/// land next to `panic.log` and the database.
///
/// Does nothing (returns immediately) when the `AOT_NO_WATCHDOG` environment
/// variable is set, so a debugger session is never affected.
pub fn start(app: AppHandle, data_dir: PathBuf) {
    // Allow the developer/CI to opt out without recompiling.
    if std::env::var_os("AOT_NO_WATCHDOG").is_some() {
        return;
    }

    // Shared millisecond timestamp of the last main-thread acknowledgement.
    // Initialised to "now" so the first 25 s of startup are never flagged.
    let ack_ms: Arc<AtomicU64> = Arc::new(AtomicU64::new(now_ms()));

    let spawn_result = std::thread::Builder::new()
        .name("ui-watchdog".to_string())
        .spawn(move || watchdog_loop(app, data_dir, ack_ms));
    // spawn() can fail only on resource exhaustion; the app still works
    // without the watchdog, so we log the error but do not propagate it.
    // The JoinHandle is intentionally dropped: the thread lives as long as
    // the process.
    if let Err(e) = spawn_result {
        eprintln!("[watchdog] failed to spawn thread: {e}");
    }
}

/// Ping interval: how often the watchdog posts a closure to the main thread.
/// Shorter intervals reduce detection latency but add overhead. Two seconds is
/// imperceptible overhead even on a heavily loaded machine.
const PING_INTERVAL: Duration = Duration::from_secs(2);

/// Silence after which the stall is written to `hang.log` (evidence only).
const LOG_AFTER_MS: u64 = 25_000;

/// Silence after which the app is relaunched. Deliberately well above
/// `LOG_AFTER_MS`: synchronous commands run on the main thread and a heavy one
/// (catalog linking, a large import) may legitimately keep it busy for a
/// while, whereas a hung `WebviewWindowBuilder::build()` never returns. A user
/// staring at a frozen window for a minute is the worse outcome we accept.
const RELAUNCH_AFTER_MS: u64 = 60_000;

/// Minimum seconds between two relaunches.
const RELAUNCH_GAP_SECS: u64 = 120;

/// At most this many relaunches within `RELAUNCH_WINDOW_SECS`.
const RELAUNCH_MAX_IN_WINDOW: usize = 2;

/// Sliding window for `RELAUNCH_MAX_IN_WINDOW`.
const RELAUNCH_WINDOW_SECS: u64 = 600;

/// If one loop iteration took longer than this (the sleep is 2 s), the
/// machine was almost certainly suspended: forgive the gap instead of
/// treating it as a stall. See `resumed_from_suspend`.
const RESUME_GAP_MS: u64 = 10_000;

fn watchdog_loop(app: AppHandle, data_dir: PathBuf, ack_ms: Arc<AtomicU64>) {
    // One report (and at most one relaunch decision) per stall episode, not
    // one per ping while the same stall continues.
    let mut logged_this_stall = false;
    let mut decided_this_stall = false;
    let mut prev_loop_ms = now_ms();
    let marker_path = data_dir.join("hang-relaunch.marker");
    let log_path = data_dir.join("hang.log");

    loop {
        std::thread::sleep(PING_INTERVAL);

        // After a suspend/hibernate the wall clock has jumped but the main
        // thread never stalled: re-baseline the ack and start over.
        let woke_at = now_ms();
        if resumed_from_suspend(woke_at, prev_loop_ms, RESUME_GAP_MS) {
            ack_ms.store(woke_at, Ordering::Relaxed);
            logged_this_stall = false;
            decided_this_stall = false;
            prev_loop_ms = woke_at;
            continue;
        }
        prev_loop_ms = woke_at;

        // Post a closure to the main thread. The closure's only job is to
        // store the current time so we can measure how long it took. Relaxed
        // ordering is enough: the value is only compared by this thread and
        // written by main-thread closures, which run strictly sequentially.
        // run_on_main_thread returns an error only when the runtime is
        // shutting down; at that point the process is exiting anyway.
        let ack_for_closure = Arc::clone(&ack_ms);
        let _ = app.run_on_main_thread(move || {
            ack_for_closure.store(now_ms(), Ordering::Relaxed);
        });

        let current_ms = now_ms();
        let last_ack = ack_ms.load(Ordering::Relaxed);
        let stalled_ms = current_ms.saturating_sub(last_ack);

        if !is_stalled(current_ms, last_ack, LOG_AFTER_MS) {
            // Main thread is alive (again) — the next stall episode gets its
            // own report.
            logged_this_stall = false;
            decided_this_stall = false;
            continue;
        }

        let unix_secs = current_ms / 1000;
        if !logged_this_stall {
            logged_this_stall = true;
            let activity = crate::scraper_engine::describe_window_activity();
            let line = format_hang_line(unix_secs, stalled_ms, &activity, "watching");
            append_to_log(&log_path, &line);
        }

        if !decided_this_stall && is_stalled(current_ms, last_ack, RELAUNCH_AFTER_MS) {
            decided_this_stall = true;
            let ages = read_relaunch_ages(&marker_path, unix_secs);
            let allowed =
                should_relaunch(&ages, RELAUNCH_GAP_SECS, RELAUNCH_MAX_IN_WINDOW, RELAUNCH_WINDOW_SECS);
            // Record the relaunch *before* spawning, so even if the spawn call
            // itself blocks briefly the child sees it in the history. If it
            // cannot be recorded (unwritable data dir) do NOT relaunch: the
            // quota would never be enforced and a stall at every start could
            // restart the app for ever.
            let will_relaunch = allowed && record_relaunch(&marker_path, unix_secs).is_ok();
            let activity = crate::scraper_engine::describe_window_activity();
            let action = if will_relaunch {
                "relaunching"
            } else if allowed {
                "logging only (cannot record the relaunch)"
            } else {
                "logging only (relaunch limit)"
            };
            append_to_log(&log_path, &format_hang_line(unix_secs, stalled_ms, &activity, action));
            if will_relaunch {
                spawn_and_exit();
            }
        }
    }
}

/// Milliseconds since the Unix epoch.  Saturates to 0 on pre-epoch or
/// extremely old systems rather than panicking.
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Read the relaunch history as "seconds ago" values. A missing or unreadable
/// file is an empty history.
fn read_relaunch_ages(path: &std::path::Path, now_secs: u64) -> Vec<u64> {
    std::fs::read_to_string(path)
        .map(|c| parse_relaunch_ages(&c, now_secs))
        .unwrap_or_default()
}

/// Append this relaunch to the history, keeping only the 10 most recent
/// entries so the file cannot grow. The caller must not relaunch when this
/// fails, otherwise the quota could never be enforced.
fn record_relaunch(path: &std::path::Path, now_secs: u64) -> std::io::Result<()> {
    let mut lines: Vec<String> = std::fs::read_to_string(path)
        .map(|c| c.lines().map(str::to_owned).collect())
        .unwrap_or_default();
    lines.push(now_secs.to_string());
    let keep_from = lines.len().saturating_sub(10);
    std::fs::write(path, lines[keep_from..].join("\n") + "\n")
}

/// Append `line` to `path`, creating the file if absent. Silently ignores
/// I/O errors so the watchdog never panics over a log-write failure.
fn append_to_log(path: &std::path::Path, line: &str) {
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = f.write_all(line.as_bytes());
    }
}

/// Spawn a fresh copy of the current executable and terminate this process.
///
/// `std::process::Command` detaches on Windows because we call neither
/// `.wait()` nor `.wait_with_output()` — the child process is created with
/// its own process group and lives independently of the parent.
fn spawn_and_exit() {
    if let Ok(exe) = std::env::current_exe() {
        let _ = std::process::Command::new(exe).spawn();
    }
    std::process::exit(0);
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── is_stalled ────────────────────────────────────────────────────────────

    #[test]
    fn stalled_when_gap_exceeds_limit() {
        // 30 000 - 4 000 = 26 000 > 25 000 → stalled
        assert!(is_stalled(30_000, 4_000, 25_000));
    }

    #[test]
    fn not_stalled_when_gap_equals_limit() {
        // 29 000 - 4 000 = 25 000, which is NOT > 25 000 → not stalled
        assert!(!is_stalled(29_000, 4_000, 25_000));
    }

    #[test]
    fn not_stalled_when_gap_less_than_limit() {
        assert!(!is_stalled(10_000, 4_000, 25_000));
    }

    #[test]
    fn not_stalled_when_clock_would_go_backwards() {
        // last_ack is in the future relative to now → saturates to 0.
        assert!(!is_stalled(1_000, 5_000, 25_000));
    }

    #[test]
    fn not_stalled_at_zero_zero() {
        assert!(!is_stalled(0, 0, 25_000));
    }

    #[test]
    fn the_relaunch_threshold_is_stricter_than_the_logging_threshold() {
        // A 40 s stall is logged but must not yet relaunch; 61 s does both.
        assert!(is_stalled(40_000, 0, LOG_AFTER_MS));
        assert!(!is_stalled(40_000, 0, RELAUNCH_AFTER_MS));
        assert!(is_stalled(61_000, 0, RELAUNCH_AFTER_MS));
    }

    // ── resumed_from_suspend ──────────────────────────────────────────────────

    #[test]
    fn resume_from_suspend_is_detected_only_for_a_big_loop_gap() {
        // A healthy iteration advances by ~2 s: not a resume.
        assert!(!resumed_from_suspend(12_000, 10_000, 10_000));
        // Exactly the gap is still not a resume (strictly greater).
        assert!(!resumed_from_suspend(20_000, 10_000, 10_000));
        // Hours asleep: resume, so the huge `now - last_ack` must be forgiven.
        assert!(resumed_from_suspend(3 * 3_600_000, 10_000, 10_000));
        // The clock going backwards is never a resume.
        assert!(!resumed_from_suspend(1_000, 5_000, 10_000));
    }

    // ── should_relaunch ───────────────────────────────────────────────────────

    #[test]
    fn relaunch_allowed_with_no_history() {
        assert!(should_relaunch(&[], 120, 2, 600));
    }

    #[test]
    fn no_relaunch_when_the_last_one_is_too_recent() {
        assert!(!should_relaunch(&[10], 120, 2, 600));
        // One second short of the minimum gap.
        assert!(!should_relaunch(&[119], 120, 2, 600));
    }

    #[test]
    fn relaunch_allowed_exactly_at_the_minimum_gap() {
        assert!(should_relaunch(&[120], 120, 2, 600));
    }

    #[test]
    fn relaunch_allowed_for_one_old_entry() {
        assert!(should_relaunch(&[900], 120, 2, 600));
    }

    #[test]
    fn relaunch_refused_once_the_window_quota_is_used() {
        // Two relaunches in the last 10 minutes (ages 130 s and 400 s), both
        // past the minimum gap: the third is refused — the loop is bounded.
        assert!(!should_relaunch(&[130, 400], 120, 2, 600));
        // History order does not matter.
        assert!(!should_relaunch(&[400, 130], 120, 2, 600));
    }

    #[test]
    fn relaunch_entries_older_than_the_window_do_not_count() {
        // One in the window, one 700 s ago (outside the 600 s window): only
        // one counts, so a second relaunch is still allowed.
        assert!(should_relaunch(&[200, 700], 120, 2, 600));
    }

    // ── parse_relaunch_ages ───────────────────────────────────────────────────

    #[test]
    fn parse_relaunch_ages_reads_seconds_ago_and_skips_garbage() {
        assert_eq!(parse_relaunch_ages("1000\nnot-a-number\n\n1900\n", 2000), vec![1000, 100]);
        assert!(parse_relaunch_ages("", 2000).is_empty());
    }

    #[test]
    fn parse_relaunch_ages_treats_a_future_timestamp_as_just_now() {
        // Clock moved back: the entry reads as age 0 (very recent), the safe side.
        assert_eq!(parse_relaunch_ages("5000\n", 2000), vec![0]);
    }

    // ── record_relaunch ───────────────────────────────────────────────────────

    #[test]
    fn record_relaunch_keeps_the_ten_newest_and_reports_failure() {
        let dir = std::env::temp_dir().join(format!("aot-wd-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("marker");
        let _ = std::fs::remove_file(&path);
        for t in 1..=12u64 {
            record_relaunch(&path, t).unwrap();
        }
        let ages = parse_relaunch_ages(&std::fs::read_to_string(&path).unwrap(), 12);
        assert_eq!(ages.len(), 10, "history is pruned to the 10 newest entries");
        assert_eq!(ages.iter().copied().max(), Some(9), "entries 1 and 2 were dropped: the oldest kept is 3, i.e. 9 s ago");
        // An unwritable location is an error, so the caller will not relaunch.
        assert!(record_relaunch(&dir.join("no-such-dir").join("marker"), 1).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── format_hang_line ──────────────────────────────────────────────────────

    #[test]
    fn format_line_contains_stall_seconds() {
        let line = format_hang_line(1_700_000_000, 26_400, "x", "watching");
        assert!(line.contains("stalled 26s"), "{line}");
    }

    #[test]
    fn format_line_contains_activity_text() {
        let line = format_hang_line(1_700_000_000, 30_000, "scraper build ok: example.com", "watching");
        assert!(line.contains("scraper build ok: example.com"), "{line}");
    }

    #[test]
    fn format_line_contains_the_action() {
        assert!(format_hang_line(1, 1_000, "a", "relaunching").contains("action: relaunching"));
        assert!(format_hang_line(1, 1_000, "a", "watching").contains("action: watching"));
    }

    #[test]
    fn format_line_ends_with_newline() {
        assert!(format_hang_line(1, 1_000, "a", "watching").ends_with('\n'));
    }
}
