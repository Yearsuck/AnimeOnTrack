//! Pure three-way merge per field: local (this device now), base (what this
//! device published and observed at its last sync), remote (the shared document).
//!
//! ## Stamp ordering
//!
//! Every stamped value carries `(at, dev)`. Stamps are totally ordered:
//! `(at, dev)` lexicographic — first by `at` (ascending), then by `dev`
//! (ascending string compare) to break ties deterministically. A remote stamp
//! counts as a change only when it **strictly exceeds** the base stamp in this
//! order. If the remote stamp is **less** than the base (stale/rolled-back file,
//! lost write race), it is **not** a change: the output keeps the base stamp
//! (republishing it) and the stale remote value is never applied locally.
//!
//! ## Lamport clock
//!
//! When this device stamps a value it uses
//! `at = max(now, base.at + 1, remote.at + 1)` (over whichever exist), so a new
//! write always orders strictly after everything the writer observed. This makes
//! the ordering safe even under clock skew.
//!
//! ## Conflict resolution
//!
//! A field changed locally when the device's state differs from what it observed
//! right after its last sync; it changed remotely when the document's stamp
//! strictly exceeds the base. When both changed to different values the more
//! engaged value wins (follow / "Ya lo vi" true, the higher watermark,
//! want > discarded > none): a device only knows its change happened since its
//! last sync, not when, so comparing timestamps would just favour whoever syncs
//! later.

use std::collections::{BTreeMap, BTreeSet};

use crate::sync::doc::{
    ApplyOp, BaseEntry, FieldChange, ShowRecord, ShowState, Stamp, SyncDoc, SYNC_SCHEMA_VERSION,
};

/// Total order on stamps: `(at, dev)` lexicographic.
/// First by `at` ascending, then by `dev` ascending (string compare).
fn stamp_gt<T>(a: &Stamp<T>, b: &Stamp<T>) -> bool {
    (a.at, &a.dev) > (b.at, &b.dev)
}

/// Lamport clock: the timestamp for a new local stamp that must order after
/// everything the writer has observed (`base` and `remote`).
fn lamport_at(now: i64, base_stamp: Option<&i64>, remote_stamp: Option<&i64>) -> i64 {
    let mut at = now;
    if let Some(&b) = base_stamp {
        at = at.max(b + 1);
    }
    if let Some(&r) = remote_stamp {
        at = at.max(r + 1);
    }
    at
}

pub struct MergeInput<'a> {
    pub local: &'a BTreeMap<String, ShowState>,
    pub base: &'a BTreeMap<String, BaseEntry>,
    pub remote: &'a SyncDoc,
    pub device: &'a str,
    pub now: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MergeOutput {
    /// The document to upload.
    pub doc: SyncDoc,
    /// Changes for the local DB, sorted by key then field (followed, watched, backlog, watermark).
    pub apply: Vec<ApplyOp>,
    /// Fields this device changed since its last sync (published now).
    pub local_changes: usize,
    /// Fields taken from the remote that differ from local state.
    pub remote_changes: usize,
}

/// Backlog status precedence: Some("want") > Some("discarded") > None.
fn backlog_precedence(b: &Option<String>) -> u8 {
    match b.as_deref() {
        Some("want") => 2,
        Some("discarded") => 1,
        _ => 0,
    }
}

/// How one field type compares, resolves a conflict and counts as "engaged".
struct FieldRules<T> {
    same: fn(&T, &T) -> bool,
    /// True when the local value beats a different remote value in a conflict.
    local_wins: fn(&T, &T) -> bool,
    /// Values worth materialising a show this device does not have yet.
    engaged: fn(&T) -> bool,
}

const BOOL_RULES: FieldRules<bool> = FieldRules {
    same: |a, b| a == b,
    local_wins: |l, _| *l,
    engaged: |v| *v,
};

const BACKLOG_RULES: FieldRules<Option<String>> = FieldRules {
    same: |a, b| a == b,
    local_wins: |l, r| backlog_precedence(l) > backlog_precedence(r),
    engaged: |v| v.is_some(),
};

const WATERMARK_RULES: FieldRules<f64> = FieldRules {
    same: |a, b| (a - b).abs() <= 1e-9,
    local_wins: |l, r| l > r,
    engaged: |v| *v > 1e-9,
};

struct FieldOutcome<T> {
    out: Option<Stamp<T>>,
    local_won: bool,
    /// Value the local DB must take, if any.
    apply: Option<T>,
}

/// Merge one field. `local` is `None` when this device's state for the key is unknown.
#[allow(clippy::too_many_arguments)]
fn merge_field<T: Clone>(
    rules: &FieldRules<T>,
    local: Option<&T>,
    observed: &T,
    base_stamp: Option<&Stamp<T>>,
    remote_stamp: Option<&Stamp<T>>,
    key_in_base: bool,
    device: &str,
    now: i64,
) -> FieldOutcome<T> {
    let local_changed = local.is_some_and(|l| !(rules.same)(l, observed));
    // A remote stamp counts as a change only when it strictly exceeds the base
    // in the total order (at, dev). If the remote stamp is *less* than the base
    // (stale/rolled-back file, lost write race), it is NOT a change and the
    // output keeps the base stamp (republishing it).
    let remote_changed = match (remote_stamp, base_stamp) {
        (Some(r), Some(b)) => stamp_gt(r, b),
        (Some(_), None) => true,
        (None, _) => false,
    };

    // Lamport clock: new local stamp must order strictly after everything observed.
    let mine = |v: &T| {
        let at = lamport_at(
            now,
            base_stamp.map(|s| &s.at),
            remote_stamp.map(|s| &s.at),
        );
        Stamp { v: v.clone(), at, dev: device.to_string() }
    };

    let (out, local_won) = match (local, remote_stamp) {
        (Some(l), _) if local_changed && !remote_changed => (Some(mine(l)), true),
        (Some(l), Some(r)) if local_changed => {
            if (rules.same)(&r.v, l) || !(rules.local_wins)(l, &r.v) {
                (Some(r.clone()), false)
            } else {
                (Some(mine(l)), true)
            }
        }
        _ if remote_changed => (remote_stamp.cloned(), false),
        // When remote < base (stale), republish base stamp, not the stale remote.
        _ => {
            let best = match (remote_stamp, base_stamp) {
                (Some(r), Some(b)) if stamp_gt(b, r) => Some(b),
                _ => remote_stamp.or(base_stamp),
            };
            (best.cloned(), false)
        }
    };

    let apply = match (local, &out) {
        // Also re-applies a value an earlier sync could not apply (shortfall).
        (Some(l), Some(s)) if !(rules.same)(&s.v, l) => Some(s.v.clone()),
        // Unknown locally AND never seen in a previous sync (key_in_base is false):
        // materialise only engaged remote values. When key_in_base is true the user
        // deleted the series on this device — do not resurrect it.
        (None, Some(s)) if remote_stamp.is_some() && !key_in_base && (rules.engaged)(&s.v) => Some(s.v.clone()),
        _ => None,
    };
    FieldOutcome { out, local_won, apply }
}

pub fn merge(input: &MergeInput) -> MergeOutput {
    let keys: BTreeSet<&str> = input
        .local
        .keys()
        .chain(input.base.keys())
        .chain(input.remote.shows.keys())
        .map(String::as_str)
        .collect();

    let mut shows = BTreeMap::new();
    let mut apply = Vec::new();
    let mut local_changes = 0;
    let mut remote_changes = 0;
    let default_observed = ShowState::default();

    for key in keys {
        let local = input.local.get(key);
        let base = input.base.get(key);
        let remote = input.remote.shows.get(key);
        let observed = base.map(|b| &b.observed).unwrap_or(&default_observed);
        let merged = base.map(|b| &b.merged);
        let key_in_base = base.is_some();

        let anilist_id = local
            .and_then(|s| s.anilist_id)
            .or_else(|| remote.and_then(|s| s.anilist_id))
            .or_else(|| merged.and_then(|m| m.anilist_id));
        let title = [local.map(|s| &s.title), remote.map(|s| &s.title), merged.map(|m| &m.title)]
            .into_iter()
            .flatten()
            .find(|t| !t.is_empty())
            .cloned()
            .unwrap_or_default();

        let mut take = |local_won: bool, change: Option<FieldChange>| {
            if local_won {
                local_changes += 1;
            }
            if let Some(change) = change {
                remote_changes += 1;
                apply.push(ApplyOp { key: key.to_string(), anilist_id, title: title.clone(), change });
            }
        };

        let f = merge_field(
            &BOOL_RULES,
            local.map(|s| &s.followed),
            &observed.followed,
            merged.and_then(|m| m.followed.as_ref()),
            remote.and_then(|r| r.followed.as_ref()),
            key_in_base,
            input.device,
            input.now,
        );
        take(f.local_won, f.apply.map(FieldChange::Followed));
        let w = merge_field(
            &BOOL_RULES,
            local.map(|s| &s.watched),
            &observed.watched,
            merged.and_then(|m| m.watched.as_ref()),
            remote.and_then(|r| r.watched.as_ref()),
            key_in_base,
            input.device,
            input.now,
        );
        take(w.local_won, w.apply.map(FieldChange::Watched));
        let b = merge_field(
            &BACKLOG_RULES,
            local.map(|s| &s.backlog),
            &observed.backlog,
            merged.and_then(|m| m.backlog.as_ref()),
            remote.and_then(|r| r.backlog.as_ref()),
            key_in_base,
            input.device,
            input.now,
        );
        take(b.local_won, b.apply.map(FieldChange::Backlog));
        let m = merge_field(
            &WATERMARK_RULES,
            local.map(|s| &s.watermark),
            &observed.watermark,
            merged.and_then(|m| m.watermark.as_ref()),
            remote.and_then(|r| r.watermark.as_ref()),
            key_in_base,
            input.device,
            input.now,
        );
        take(m.local_won, m.apply.map(FieldChange::Watermark));

        let record = ShowRecord {
            anilist_id,
            title,
            followed: f.out,
            watched: w.out,
            backlog: b.out,
            watermark: m.out,
        };
        if !record.is_empty() {
            shows.insert(key.to_string(), record);
        }
    }

    MergeOutput {
        doc: SyncDoc {
            schema: SYNC_SCHEMA_VERSION,
            updated_at: input.now,
            updated_by: input.device.to_string(),
            shows,
        },
        apply,
        local_changes,
        remote_changes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stamp<T>(v: T, at: i64, dev: &str) -> Option<Stamp<T>> {
        Some(Stamp {
            v,
            at,
            dev: dev.to_string(),
        })
    }

    fn state(followed: bool, watched: bool, backlog: Option<&str>, watermark: f64) -> ShowState {
        ShowState {
            anilist_id: None,
            title: String::new(),
            followed,
            watched,
            backlog: backlog.map(String::from),
            watermark,
        }
    }

    fn base_entry(merged: ShowRecord, observed: ShowState) -> BaseEntry {
        BaseEntry { merged, observed }
    }

    #[test]
    fn first_sync_empty_base_and_remote() {
        let mut local = BTreeMap::new();
        local.insert(
            "al:1".to_string(),
            ShowState {
                anilist_id: Some(1),
                title: "One".to_string(),
                followed: true,
                watched: false,
                backlog: None,
                watermark: 3.0,
            },
        );
        let base = BTreeMap::new();
        let remote = SyncDoc::default();
        let input = MergeInput {
            local: &local,
            base: &base,
            remote: &remote,
            device: "pc",
            now: 100,
        };
        let out = merge(&input);

        assert_eq!(out.local_changes, 2);
        assert_eq!(out.remote_changes, 0);
        assert!(out.apply.is_empty());
        let rec = &out.doc.shows["al:1"];
        assert_eq!(rec.followed, stamp(true, 100, "pc"));
        assert_eq!(rec.watermark, stamp(3.0, 100, "pc"));
        assert_eq!(rec.watched, None);
        assert_eq!(rec.backlog, None);
    }

    #[test]
    fn remote_followed_newer_key_unknown_locally() {
        let local = BTreeMap::new();
        let base = BTreeMap::new();
        let mut remote = SyncDoc::default();
        remote.shows.insert(
            "al:2".to_string(),
            ShowRecord {
                anilist_id: Some(2),
                title: "Two".to_string(),
                followed: stamp(true, 50, "phone"),
                ..Default::default()
            },
        );
        let input = MergeInput {
            local: &local,
            base: &base,
            remote: &remote,
            device: "pc",
            now: 100,
        };
        let out = merge(&input);

        assert_eq!(out.local_changes, 0);
        assert_eq!(out.remote_changes, 1);
        assert_eq!(
            out.apply,
            vec![ApplyOp {
                key: "al:2".to_string(),
                anilist_id: Some(2),
                title: "Two".to_string(),
                change: FieldChange::Followed(true),
            }]
        );
        assert_eq!(out.doc.shows["al:2"].followed, stamp(true, 50, "phone"));
    }

    #[test]
    fn remote_unfollow() {
        let mut local = BTreeMap::new();
        local.insert("al:1".to_string(), state(true, false, None, 0.0));

        let mut base = BTreeMap::new();
        base.insert(
            "al:1".to_string(),
            base_entry(
                ShowRecord {
                    followed: stamp(true, 10, "pc"),
                    ..Default::default()
                },
                state(true, false, None, 0.0),
            ),
        );

        let mut remote = SyncDoc::default();
        remote.shows.insert(
            "al:1".to_string(),
            ShowRecord {
                followed: stamp(false, 20, "phone"),
                ..Default::default()
            },
        );

        let input = MergeInput {
            local: &local,
            base: &base,
            remote: &remote,
            device: "pc",
            now: 100,
        };
        let out = merge(&input);

        assert_eq!(out.local_changes, 0);
        assert_eq!(out.remote_changes, 1);
        assert_eq!(
            out.apply,
            vec![ApplyOp {
                key: "al:1".to_string(),
                anilist_id: None,
                title: String::new(),
                change: FieldChange::Followed(false),
            }]
        );
        assert_eq!(out.doc.shows["al:1"].followed, stamp(false, 20, "phone"));
    }

    #[test]
    fn local_unmark_wins_over_unchanged_remote() {
        let mut local = BTreeMap::new();
        local.insert("al:1".to_string(), state(false, false, None, 4.0));

        let mut base = BTreeMap::new();
        base.insert(
            "al:1".to_string(),
            base_entry(
                ShowRecord {
                    watermark: stamp(10.0, 10, "pc"),
                    ..Default::default()
                },
                state(false, false, None, 10.0),
            ),
        );

        let mut remote = SyncDoc::default();
        remote.shows.insert(
            "al:1".to_string(),
            ShowRecord {
                watermark: stamp(10.0, 10, "pc"),
                ..Default::default()
            },
        );

        let input = MergeInput {
            local: &local,
            base: &base,
            remote: &remote,
            device: "pc",
            now: 100,
        };
        let out = merge(&input);

        assert_eq!(out.local_changes, 1);
        assert_eq!(out.remote_changes, 0);
        assert!(out.apply.is_empty());
        assert_eq!(out.doc.shows["al:1"].watermark, stamp(4.0, 100, "pc"));
    }

    #[test]
    fn remote_unmark() {
        let mut local = BTreeMap::new();
        local.insert("al:1".to_string(), state(false, false, None, 10.0));

        let mut base = BTreeMap::new();
        base.insert(
            "al:1".to_string(),
            base_entry(
                ShowRecord {
                    watermark: stamp(10.0, 10, "pc"),
                    ..Default::default()
                },
                state(false, false, None, 10.0),
            ),
        );

        let mut remote = SyncDoc::default();
        remote.shows.insert(
            "al:1".to_string(),
            ShowRecord {
                watermark: stamp(4.0, 20, "phone"),
                ..Default::default()
            },
        );

        let input = MergeInput {
            local: &local,
            base: &base,
            remote: &remote,
            device: "pc",
            now: 100,
        };
        let out = merge(&input);

        assert_eq!(out.local_changes, 0);
        assert_eq!(out.remote_changes, 1);
        assert_eq!(
            out.apply,
            vec![ApplyOp {
                key: "al:1".to_string(),
                anilist_id: None,
                title: String::new(),
                change: FieldChange::Watermark(4.0),
            }]
        );
        assert_eq!(out.doc.shows["al:1"].watermark, stamp(4.0, 20, "phone"));
    }

    #[test]
    fn shortfall_is_not_an_unmark_retries_remote() {
        let mut local = BTreeMap::new();
        local.insert("al:1".to_string(), state(false, false, None, 8.0));

        let mut base = BTreeMap::new();
        base.insert(
            "al:1".to_string(),
            base_entry(
                ShowRecord {
                    watermark: stamp(12.0, 10, "phone"),
                    ..Default::default()
                },
                state(false, false, None, 8.0), // observed was 8 (could not apply 12)
            ),
        );

        let mut remote = SyncDoc::default();
        remote.shows.insert(
            "al:1".to_string(),
            ShowRecord {
                watermark: stamp(12.0, 10, "phone"),
                ..Default::default()
            },
        );

        let input = MergeInput {
            local: &local,
            base: &base,
            remote: &remote,
            device: "pc",
            now: 100,
        };
        let out = merge(&input);

        assert_eq!(out.local_changes, 0);
        assert_eq!(out.remote_changes, 1);
        assert_eq!(out.doc.shows["al:1"].watermark, stamp(12.0, 10, "phone"));
        assert_eq!(
            out.apply,
            vec![ApplyOp {
                key: "al:1".to_string(),
                anilist_id: None,
                title: String::new(),
                change: FieldChange::Watermark(12.0),
            }]
        );
    }

    #[test]
    fn conflict_watermark_larger_value_wins() {
        // Part 1: remote wins (9 > 7)
        {
            let mut local = BTreeMap::new();
            local.insert("al:1".to_string(), state(false, false, None, 7.0));

            let mut base = BTreeMap::new();
            base.insert(
                "al:1".to_string(),
                base_entry(
                    ShowRecord {
                        watermark: stamp(5.0, 10, "base"),
                        ..Default::default()
                    },
                    state(false, false, None, 5.0),
                ),
            );

            let mut remote = SyncDoc::default();
            remote.shows.insert(
                "al:1".to_string(),
                ShowRecord {
                    watermark: stamp(9.0, 20, "phone"),
                    ..Default::default()
                },
            );

            let input = MergeInput {
                local: &local,
                base: &base,
                remote: &remote,
                device: "pc",
                now: 100,
            };
            let out = merge(&input);

            assert_eq!(out.local_changes, 0);
            assert_eq!(out.remote_changes, 1);
            assert_eq!(out.doc.shows["al:1"].watermark, stamp(9.0, 20, "phone"));
            assert_eq!(
                out.apply,
                vec![ApplyOp {
                    key: "al:1".to_string(),
                    anilist_id: None,
                    title: String::new(),
                    change: FieldChange::Watermark(9.0),
                }]
            );
        }

        // Part 2: local wins (11 > 9)
        {
            let mut local = BTreeMap::new();
            local.insert("al:1".to_string(), state(false, false, None, 11.0));

            let mut base = BTreeMap::new();
            base.insert(
                "al:1".to_string(),
                base_entry(
                    ShowRecord {
                        watermark: stamp(5.0, 10, "base"),
                        ..Default::default()
                    },
                    state(false, false, None, 5.0),
                ),
            );

            let mut remote = SyncDoc::default();
            remote.shows.insert(
                "al:1".to_string(),
                ShowRecord {
                    watermark: stamp(9.0, 20, "phone"),
                    ..Default::default()
                },
            );

            let input = MergeInput {
                local: &local,
                base: &base,
                remote: &remote,
                device: "pc",
                now: 100,
            };
            let out = merge(&input);

            assert_eq!(out.local_changes, 1);
            assert_eq!(out.remote_changes, 0);
            assert_eq!(out.doc.shows["al:1"].watermark, stamp(11.0, 100, "pc"));
            assert!(out.apply.is_empty());
        }
    }

    #[test]
    fn conflict_followed_true_wins() {
        // local true vs remote false: local wins
        {
            let mut local = BTreeMap::new();
            local.insert("al:1".to_string(), state(true, false, None, 0.0));

            let mut base = BTreeMap::new();
            base.insert(
                "al:1".to_string(),
                base_entry(
                    ShowRecord {
                        followed: stamp(false, 10, "base"),
                        ..Default::default()
                    },
                    state(false, false, None, 0.0),
                ),
            );

            let mut remote = SyncDoc::default();
            remote.shows.insert(
                "al:1".to_string(),
                ShowRecord {
                    followed: stamp(false, 20, "phone"),
                    ..Default::default()
                },
            );

            let input = MergeInput {
                local: &local,
                base: &base,
                remote: &remote,
                device: "pc",
                now: 100,
            };
            let out = merge(&input);

            assert_eq!(out.local_changes, 1);
            assert_eq!(out.remote_changes, 0);
            assert_eq!(out.doc.shows["al:1"].followed, stamp(true, 100, "pc"));
            assert!(out.apply.is_empty());
        }

        // local false vs remote true: remote wins
        {
            let mut local = BTreeMap::new();
            local.insert("al:1".to_string(), state(false, false, None, 0.0));

            let mut base = BTreeMap::new();
            base.insert(
                "al:1".to_string(),
                base_entry(
                    ShowRecord {
                        followed: stamp(true, 10, "base"),
                        ..Default::default()
                    },
                    state(true, false, None, 0.0),
                ),
            );

            let mut remote = SyncDoc::default();
            remote.shows.insert(
                "al:1".to_string(),
                ShowRecord {
                    followed: stamp(true, 20, "phone"),
                    ..Default::default()
                },
            );

            let input = MergeInput {
                local: &local,
                base: &base,
                remote: &remote,
                device: "pc",
                now: 100,
            };
            let out = merge(&input);

            assert_eq!(out.local_changes, 0);
            assert_eq!(out.remote_changes, 1);
            assert_eq!(out.doc.shows["al:1"].followed, stamp(true, 20, "phone"));
            assert_eq!(
                out.apply,
                vec![ApplyOp {
                    key: "al:1".to_string(),
                    anilist_id: None,
                    title: String::new(),
                    change: FieldChange::Followed(true),
                }]
            );
        }
    }

    #[test]
    fn conflict_backlog_precedence() {
        let mut local = BTreeMap::new();
        local.insert(
            "al:1".to_string(),
            state(false, false, Some("discarded"), 0.0),
        );

        let mut base = BTreeMap::new();
        base.insert(
            "al:1".to_string(),
            base_entry(
                ShowRecord {
                    backlog: stamp(None, 10, "base"),
                    ..Default::default()
                },
                state(false, false, None, 0.0),
            ),
        );

        let mut remote = SyncDoc::default();
        remote.shows.insert(
            "al:1".to_string(),
            ShowRecord {
                backlog: stamp(Some("want".to_string()), 20, "phone"),
                ..Default::default()
            },
        );

        let input = MergeInput {
            local: &local,
            base: &base,
            remote: &remote,
            device: "pc",
            now: 100,
        };
        let out = merge(&input);

        // want > discarded, remote wins
        assert_eq!(out.local_changes, 0);
        assert_eq!(out.remote_changes, 1);
        assert_eq!(
            out.doc.shows["al:1"].backlog,
            stamp(Some("want".to_string()), 20, "phone")
        );
        assert_eq!(
            out.apply,
            vec![ApplyOp {
                key: "al:1".to_string(),
                anilist_id: None,
                title: String::new(),
                change: FieldChange::Backlog(Some("want".to_string())),
            }]
        );
    }

    #[test]
    fn orthogonal_edits() {
        let mut local = BTreeMap::new();
        local.insert("al:1".to_string(), state(true, false, None, 5.0));

        let mut base = BTreeMap::new();
        base.insert(
            "al:1".to_string(),
            base_entry(
                ShowRecord {
                    followed: stamp(false, 10, "base"),
                    watermark: stamp(5.0, 10, "base"),
                    ..Default::default()
                },
                state(false, false, None, 5.0),
            ),
        );

        let mut remote = SyncDoc::default();
        remote.shows.insert(
            "al:1".to_string(),
            ShowRecord {
                followed: stamp(false, 10, "base"),
                watermark: stamp(8.0, 20, "phone"),
                ..Default::default()
            },
        );

        let input = MergeInput {
            local: &local,
            base: &base,
            remote: &remote,
            device: "pc",
            now: 100,
        };
        let out = merge(&input);

        assert_eq!(out.local_changes, 1);
        assert_eq!(out.remote_changes, 1);
        assert_eq!(out.doc.shows["al:1"].followed, stamp(true, 100, "pc"));
        assert_eq!(out.doc.shows["al:1"].watermark, stamp(8.0, 20, "phone"));
        assert_eq!(
            out.apply,
            vec![ApplyOp {
                key: "al:1".to_string(),
                anilist_id: None,
                title: String::new(),
                change: FieldChange::Watermark(8.0),
            }]
        );
    }

    #[test]
    fn lost_remote_keeps_base_stamps_and_no_unwanted_ops() {
        let mut local = BTreeMap::new();
        local.insert("al:1".to_string(), state(true, false, None, 10.0));

        let mut base = BTreeMap::new();
        base.insert(
            "al:1".to_string(),
            base_entry(
                ShowRecord {
                    followed: stamp(true, 10, "pc"),
                    watermark: stamp(10.0, 10, "pc"),
                    ..Default::default()
                },
                state(true, false, None, 10.0),
            ),
        );

        let remote = SyncDoc::default(); // empty remote

        let input = MergeInput {
            local: &local,
            base: &base,
            remote: &remote,
            device: "pc",
            now: 100,
        };
        let out = merge(&input);

        assert_eq!(out.local_changes, 0);
        assert_eq!(out.remote_changes, 0);
        assert!(out.apply.is_empty());
        assert_eq!(out.doc.shows["al:1"].followed, stamp(true, 10, "pc"));
        assert_eq!(out.doc.shows["al:1"].watermark, stamp(10.0, 10, "pc"));
    }

    #[test]
    fn key_not_in_local_but_in_base_remote_changed_to_false_no_op() {
        let local = BTreeMap::new();

        let mut base = BTreeMap::new();
        base.insert(
            "al:1".to_string(),
            base_entry(
                ShowRecord {
                    followed: stamp(true, 10, "pc"),
                    ..Default::default()
                },
                state(true, false, None, 0.0),
            ),
        );

        let mut remote = SyncDoc::default();
        remote.shows.insert(
            "al:1".to_string(),
            ShowRecord {
                followed: stamp(false, 20, "phone"),
                ..Default::default()
            },
        );

        let input = MergeInput {
            local: &local,
            base: &base,
            remote: &remote,
            device: "pc",
            now: 100,
        };
        let out = merge(&input);

        assert_eq!(out.local_changes, 0);
        assert_eq!(out.remote_changes, 0);
        assert!(out.apply.is_empty());
        assert_eq!(out.doc.shows["al:1"].followed, stamp(false, 20, "phone"));
    }

    #[test]
    fn determinism_same_input_twice_equal_output() {
        let mut local = BTreeMap::new();
        local.insert("al:1".to_string(), state(true, false, Some("want"), 5.0));
        local.insert("al:2".to_string(), state(false, true, None, 12.0));

        let mut base = BTreeMap::new();
        base.insert(
            "al:1".to_string(),
            base_entry(
                ShowRecord {
                    followed: stamp(false, 10, "pc"),
                    watermark: stamp(3.0, 10, "pc"),
                    ..Default::default()
                },
                state(false, false, None, 3.0),
            ),
        );

        let mut remote = SyncDoc::default();
        remote.shows.insert(
            "al:2".to_string(),
            ShowRecord {
                watched: stamp(false, 20, "phone"),
                watermark: stamp(15.0, 20, "phone"),
                ..Default::default()
            },
        );
        remote.shows.insert(
            "al:3".to_string(),
            ShowRecord {
                followed: stamp(true, 25, "phone"),
                ..Default::default()
            },
        );

        let input = MergeInput {
            local: &local,
            base: &base,
            remote: &remote,
            device: "pc",
            now: 100,
        };

        let out1 = merge(&input);
        let out2 = merge(&input);

        assert_eq!(out1, out2);
    }

    #[test]
    fn must_not_happen_never_apply_op_non_engaged_absent_key() {
        let local = BTreeMap::new();
        let base = BTreeMap::new();
        let mut remote = SyncDoc::default();
        remote.shows.insert(
            "al:99".to_string(),
            ShowRecord {
                followed: stamp(false, 20, "phone"),
                watched: stamp(false, 20, "phone"),
                backlog: stamp(None, 20, "phone"),
                watermark: stamp(0.0, 20, "phone"),
                ..Default::default()
            },
        );

        let input = MergeInput {
            local: &local,
            base: &base,
            remote: &remote,
            device: "pc",
            now: 100,
        };
        let out = merge(&input);

        assert!(out.apply.is_empty());
        assert_eq!(out.remote_changes, 0);
    }

    #[test]
    fn must_not_happen_never_doc_record_with_all_four_fields_none() {
        let mut local = BTreeMap::new();
        // local state matching default unengaged observed state
        local.insert("al:1".to_string(), state(false, false, None, 0.0));
        let base = BTreeMap::new();
        let remote = SyncDoc::default();

        let input = MergeInput {
            local: &local,
            base: &base,
            remote: &remote,
            device: "pc",
            now: 100,
        };
        let out = merge(&input);

        assert!(out.doc.shows.is_empty());
    }

    #[test]
    fn remote_file_rolled_back_republishes_base_and_keeps_local() {
        let mut local = BTreeMap::new();
        local.insert("al:1".to_string(), state(true, false, None, 10.0));

        let mut base = BTreeMap::new();
        base.insert(
            "al:1".to_string(),
            base_entry(
                ShowRecord {
                    followed: stamp(true, 50, "pc"),
                    watermark: stamp(10.0, 50, "pc"),
                    ..Default::default()
                },
                state(true, false, None, 10.0),
            ),
        );

        // Remote was rolled back to an older file: older timestamps, unengaged values
        let mut remote = SyncDoc::default();
        remote.shows.insert(
            "al:1".to_string(),
            ShowRecord {
                followed: stamp(false, 20, "phone"),
                watermark: stamp(4.0, 20, "phone"),
                ..Default::default()
            },
        );

        let input = MergeInput {
            local: &local,
            base: &base,
            remote: &remote,
            device: "pc",
            now: 100,
        };
        let out = merge(&input);

        assert_eq!(out.local_changes, 0);
        assert_eq!(out.remote_changes, 0);
        assert!(out.apply.is_empty(), "Stale remote must never be applied locally");
        assert_eq!(out.doc.shows["al:1"].followed, stamp(true, 50, "pc"), "Doc must republish base follow");
        assert_eq!(out.doc.shows["al:1"].watermark, stamp(10.0, 50, "pc"), "Doc must republish base watermark");
    }

    #[test]
    fn concurrent_writes_two_devices_converge_no_flip_flop() {
        // Initial state at t=10: both devices have watermark 5.0
        let base_rec = ShowRecord {
            watermark: stamp(5.0, 10, "dev_a"),
            ..Default::default()
        };
        let base_st = state(false, false, None, 5.0);

        // Device A advances watermark to 8.0 at now=100
        let mut local_a = BTreeMap::new();
        local_a.insert("al:1".to_string(), state(false, false, None, 8.0));
        let mut base_a = BTreeMap::new();
        base_a.insert("al:1".to_string(), base_entry(base_rec.clone(), base_st.clone()));

        // Device B advances watermark to 10.0 at now=100
        let mut local_b = BTreeMap::new();
        local_b.insert("al:1".to_string(), state(false, false, None, 10.0));
        let mut base_b = BTreeMap::new();
        base_b.insert("al:1".to_string(), base_entry(base_rec.clone(), base_st.clone()));

        let initial_remote = SyncDoc {
            schema: SYNC_SCHEMA_VERSION,
            updated_at: 10,
            updated_by: "dev_a".into(),
            shows: BTreeMap::from([("al:1".to_string(), base_rec)]),
        };

        // Device A syncs first
        let out_a1 = merge(&MergeInput {
            local: &local_a,
            base: &base_a,
            remote: &initial_remote,
            device: "dev_a",
            now: 100,
        });
        assert_eq!(out_a1.doc.shows["al:1"].watermark, stamp(8.0, 100, "dev_a"));

        // Device B syncs (concurrently, against the same initial_remote)
        let out_b1 = merge(&MergeInput {
            local: &local_b,
            base: &base_b,
            remote: &initial_remote,
            device: "dev_b",
            now: 100,
        });
        assert_eq!(out_b1.doc.shows["al:1"].watermark, stamp(10.0, 100, "dev_b"));

        // Simulate Order 1: B uploaded first (out_b1.doc is remote). A syncs next against B's doc.
        {
            let mut base_a_after = BTreeMap::new();
            base_a_after.insert("al:1".to_string(), base_entry(out_a1.doc.shows["al:1"].clone(), state(false, false, None, 8.0)));
            let out_a2 = merge(&MergeInput {
                local: &local_a,
                base: &base_a_after,
                remote: &out_b1.doc,
                device: "dev_a",
                now: 110,
            });
            // Watermark 10.0 > 8.0, remote wins on A
            assert_eq!(out_a2.apply, vec![ApplyOp {
                key: "al:1".to_string(),
                anilist_id: None,
                title: String::new(),
                change: FieldChange::Watermark(10.0),
            }]);
            assert_eq!(out_a2.doc.shows["al:1"].watermark, stamp(10.0, 100, "dev_b"));

            // Device A applies 10.0 and saves base
            let mut local_a_converged = BTreeMap::new();
            local_a_converged.insert("al:1".to_string(), state(false, false, None, 10.0));
            let mut base_a_converged = BTreeMap::new();
            base_a_converged.insert("al:1".to_string(), base_entry(out_a2.doc.shows["al:1"].clone(), state(false, false, None, 10.0)));

            // Next sync on A against the same doc has 0 changes
            let out_a3 = merge(&MergeInput {
                local: &local_a_converged,
                base: &base_a_converged,
                remote: &out_a2.doc,
                device: "dev_a",
                now: 120,
            });
            assert_eq!(out_a3.local_changes, 0);
            assert_eq!(out_a3.remote_changes, 0);
            assert!(out_a3.apply.is_empty());
        }

        // Simulate Order 2: A uploaded first (out_a1.doc is remote). B syncs next against A's doc.
        {
            let mut base_b_after = BTreeMap::new();
            base_b_after.insert("al:1".to_string(), base_entry(out_b1.doc.shows["al:1"].clone(), state(false, false, None, 10.0)));
            let out_b2 = merge(&MergeInput {
                local: &local_b,
                base: &base_b_after,
                remote: &out_a1.doc,
                device: "dev_b",
                now: 110,
            });
            // B has 10.0 vs remote 8.0: B's 10.0 wins (more engaged)
            assert!(out_b2.apply.is_empty());
            assert_eq!(out_b2.doc.shows["al:1"].watermark, stamp(10.0, 100, "dev_b"));

            // Now A syncs against B's updated doc
            let mut base_a_after = BTreeMap::new();
            base_a_after.insert("al:1".to_string(), base_entry(out_a1.doc.shows["al:1"].clone(), state(false, false, None, 8.0)));
            let out_a_final = merge(&MergeInput {
                local: &local_a,
                base: &base_a_after,
                remote: &out_b2.doc,
                device: "dev_a",
                now: 120,
            });
            assert_eq!(out_a_final.apply, vec![ApplyOp {
                key: "al:1".to_string(),
                anilist_id: None,
                title: String::new(),
                change: FieldChange::Watermark(10.0),
            }]);
            assert_eq!(out_a_final.doc.shows["al:1"].watermark, stamp(10.0, 100, "dev_b"));
        }
    }

    #[test]
    fn clock_skew_behind_remote_orders_after_it() {
        let mut local = BTreeMap::new();
        // User locally marks followed at local now = 100
        local.insert("al:1".to_string(), state(true, false, None, 0.0));

        let mut base = BTreeMap::new();
        base.insert(
            "al:1".to_string(),
            base_entry(
                ShowRecord {
                    followed: stamp(false, 500, "phone"),
                    ..Default::default()
                },
                state(false, false, None, 0.0),
            ),
        );

        let mut remote = SyncDoc::default();
        remote.shows.insert(
            "al:1".to_string(),
            ShowRecord {
                followed: stamp(false, 500, "phone"),
                ..Default::default()
            },
        );

        // Local clock is at 100, far behind remote stamp 500
        let input = MergeInput {
            local: &local,
            base: &base,
            remote: &remote,
            device: "pc",
            now: 100,
        };
        let out = merge(&input);

        assert_eq!(out.local_changes, 1);
        let stamped = out.doc.shows["al:1"].followed.as_ref().unwrap();
        // Lamport clock ensures timestamp is at least remote.at + 1 = 501
        assert!(stamped.v);
        assert_eq!(stamped.at, 501);
        assert_eq!(stamped.dev, "pc");
        assert!(stamp_gt(stamped, remote.shows["al:1"].followed.as_ref().unwrap()));
    }

    #[test]
    fn deleted_series_in_base_not_resurrected_when_remote_engaged() {
        // Local does NOT have "al:1" (user deleted it on this device)
        let local = BTreeMap::new();

        // Base has "al:1" from a previous sync
        let mut base = BTreeMap::new();
        base.insert(
            "al:1".to_string(),
            base_entry(
                ShowRecord {
                    followed: stamp(true, 10, "pc"),
                    watermark: stamp(5.0, 10, "pc"),
                    ..Default::default()
                },
                state(true, false, None, 5.0),
            ),
        );

        // Remote has engaged values: followed = true, watermark = 12.0
        let mut remote = SyncDoc::default();
        remote.shows.insert(
            "al:1".to_string(),
            ShowRecord {
                anilist_id: Some(1),
                title: "One".to_string(),
                followed: stamp(true, 20, "phone"),
                watermark: stamp(12.0, 20, "phone"),
                ..Default::default()
            },
        );

        let input = MergeInput {
            local: &local,
            base: &base,
            remote: &remote,
            device: "pc",
            now: 100,
        };
        let out = merge(&input);

        // Must not materialise or apply any op for locally deleted series
        assert_eq!(out.local_changes, 0);
        assert_eq!(out.remote_changes, 0);
        assert!(out.apply.is_empty(), "Must not resurrect locally deleted series");
    }
}
