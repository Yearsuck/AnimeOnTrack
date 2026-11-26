//! Database side of device sync (see `crate::sync`): read this device's state per
//! canonical key, remember the last sync's base, apply merged changes.
use super::*;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use crate::db::episode_align::format_ep_number;
use crate::db::episodes::parse_ep_number;
use crate::db::library::canon_key;
use crate::models::Series;
use crate::sync::doc::{ApplyOp, BaseEntry, FieldChange, ShowRecord, ShowState};

/// Summary of changes performed by [`Db::apply_sync_ops`].
#[derive(Debug, Default, Clone, PartialEq, serde::Serialize)]
pub struct ApplyReport {
    pub applied: usize,
    pub materialized: usize,
    pub skipped: usize,
}

/// Precedence for a canonical entry's backlog status: 'want' outranks 'discarded'.
fn merge_backlog(current: Option<String>, incoming: Option<String>) -> Option<String> {
    fn rank(s: Option<&str>) -> u8 {
        match s {
            Some("want") => 2,
            Some("discarded") => 1,
            _ => 0,
        }
    }
    if rank(incoming.as_deref()) > rank(current.as_deref()) {
        incoming
    } else {
        current
    }
}

/// Choose the display title among members (shortest, tie-break alphabetical).
fn is_better_title(candidate: &str, current: &str) -> bool {
    let shorter = candidate.chars().count() < current.chars().count();
    let tied_earlier = candidate.chars().count() == current.chars().count() && candidate < current;
    shorter || tied_earlier
}

/// True when the op change represents an engaged value that warrants materializing
/// a synthetic placeholder row if no row currently exists.
fn is_engaged(change: &FieldChange) -> bool {
    match change {
        FieldChange::Followed(true) => true,
        FieldChange::Watched(true) => true,
        FieldChange::Backlog(Some(_)) => true,
        FieldChange::Watermark(w) => *w > 0.0,
        _ => false,
    }
}

impl Db {
    /// Read this device's current state for all known library shows into a canonical map.
    ///
    /// Rebuilds the library projection from `series` first, then includes discarded backlog rows
    /// (which the library deliberately excludes), and finally fills in any requested `extra_keys`
    /// that still have at least one member row in `series`.
    pub fn sync_projection(&self, extra_keys: &BTreeSet<String>) -> Result<BTreeMap<String, ShowState>> {
        self.sync_library_from_series()?;

        let mut map: BTreeMap<String, ShowState> = BTreeMap::new();
        {
            let mut stmt = self.conn.prepare(
                "SELECT canon_key, anilist_id, display_title, followed, backlog_status, watched_externally, seen_watermark
                 FROM library",
            )?;
            let lib_rows = stmt.query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, Option<i64>>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, i64>(3)? != 0,
                    r.get::<_, Option<String>>(4)?,
                    r.get::<_, i64>(5)? != 0,
                    r.get::<_, f64>(6)?,
                ))
            })?;
            for r in lib_rows {
                let (canon_key, anilist_id, title, followed, backlog, watched, watermark) = r?;
                map.insert(
                    canon_key,
                    ShowState {
                        anilist_id,
                        title,
                        followed,
                        watched,
                        backlog,
                        watermark,
                    },
                );
            }
        }

        let group_keys = self.canonical_group_keys()?;

        // Add discarded backlog rows: library leaves them out on purpose.
        // Only keys starting with "al:".
        {
            let mut disc_stmt = self.conn.prepare(
                "SELECT s.id, s.anilist_id, s.title
                 FROM series s
                 WHERE s.backlog_status = 'discarded'
                 ORDER BY s.id ASC",
            )?;
            struct DiscRow {
                id: i64,
                anilist_id: Option<i64>,
                title: String,
            }
            let disc_rows: Vec<DiscRow> = disc_stmt
                .query_map([], |r| {
                    Ok(DiscRow {
                        id: r.get(0)?,
                        anilist_id: r.get(1)?,
                        title: r.get(2)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;

            for row in disc_rows {
                let key = group_keys
                    .get(&row.id)
                    .cloned()
                    .unwrap_or_else(|| canon_key(row.anilist_id, &row.title));
                if !key.starts_with("al:") {
                    continue;
                }
                if let Some(existing) = map.get_mut(&key) {
                    if is_better_title(&row.title, &existing.title) {
                        existing.title = row.title;
                    }
                } else {
                    let aid = row.anilist_id.or_else(|| key.strip_prefix("al:").and_then(|s| s.parse().ok()));
                    map.insert(
                        key,
                        ShowState {
                            anilist_id: aid,
                            title: row.title,
                            followed: false,
                            watched: false,
                            backlog: Some("discarded".to_string()),
                            watermark: 0.0,
                        },
                    );
                }
            }
        }

        // For each key in extra_keys not yet in the map: compute state from member rows.
        let mut key_to_sids: HashMap<String, Vec<i64>> = HashMap::new();
        for (&sid, k) in &group_keys {
            key_to_sids.entry(k.clone()).or_default().push(sid);
        }

        let now = chrono::Utc::now().timestamp();
        for key in extra_keys {
            if map.contains_key(key) {
                continue;
            }
            let Some(sids) = key_to_sids.get(key) else {
                continue;
            };
            if sids.is_empty() {
                continue;
            }

            let mut followed = false;
            let mut watched = false;
            let mut backlog: Option<String> = None;
            let mut watermark = 0.0f64;
            let mut anilist_id: Option<i64> = None;
            let mut display_title = String::new();
            let mut first = true;

            for &sid in sids {
                let mut s_stmt = self.conn.prepare(
                    "SELECT anilist_id, title, followed, watched_externally, backlog_status
                     FROM series WHERE id=?1",
                )?;
                let s_row = s_stmt.query_row([sid], |r| {
                    Ok((
                        r.get::<_, Option<i64>>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, i64>(2)? != 0,
                        r.get::<_, i64>(3)? != 0,
                        r.get::<_, Option<String>>(4)?,
                    ))
                })?;
                let (r_aid, r_title, r_fol, r_wat, r_back) = s_row;

                followed |= r_fol;
                watched |= r_wat;
                backlog = merge_backlog(backlog, r_back);
                anilist_id = anilist_id.or(r_aid);

                if first || is_better_title(&r_title, &display_title) {
                    display_title = r_title;
                    first = false;
                }

                let align = self.row_align(sid, now)?;
                let mut ep_stmt = self.conn.prepare(
                    "SELECT number FROM episodes WHERE series_id=?1 AND seen=1",
                )?;
                let ep_rows = ep_stmt.query_map([sid], |r| r.get::<_, String>(0))?;
                for ep_res in ep_rows {
                    let ep_str = ep_res?;
                    if let Some(parsed) = parse_ep_number(&ep_str) {
                        if let Some(canon) = align.to_canonical(parsed) {
                            if canon > watermark {
                                watermark = canon;
                            }
                        }
                    }
                }
            }

            let final_backlog = if followed || watched { None } else { backlog };
            let final_anilist_id = anilist_id.or_else(|| key.strip_prefix("al:").and_then(|s| s.parse().ok()));

            map.insert(
                key.clone(),
                ShowState {
                    anilist_id: final_anilist_id,
                    title: display_title,
                    followed,
                    watched,
                    backlog: final_backlog,
                    watermark,
                },
            );
        }

        Ok(map)
    }

    /// Load the remembered sync base from SQLite. Invalid JSON rows are skipped with an eprintln.
    pub fn load_sync_base(&self) -> Result<BTreeMap<String, BaseEntry>> {
        let mut stmt = self.conn.prepare(
            "SELECT canon_key, merged, observed FROM sync_base ORDER BY canon_key ASC",
        )?;
        let mut map = BTreeMap::new();
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?;
        for row in rows {
            let (key, merged_raw, observed_raw) = row?;
            let merged: ShowRecord = match serde_json::from_str(&merged_raw) {
                Ok(m) => m,
                Err(e) => {
                    eprintln!("[sync] failed to parse merged JSON for {key}: {e}");
                    continue;
                }
            };
            let observed: ShowState = match serde_json::from_str(&observed_raw) {
                Ok(o) => o,
                Err(e) => {
                    eprintln!("[sync] failed to parse observed JSON for {key}: {e}");
                    continue;
                }
            };
            map.insert(key, BaseEntry { merged, observed });
        }
        Ok(map)
    }

    /// Persist the sync base, replacing the whole table in a single transaction.
    pub fn save_sync_base(&self, base: &BTreeMap<String, BaseEntry>) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute("DELETE FROM sync_base", [])?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO sync_base (canon_key, merged, observed) VALUES (?1, ?2, ?3)",
            )?;
            for (key, entry) in base {
                let merged_json = serde_json::to_string(&entry.merged)?;
                let observed_json = serde_json::to_string(&entry.observed)?;
                stmt.execute((key, &merged_json, &observed_json))?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Get or create the unique sync device ID for this install (e.g. `win-0123456789abcdef`).
    pub fn sync_device_id(&self) -> Result<String> {
        if let Some(existing) = self.get_setting("sync_device_id")? {
            let trimmed = existing.trim();
            if !trimmed.is_empty() {
                return Ok(trimmed.to_string());
            }
        }
        let prefix = if cfg!(windows) {
            "win-"
        } else if cfg!(target_os = "android") {
            "android-"
        } else {
            "dev-"
        };
        let mut bytes = [0u8; 8];
        getrandom::fill(&mut bytes).map_err(|e| anyhow::anyhow!("getrandom failed: {e}"))?;
        let hex = bytes.iter().map(|b| format!("{b:02x}")).collect::<String>();
        let device_id = format!("{prefix}{hex}");
        self.set_setting("sync_device_id", &device_id)?;
        Ok(device_id)
    }

    /// Apply merged sync operations onto the local database.
    ///
    /// All changes are wrapped in a single transaction: if any operation fails
    /// the entire batch is rolled back so we never leave the DB in a half-applied
    /// state.
    pub fn apply_sync_ops(&self, source_id: i64, ops: &[ApplyOp], now: i64) -> Result<ApplyReport> {
        let tx = self.conn.unchecked_transaction()?;
        let mut report = ApplyReport::default();
        let mut group_keys = self.canonical_group_keys()?;

        for op in ops {
            let mut members: Vec<i64> = group_keys
                .iter()
                .filter_map(|(&sid, k)| if k == &op.key { Some(sid) } else { None })
                .collect();
            members.sort_unstable();

            let mut row_materialized = false;
            if members.is_empty() {
                let engaged = is_engaged(&op.change);
                if let (true, Some(id)) = (engaged, op.anilist_id) {
                    let series = Series {
                        id: 0,
                        slug: format!("anilist-{id}"),
                        title: op.title.clone(),
                        url: format!("https://anilist.co/anime/{id}"),
                        cover_url: None,
                        is_airing: false,
                        followed: false,
                        next_episode_at: None,
                        site_episode_count: None,
                    };
                    let sid = self.upsert_series(source_id, &series)?;
                    self.set_anilist_id(sid, id)?;
                    report.materialized += 1;
                    row_materialized = true;
                    members = vec![sid];
                    group_keys = self.canonical_group_keys()?;
                } else {
                    report.skipped += 1;
                    continue;
                }
            }

            let mut changed = row_materialized;

            match &op.change {
                FieldChange::Followed(v) => {
                    let any_diff = members.iter().any(|&sid| {
                        let cur: Option<bool> = tx
                            .query_row("SELECT followed FROM series WHERE id=?1", [sid], |r| {
                                Ok(r.get::<_, i64>(0)? != 0)
                            })
                            .ok();
                        cur != Some(*v)
                    });
                    // Update ALL member rows, not just the first.
                    for &sid in &members {
                        self.set_followed_canonical(sid, *v)?;
                    }
                    if any_diff {
                        changed = true;
                    }
                }
                FieldChange::Watched(v) => {
                    for &sid in &members {
                        let cur: Option<bool> = tx
                            .query_row(
                                "SELECT watched_externally FROM series WHERE id=?1",
                                [sid],
                                |r| Ok(r.get::<_, i64>(0)? != 0),
                            )
                            .ok();
                        if cur != Some(*v) {
                            changed = true;
                        }
                        self.set_watched_externally(sid, *v)?;
                    }
                }
                FieldChange::Backlog(v) => {
                    for &sid in &members {
                        let cur: Option<Option<String>> = tx
                            .query_row("SELECT backlog_status FROM series WHERE id=?1", [sid], |r| {
                                r.get(0)
                            })
                            .ok();
                        if cur != Some(v.clone()) {
                            changed = true;
                        }
                        self.set_backlog_status(sid, v.as_deref())?;
                    }
                }
                FieldChange::Watermark(w) => {
                    let w = *w;
                    for &sid in &members {
                        let mut stmt = tx
                            .prepare("SELECT id, number, seen FROM episodes WHERE series_id=?1")?;
                        let eps: Vec<(i64, f64, bool)> = stmt
                            .query_map([sid], |r| {
                                let id: i64 = r.get(0)?;
                                let num: String = r.get(1)?;
                                let seen: bool = r.get::<_, i64>(2)? != 0;
                                Ok((id, num, seen))
                            })?
                            .filter_map(|r| r.ok())
                            .filter_map(|(id, num, seen)| {
                                parse_ep_number(&num).map(|p| (id, p, seen))
                            })
                            .collect();

                        if eps.is_empty() {
                            continue;
                        }

                        let align = self.row_align(sid, now)?;
                        let target = align.to_row(w);

                        let needs_raise = w > 0.0 && eps.iter().any(|(_, num, seen)| *num <= target && !*seen);
                        if needs_raise {
                            self.set_seen_cascade(sid, &format_ep_number(target), true)?;
                            changed = true;
                        }

                        // `num > target` also covers w == 0: on a cumulative row (one slug
                        // for every season) `target` is then the season's offset, so the
                        // earlier seasons' episodes stay seen.
                        let eps_to_unmark: Vec<i64> = eps
                            .iter()
                            .filter(|(_, num, seen)| *seen && *num > target)
                            .map(|(id, _, _)| *id)
                            .collect();

                        if !eps_to_unmark.is_empty() {
                            for ep_id in eps_to_unmark {
                                tx.execute(
                                    "UPDATE episodes SET seen=0, seen_at=NULL WHERE id=?1",
                                    [ep_id],
                                )?;
                            }
                            changed = true;
                        }

                        let mut num_stmt = tx
                            .prepare("SELECT number FROM episodes WHERE series_id=?1 AND seen=1")?;
                        let mut real_watermark = 0.0f64;
                        let nums = num_stmt.query_map([sid], |r| r.get::<_, String>(0))?;
                        for n_res in nums {
                            let n_str = n_res?;
                            if let Some(parsed) = parse_ep_number(&n_str) {
                                if parsed > real_watermark {
                                    real_watermark = parsed;
                                }
                            }
                        }

                        let cur_applied: Option<f64> = tx
                            .query_row(
                                "SELECT sync_watermark_applied FROM series WHERE id=?1",
                                [sid],
                                |r| r.get(0),
                            )
                            .ok()
                            .flatten();
                        if cur_applied != Some(real_watermark) {
                            changed = true;
                        }

                        tx.execute(
                            "UPDATE series SET sync_watermark_applied=?2 WHERE id=?1",
                            (sid, real_watermark),
                        )?;
                    }
                }
            }

            if changed {
                report.applied += 1;
            }
        }

        tx.commit()?;
        Ok(report)
    }

}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_support::*;
    use crate::sync::doc::{ApplyOp, BaseEntry, FieldChange, ShowRecord, ShowState, Stamp};
    use std::collections::{BTreeMap, BTreeSet};

    fn stamp<T>(v: T) -> Option<Stamp<T>> {
        Some(Stamp { v, at: 1000, dev: "test".into() })
    }

    #[test]
    fn projection_followed_series_with_seen_episodes() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "https://animeyt.cc", "animeytx").unwrap();
        let s = Series {
            id: 0,
            slug: "show-10".into(),
            title: "Show 10".into(),
            url: "https://animeyt.cc/tv/show-10".into(),
            cover_url: None,
            is_airing: true,
            followed: true,
            next_episode_at: None,
            site_episode_count: None,
        };
        let sid = db.upsert_series(src, &s).unwrap();
        db.set_anilist_id(sid, 10).unwrap();
        // `upsert_series` never writes `followed` (scans must not un-follow).
        db.set_followed_canonical(sid, true).unwrap();
        insert_eps_seen_up_to(&db, sid, 12, 5);

        let extra_keys = BTreeSet::new();
        let proj = db.sync_projection(&extra_keys).unwrap();
        let state = proj.get("al:10").expect("al:10 must be in projection");
        assert!(state.followed);
        assert_eq!(state.watermark, 5.0);
        assert_eq!(state.anilist_id, Some(10));
    }

    #[test]
    fn projection_discarded_swipe_series() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "https://animeyt.cc", "animeytx").unwrap();
        let s = Series {
            id: 0,
            slug: "anilist-20".into(),
            title: "Show 20".into(),
            url: "https://anilist.co/anime/20".into(),
            cover_url: None,
            is_airing: false,
            followed: false,
            next_episode_at: None,
            site_episode_count: None,
        };
        let sid = db.upsert_series(src, &s).unwrap();
        db.set_anilist_id(sid, 20).unwrap();
        db.set_backlog_status(sid, Some("discarded")).unwrap();

        let extra_keys = BTreeSet::new();
        let proj = db.sync_projection(&extra_keys).unwrap();
        let state = proj.get("al:20").expect("al:20 must be in projection");
        assert_eq!(state.backlog.as_deref(), Some("discarded"));
        assert!(!state.followed);
        assert_eq!(state.anilist_id, Some(20));
    }

    #[test]
    fn projection_with_extra_keys_unfollowed_and_absent() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "https://animeyt.cc", "animeytx").unwrap();
        let s = Series {
            id: 0,
            slug: "anilist-30".into(),
            title: "Show 30".into(),
            url: "https://anilist.co/anime/30".into(),
            cover_url: None,
            is_airing: false,
            followed: false,
            next_episode_at: None,
            site_episode_count: None,
        };
        let sid = db.upsert_series(src, &s).unwrap();
        db.set_anilist_id(sid, 30).unwrap();

        let mut extra_keys = BTreeSet::new();
        extra_keys.insert("al:30".to_string());
        extra_keys.insert("al:999".to_string());

        let proj = db.sync_projection(&extra_keys).unwrap();
        let state_30 = proj.get("al:30").expect("al:30 must be present");
        assert!(!state_30.followed);
        assert_eq!(state_30.anilist_id, Some(30));
        assert!(!proj.contains_key("al:999"), "al:999 must be absent");
    }

    #[test]
    fn base_round_trip_and_invalid_json_skipped() {
        let db = Db::open(":memory:").unwrap();
        let mut base = BTreeMap::new();
        base.insert(
            "al:1".to_string(),
            BaseEntry {
                merged: ShowRecord {
                    anilist_id: Some(1),
                    title: "One".into(),
                    followed: stamp(true),
                    ..Default::default()
                },
                observed: ShowState {
                    anilist_id: Some(1),
                    title: "One".into(),
                    followed: true,
                    ..Default::default()
                },
            },
        );
        base.insert(
            "al:2".to_string(),
            BaseEntry {
                merged: ShowRecord {
                    anilist_id: Some(2),
                    title: "Two".into(),
                    watched: stamp(true),
                    ..Default::default()
                },
                observed: ShowState {
                    anilist_id: Some(2),
                    title: "Two".into(),
                    watched: true,
                    ..Default::default()
                },
            },
        );

        db.save_sync_base(&base).unwrap();
        let loaded = db.load_sync_base().unwrap();
        assert_eq!(loaded, base);

        // Corrupt row should be skipped with eprintln, not error
        db.conn
            .execute(
                "INSERT INTO sync_base (canon_key, merged, observed) VALUES (?1, ?2, ?3)",
                ("al:corrupt", "{invalid json", "{invalid json"),
            )
            .unwrap();

        let loaded2 = db.load_sync_base().unwrap();
        assert_eq!(loaded2.len(), 2);
        assert_eq!(loaded2, base);
    }

    #[test]
    fn sync_device_id_stable_and_format() {
        let db = Db::open(":memory:").unwrap();
        let id1 = db.sync_device_id().unwrap();
        let id2 = db.sync_device_id().unwrap();
        assert_eq!(id1, id2, "Device ID must be stable across two calls");

        let hex = ["win-", "android-", "dev-"]
            .iter()
            .find_map(|p| id1.strip_prefix(p))
            .unwrap_or_else(|| panic!("Device ID prefix must be win-, android-, or dev-: {id1}"));
        assert_eq!(hex.len(), 16, "Must be 16 lowercase hex chars");
        assert!(hex.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
    }

    #[test]
    fn apply_followed_true_existing_unfollowed() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "https://animeyt.cc", "animeytx").unwrap();
        let s = Series {
            id: 0,
            slug: "show-existing".into(),
            title: "Show Existing".into(),
            url: "https://animeyt.cc/tv/show-existing".into(),
            cover_url: None,
            is_airing: true,
            followed: false,
            next_episode_at: None,
            site_episode_count: None,
        };
        let sid = db.upsert_series(src, &s).unwrap();
        db.set_anilist_id(sid, 100).unwrap();

        let op = ApplyOp {
            key: "al:100".into(),
            anilist_id: Some(100),
            title: "Show Existing".into(),
            change: FieldChange::Followed(true),
        };
        let report = db.apply_sync_ops(src, &[op], 1000).unwrap();
        assert_eq!(report.applied, 1);
        assert_eq!(report.materialized, 0);
        assert_eq!(report.skipped, 0);

        let followed: i64 = db
            .conn
            .query_row("SELECT followed FROM series WHERE id=?1", [sid], |r| r.get(0))
            .unwrap();
        assert_eq!(followed, 1);
    }

    #[test]
    fn apply_followed_true_materializes_synthetic_row() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "https://animeyt.cc", "animeytx").unwrap();

        let op = ApplyOp {
            key: "al:40".into(),
            anilist_id: Some(40),
            title: "Show 40".into(),
            change: FieldChange::Followed(true),
        };
        let report = db.apply_sync_ops(src, &[op], 1000).unwrap();
        assert_eq!(report.materialized, 1);
        assert_eq!(report.applied, 1);
        assert_eq!(report.skipped, 0);

        let (slug, title, url, anilist_id, followed, is_airing): (String, String, String, Option<i64>, i64, i64) = db
            .conn
            .query_row(
                "SELECT slug, title, url, anilist_id, followed, is_airing FROM series WHERE source_id=?1",
                [src],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
            )
            .unwrap();
        assert_eq!(slug, "anilist-40");
        assert_eq!(title, "Show 40");
        assert_eq!(url, "https://anilist.co/anime/40");
        assert_eq!(anilist_id, Some(40));
        assert_eq!(followed, 1);
        assert_eq!(is_airing, 0);
    }

    #[test]
    fn apply_followed_true_title_key_no_rows_skipped() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "https://animeyt.cc", "animeytx").unwrap();

        let op = ApplyOp {
            key: "t:foo".into(),
            anilist_id: None,
            title: "foo".into(),
            change: FieldChange::Followed(true),
        };
        let report = db.apply_sync_ops(src, &[op], 1000).unwrap();
        assert_eq!(report.skipped, 1);
        assert_eq!(report.applied, 0);
        assert_eq!(report.materialized, 0);

        let count: i64 = db.conn.query_row("SELECT count(*) FROM series", [], |r| r.get(0)).unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn apply_followed_false_no_rows_skipped() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "https://animeyt.cc", "animeytx").unwrap();

        let op = ApplyOp {
            key: "al:41".into(),
            anilist_id: Some(41),
            title: "Show 41".into(),
            change: FieldChange::Followed(false),
        };
        let report = db.apply_sync_ops(src, &[op], 1000).unwrap();
        assert_eq!(report.skipped, 1);
        assert_eq!(report.applied, 0);
        assert_eq!(report.materialized, 0);

        let count: i64 = db.conn.query_row("SELECT count(*) FROM series", [], |r| r.get(0)).unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn apply_watermark_raise_then_lower() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "https://animeyt.cc", "animeytx").unwrap();
        let s = Series {
            id: 0,
            slug: "show-50".into(),
            title: "Show 50".into(),
            url: "https://animeyt.cc/tv/show-50".into(),
            cover_url: None,
            is_airing: true,
            followed: true,
            next_episode_at: None,
            site_episode_count: None,
        };
        let sid = db.upsert_series(src, &s).unwrap();
        db.set_anilist_id(sid, 50).unwrap();
        insert_eps_seen_up_to(&db, sid, 12, 3);

        // Raise to 8.0
        let op_raise = ApplyOp {
            key: "al:50".into(),
            anilist_id: Some(50),
            title: "Show 50".into(),
            change: FieldChange::Watermark(8.0),
        };
        let r1 = db.apply_sync_ops(src, &[op_raise], 1000).unwrap();
        assert_eq!(r1.applied, 1);

        let mut stmt = db.conn.prepare("SELECT number, seen FROM episodes WHERE series_id=?1 ORDER BY CAST(number AS INTEGER)").unwrap();
        let ep_rows: Vec<(String, i64)> = stmt
            .query_map([sid], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        for (num_str, seen) in &ep_rows {
            let n: i64 = num_str.parse().unwrap();
            if n <= 8 {
                assert_eq!(*seen, 1, "Episode {n} must be seen");
            } else {
                assert_eq!(*seen, 0, "Episode {n} must be unseen");
            }
        }

        // Lower to 2.0 afterwards
        let op_lower = ApplyOp {
            key: "al:50".into(),
            anilist_id: Some(50),
            title: "Show 50".into(),
            change: FieldChange::Watermark(2.0),
        };
        let r2 = db.apply_sync_ops(src, &[op_lower], 1000).unwrap();
        assert_eq!(r2.applied, 1);

        let ep_rows2: Vec<(String, i64)> = stmt
            .query_map([sid], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        for (num_str, seen) in &ep_rows2 {
            let n: i64 = num_str.parse().unwrap();
            if n <= 2 {
                assert_eq!(*seen, 1, "Episode {n} must be seen");
            } else {
                assert_eq!(*seen, 0, "Episode {n} must be unseen");
            }
        }

        let applied_wm: Option<f64> = db
            .conn
            .query_row("SELECT sync_watermark_applied FROM series WHERE id=?1", [sid], |r| r.get(0))
            .unwrap();
        assert_eq!(applied_wm, Some(2.0));
    }

    #[test]
    fn apply_watermark_beyond_existing_episodes() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "https://animeyt.cc", "animeytx").unwrap();
        let s = Series {
            id: 0,
            slug: "show-60".into(),
            title: "Show 60".into(),
            url: "https://animeyt.cc/tv/show-60".into(),
            cover_url: None,
            is_airing: true,
            followed: true,
            next_episode_at: None,
            site_episode_count: None,
        };
        let sid = db.upsert_series(src, &s).unwrap();
        db.set_anilist_id(sid, 60).unwrap();
        insert_eps_seen_up_to(&db, sid, 12, 0);

        let op = ApplyOp {
            key: "al:60".into(),
            anilist_id: Some(60),
            title: "Show 60".into(),
            change: FieldChange::Watermark(20.0),
        };
        let report = db.apply_sync_ops(src, &[op], 1000).unwrap();
        assert_eq!(report.applied, 1);

        let unseen_count: i64 = db
            .conn
            .query_row("SELECT count(*) FROM episodes WHERE series_id=?1 AND seen=0", [sid], |r| r.get(0))
            .unwrap();
        assert_eq!(unseen_count, 0, "All 12 episodes must be seen");
    }

    #[test]
    fn apply_watermark_row_with_no_episodes() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "https://animeyt.cc", "animeytx").unwrap();
        let s = Series {
            id: 0,
            slug: "show-70".into(),
            title: "Show 70".into(),
            url: "https://animeyt.cc/tv/show-70".into(),
            cover_url: None,
            is_airing: true,
            followed: true,
            next_episode_at: None,
            site_episode_count: None,
        };
        let sid = db.upsert_series(src, &s).unwrap();
        db.set_anilist_id(sid, 70).unwrap();

        let op = ApplyOp {
            key: "al:70".into(),
            anilist_id: Some(70),
            title: "Show 70".into(),
            change: FieldChange::Watermark(5.0),
        };
        let report = db.apply_sync_ops(src, &[op], 1000).unwrap();
        assert_eq!(report.applied, 0);
        assert_eq!(report.materialized, 0);
        assert_eq!(report.skipped, 0);
    }

    #[test]
    fn apply_watermark_zero_unmarks_all() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "https://animeyt.cc", "animeytx").unwrap();
        let s = Series {
            id: 0,
            slug: "show-zero".into(),
            title: "Show Zero".into(),
            url: "https://animeyt.cc/tv/show-zero".into(),
            cover_url: None,
            is_airing: true,
            followed: true,
            next_episode_at: None,
            site_episode_count: None,
        };
        let sid = db.upsert_series(src, &s).unwrap();
        db.set_anilist_id(sid, 95).unwrap();
        insert_eps_seen_up_to(&db, sid, 5, 3);

        let op = ApplyOp {
            key: "al:95".into(),
            anilist_id: Some(95),
            title: "Show Zero".into(),
            change: FieldChange::Watermark(0.0),
        };
        let report = db.apply_sync_ops(src, &[op], 1000).unwrap();
        assert_eq!(report.applied, 1);

        let seen_count: i64 = db
            .conn
            .query_row("SELECT count(*) FROM episodes WHERE series_id=?1 AND seen=1", [sid], |r| r.get(0))
            .unwrap();
        assert_eq!(seen_count, 0, "w == 0.0 means nothing seen");

        let applied_wm: Option<f64> = db
            .conn
            .query_row("SELECT sync_watermark_applied FROM series WHERE id=?1", [sid], |r| r.get(0))
            .unwrap();
        assert_eq!(applied_wm, Some(0.0));
    }

    #[test]
    fn apply_backlog_and_watched_to_existing_rows() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "https://animeyt.cc", "animeytx").unwrap();
        let s = Series {
            id: 0,
            slug: "show-80".into(),
            title: "Show 80".into(),
            url: "https://animeyt.cc/tv/show-80".into(),
            cover_url: None,
            is_airing: true,
            followed: false,
            next_episode_at: None,
            site_episode_count: None,
        };
        let sid = db.upsert_series(src, &s).unwrap();
        db.set_anilist_id(sid, 80).unwrap();

        let ops = vec![
            ApplyOp {
                key: "al:80".into(),
                anilist_id: Some(80),
                title: "Show 80".into(),
                change: FieldChange::Backlog(Some("want".into())),
            },
            ApplyOp {
                key: "al:80".into(),
                anilist_id: Some(80),
                title: "Show 80".into(),
                change: FieldChange::Watched(true),
            },
        ];
        let report = db.apply_sync_ops(src, &ops, 1000).unwrap();
        assert_eq!(report.applied, 2);

        let (backlog, watched): (Option<String>, i64) = db
            .conn
            .query_row(
                "SELECT backlog_status, watched_externally FROM series WHERE id=?1",
                [sid],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(backlog.as_deref(), Some("want"));
        assert_eq!(watched, 1);
    }

    #[test]
    fn must_not_change_unrelated_tables() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "https://animeyt.cc", "animeytx").unwrap();
        let s = Series {
            id: 0,
            slug: "show-90".into(),
            title: "Show 90".into(),
            url: "https://animeyt.cc/tv/show-90".into(),
            cover_url: None,
            is_airing: true,
            followed: false,
            next_episode_at: None,
            site_episode_count: None,
        };
        let sid = db.upsert_series(src, &s).unwrap();
        db.set_anilist_id(sid, 90).unwrap();
        insert_eps_seen_up_to(&db, sid, 5, 2);

        let sources_count_before: i64 = db.conn.query_row("SELECT count(*) FROM sources", [], |r| r.get(0)).unwrap();

        let ops = vec![
            ApplyOp {
                key: "al:90".into(),
                anilist_id: Some(90),
                title: "Show 90".into(),
                change: FieldChange::Followed(true),
            },
            ApplyOp {
                key: "al:90".into(),
                anilist_id: Some(90),
                title: "Show 90".into(),
                change: FieldChange::Watermark(4.0),
            },
        ];
        db.apply_sync_ops(src, &ops, 1000).unwrap();

        let sources_count_after: i64 = db.conn.query_row("SELECT count(*) FROM sources", [], |r| r.get(0)).unwrap();
        assert_eq!(sources_count_before, sources_count_after);
    }

    #[test]
    fn apply_followed_updates_all_member_rows_in_group() {
        let db = Db::open(":memory:").unwrap();
        let src1 = db.upsert_source("AnimeYT", "https://animeyt.cc", "animeytx").unwrap();
        let src2 = db.upsert_source("TioAnime", "https://tioanime.com", "tioanime").unwrap();

        // Two series rows with same anilist_id = 200 (a canonical group of 2 members)
        let s1 = Series {
            id: 0,
            slug: "show-group-1".into(),
            title: "Show Group 1".into(),
            url: "https://animeyt.cc/tv/show-group-1".into(),
            cover_url: None,
            is_airing: true,
            followed: false,
            next_episode_at: None,
            site_episode_count: None,
        };
        let sid1 = db.upsert_series(src1, &s1).unwrap();
        db.set_anilist_id(sid1, 200).unwrap();

        let s2 = Series {
            id: 0,
            slug: "show-group-2".into(),
            title: "Show Group 2".into(),
            url: "https://tioanime.com/anime/show-group-2".into(),
            cover_url: None,
            is_airing: true,
            followed: false,
            next_episode_at: None,
            site_episode_count: None,
        };
        let sid2 = db.upsert_series(src2, &s2).unwrap();
        db.set_anilist_id(sid2, 200).unwrap();

        let op = ApplyOp {
            key: "al:200".into(),
            anilist_id: Some(200),
            title: "Show Group".into(),
            change: FieldChange::Followed(true),
        };

        let report = db.apply_sync_ops(src1, &[op], 1000).unwrap();
        assert_eq!(report.applied, 1);

        // Verify BOTH member rows were updated to followed = 1
        let f1: i64 = db.conn.query_row("SELECT followed FROM series WHERE id=?1", [sid1], |r| r.get(0)).unwrap();
        let f2: i64 = db.conn.query_row("SELECT followed FROM series WHERE id=?1", [sid2], |r| r.get(0)).unwrap();
        assert_eq!(f1, 1, "First member of group must be followed");
        assert_eq!(f2, 1, "Second member of group must also be followed");
    }
}
