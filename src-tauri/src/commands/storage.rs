use super::*;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrphanFile {
    pub path: String,
    pub size_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StorageReport {
    pub downloads_bytes: u64,
    pub sqlite_bytes: u64,
    pub covers_bytes: u64,
    pub logs_bytes: u64,
    pub orphan_bytes: u64,
}

/// Whether `p` is an in-flight `.part` file (any case).
fn is_part_file(p: &Path) -> bool {
    p.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("part"))
}

/// The episode id a download file is named after (`<episode_id>.<ext>`).
fn episode_id_of(p: &Path) -> Option<i64> {
    p.file_stem()?.to_str()?.parse().ok()
}

/// Strips Windows verbatim/extended-length prefixes (`\\?\` or `\\?\UNC\`) from a path.
/// Returns a normalized `PathBuf`.
/// On non-Windows platforms, this is a no-op that returns `p.to_path_buf()`.
pub fn strip_verbatim(p: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        let s = p.to_string_lossy();
        if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
            return PathBuf::from(format!(r"\\{}", rest));
        }
        if let Some(rest) = s.strip_prefix(r"\\?\") {
            return PathBuf::from(rest);
        }
        p.to_path_buf()
    }
    #[cfg(not(windows))]
    {
        p.to_path_buf()
    }
}

/// Everything that protects a file from being treated as an orphan: the
/// canonical paths of finished downloads, and the ids of episodes whose
/// download is running or being finalized (covers the moment between the `.part`
/// rename and the row update).
#[derive(Default)]
pub struct Tracked {
    pub paths: HashSet<PathBuf>,
    pub active_ids: HashSet<i64>,
}

impl Tracked {
    fn covers(&self, p: &Path) -> bool {
        let canon = p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
        let stripped = strip_verbatim(&canon);
        self.paths.contains(&canon)
            || self.paths.contains(&stripped)
            || self.paths.iter().any(|tracked| strip_verbatim(tracked) == stripped)
            || episode_id_of(p).is_some_and(|id| self.active_ids.contains(&id))
    }
}

/// Validates that `file_path` is a regular file located strictly inside `base_dir`
/// without symlink escape.
pub fn is_safe_file_in_dir(base_dir: &Path, file_path: &Path) -> bool {
    if !file_path.is_file() {
        return false;
    }
    let (Ok(canon_base), Ok(canon_file)) = (base_dir.canonicalize(), file_path.canonicalize()) else {
        return false;
    };
    canon_file.starts_with(&canon_base) && canon_file != canon_base
}

/// Attempts to remove an empty directory strictly inside `base_dir` using `std::fs::remove_dir`.
/// Never uses `remove_dir_all`. Returns true if the directory was removed.
pub fn remove_empty_dir_in_dir(base_dir: &Path, dir_path: &Path) -> bool {
    if !dir_path.is_dir() {
        return false;
    }
    let (Ok(canon_base), Ok(canon_dir)) = (base_dir.canonicalize(), dir_path.canonicalize()) else {
        return false;
    };
    if !canon_dir.starts_with(&canon_base) || canon_dir == canon_base {
        return false;
    }
    std::fs::remove_dir(&canon_dir).is_ok()
}

/// Deletes the given downloaded file paths for a series and removes `<downloads_dir>/<series_id>`
/// if it becomes empty. Never uses `remove_dir_all`.
pub fn delete_series_files_and_dir(downloads_dir: &Path, series_id: i64, file_paths: &[String]) {
    for path_str in file_paths {
        let p = Path::new(path_str);
        if is_safe_file_in_dir(downloads_dir, p) {
            let _ = std::fs::remove_file(p);
        }
    }
    let series_dir = downloads_dir.join(series_id.to_string());
    remove_empty_dir_in_dir(downloads_dir, &series_dir);
}

/// Read-only walk of `downloads_dir` collecting files `tracked` does not cover.
/// Skips `*.part` files.
pub fn scan_orphans_in_dir(downloads_dir: &Path, tracked: &Tracked) -> Vec<OrphanFile> {
    let mut orphans = Vec::new();
    let Ok(entries) = std::fs::read_dir(downloads_dir) else {
        return orphans;
    };

    for entry in entries.flatten() {
        if entry.file_type().map(|t| t.is_symlink()).unwrap_or(true) {
            continue;
        }
        let series_dir = entry.path();
        if !series_dir.is_dir() {
            // Stray file in downloads root
            if series_dir.is_file() {
                if is_part_file(&series_dir) {
                    continue;
                }
                if !tracked.covers(&series_dir) {
                    let size_bytes = std::fs::metadata(&series_dir).map(|m| m.len()).unwrap_or(0);
                    orphans.push(OrphanFile {
                        path: series_dir.to_string_lossy().to_string(),
                        size_bytes,
                    });
                }
            }
            continue;
        }

        let Ok(files) = std::fs::read_dir(&series_dir) else {
            continue;
        };
        for f in files.flatten() {
            if f.file_type().map(|t| t.is_symlink()).unwrap_or(true) {
                continue;
            }
            let p = f.path();
            if !p.is_file() {
                continue;
            }
            if is_part_file(&p) {
                continue;
            }
            if !tracked.covers(&p) {
                let size_bytes = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
                orphans.push(OrphanFile {
                    path: p.to_string_lossy().to_string(),
                    size_bytes,
                });
            }
        }
    }

    orphans.sort_by(|a, b| a.path.cmp(&b.path));
    orphans
}

/// Re-validates each path is inside `downloads_dir` and still an orphan before deleting.
/// Best-effort removes parent series folder if left empty. Returns reclaimed bytes.
pub fn delete_orphans_in_dir(downloads_dir: &Path, paths: &[String], tracked: &Tracked) -> u64 {
    let Ok(canon_base) = downloads_dir.canonicalize() else {
        return 0;
    };
    let mut reclaimed = 0u64;
    for path_str in paths {
        let p = Path::new(path_str);
        if !is_safe_file_in_dir(downloads_dir, p) || is_part_file(p) {
            continue;
        }
        let Ok(canon) = p.canonicalize() else {
            continue;
        };
        // Only the two levels the scan lists: `downloads/<file>` and
        // `downloads/<series_id>/<file>`.
        let depth = canon.strip_prefix(&canon_base).map(|r| r.components().count()).unwrap_or(0);
        if !(1..=2).contains(&depth) {
            continue;
        }
        // Must still be an orphan at delete time.
        if tracked.covers(p) {
            continue;
        }
        let size = std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
        if std::fs::remove_file(p).is_ok() {
            reclaimed += size;
            if let Some(parent) = p.parent() {
                remove_empty_dir_in_dir(downloads_dir, parent);
            }
        }
    }
    reclaimed
}

/// Recursively computes total bytes of all files in a directory.
pub fn dir_size_recursive(dir: &Path) -> u64 {
    let mut total = 0u64;
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    for entry in entries.flatten() {
        // `file_type` does not follow links: a junction must not pull in files
        // outside the app folder or loop.
        let Ok(ft) = entry.file_type() else { continue };
        if ft.is_file() {
            total += entry.metadata().map(|m| m.len()).unwrap_or(0);
        } else if ft.is_dir() {
            total += dir_size_recursive(&entry.path());
        }
    }
    total
}

/// Size in bytes of SQLite database files (`animeontrack.sqlite*`).
pub fn sqlite_size_bytes(app_data_dir: &Path) -> u64 {
    let mut total = 0u64;
    let Ok(entries) = std::fs::read_dir(app_data_dir) else {
        return 0;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file() {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name.starts_with("animeontrack.sqlite") {
                total += std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            }
        }
    }
    total
}

/// Size in bytes of log files (`aot.log*`, `panic.log*`, `hang.log*`, `hang-relaunch.marker`).
pub fn logs_size_bytes(app_data_dir: &Path) -> u64 {
    let mut total = 0u64;
    let Ok(entries) = std::fs::read_dir(app_data_dir) else {
        return 0;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file() {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name.starts_with("aot.log")
                || name.starts_with("panic.log")
                || name.starts_with("hang.log")
                || name == "hang-relaunch.marker"
            {
                total += std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            }
        }
    }
    total
}

/// Computes the storage usage report across downloads, database, covers, logs, and orphans.
pub fn compute_storage_report(app_data_dir: &Path, tracked: &Tracked) -> StorageReport {
    let downloads_dir = app_data_dir.join("downloads");
    let covers_dir = app_data_dir.join("covers");
    let downloads_bytes = dir_size_recursive(&downloads_dir);
    let covers_bytes = dir_size_recursive(&covers_dir);
    let sqlite_bytes = sqlite_size_bytes(app_data_dir);
    let logs_bytes = logs_size_bytes(app_data_dir);
    let orphans = scan_orphans_in_dir(&downloads_dir, tracked);
    let orphan_bytes = orphans.iter().map(|o| o.size_bytes).sum();

    StorageReport {
        downloads_bytes,
        sqlite_bytes,
        covers_bytes,
        logs_bytes,
        orphan_bytes,
    }
}

/// If `log_path` exists and its size exceeds `max_bytes`, renames it to `<log_path>.1`,
/// overwriting any previous `.1` file.
pub fn rotate_log_if_exceeds(log_path: &Path, max_bytes: u64) -> std::io::Result<bool> {
    if let Ok(meta) = std::fs::metadata(log_path) {
        if meta.len() >= max_bytes {
            let mut backup_name = log_path.file_name().map(|n| n.to_os_string()).unwrap_or_default();
            backup_name.push(".1");
            let backup_path = log_path.with_file_name(backup_name);
            std::fs::rename(log_path, backup_path)?;
            return Ok(true);
        }
    }
    Ok(false)
}

// ---------------------------------------------------------------------------
// Tauri Commands
// ---------------------------------------------------------------------------

/// Snapshot of what is tracked: canonical paths of all recorded downloads from the DB,
/// and episode ids whose download is currently active in memory. `ACTIVE` is read
/// BEFORE the DB and the two locks are never held together: a download only moves
/// from `ACTIVE` to the DB (the row is written before the id leaves `ACTIVE`), so
/// an episode finishing between the two reads is seen by at least one of them.
fn tracked_downloads(state: &State<'_, AppState>) -> Result<Tracked, String> {
    let active_ids = crate::commands::download::active_episode_ids();
    let raw_paths = {
        let db = state.db.lock().unwrap();
        db.downloaded_episode_canonical_paths().map_err(|e| e.to_string())?
    };
    let mut paths = HashSet::with_capacity(raw_paths.len() * 2);
    for p in raw_paths {
        paths.insert(strip_verbatim(&p));
        paths.insert(p);
    }
    Ok(Tracked { paths, active_ids })
}

/// Read-only walk of the downloads directory returning untracked files.
#[tauri::command]
pub async fn scan_download_orphans(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Vec<OrphanFile>, String> {
    let downloads_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?
        .join("downloads");
    let tracked = tracked_downloads(&state)?;
    tokio::task::spawn_blocking(move || Ok(scan_orphans_in_dir(&downloads_dir, &tracked)))
        .await
        .map_err(|e| e.to_string())?
}

/// Re-validates that each given path is inside downloads/ and still an orphan, then deletes it.
#[tauri::command]
pub async fn delete_download_orphans(
    app: AppHandle,
    state: State<'_, AppState>,
    paths: Vec<String>,
) -> Result<u64, String> {
    let downloads_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?
        .join("downloads");
    let tracked = tracked_downloads(&state)?;
    tokio::task::spawn_blocking(move || {
        Ok(delete_orphans_in_dir(&downloads_dir, &paths, &tracked))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Returns a storage breakdown report of downloads, sqlite, covers, logs, and orphan files.
#[tauri::command]
pub async fn get_storage_report(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<StorageReport, String> {
    let app_data_dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    let tracked = tracked_downloads(&state)?;
    tokio::task::spawn_blocking(move || Ok(compute_storage_report(&app_data_dir, &tracked)))
        .await
        .map_err(|e| e.to_string())?
}

/// Retrieves the auto-delete on seen preference (default true).
#[tauri::command]
pub async fn get_auto_delete_seen_downloads(
    state: State<'_, AppState>,
) -> Result<bool, String> {
    let db = state.db.lock().unwrap();
    db.get_auto_delete_seen_downloads().map_err(|e| e.to_string())
}

/// Persists the auto-delete on seen preference.
#[tauri::command]
pub async fn set_auto_delete_seen_downloads(
    state: State<'_, AppState>,
    enabled: bool,
) -> Result<(), String> {
    let db = state.db.lock().unwrap();
    db.set_auto_delete_seen_downloads(enabled).map_err(|e| e.to_string())
}

/// Retrieves the preferred download quality (best, 1080, 720, 480). Default: best.
#[tauri::command]
pub async fn get_download_quality(
    state: State<'_, AppState>,
) -> Result<String, String> {
    let db = state.db.lock().unwrap();
    db.get_download_quality().map_err(|e| e.to_string())
}

/// Persists the preferred download quality (best, 1080, 720, 480).
#[tauri::command]
pub async fn set_download_quality(
    state: State<'_, AppState>,
    quality: String,
) -> Result<(), String> {
    let db = state.db.lock().unwrap();
    db.set_download_quality(&quality).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_dir(name: &str) -> PathBuf {
        static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "aot_storage_test_{}_{}_{}",
            name,
            std::process::id(),
            N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn tracked_covers_normalizes_verbatim_prefix() {
        #[cfg(windows)]
        {
            let mut tracked = Tracked::default();
            tracked.paths.insert(PathBuf::from(r"\\?\C:\x\1.mp4"));
            assert!(
                tracked.covers(Path::new(r"C:\x\1.mp4")),
                "verbatim tracked path must cover plain path"
            );

            let mut tracked2 = Tracked::default();
            tracked2.paths.insert(PathBuf::from(r"C:\x\1.mp4"));
            assert!(
                tracked2.covers(Path::new(r"\\?\C:\x\1.mp4")),
                "plain tracked path must cover verbatim path"
            );
        }
    }

    #[test]
    fn strip_verbatim_normalizes_prefixes() {
        #[cfg(windows)]
        {
            assert_eq!(
                strip_verbatim(Path::new(r"\\?\C:\x\1.mp4")),
                PathBuf::from(r"C:\x\1.mp4")
            );
            assert_eq!(
                strip_verbatim(Path::new(r"\\?\UNC\server\share\file.mp4")),
                PathBuf::from(r"\\server\share\file.mp4")
            );
            assert_eq!(
                strip_verbatim(Path::new(r"C:\x\1.mp4")),
                PathBuf::from(r"C:\x\1.mp4")
            );
        }
        #[cfg(not(windows))]
        {
            assert_eq!(
                strip_verbatim(Path::new("/x/1.mp4")),
                PathBuf::from("/x/1.mp4")
            );
        }
    }

    #[test]
    fn is_safe_file_in_dir_validates_path_boundaries() {
        let temp = create_test_dir("safe_path");
        let downloads = temp.join("downloads");
        let series_dir = downloads.join("42");
        std::fs::create_dir_all(&series_dir).unwrap();

        let valid_file = series_dir.join("101.mp4");
        std::fs::write(&valid_file, b"content").unwrap();

        // Valid file inside downloads
        assert!(is_safe_file_in_dir(&downloads, &valid_file));

        // Directory itself is not a safe file
        assert!(!is_safe_file_in_dir(&downloads, &series_dir));

        // Non-existent file is not safe
        assert!(!is_safe_file_in_dir(&downloads, &series_dir.join("missing.mp4")));

        // File outside downloads directory is rejected
        let outside_file = temp.join("outside.txt");
        std::fs::write(&outside_file, b"secret").unwrap();
        assert!(!is_safe_file_in_dir(&downloads, &outside_file));

        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn remove_empty_dir_only_removes_empty_subdirs() {
        let temp = create_test_dir("remove_empty");
        let downloads = temp.join("downloads");
        let series_empty = downloads.join("empty_series");
        let series_with_file = downloads.join("full_series");
        std::fs::create_dir_all(&series_empty).unwrap();
        std::fs::create_dir_all(&series_with_file).unwrap();
        std::fs::write(series_with_file.join("ep.mp4"), b"video").unwrap();

        // Empty dir is removed
        assert!(remove_empty_dir_in_dir(&downloads, &series_empty));
        assert!(!series_empty.exists());

        // Non-empty dir is NOT removed
        assert!(!remove_empty_dir_in_dir(&downloads, &series_with_file));
        assert!(series_with_file.exists());

        // Base dir itself is never removed
        assert!(!remove_empty_dir_in_dir(&downloads, &downloads));
        assert!(downloads.exists());

        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn delete_series_files_and_dir_removes_files_and_empty_folder() {
        let temp = create_test_dir("del_series");
        let downloads = temp.join("downloads");
        let series_dir = downloads.join("10");
        std::fs::create_dir_all(&series_dir).unwrap();

        let file1 = series_dir.join("1.mp4");
        let file2 = series_dir.join("2.mp4");
        std::fs::write(&file1, b"vid1").unwrap();
        std::fs::write(&file2, b"vid2").unwrap();

        let paths = vec![
            file1.to_str().unwrap().to_string(),
            file2.to_str().unwrap().to_string(),
        ];

        delete_series_files_and_dir(&downloads, 10, &paths);

        assert!(!file1.exists());
        assert!(!file2.exists());
        assert!(!series_dir.exists(), "Series dir should be removed when empty");

        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn scan_and_delete_download_orphans() {
        let temp = create_test_dir("orphans");
        let downloads = temp.join("downloads");
        let series_dir = downloads.join("5");
        std::fs::create_dir_all(&series_dir).unwrap();

        let tracked_file = series_dir.join("1.mp4");
        let orphan_file = series_dir.join("2.mp4");
        let part_file = series_dir.join("3.part");

        std::fs::write(&tracked_file, b"tracked_content").unwrap(); // 15 bytes
        std::fs::write(&orphan_file, b"orphan_bytes_content").unwrap(); // 20 bytes
        std::fs::write(&part_file, b"part_data").unwrap();

        // Finished but its row not yet written: protected by the episode id.
        let finishing_file = series_dir.join("4.mp4");
        std::fs::write(&finishing_file, b"just_renamed").unwrap();
        let upper_part = series_dir.join("6.PART");
        std::fs::write(&upper_part, b"part_data").unwrap();

        let mut tracked = Tracked::default();
        tracked.paths.insert(tracked_file.canonicalize().unwrap());
        tracked.active_ids.insert(4);

        // Scan orphans: only orphan_file (tracked path, tracked id and *.part skipped)
        let orphans = scan_orphans_in_dir(&downloads, &tracked);
        assert_eq!(orphans.len(), 1);
        assert_eq!(orphans[0].size_bytes, 20);
        assert!(orphans[0].path.contains("2.mp4"));

        // Delete orphans: tracked ones are refused even when asked for
        let all: Vec<String> = [&orphan_file, &tracked_file, &finishing_file, &part_file, &upper_part]
            .iter()
            .map(|p| p.to_str().unwrap().to_string())
            .collect();
        let reclaimed = delete_orphans_in_dir(&downloads, &all, &tracked);
        assert_eq!(reclaimed, 20);
        assert!(!orphan_file.exists());
        assert!(tracked_file.exists());
        assert!(finishing_file.exists());
        assert!(part_file.exists());
        assert!(upper_part.exists());

        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn delete_orphans_refuses_paths_deeper_than_the_scan_lists() {
        let temp = create_test_dir("orphans_depth");
        let downloads = temp.join("downloads");
        let deep_dir = downloads.join("5").join("nested");
        std::fs::create_dir_all(&deep_dir).unwrap();
        let deep_file = deep_dir.join("x.mp4");
        std::fs::write(&deep_file, b"deep").unwrap();

        let reclaimed = delete_orphans_in_dir(
            &downloads,
            &[deep_file.to_str().unwrap().to_string()],
            &Tracked::default(),
        );
        assert_eq!(reclaimed, 0);
        assert!(deep_file.exists());

        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn compute_storage_report_aggregates_sizes() {
        let app_data = create_test_dir("storage_report");
        let downloads = app_data.join("downloads").join("1");
        let covers = app_data.join("covers");
        std::fs::create_dir_all(&downloads).unwrap();
        std::fs::create_dir_all(&covers).unwrap();

        let dl_file = downloads.join("ep1.mp4");
        std::fs::write(&dl_file, b"1234567890").unwrap(); // 10 bytes

        let cover_file = covers.join("cover.jpg");
        std::fs::write(&cover_file, b"cover_bytes").unwrap(); // 11 bytes

        let sqlite_file = app_data.join("animeontrack.sqlite");
        std::fs::write(&sqlite_file, b"sqlite_db_data").unwrap(); // 14 bytes

        let log_file = app_data.join("aot.log");
        std::fs::write(&log_file, b"log_line").unwrap(); // 8 bytes

        // No valid paths -> dl_file is counted as orphan
        let report = compute_storage_report(&app_data, &Tracked::default());

        assert_eq!(report.downloads_bytes, 10);
        assert_eq!(report.covers_bytes, 11);
        assert_eq!(report.sqlite_bytes, 14);
        assert_eq!(report.logs_bytes, 8);
        assert_eq!(report.orphan_bytes, 10);

        let _ = std::fs::remove_dir_all(&app_data);
    }

    #[test]
    fn rotate_log_if_exceeds_threshold() {
        let temp = create_test_dir("log_rotate");
        let log = temp.join("test.log");
        let backup = temp.join("test.log.1");

        // Write 100 bytes
        std::fs::write(&log, vec![b'a'; 100]).unwrap();

        // Threshold 200: should NOT rotate
        let rotated = rotate_log_if_exceeds(&log, 200).unwrap();
        assert!(!rotated);
        assert!(log.exists());
        assert!(!backup.exists());

        // Threshold 100: should rotate
        let rotated = rotate_log_if_exceeds(&log, 100).unwrap();
        assert!(rotated);
        assert!(!log.exists());
        assert!(backup.exists());
        assert_eq!(std::fs::metadata(&backup).unwrap().len(), 100);

        // Second rotation overwrites old .1
        std::fs::write(&log, vec![b'b'; 150]).unwrap();
        let rotated2 = rotate_log_if_exceeds(&log, 100).unwrap();
        assert!(rotated2);
        assert_eq!(std::fs::metadata(&backup).unwrap().len(), 150);

        let _ = std::fs::remove_dir_all(&temp);
    }
    #[test]
    fn scan_orphans_reports_stale_variant_but_protects_tracked_and_active() {
        let temp = create_test_dir("stale_variants");
        let downloads = temp.join("downloads");
        let series_dir = downloads.join("10");
        std::fs::create_dir_all(&series_dir).unwrap();

        // Episode 1: tracked path is 1.mp4, but a stale 1.mkv also exists on disk
        let ep1_tracked = series_dir.join("1.mp4");
        let ep1_stale = series_dir.join("1.mkv");
        std::fs::write(&ep1_tracked, b"tracked video").unwrap();
        std::fs::write(&ep1_stale, b"stale video").unwrap();

        // Episode 2: currently in-flight/being finalized (in active_ids), row not yet in DB
        let ep2_active = series_dir.join("2.mp4");
        std::fs::write(&ep2_active, b"active video").unwrap();

        // Episode 3: totally untracked orphan
        let ep3_orphan = series_dir.join("3.mp4");
        std::fs::write(&ep3_orphan, b"orphan video").unwrap();

        let mut tracked = Tracked::default();
        tracked.paths.insert(ep1_tracked.canonicalize().unwrap());
        tracked.active_ids.insert(2);

        let orphans = scan_orphans_in_dir(&downloads, &tracked);
        let orphan_paths: Vec<&str> = orphans.iter().map(|o| o.path.as_str()).collect();

        // ep1_stale (1.mkv) and ep3_orphan (3.mp4) MUST be reported as orphans
        assert!(orphan_paths.iter().any(|p| p.ends_with("1.mkv")), "stale variant 1.mkv must be reported as orphan");
        assert!(orphan_paths.iter().any(|p| p.ends_with("3.mp4")), "untracked 3.mp4 must be reported as orphan");

        // ep1_tracked (1.mp4) and ep2_active (2.mp4) must NOT be reported as orphans
        assert!(!orphan_paths.iter().any(|p| p.ends_with("1.mp4")), "tracked file 1.mp4 must not be orphan");
        assert!(!orphan_paths.iter().any(|p| p.ends_with("2.mp4")), "in-flight active file 2.mp4 must not be orphan");

        // Delete orphans: only 1.mkv and 3.mp4 should be deleted
        let to_delete: Vec<String> = vec![
            ep1_tracked.to_str().unwrap().to_string(),
            ep1_stale.to_str().unwrap().to_string(),
            ep2_active.to_str().unwrap().to_string(),
            ep3_orphan.to_str().unwrap().to_string(),
        ];
        delete_orphans_in_dir(&downloads, &to_delete, &tracked);

        assert!(ep1_tracked.exists(), "tracked file must be preserved");
        assert!(!ep1_stale.exists(), "stale variant must be deleted");
        assert!(ep2_active.exists(), "active file must be preserved");
        assert!(!ep3_orphan.exists(), "orphan must be deleted");

        let _ = std::fs::remove_dir_all(&temp);
    }
}
