use super::*;
use chrono::Local;
use std::path::Path;
use tauri::{AppHandle, Manager, State};

pub const MAX_LOG_LINES: usize = 200;
pub const MAX_LOG_CHARS: usize = 20_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiagnosticsInfo {
    pub app_version: String,
    pub os: String,
    pub generated_at: String,
    pub series_count: i64,
    pub followed_count: i64,
    pub catalog_count: i64,
    pub setting_keys: Vec<String>,
    pub panic_log_tail: Option<String>,
    pub hang_log_tail: Option<String>,
}

pub fn is_sensitive_setting_key(key: &str) -> bool {
    let lower = key.to_ascii_lowercase();
    lower.starts_with("google_")
        || lower.starts_with("gdrive_")
        || lower.starts_with("backup_")
        || lower.starts_with("anilist_token")
        || lower.contains("token")
        || lower.contains("secret")
        || lower.contains("password")
}

pub fn filter_setting_keys<I, S>(keys: I) -> Vec<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut filtered: Vec<String> = keys
        .into_iter()
        .map(|s| s.as_ref().to_string())
        .filter(|k| !is_sensitive_setting_key(k))
        .collect();
    filtered.sort();
    filtered.dedup();
    filtered
}

pub fn tail_lines(text: &str, n: usize) -> String {
    if text.is_empty() || n == 0 {
        return String::new();
    }
    let lines: Vec<&str> = text.lines().collect();
    if lines.is_empty() {
        return String::new();
    }
    let start = lines.len().saturating_sub(n);
    lines[start..].join("\n")
}

pub fn cap_tail(text: &str, max_chars: usize) -> String {
    if max_chars == 0 {
        return String::new();
    }
    let char_count = text.chars().count();
    if char_count <= max_chars {
        return text.to_string();
    }
    let skip = char_count - max_chars;
    match text.char_indices().nth(skip) {
        Some((byte_offset, _)) => text[byte_offset..].to_string(),
        None => String::new(),
    }
}

pub fn is_path_inside_dir(path: &Path, dir: &Path) -> bool {
    let Ok(canon_dir) = dir.canonicalize() else {
        return false;
    };
    let Ok(canon_path) = path.canonicalize() else {
        return false;
    };
    canon_path.starts_with(&canon_dir) && canon_path != canon_dir
}

pub fn build_report(info: &DiagnosticsInfo) -> String {
    let mut out = String::new();
    out.push_str("=== AnimeOnTrack Diagnostics Report ===\n\n");
    out.push_str(&format!("app_version: {}\n", info.app_version));
    out.push_str(&format!("os: {}\n", info.os));
    out.push_str(&format!("generated_at: {}\n", info.generated_at));
    out.push_str(&format!("series_count: {}\n", info.series_count));
    out.push_str(&format!("followed_count: {}\n", info.followed_count));
    out.push_str(&format!("catalog_count: {}\n", info.catalog_count));

    let keys = filter_setting_keys(&info.setting_keys);
    out.push_str("\nSettings keys (values are never exported):\n");
    if keys.is_empty() {
        out.push_str("(none)\n");
    } else {
        for key in &keys {
            out.push_str(&format!("- {key}\n"));
        }
    }

    out.push_str(&format!("\npanic.log (last {MAX_LOG_LINES} lines):\n"));
    match &info.panic_log_tail {
        Some(tail) if !tail.trim().is_empty() => {
            out.push_str(tail);
            if !tail.ends_with('\n') {
                out.push('\n');
            }
        }
        _ => out.push_str("(none)\n"),
    }

    out.push_str(&format!("\nhang.log (last {MAX_LOG_LINES} lines):\n"));
    match &info.hang_log_tail {
        Some(tail) if !tail.trim().is_empty() => {
            out.push_str(tail);
            if !tail.ends_with('\n') {
                out.push('\n');
            }
        }
        _ => out.push_str("(none)\n"),
    }

    out
}

fn query_series_count(conn: &rusqlite::Connection) -> Result<i64, rusqlite::Error> {
    conn.query_row("SELECT COUNT(*) FROM series", [], |r| r.get(0))
}

fn query_followed_count(conn: &rusqlite::Connection) -> Result<i64, rusqlite::Error> {
    conn.query_row("SELECT COUNT(*) FROM series WHERE followed = 1", [], |r| r.get(0))
}

fn query_catalog_count(conn: &rusqlite::Connection) -> Result<i64, rusqlite::Error> {
    conn.query_row("SELECT COUNT(*) FROM anilist_catalog", [], |r| r.get(0))
}

fn query_setting_keys(conn: &rusqlite::Connection) -> Result<Vec<String>, rusqlite::Error> {
    let mut stmt = conn.prepare("SELECT key FROM settings")?;
    let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
    Ok(rows.flatten().collect())
}

fn read_log_tail(path: &Path) -> Option<String> {
    if !path.exists() {
        return None;
    }
    let content = std::fs::read_to_string(path).ok()?;
    if content.trim().is_empty() {
        return None;
    }
    let tailed = tail_lines(&content, MAX_LOG_LINES);
    let capped = cap_tail(&tailed, MAX_LOG_CHARS);
    if capped.trim().is_empty() {
        None
    } else {
        Some(capped)
    }
}

#[tauri::command]
pub fn export_diagnostics(app: AppHandle, state: State<'_, AppState>) -> Result<String, String> {
    let (series_count, followed_count, catalog_count, raw_keys) = {
        let db = state.db.lock().unwrap();
        let s_count = query_series_count(&db.conn).unwrap_or(0);
        let f_count = query_followed_count(&db.conn).unwrap_or(0);
        let c_count = query_catalog_count(&db.conn).unwrap_or(0);
        let keys = query_setting_keys(&db.conn).unwrap_or_default();
        (s_count, f_count, c_count, keys)
    };

    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;

    let panic_log_tail = read_log_tail(&dir.join("panic.log"));
    let hang_log_tail = read_log_tail(&dir.join("hang.log"));

    let now = Local::now();
    let generated_at = now.format("%Y-%m-%d %H:%M:%S").to_string();
    let filename = now.format("diagnostics-%Y%m%d-%H%M%S.txt").to_string();
    let report_path = dir.join(filename);

    let info = DiagnosticsInfo {
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        os: std::env::consts::OS.to_string(),
        generated_at,
        series_count,
        followed_count,
        catalog_count,
        setting_keys: filter_setting_keys(raw_keys),
        panic_log_tail,
        hang_log_tail,
    };

    let report = build_report(&info);
    std::fs::write(&report_path, report)
        .map_err(|e| format!("cannot write diagnostics report: {e}"))?;

    Ok(report_path.to_string_lossy().to_string())
}

#[tauri::command]
pub fn reveal_in_folder(app: AppHandle, path: String) -> Result<(), String> {
    let app_data_dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    let target = Path::new(&path);

    if !is_path_inside_dir(target, &app_data_dir) {
        return Err("path is not inside app data directory".into());
    }

    #[cfg(windows)]
    {
        let canon = target.canonicalize().map_err(|e| e.to_string())?;
        let canon_str = canon.to_string_lossy();
        let display_path = canon_str.strip_prefix(r"\\?\").unwrap_or(&canon_str);
        std::process::Command::new("explorer")
            .arg(format!("/select,{display_path}"))
            .spawn()
            .map_err(|e| format!("failed to launch explorer: {e}"))?;
        Ok(())
    }

    #[cfg(not(windows))]
    {
        Err("not supported".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tail_lines_cases() {
        assert_eq!(tail_lines("a\nb\nc\nd", 2), "c\nd");
        assert_eq!(tail_lines("a\nb\nc\nd", 10), "a\nb\nc\nd");
        assert_eq!(tail_lines("", 5), "");
        assert_eq!(tail_lines("single line", 1), "single line");
    }

    #[test]
    fn test_build_report_contains_version_os_counts_and_sorted_keys() {
        let info = DiagnosticsInfo {
            app_version: "0.8.0".into(),
            os: "windows".into(),
            generated_at: "2026-10-14 01:25:30".into(),
            series_count: 42,
            followed_count: 10,
            catalog_count: 22000,
            setting_keys: vec!["theme".into(), "active_site_id".into()],
            panic_log_tail: None,
            hang_log_tail: None,
        };
        let report = build_report(&info);
        assert!(report.contains("0.8.0"));
        assert!(report.contains("windows"));
        assert!(report.contains("42"));
        assert!(report.contains("10"));
        assert!(report.contains("22000"));
        assert!(report.contains("active_site_id"));
        assert!(report.contains("theme"));
        let pos_site = report.find("active_site_id").unwrap();
        let pos_theme = report.find("theme").unwrap();
        assert!(pos_site < pos_theme);
        assert!(report.contains("(none)"));
    }

    #[test]
    fn test_build_report_with_logs() {
        let info = DiagnosticsInfo {
            app_version: "0.8.0".into(),
            os: "windows".into(),
            generated_at: "2026-10-14 01:25:30".into(),
            series_count: 5,
            followed_count: 2,
            catalog_count: 100,
            setting_keys: vec!["site_priority".into()],
            panic_log_tail: Some("panic at line 42".into()),
            hang_log_tail: Some("hang detected".into()),
        };
        let report = build_report(&info);
        assert!(report.contains("panic at line 42"));
        assert!(report.contains("hang detected"));
        assert!(report.contains("site_priority"));
    }

    #[test]
    fn test_key_filtering_drops_sensitive_keys() {
        let raw_keys = vec![
            "google_client_secret".to_string(),
            "gdrive_file_id".to_string(),
            "backup_sig".to_string(),
            "anilist_token".to_string(),
            "my_API_TOKEN".to_string(),
            "site_priority".to_string(),
            "active_site_id".to_string(),
        ];
        let filtered = filter_setting_keys(&raw_keys);
        assert_eq!(filtered, vec!["active_site_id", "site_priority"]);
    }

    #[test]
    fn test_secret_value_and_key_never_appear() {
        let secret_val = "SUPER_SECRET_12345";
        let raw_keys = vec![
            "google_client_secret".to_string(),
            secret_val.to_string(),
            "active_site_id".to_string(),
        ];
        let filtered = filter_setting_keys(&raw_keys);
        let info = DiagnosticsInfo {
            app_version: "0.8.0".into(),
            os: "windows".into(),
            generated_at: "2026-10-14 01:25:30".into(),
            series_count: 1,
            followed_count: 1,
            catalog_count: 1,
            setting_keys: filtered,
            panic_log_tail: None,
            hang_log_tail: None,
        };
        let report = build_report(&info);
        assert!(!report.contains("google_client_secret"));
        assert!(!report.contains(secret_val));
        assert!(report.contains("active_site_id"));
    }

    #[test]
    fn test_tail_cap_keeps_end_of_text() {
        let text = "x".repeat(30_000) + &"y".repeat(20_000);
        assert_eq!(text.chars().count(), 50_000);
        let capped = cap_tail(&text, 20_000);
        assert_eq!(capped.chars().count(), 20_000);
        assert_eq!(capped, "y".repeat(20_000));
    }

    #[test]
    fn test_path_inside_dir_validation() {
        let temp_dir = std::env::temp_dir();
        let sub_dir = temp_dir.join("aot_test_sub_diag");
        let _ = std::fs::create_dir_all(&sub_dir);
        let file_inside = sub_dir.join("inside.txt");
        std::fs::write(&file_inside, "inside").unwrap();
        let file_outside = temp_dir.join("aot_test_outside_diag.txt");
        std::fs::write(&file_outside, "outside").unwrap();

        assert!(is_path_inside_dir(&file_inside, &sub_dir));
        assert!(!is_path_inside_dir(&file_outside, &sub_dir));
        assert!(!is_path_inside_dir(&sub_dir, &sub_dir));

        let _ = std::fs::remove_file(file_inside);
        let _ = std::fs::remove_dir(sub_dir);
        let _ = std::fs::remove_file(file_outside);
    }
}
