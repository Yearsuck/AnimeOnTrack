pub mod credentials;
pub mod drive;
pub mod oauth;

use crate::db::Db;
use rusqlite::Connection;

pub const BACKUP_FILE_NAME: &str = "animeontrack.sqlite";
const AUTO_BACKUP_INTERVAL_SECS: i64 = 24 * 60 * 60;

/// Reject anything that isn't a healthy AnimeOnTrack database before it can
/// overwrite the live one: must open, pass integrity_check, and contain our
/// core tables. Writes to a temp file because rusqlite opens paths, not bytes.
pub fn validate_restore_bytes(bytes: &[u8]) -> Result<(), String> {
    // Process id alone collides across threads of the same test binary run
    // in parallel (multiple #[test] fns hit this same path concurrently) —
    // mix in the thread id and a timestamp so concurrent validations never
    // read back each other's temp file.
    let unique = format!(
        "{}_{:?}_{}",
        std::process::id(),
        std::thread::current().id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
    );
    let tmp = std::env::temp_dir().join(format!("aot_restore_check_{unique}.sqlite"));
    std::fs::write(&tmp, bytes).map_err(|e| format!("write temp: {e}"))?;
    let result = (|| -> Result<(), String> {
        let conn = Connection::open(&tmp).map_err(|e| format!("open: {e}"))?;
        let ok: String = conn
            .query_row("PRAGMA integrity_check", [], |r| r.get(0))
            .map_err(|e| format!("integrity: {e}"))?;
        if ok != "ok" {
            return Err(format!("integrity_check returned {ok}"));
        }
        for table in ["sources", "series"] {
            let n: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
                    [table],
                    |r| r.get(0),
                )
                .map_err(|e| format!("schema check: {e}"))?;
            if n != 1 {
                return Err(format!("missing table {table}"));
            }
        }
        Ok(())
    })();
    std::fs::remove_file(&tmp).ok();
    result
}

pub fn signature_string(counts: (i64, i64, i64, Option<String>)) -> String {
    let (series, eps, max_ep, max_seen) = counts;
    format!("{series}:{eps}:{max_ep}:{}", max_seen.unwrap_or_default())
}

/// Pure decision for the startup/after-refresh auto-backup. `now`/`last_at`
/// are unix seconds.
pub fn is_auto_backup_due(last_at: Option<i64>, now: i64, last_sig: &str, cur_sig: &str) -> bool {
    match last_at {
        None => true,
        Some(prev) => now - prev >= AUTO_BACKUP_INTERVAL_SECS && last_sig != cur_sig,
    }
}

const RESTORE_STAGED: &str = "animeontrack.sqlite.restored";
const RESTORE_MARKER: &str = ".restore_pending";

/// Produce the consistent snapshot bytes for upload. `db_path` is the live DB
/// file's directory-mate: we snapshot next to it then read+delete.
pub fn snapshot_bytes(db: &Db, dir: &std::path::Path) -> Result<Vec<u8>, String> {
    let tmp = dir.join(format!("animeontrack.snapshot.{}.sqlite", std::process::id()));
    db.snapshot_to(tmp.to_str().ok_or("bad path")?).map_err(|e| format!("snapshot: {e}"))?;
    let bytes = std::fs::read(&tmp).map_err(|e| format!("read snapshot: {e}"))?;
    std::fs::remove_file(&tmp).ok();
    Ok(bytes)
}

/// Get a fresh access token from the stored refresh token, or an error the
/// caller surfaces to the UI.
pub async fn access_token(
    client: &(String, String),
    db_refresh: &str,
) -> Result<String, String> {
    oauth::refresh_access_token(&client.0, &client.1, db_refresh).await
}

/// The configured OAuth client pair, read from settings with the compile-time
/// pair as fallback. Kept here so every caller resolves it the same way rather
/// than each reaching into `settings` with its own key spelling.
pub fn configured_client(db: &Db) -> Option<(String, String)> {
    credentials::resolve(
        db.get_setting("google_client_id").ok().flatten(),
        db.get_setting("google_client_secret").ok().flatten(),
    )
}

/// Stage validated restore bytes and write the marker; the swap happens on the
/// next startup, before the DB is opened. Returns Ok once staged.
pub fn stage_restore(bytes: &[u8], dir: &std::path::Path) -> Result<(), String> {
    validate_restore_bytes(bytes)?;
    std::fs::write(dir.join(RESTORE_STAGED), bytes).map_err(|e| format!("stage write: {e}"))?;
    std::fs::write(dir.join(RESTORE_MARKER), b"1").map_err(|e| format!("marker write: {e}"))?;
    Ok(())
}

/// Called from `.setup` BEFORE `Db::open`. If a validated staged restore is
/// pending, swap it over the live file. Never leaves the app unopenable: on
/// any inconsistency it clears the marker and returns without swapping.
pub fn apply_pending_restore(dir: &std::path::Path) {
    let marker = dir.join(RESTORE_MARKER);
    let staged = dir.join(RESTORE_STAGED);
    if !marker.exists() { return; }
    if staged.exists() {
        let live = dir.join(BACKUP_FILE_NAME);
        // Best-effort atomic-ish swap: remove live, rename staged in.
        let _ = std::fs::remove_file(&live);
        if std::fs::rename(&staged, &live).is_err() {
            // Cross-device fallback: copy then delete.
            let _ = std::fs::copy(&staged, &live);
            let _ = std::fs::remove_file(&staged);
        }
    }
    let _ = std::fs::remove_file(&marker);
    let _ = std::fs::remove_file(&staged);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_accepts_a_real_snapshot() {
        let db = crate::db::Db::open(":memory:").unwrap();
        let tmp = std::env::temp_dir().join(format!("aot_val_ok_{}.sqlite", std::process::id()));
        db.snapshot_to(tmp.to_str().unwrap()).unwrap();
        let bytes = std::fs::read(&tmp).unwrap();
        assert!(validate_restore_bytes(&bytes).is_ok());
        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn validate_rejects_random_bytes() {
        assert!(validate_restore_bytes(b"not a sqlite file at all").is_err());
    }

    #[test]
    fn validate_rejects_sqlite_without_our_tables() {
        let tmp = std::env::temp_dir().join(format!("aot_val_bad_{}.sqlite", std::process::id()));
        let conn = rusqlite::Connection::open(&tmp).unwrap();
        conn.execute("CREATE TABLE foo(x)", []).unwrap();
        drop(conn);
        let bytes = std::fs::read(&tmp).unwrap();
        assert!(validate_restore_bytes(&bytes).is_err());
        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn signature_string_is_stable_and_distinct() {
        assert_eq!(signature_string((1, 2, 3, None)), signature_string((1, 2, 3, None)));
        assert_ne!(
            signature_string((1, 2, 3, None)),
            signature_string((1, 2, 3, Some("2026-07-14".into())))
        );
    }

    #[test]
    fn auto_backup_due_only_when_stale_and_changed() {
        // >24h since last AND signature changed → due.
        assert!(is_auto_backup_due(Some(0), 90_000, "old", "new"));
        // <24h → not due even if changed.
        assert!(!is_auto_backup_due(Some(80_000), 90_000, "old", "new"));
        // stale but unchanged → not due.
        assert!(!is_auto_backup_due(Some(0), 90_000, "same", "same"));
        // never backed up → due.
        assert!(is_auto_backup_due(None, 90_000, "x", "x"));
    }
}
