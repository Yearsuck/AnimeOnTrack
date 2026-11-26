//! Device-independent sync step, testable without Tauri.
//!
//! [`sync_once`] locks the DB only around the reads and the writes, never
//! across awaits (the `Mutex` guard is dropped before each `.await`).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;

use crate::db::Db;
use crate::models::SyncReport;
use crate::sync::doc::{BaseEntry, FieldChange, ShowState};
use crate::sync::merge::{merge, MergeInput};
use crate::sync::remote::{run_cycle, SyncRemote};

/// Everything the merge needs, read under one short DB lock.
pub struct SyncInputs {
    pub local: BTreeMap<String, ShowState>,
    pub base: BTreeMap<String, BaseEntry>,
    pub device: String,
}

/// Read the local state, the last-sync base, and the device id.
pub fn read_inputs(db: &Db) -> anyhow::Result<SyncInputs> {
    let base = db.load_sync_base()?;
    let extra_keys: BTreeSet<String> = base.keys().cloned().collect();
    let local = db.sync_projection(&extra_keys)?;
    let device = db.sync_device_id()?;
    Ok(SyncInputs { local, base, device })
}

/// Full cycle against `remote`. Locks `db` only around the reads and the
/// writes, never across awaits.
pub async fn sync_once<R: SyncRemote>(
    db: &Mutex<Db>,
    source_id: i64,
    remote: &R,
    now: i64,
) -> Result<SyncReport, String> {
    // --- read phase (lock, release) ---
    let inputs = {
        let db = db.lock().unwrap();
        read_inputs(&db).map_err(|e| e.to_string())?
    };

    let local = inputs.local;
    let old_base = inputs.base;
    let device = inputs.device;

    // --- network phase (no lock held) ---
    let outcome = run_cycle(remote, |doc| {
        merge(&MergeInput {
            local: &local,
            base: &old_base,
            remote: doc,
            device: &device,
            now,
        })
    })
    .await?;

    // --- write phase (lock, release) ---
    let report = {
        let db = db.lock().unwrap();

        // Task 5: re-read the local projection for the keys being applied.
        // If a field's current local value differs from the snapshot used for
        // the merge (the user changed it meanwhile), skip that op — do not
        // overwrite a concurrent local edit.
        let apply_keys: BTreeSet<String> = outcome.output.apply.iter().map(|op| op.key.clone()).collect();
        let fresh_local = if !apply_keys.is_empty() {
            db.sync_projection(&apply_keys).map_err(|e| e.to_string())?
        } else {
            BTreeMap::new()
        };

        let filtered_ops: Vec<_> = outcome
            .output
            .apply
            .iter()
            .filter(|op| {
                let snap = local.get(&op.key);
                let cur = fresh_local.get(&op.key);
                // If the current local state for the field being applied differs
                // from the snapshot, the user changed it during the network
                // window — skip this op.
                match &op.change {
                    FieldChange::Followed(v) => {
                        let snap_v = snap.map(|s| s.followed);
                        let cur_v = cur.map(|s| s.followed);
                        if snap_v != cur_v { return false; }
                        // Also skip if current already matches what we'd apply
                        // (no-op)
                        cur_v != Some(*v) || snap_v.is_none()
                    }
                    FieldChange::Watched(v) => {
                        let snap_v = snap.map(|s| s.watched);
                        let cur_v = cur.map(|s| s.watched);
                        if snap_v != cur_v { return false; }
                        cur_v != Some(*v) || snap_v.is_none()
                    }
                    FieldChange::Backlog(v) => {
                        let snap_v = snap.map(|s| &s.backlog);
                        let cur_v = cur.map(|s| &s.backlog);
                        if snap_v != cur_v { return false; }
                        cur_v != Some(v) || snap_v.is_none()
                    }
                    FieldChange::Watermark(_) => {
                        let snap_v = snap.map(|s| s.watermark);
                        let cur_v = cur.map(|s| s.watermark);
                        match (snap_v, cur_v) {
                            (Some(sv), Some(cv)) if (sv - cv).abs() > 1e-9 => return false,
                            (Some(_), None) | (None, Some(_)) => return false,
                            _ => {}
                        }
                        true
                    }
                }
            })
            .cloned()
            .collect();

        let apply_report = db
            .apply_sync_ops(source_id, &filtered_ops, now)
            .map_err(|e| e.to_string())?;

        // Recompute projection including all keys from the new doc and old base.
        let all_keys: BTreeSet<String> = outcome
            .output
            .doc
            .shows
            .keys()
            .chain(old_base.keys())
            .cloned()
            .collect();
        let new_projection = db.sync_projection(&all_keys).map_err(|e| e.to_string())?;

        // Build the new base: for every key in the output doc, the merged record
        // and what this device "observed". Fields this sync applied take their
        // value right after applying (so a value that could not be fully applied
        // is not read as a local change later); every other field keeps the value
        // read before the network round trip, so a change the user made while the
        // sync was in flight still shows up as a local change next time.
        let mut new_base = BTreeMap::new();
        for (key, record) in &outcome.output.doc.shows {
            let after = new_projection.get(key);
            let before = local.get(key).or_else(|| old_base.get(key).map(|b| &b.observed));
            let mut observed = before.or(after).cloned().unwrap_or_default();
            if let Some(after) = after {
                for op in filtered_ops.iter().filter(|op| &op.key == key) {
                    match op.change {
                        FieldChange::Followed(_) => observed.followed = after.followed,
                        FieldChange::Watched(_) => observed.watched = after.watched,
                        FieldChange::Backlog(_) => observed.backlog = after.backlog.clone(),
                        FieldChange::Watermark(_) => observed.watermark = after.watermark,
                    }
                }
                if observed.anilist_id.is_none() {
                    observed.anilist_id = after.anilist_id;
                }
            }
            // Ops skipped because the user edited the field mid-sync: record the
            // pre-sync value (unknown -> default) so the next sync sees the edit as
            // a local change, even for a show the snapshot did not contain.
            let snap = local.get(key).cloned().unwrap_or_default();
            for op in outcome.output.apply.iter().filter(|op| &op.key == key && !filtered_ops.contains(op)) {
                match op.change {
                    FieldChange::Followed(_) => observed.followed = snap.followed,
                    FieldChange::Watched(_) => observed.watched = snap.watched,
                    FieldChange::Backlog(_) => observed.backlog = snap.backlog.clone(),
                    FieldChange::Watermark(_) => observed.watermark = snap.watermark,
                }
            }
            new_base.insert(
                key.clone(),
                BaseEntry {
                    merged: record.clone(),
                    observed,
                },
            );
        }

        db.save_sync_base(&new_base).map_err(|e| e.to_string())?;

        SyncReport {
            local_changes: outcome.output.local_changes,
            remote_changes: outcome.output.remote_changes,
            applied: apply_report.applied,
            materialized: apply_report.materialized,
            skipped: apply_report.skipped,
            at: now,
        }
    };

    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_support::*;
    use crate::models::Series;
    use crate::sync::remote::{LocalFolderRemote, RemoteError, SyncRemote};
    use std::path::PathBuf;

    struct TestDir {
        path: PathBuf,
    }

    impl TestDir {
        fn new(test_name: &str) -> Self {
            use std::time::SystemTime;
            let nanos = SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            let pid = std::process::id();
            let path = std::env::temp_dir().join(format!("aot_sync_engine_{pid}_{test_name}_{nanos}"));
            let _ = std::fs::create_dir_all(&path);
            Self { path }
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    fn make_db() -> Mutex<Db> {
        Mutex::new(Db::open(":memory:").unwrap())
    }

    fn setup_source(db: &Db) -> i64 {
        db.upsert_source("AnimeYT", "https://animeyt.cc", "animeytx").unwrap()
    }

    fn insert_series_with_anilist(db: &Db, src: i64, anilist_id: i64, title: &str, total_eps: i64, seen_up_to: i64) -> i64 {
        let s = Series {
            id: 0,
            slug: format!("anilist-{anilist_id}"),
            title: title.into(),
            url: format!("https://anilist.co/anime/{anilist_id}"),
            cover_url: None,
            is_airing: false,
            followed: false,
            next_episode_at: None,
            site_episode_count: None,
        };
        let sid = db.upsert_series(src, &s).unwrap();
        db.set_anilist_id(sid, anilist_id).unwrap();
        if total_eps > 0 {
            insert_eps_seen_up_to(db, sid, total_eps, seen_up_to);
        }
        sid
    }

    /// A follows a series (anilist 1, 12 episodes, 5 seen); B has a source row
    /// but no series. sync A, sync B -> B has an `anilist-1` series row,
    /// followed; B's report `materialized == 1`.
    #[test]
    fn sync_a_followed_to_empty_b_materializes() {
        let dir = TestDir::new("a_to_b");
        let remote = LocalFolderRemote::new(&dir.path);

        let db_a = make_db();
        let db_b = make_db();

        let src_a;
        {
            let d = db_a.lock().unwrap();
            src_a = setup_source(&d);
            let sid = insert_series_with_anilist(&d, src_a, 1, "Show One", 12, 5);
            d.set_followed_canonical(sid, true).unwrap();
        }

        let src_b;
        {
            let d = db_b.lock().unwrap();
            src_b = setup_source(&d);
        }

        tauri::async_runtime::block_on(async {
            // Sync A
            let report_a = sync_once(&db_a, src_a, &remote, 1000).await.unwrap();
            assert!(report_a.local_changes > 0, "A must publish its follow and watermark");

            // Sync B
            let report_b = sync_once(&db_b, src_b, &remote, 2000).await.unwrap();
            assert_eq!(report_b.materialized, 1, "B must materialize the series");

            // Verify B has the series followed
            let d = db_b.lock().unwrap();
            let followed: i64 = d.conn
                .query_row(
                    "SELECT followed FROM series WHERE slug='anilist-1' AND source_id=?1",
                    [src_b],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(followed, 1, "materialized series must be followed");
        });
    }

    /// Then B unfollows it, sync B, sync A -> A's row is unfollowed.
    #[test]
    fn sync_unfollow_propagates() {
        let dir = TestDir::new("unfollow");
        let remote = LocalFolderRemote::new(&dir.path);

        let db_a = make_db();
        let db_b = make_db();

        let src_a;
        {
            let d = db_a.lock().unwrap();
            src_a = setup_source(&d);
            let sid = insert_series_with_anilist(&d, src_a, 1, "Show One", 12, 5);
            d.set_followed_canonical(sid, true).unwrap();
        }

        let src_b;
        {
            let d = db_b.lock().unwrap();
            src_b = setup_source(&d);
        }

        tauri::async_runtime::block_on(async {
            // Sync A -> remote
            sync_once(&db_a, src_a, &remote, 1000).await.unwrap();
            // Sync B <- remote (materializes)
            sync_once(&db_b, src_b, &remote, 2000).await.unwrap();

            // B unfollows
            {
                let d = db_b.lock().unwrap();
                let sid_b: i64 = d.conn
                    .query_row(
                        "SELECT id FROM series WHERE slug='anilist-1' AND source_id=?1",
                        [src_b],
                        |r| r.get(0),
                    )
                    .unwrap();
                d.set_followed_canonical(sid_b, false).unwrap();
            }

            // Sync B -> remote
            sync_once(&db_b, src_b, &remote, 3000).await.unwrap();
            // Sync A <- remote
            let report_a2 = sync_once(&db_a, src_a, &remote, 4000).await.unwrap();
            assert!(report_a2.applied > 0, "A must apply the unfollow");

            let d = db_a.lock().unwrap();
            let followed: i64 = d.conn
                .query_row(
                    "SELECT followed FROM series WHERE slug='anilist-1' AND source_id=?1",
                    [src_a],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(followed, 0, "A's row must be unfollowed after sync");
        });
    }

    /// A marks episodes up to 8 seen, sync A, sync B where B's row has 12
    /// episodes -> B has 1..8 seen.
    #[test]
    fn sync_watermark_propagates() {
        let dir = TestDir::new("watermark");
        let remote = LocalFolderRemote::new(&dir.path);

        let db_a = make_db();
        let db_b = make_db();

        let src_a;
        {
            let d = db_a.lock().unwrap();
            src_a = setup_source(&d);
            let sid = insert_series_with_anilist(&d, src_a, 1, "Show One", 12, 5);
            d.set_followed_canonical(sid, true).unwrap();
        }

        let src_b;
        {
            let d = db_b.lock().unwrap();
            src_b = setup_source(&d);
        }

        tauri::async_runtime::block_on(async {
            // Initial sync: A -> remote, B <- remote
            sync_once(&db_a, src_a, &remote, 1000).await.unwrap();
            sync_once(&db_b, src_b, &remote, 2000).await.unwrap();

            // A marks up to 8
            {
                let d = db_a.lock().unwrap();
                let sid_a: i64 = d.conn
                    .query_row(
                        "SELECT id FROM series WHERE slug='anilist-1' AND source_id=?1",
                        [src_a],
                        |r| r.get(0),
                    )
                    .unwrap();
                d.set_seen_cascade(sid_a, "8", true).unwrap();
            }

            // Sync A -> remote
            sync_once(&db_a, src_a, &remote, 3000).await.unwrap();

            // B inserts 12 episodes for its materialized row
            {
                let d = db_b.lock().unwrap();
                let sid_b: i64 = d.conn
                    .query_row(
                        "SELECT id FROM series WHERE slug='anilist-1' AND source_id=?1",
                        [src_b],
                        |r| r.get(0),
                    )
                    .unwrap();
                insert_eps_seen_up_to(&d, sid_b, 12, 0);
            }

            // Sync B <- remote
            let report_b2 = sync_once(&db_b, src_b, &remote, 4000).await.unwrap();
            assert!(report_b2.applied > 0, "B must apply the watermark");

            // Verify B has 1..8 seen
            let d = db_b.lock().unwrap();
            let sid_b: i64 = d.conn
                .query_row(
                    "SELECT id FROM series WHERE slug='anilist-1' AND source_id=?1",
                    [src_b],
                    |r| r.get(0),
                )
                .unwrap();
            for ep_num in 1..=12 {
                let seen: i64 = d.conn
                    .query_row(
                        "SELECT seen FROM episodes WHERE series_id=?1 AND number=?2",
                        rusqlite::params![sid_b, ep_num.to_string()],
                        |r| r.get(0),
                    )
                    .unwrap();
                if ep_num <= 8 {
                    assert_eq!(seen, 1, "Episode {ep_num} must be seen on B");
                } else {
                    assert_eq!(seen, 0, "Episode {ep_num} must be unseen on B");
                }
            }
        });
    }

    /// Running sync_once twice in a row on the same device with no changes ->
    /// second report has local_changes == 0 && remote_changes == 0, and the
    /// file bytes are unchanged except `updated_at`.
    #[test]
    fn idempotent_sync_no_changes() {
        let dir = TestDir::new("idempotent");
        let remote = LocalFolderRemote::new(&dir.path);

        let db_a = make_db();

        let src_a;
        {
            let d = db_a.lock().unwrap();
            src_a = setup_source(&d);
            let sid = insert_series_with_anilist(&d, src_a, 1, "Show One", 12, 5);
            d.set_followed_canonical(sid, true).unwrap();
        }

        tauri::async_runtime::block_on(async {
            sync_once(&db_a, src_a, &remote, 1000).await.unwrap();

            let bytes1 = std::fs::read(remote.file_path()).unwrap();

            let report2 = sync_once(&db_a, src_a, &remote, 2000).await.unwrap();
            assert_eq!(report2.local_changes, 0, "no local changes on second sync");
            assert_eq!(report2.remote_changes, 0, "no remote changes on second sync");

            // File bytes differ only in updated_at (timestamp changed).
            let bytes2 = std::fs::read(remote.file_path()).unwrap();
            let doc1: serde_json::Value = serde_json::from_slice(&bytes1).unwrap();
            let doc2: serde_json::Value = serde_json::from_slice(&bytes2).unwrap();
            assert_eq!(doc1["shows"], doc2["shows"], "shows must be identical");
            assert_eq!(doc1["schema"], doc2["schema"]);
        });
    }

    /// Corrupt sync file -> sync_once returns Err, the file is unchanged, the
    /// base table is unchanged.
    #[test]
    fn corrupt_file_returns_error() {
        let dir = TestDir::new("corrupt");
        let remote = LocalFolderRemote::new(&dir.path);

        let db_a = make_db();

        let src_a;
        {
            let d = db_a.lock().unwrap();
            src_a = setup_source(&d);
        }

        tauri::async_runtime::block_on(async {
            // Write a corrupt file
            let corrupt = b"not valid json {{";
            std::fs::write(remote.file_path(), corrupt).unwrap();

            let result = sync_once(&db_a, src_a, &remote, 1000).await;
            assert!(result.is_err(), "corrupt file must return an error");

            // File unchanged
            let on_disk = std::fs::read(remote.file_path()).unwrap();
            assert_eq!(on_disk, corrupt);

            // Base unchanged (empty)
            let d = db_a.lock().unwrap();
            let base = d.load_sync_base().unwrap();
            assert!(base.is_empty(), "base must remain empty on failure");
        });
    }

    struct InterceptingRemote<R> {
        inner: R,
        on_get: std::sync::Mutex<Option<Box<dyn FnOnce() + Send>>>,
    }

    impl<R: SyncRemote> SyncRemote for InterceptingRemote<R> {
        async fn get(&self) -> Result<Option<(Vec<u8>, String)>, RemoteError> {
            if let Some(cb) = self.on_get.lock().unwrap().take() {
                cb();
            }
            self.inner.get().await
        }

        async fn put(&self, bytes: &[u8], if_match: Option<&str>) -> Result<String, RemoteError> {
            self.inner.put(bytes, if_match).await
        }
    }

    #[test]
    fn concurrent_edit_during_network_window_skipped_and_detected_next_sync() {
        let dir = TestDir::new("concurrent_edit");
        let folder_remote = LocalFolderRemote::new(&dir.path);

        let db_a = std::sync::Arc::new(make_db());
        let db_b = make_db();

        let src_a;
        let sid_a;
        {
            let d = db_a.lock().unwrap();
            src_a = setup_source(&d);
            // Device A has anilist-1, followed = false
            sid_a = insert_series_with_anilist(&d, src_a, 1, "One", 12, 0);
        }

        let src_b;
        {
            let d = db_b.lock().unwrap();
            src_b = setup_source(&d);
            // Device B has anilist-1, followed = true
            let sid_b = insert_series_with_anilist(&d, src_b, 1, "One", 12, 0);
            d.set_followed_canonical(sid_b, true).unwrap();
        }

        tauri::async_runtime::block_on(async {
            // 1. Device B syncs to remote: publishes followed = true
            let res_b = sync_once(&db_b, src_b, &folder_remote, 1000).await.unwrap();
            assert_eq!(res_b.local_changes, 1);

            // 2. Now Device A syncs. During the network window (in get()),
            // Device A's user edits the show locally: marks followed = true.
            let db_a_clone = std::sync::Arc::clone(&db_a);
            let interceptor = InterceptingRemote {
                inner: LocalFolderRemote::new(&dir.path),
                on_get: std::sync::Mutex::new(Some(Box::new(move || {
                    let d = db_a_clone.lock().unwrap();
                    d.set_followed_canonical(sid_a, true).unwrap();
                }))),
            };

            let res_a1 = sync_once(&db_a, src_a, &interceptor, 1100).await.unwrap();
            // The remote Followed(true) op was skipped because the current local value
            // (true) differed from the snapshot (false).
            assert_eq!(res_a1.applied, 0, "Concurrent edit must skip the op");

            // Verify local DB kept the user's edit
            {
                let d = db_a.lock().unwrap();
                let followed: i64 = d.conn.query_row("SELECT followed FROM series WHERE id=?1", [sid_a], |r| r.get(0)).unwrap();
                assert_eq!(followed, 1, "User's concurrent follow edit must be preserved");

                // Verify base did NOT record the new value as observed
                let base = d.load_sync_base().unwrap();
                assert!(!base["al:1"].observed.followed, "Base must not record skipped op as observed");
            }

            // 3. Next sync on Device A: detects the local change because local (true) != observed (false)
            let res_a2 = sync_once(&db_a, src_a, &folder_remote, 1200).await.unwrap();
            assert_eq!(res_a2.local_changes, 1, "Next sync must detect the local change");
            assert_eq!(res_a2.applied, 0);

            // After this sync, base observes true
            {
                let d = db_a.lock().unwrap();
                let base = d.load_sync_base().unwrap();
                assert!(base["al:1"].observed.followed);
            }
        });
    }
}
