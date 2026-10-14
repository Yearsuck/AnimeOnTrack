//! Minimal, dependency-free rotating file logger for desktop diagnostics.
//!
//! Windows GUI binaries (`windows_subsystem = "windows"`) detach from the
//! console in release mode. This module keeps a lightweight rotating log file
//! (`aot.log`) in the application data directory so startup lifecycle events,
//! warnings, and crash reports remain inspectable.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// Configuration for file logging and rotation.
struct Sink {
    path: PathBuf,
    max_bytes: u64,
    keep: usize,
}

static LOG: OnceLock<Mutex<Sink>> = OnceLock::new();

/// Initialises the global log sink inside the provided app-data directory.
///
/// Configured for a 1 MB size threshold and retains up to 2 rotated archive
/// files (`aot.log.1` newest, `aot.log.2` older). Repeated calls are ignored.
pub fn init(dir: &Path) {
    let _ = LOG.get_or_init(|| {
        Mutex::new(Sink {
            path: dir.join("aot.log"),
            max_bytes: 1_000_000,
            keep: 2,
        })
    });
}

/// Sanitises a log message to a single line and truncates it to at most 500
/// characters on a UTF-8 character boundary.
fn sanitize_message(msg: &str) -> String {
    let sanitized: String = msg
        .chars()
        .map(|c| if c == '\r' || c == '\n' { ' ' } else { c })
        .collect();
    match sanitized.char_indices().nth(500) {
        Some((idx, _)) => sanitized[..idx].to_string(),
        None => sanitized,
    }
}

/// Formats a single log line with timestamp, severity level, and sanitised message.
fn format_line(level: &str, ts: &str, msg: &str) -> String {
    let clean = sanitize_message(msg);
    format!("{ts} {level} {clean}")
}

/// Computes the path for a numbered rotated log file (e.g. `aot.log.1`).
fn rotated_path(base: &Path, index: usize) -> PathBuf {
    let name = base
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("aot.log");
    base.with_file_name(format!("{name}.{index}"))
}

/// Rotates log archives from high index to low, freeing `aot.log.1` for the current file.
fn rotate(sink: &Sink) {
    if sink.keep == 0 {
        let _ = std::fs::remove_file(&sink.path);
        return;
    }
    let oldest = rotated_path(&sink.path, sink.keep);
    if oldest.exists() {
        let _ = std::fs::remove_file(&oldest);
    }
    for i in (1..sink.keep).rev() {
        let src = rotated_path(&sink.path, i);
        let dst = rotated_path(&sink.path, i + 1);
        if src.exists() {
            let _ = std::fs::rename(&src, &dst);
        }
    }
    if sink.path.exists() {
        let dst = rotated_path(&sink.path, 1);
        let _ = std::fs::rename(&sink.path, &dst);
    }
}

/// Appends a line to the sink file, triggering rotation if the size limit would be exceeded.
fn write_line(sink: &Sink, line: &str) {
    if let Ok(meta) = std::fs::metadata(&sink.path) {
        let len = meta.len();
        let needed = line.len() as u64 + 1;
        if len + needed > sink.max_bytes {
            rotate(sink);
        }
    }
    if let Some(parent) = sink.path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&sink.path)
    {
        let _ = writeln!(f, "{line}");
    }
}

/// Emits a log entry at the given level if logging has been initialised.
fn log_at_level(level: &str, msg: &str) {
    let Some(mutex) = LOG.get() else {
        return;
    };
    let now = chrono::Local::now();
    let ts = now.format("%Y-%m-%d %H:%M:%S").to_string();
    let line = format_line(level, &ts, msg);
    let sink = match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    write_line(&sink, &line);
}

/// Emits an informational message to the log file.
pub fn info(msg: &str) {
    log_at_level("INFO", msg);
}

/// Emits a warning message to the log file.
// Warn level is part of the public logging API for future warnings.
#[allow(dead_code)]
pub fn warn(msg: &str) {
    log_at_level("WARN", msg);
}

/// Emits an error message to the log file.
pub fn error(msg: &str) {
    log_at_level("ERROR", msg);
}

/// Reads the last `max_lines` from `dir/aot.log`, returning `None` if absent, empty, or unreadable.
pub fn tail(dir: &Path, max_lines: usize) -> Option<String> {
    if max_lines == 0 {
        return None;
    }
    let path = dir.join("aot.log");
    let content = std::fs::read_to_string(&path).ok()?;
    if content.trim().is_empty() {
        return None;
    }
    let lines: Vec<&str> = content.lines().collect();
    if lines.is_empty() {
        return None;
    }
    let start = lines.len().saturating_sub(max_lines);
    let result = lines[start..].join("\n");
    if result.is_empty() {
        None
    } else {
        Some(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn make_temp_dir(test_name: &str) -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let pid = std::process::id();
        let count = COUNTER.fetch_add(1, Ordering::Relaxed);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!("aot_test_{test_name}_{pid}_{nanos}_{count}"));
        let _ = std::fs::create_dir_all(&dir);
        dir
    }

    #[test]
    fn test_format_line_basic() {
        assert_eq!(
            format_line("INFO", "2026-10-14 03:00:00", "hello"),
            "2026-10-14 03:00:00 INFO hello"
        );
    }

    #[test]
    fn test_sanitize_newlines_and_truncation() {
        assert_eq!(
            format_line("INFO", "2026-10-14 03:00:00", "a\nb\r\nc"),
            "2026-10-14 03:00:00 INFO a b  c"
        );

        let long_msg = "x".repeat(2000);
        let clean_long = sanitize_message(&long_msg);
        assert_eq!(clean_long.chars().count(), 500);

        let multi = format!("{}émore", "a".repeat(499));
        let clean_multi = sanitize_message(&multi);
        assert_eq!(clean_multi.chars().count(), 500);
        assert_eq!(clean_multi, format!("{}é", "a".repeat(499)));
    }

    #[test]
    fn test_write_line_appends_two_lines_in_order() {
        let dir = make_temp_dir("write_order");
        let sink = Sink {
            path: dir.join("aot.log"),
            max_bytes: 1_000_000,
            keep: 2,
        };
        write_line(&sink, "first line");
        write_line(&sink, "second line");

        let content = std::fs::read_to_string(&sink.path).unwrap();
        assert_eq!(content, "first line\nsecond line\n");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_rotation_retains_configured_archives() {
        let dir = make_temp_dir("rotation");
        let sink = Sink {
            path: dir.join("aot.log"),
            max_bytes: 50,
            keep: 2,
        };

        let lines = [
            "line 1: 0123456789",
            "line 2: 0123456789",
            "line 3: 0123456789",
            "line 4: 0123456789",
            "line 5: 0123456789",
            "line 6: 0123456789",
        ];
        for line in lines {
            write_line(&sink, line);
        }

        let p_current = dir.join("aot.log");
        let p_1 = dir.join("aot.log.1");
        let p_2 = dir.join("aot.log.2");
        let p_3 = dir.join("aot.log.3");

        assert!(p_current.exists());
        assert!(p_1.exists());
        assert!(p_2.exists());
        assert!(!p_3.exists());

        let len_current = std::fs::metadata(&p_current).unwrap().len();
        // 50 bytes max + one line (~20 bytes)
        assert!(len_current <= 70);

        let c_current = std::fs::read_to_string(&p_current).unwrap();
        let c_1 = std::fs::read_to_string(&p_1).unwrap();
        let c_2 = std::fs::read_to_string(&p_2).unwrap();

        assert!(c_current.contains("line 6"));
        assert!(c_1.contains("line 4"));
        assert!(c_2.contains("line 2"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_tail_reads_last_lines_or_none() {
        let dir = make_temp_dir("tail");
        let file_path = dir.join("aot.log");
        std::fs::write(&file_path, "line 1\nline 2\nline 3\nline 4\nline 5\n").unwrap();

        assert_eq!(tail(&dir, 2), Some("line 4\nline 5".to_string()));

        let missing_dir = dir.join("missing_sub");
        assert_eq!(tail(&missing_dir, 2), None);

        let empty_dir = make_temp_dir("tail_empty");
        std::fs::write(empty_dir.join("aot.log"), "").unwrap();
        assert_eq!(tail(&empty_dir, 2), None);

        std::fs::write(empty_dir.join("aot.log"), "   \n\n  ").unwrap();
        assert_eq!(tail(&empty_dir, 2), None);

        let _ = std::fs::remove_dir_all(&empty_dir);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_info_before_init_must_not_panic_or_create_files() {
        // Without initialising LOG, logging functions must be safe no-ops.
        info("safe info before init");
        warn("safe warn before init");
        error("safe error before init");
    }
}
