pub mod credentials;
pub mod drive;
pub mod oauth;
pub mod secure_store;

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

/// Render a `SignatureCounts` to the opaque string stored in the
/// `backup_signature` setting and compared by `is_auto_backup_due`. Field
/// order is fixed and every field is included, so adding a field to
/// `SignatureCounts` automatically widens what the auto-backup notices.
///
/// The string is compared, never parsed, so its shape is free to change — an
/// older stored signature simply won't match the new rendering, which makes
/// the next due check take one backup and settle. That is the safe direction
/// to fail in: one redundant upload beats never uploading again.
pub fn signature_string(c: crate::db::SignatureCounts) -> String {
    format!(
        "{}:{}:{}:{}:{}:{}:{}:{}:{}:{}:{}",
        c.series,
        c.episodes,
        c.max_episode_id,
        c.max_seen_at.unwrap_or_default(),
        c.seen_episodes,
        c.followed,
        c.watched_externally,
        c.backlog_want,
        c.backlog_discarded,
        c.anilist_linked,
        c.settings_hash,
    )
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
        db.get_setting("google_client_secret")
            .ok()
            .flatten()
            .map(|s| secure_store::unprotect(&s)),
    )
}

/// Stage validated restore bytes and write the marker; the swap happens on the
/// next startup, before the DB is opened. Returns Ok once staged.
///
/// The bytes go to a temp file that is *renamed* into place, and the marker is
/// only written once that rename succeeded — so a crash or a full disk part-way
/// through the download can never leave a half-written `.restored` file that a
/// marker points at. (`apply_pending_restore` re-validates anyway; this just
/// means the common failure never produces a file to re-validate.)
pub fn stage_restore(bytes: &[u8], dir: &std::path::Path) -> Result<(), String> {
    validate_restore_bytes(bytes)?;
    let tmp = dir.join(format!("{RESTORE_STAGED}.part.{}", std::process::id()));
    std::fs::write(&tmp, bytes).map_err(|e| format!("stage write: {e}"))?;
    if let Err(e) = std::fs::rename(&tmp, dir.join(RESTORE_STAGED)) {
        std::fs::remove_file(&tmp).ok();
        return Err(format!("stage swap: {e}"));
    }
    std::fs::write(dir.join(RESTORE_MARKER), b"1").map_err(|e| format!("marker write: {e}"))?;
    Ok(())
}

/// SQLite's rollback journal / WAL sidecars, which live next to the main
/// database file and share its name.
const DB_SIDECAR_SUFFIXES: [&str; 3] = ["-journal", "-wal", "-shm"];

/// The prior live database, kept next to the restored one so a restore of the
/// *wrong* snapshot is itself recoverable — the user renames this back by hand.
/// Deliberately not surfaced in the UI: there is no restore-of-a-restore flow,
/// this is a safety net, not a feature.
const PRE_RESTORE_BACKUP: &str = "animeontrack.sqlite.pre-restore";

/// The stored `gdrive_refresh_token` setting as a usable token, or `None` when
/// there is effectively no connection.
///
/// The setting holds a DPAPI-protected blob (see `secure_store`), and DPAPI is
/// scoped to one Windows account on one machine: after restoring a snapshot
/// taken elsewhere, the blob is present but `unprotect` can't decrypt it and
/// returns an empty string. Every caller must therefore judge "connected" on
/// the *decrypted* value, never on the raw setting's mere presence — which is
/// exactly what `backup_status` used to do, reporting `connected: true` while
/// every real backup bailed out with "Not connected to Google Drive".
pub fn connected_refresh_token(stored: Option<String>) -> Option<String> {
    stored.map(|s| secure_store::unprotect(&s)).filter(|s| !s.is_empty())
}

/// Called from `.setup` BEFORE `Db::open`. If a validated staged restore is
/// pending, swap it over the live file. Never leaves the app unopenable: the
/// live file is never removed except by `rename` itself replacing it, so a
/// failed swap leaves the existing DB completely untouched and the marker in
/// place for a retry on the next startup — it is only cleared once the swap
/// has actually succeeded.
///
/// `rename` (not remove-then-copy) is what makes this safe: on Windows,
/// `std::fs::rename` replaces an existing destination file, and `staged`
/// lives in the same directory as `live` so this is always a same-volume
/// rename, never the cross-device case that would need a copy fallback. An
/// older version of this function deleted `live` unconditionally before
/// attempting the swap and swallowed the fallback's error — if both steps
/// failed (locked file, transient disk error), the app was left with the
/// live DB gone and no indication anything went wrong.
///
/// Three things happen around that rename, in order:
///
/// 1. **Re-validate the staged bytes.** `stage_restore` validated what it
///    downloaded, but that was a whole app session ago; the file has sat on
///    disk across a restart. Reading it back and running the same
///    `validate_restore_bytes` means a file truncated or corrupted in the
///    meantime is refused here exactly as it would have been at stage time,
///    instead of being renamed over a perfectly good database.
/// 2. **Preserve the outgoing database** as `PRE_RESTORE_BACKUP`. A *copy*,
///    not a rename, so `live` keeps existing until the swap replaces it — the
///    invariant above stays true. If the copy fails we refuse the swap rather
///    than clobber a database we couldn't preserve.
/// 3. **Delete the sidecars.** SQLite's `-journal`/`-wal`/`-shm` files belong
///    to the database that was just replaced. Left in place, a hot rollback
///    journal from the *old* file would be replayed into the *restored* one on
///    the next open, silently undoing part of the restore.
pub fn apply_pending_restore(dir: &std::path::Path) {
    let marker = dir.join(RESTORE_MARKER);
    let staged = dir.join(RESTORE_STAGED);
    if !marker.exists() { return; }
    if staged.exists() {
        let live = dir.join(BACKUP_FILE_NAME);
        // (1) The staged file must still be a healthy database *now*.
        let Ok(bytes) = std::fs::read(&staged) else { return };
        if validate_restore_bytes(&bytes).is_err() {
            return;
        }
        // (2) Keep the database we're about to replace.
        if live.exists() && std::fs::copy(&live, dir.join(PRE_RESTORE_BACKUP)).is_err() {
            return;
        }
        if std::fs::rename(&staged, &live).is_err() {
            return;
        }
        // (3) The replaced database's sidecars must not outlive it.
        for suffix in DB_SIDECAR_SUFFIXES {
            let _ = std::fs::remove_file(dir.join(format!("{BACKUP_FILE_NAME}{suffix}")));
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

    fn counts() -> crate::db::SignatureCounts {
        crate::db::SignatureCounts {
            series: 1,
            episodes: 2,
            max_episode_id: 3,
            max_seen_at: None,
            seen_episodes: 0,
            followed: 0,
            watched_externally: 0,
            backlog_want: 0,
            backlog_discarded: 0,
            anilist_linked: 0,
            settings_hash: 0,
        }
    }

    #[test]
    fn signature_string_is_stable_and_distinct() {
        assert_eq!(signature_string(counts()), signature_string(counts()));
        let mut seen = counts();
        seen.max_seen_at = Some("2026-07-14".into());
        assert_ne!(signature_string(counts()), signature_string(seen));
    }

    /// Every field must reach the rendered string — a field that silently
    /// doesn't is a whole class of change the auto-backup goes blind to, which
    /// is exactly the bug this signature was widened to fix.
    #[test]
    fn signature_string_reflects_every_field() {
        let base = signature_string(counts());
        // One block per field, deliberately spelled out: a field silently
        // missing from the format string is a whole class of change the
        // auto-backup goes blind to, which is the bug this was widened to fix.
        let mut c = counts();
        c.series += 1;
        assert_ne!(signature_string(c), base, "series");
        let mut c = counts();
        c.episodes += 1;
        assert_ne!(signature_string(c), base, "episodes");
        let mut c = counts();
        c.max_episode_id += 1;
        assert_ne!(signature_string(c), base, "max_episode_id");
        let mut c = counts();
        c.max_seen_at = Some("2026-07-14".into());
        assert_ne!(signature_string(c), base, "max_seen_at");
        let mut c = counts();
        c.seen_episodes += 1;
        assert_ne!(signature_string(c), base, "seen_episodes");
        let mut c = counts();
        c.followed += 1;
        assert_ne!(signature_string(c), base, "followed");
        let mut c = counts();
        c.watched_externally += 1;
        assert_ne!(signature_string(c), base, "watched_externally");
        let mut c = counts();
        c.backlog_want += 1;
        assert_ne!(signature_string(c), base, "backlog_want");
        let mut c = counts();
        c.backlog_discarded += 1;
        assert_ne!(signature_string(c), base, "backlog_discarded");
        let mut c = counts();
        c.anilist_linked += 1;
        assert_ne!(signature_string(c), base, "anilist_linked");
        let mut c = counts();
        c.settings_hash = 42;
        assert_ne!(signature_string(c), base, "settings_hash");
    }

    /// Bytes of a real, freshly-created AnimeOnTrack database — what
    /// `apply_pending_restore` now insists a staged file actually is before it
    /// will swap it over the live one.
    fn valid_db_bytes() -> Vec<u8> {
        let db = crate::db::Db::open(":memory:").unwrap();
        let tmp = std::env::temp_dir().join(format!(
            "aot_valid_db_{}_{:?}.sqlite",
            std::process::id(),
            std::thread::current().id()
        ));
        db.snapshot_to(tmp.to_str().unwrap()).unwrap();
        let bytes = std::fs::read(&tmp).unwrap();
        std::fs::remove_file(&tmp).ok();
        bytes
    }

    /// A fresh, empty temp directory per test. Process id alone collides
    /// across the threads of one test binary, so the thread id is mixed in.
    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "aot_{tag}_{}_{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn apply_pending_restore_swaps_staged_over_live_and_clears_markers() {
        let dir = temp_dir("restore_ok");
        let staged_bytes = valid_db_bytes();
        std::fs::write(dir.join(BACKUP_FILE_NAME), b"old live bytes").unwrap();
        std::fs::write(dir.join(RESTORE_STAGED), &staged_bytes).unwrap();
        std::fs::write(dir.join(RESTORE_MARKER), b"1").unwrap();

        apply_pending_restore(&dir);

        assert_eq!(std::fs::read(dir.join(BACKUP_FILE_NAME)).unwrap(), staged_bytes);
        assert!(!dir.join(RESTORE_MARKER).exists());
        assert!(!dir.join(RESTORE_STAGED).exists());
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The staged file was validated when it was downloaded, but that was a
    /// whole app session ago. If it is no longer a healthy database at apply
    /// time it must be refused, not renamed over the live one.
    #[test]
    fn apply_pending_restore_refuses_a_staged_file_corrupted_after_staging() {
        let dir = temp_dir("restore_corrupt");
        let mut staged_bytes = valid_db_bytes();
        std::fs::write(dir.join(BACKUP_FILE_NAME), b"live database").unwrap();
        // Truncated mid-file, the shape a crash or a full disk leaves behind.
        staged_bytes.truncate(staged_bytes.len() / 3);
        std::fs::write(dir.join(RESTORE_STAGED), &staged_bytes).unwrap();
        std::fs::write(dir.join(RESTORE_MARKER), b"1").unwrap();

        apply_pending_restore(&dir);

        assert_eq!(
            std::fs::read(dir.join(BACKUP_FILE_NAME)).unwrap(),
            b"live database",
            "a corrupted staged file must never reach the live path"
        );
        assert!(dir.join(RESTORE_MARKER).exists(), "marker survives for a retry");
        assert!(!dir.join(PRE_RESTORE_BACKUP).exists(), "nothing was replaced, nothing to preserve");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A leftover hot rollback journal belongs to the database that was just
    /// replaced; SQLite would replay it into the restored file on the next
    /// open, silently undoing part of the restore.
    #[test]
    fn apply_pending_restore_removes_sqlite_sidecars_next_to_live() {
        let dir = temp_dir("restore_sidecars");
        let staged_bytes = valid_db_bytes();
        std::fs::write(dir.join(BACKUP_FILE_NAME), b"old live bytes").unwrap();
        for suffix in DB_SIDECAR_SUFFIXES {
            std::fs::write(dir.join(format!("{BACKUP_FILE_NAME}{suffix}")), b"stale").unwrap();
        }
        std::fs::write(dir.join(RESTORE_STAGED), &staged_bytes).unwrap();
        std::fs::write(dir.join(RESTORE_MARKER), b"1").unwrap();

        apply_pending_restore(&dir);

        assert_eq!(std::fs::read(dir.join(BACKUP_FILE_NAME)).unwrap(), staged_bytes);
        for suffix in DB_SIDECAR_SUFFIXES {
            assert!(
                !dir.join(format!("{BACKUP_FILE_NAME}{suffix}")).exists(),
                "{suffix} sidecar of the replaced database must be gone"
            );
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Restoring the wrong snapshot used to be unrecoverable — the rename ate
    /// the live database. The prior contents now sit next to it, renameable
    /// back by hand.
    #[test]
    fn apply_pending_restore_keeps_a_pre_restore_copy_of_the_old_live_db() {
        let dir = temp_dir("restore_prev");
        let staged_bytes = valid_db_bytes();
        std::fs::write(dir.join(BACKUP_FILE_NAME), b"irreplaceable local progress").unwrap();
        std::fs::write(dir.join(RESTORE_STAGED), &staged_bytes).unwrap();
        std::fs::write(dir.join(RESTORE_MARKER), b"1").unwrap();

        apply_pending_restore(&dir);

        assert_eq!(std::fs::read(dir.join(BACKUP_FILE_NAME)).unwrap(), staged_bytes);
        assert_eq!(
            std::fs::read(dir.join(PRE_RESTORE_BACKUP)).unwrap(),
            b"irreplaceable local progress",
            "the replaced database must be recoverable from disk"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A first restore onto a machine with no database yet must still work —
    /// there is simply nothing to preserve.
    #[test]
    fn apply_pending_restore_works_with_no_existing_live_file() {
        let dir = temp_dir("restore_fresh");
        let staged_bytes = valid_db_bytes();
        std::fs::write(dir.join(RESTORE_STAGED), &staged_bytes).unwrap();
        std::fs::write(dir.join(RESTORE_MARKER), b"1").unwrap();

        apply_pending_restore(&dir);

        assert_eq!(std::fs::read(dir.join(BACKUP_FILE_NAME)).unwrap(), staged_bytes);
        assert!(!dir.join(PRE_RESTORE_BACKUP).exists());
        std::fs::remove_dir_all(&dir).ok();
    }

    /// `stage_restore` writes through a temp file and renames, so the marker
    /// is never pointing at a half-written staged file.
    #[test]
    fn stage_restore_leaves_no_partial_file_behind() {
        let dir = temp_dir("stage_atomic");
        let bytes = valid_db_bytes();
        stage_restore(&bytes, &dir).unwrap();
        assert_eq!(std::fs::read(dir.join(RESTORE_STAGED)).unwrap(), bytes);
        assert!(dir.join(RESTORE_MARKER).exists());
        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok().map(|e| e.file_name().to_string_lossy().into_owned()))
            .filter(|n| n.contains(".part."))
            .collect();
        assert!(leftovers.is_empty(), "temp staging file left behind: {leftovers:?}");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Invalid bytes must be rejected before anything is staged at all — the
    /// marker in particular must not exist, or startup would keep retrying a
    /// restore that can never apply.
    #[test]
    fn stage_restore_rejects_invalid_bytes_without_writing_a_marker() {
        let dir = temp_dir("stage_reject");
        assert!(stage_restore(b"definitely not sqlite", &dir).is_err());
        assert!(!dir.join(RESTORE_STAGED).exists());
        assert!(!dir.join(RESTORE_MARKER).exists());
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The DPAPI blob only decrypts for the Windows account that wrote it, so
    /// after a restore from another machine `unprotect` yields "" — which must
    /// read as *not connected*, not as a present-therefore-live token.
    #[test]
    fn a_token_that_fails_to_decrypt_counts_as_not_connected() {
        assert_eq!(connected_refresh_token(None), None);
        assert_eq!(connected_refresh_token(Some(String::new())), None);
        // A `dpapi1:`-prefixed blob this account cannot decrypt.
        assert_eq!(connected_refresh_token(Some("dpapi1:not-valid-base64!!!".into())), None);
        // Legacy plaintext (no prefix) still reads back as a live connection.
        assert_eq!(
            connected_refresh_token(Some("1//legacy-plaintext".into())),
            Some("1//legacy-plaintext".to_string())
        );
    }

    #[test]
    fn apply_pending_restore_is_a_noop_without_a_marker() {
        let dir = temp_dir("restore_noop");
        std::fs::write(dir.join(BACKUP_FILE_NAME), b"untouched").unwrap();

        apply_pending_restore(&dir);

        assert_eq!(std::fs::read(dir.join(BACKUP_FILE_NAME)).unwrap(), b"untouched");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The critical property: if the swap can't happen, `live` must never be
    /// touched. Simulated by pointing `live` at a path that is neither
    /// copyable nor a valid rename target (a directory, not a file) — standing
    /// in for a locked-file/transient-error failure without needing to
    /// actually lock a file in a unit test. The pre-fix version's separate
    /// `remove_file(&live)` would already have deleted `live` before this
    /// point, unconditionally.
    #[test]
    fn apply_pending_restore_leaves_live_and_marker_alone_when_swap_fails() {
        let dir = temp_dir("restore_fail");
        std::fs::create_dir_all(dir.join(BACKUP_FILE_NAME)).unwrap();
        // Valid bytes, so the refusal is proven to come from the swap failing
        // rather than from the apply-time re-validation.
        std::fs::write(dir.join(RESTORE_STAGED), valid_db_bytes()).unwrap();
        std::fs::write(dir.join(RESTORE_MARKER), b"1").unwrap();

        apply_pending_restore(&dir);

        assert!(dir.join(BACKUP_FILE_NAME).is_dir(), "live must be untouched on swap failure");
        assert!(dir.join(RESTORE_MARKER).exists(), "marker must survive for a retry");
        assert!(dir.join(RESTORE_STAGED).exists(), "staged bytes must survive for a retry");
        std::fs::remove_dir_all(&dir).ok();
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
