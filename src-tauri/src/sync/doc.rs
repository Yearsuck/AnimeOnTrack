//! The device-sync document: what one device publishes about the user's own
//! decisions, keyed by the site-agnostic canonical key of the `library` table
//! (`al:<anilist id>`, else `t:<normalized title>`), so row ids that differ per
//! device never cross the wire. Only user decisions travel: follow, "Ya lo vi"
//! (`watched_externally`), the backlog tag and the seen watermark. No secrets,
//! no paths, no scraped or catalog data.
//!
//! Every field is a [`Stamp`]: the value plus when and on which device it was
//! last changed. A field that was never set by any device is `None`, which is
//! different from a stamp holding `false`/`0`/`null` (an explicit unfollow,
//! un-mark or cleared tag). A remote can therefore never "delete" a decision:
//! deleting is itself a stamped value.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// File name of the document, in Drive's hidden `appDataFolder` or in a local folder.
pub const SYNC_FILE_NAME: &str = "animeontrack-sync.json";

/// Highest schema this build understands. A document with a newer `schema` is
/// refused (never merged, never overwritten): an older build would silently drop
/// the fields it does not know when it writes the merged document back.
pub const SYNC_SCHEMA_VERSION: u32 = 1;

/// A value plus when (unix seconds) and on which device it was last changed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Stamp<T> {
    pub v: T,
    pub at: i64,
    pub dev: String,
}

/// One show in the document.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct ShowRecord {
    #[serde(default)]
    pub anilist_id: Option<i64>,
    #[serde(default)]
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub followed: Option<Stamp<bool>>,
    /// `series.watched_externally` ("Ya lo vi").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub watched: Option<Stamp<bool>>,
    /// `series.backlog_status`: `Some("want")`, `Some("discarded")` or `None` (cleared).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backlog: Option<Stamp<Option<String>>>,
    /// Highest canonical episode number seen (`library.seen_watermark`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub watermark: Option<Stamp<f64>>,
}

impl ShowRecord {
    /// True when no field carries a stamp (such a record is not written out).
    pub fn is_empty(&self) -> bool {
        self.followed.is_none() && self.watched.is_none() && self.backlog.is_none() && self.watermark.is_none()
    }
}

/// The whole document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SyncDoc {
    pub schema: u32,
    /// Unix seconds of the last write, and the device that made it (informational).
    #[serde(default)]
    pub updated_at: i64,
    #[serde(default)]
    pub updated_by: String,
    #[serde(default)]
    pub shows: BTreeMap<String, ShowRecord>,
}

impl Default for SyncDoc {
    fn default() -> Self {
        SyncDoc { schema: SYNC_SCHEMA_VERSION, updated_at: 0, updated_by: String::new(), shows: BTreeMap::new() }
    }
}

/// This device's current state for one show, read from the local database
/// (the `library` projection plus discarded backlog rows).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct ShowState {
    #[serde(default)]
    pub anilist_id: Option<i64>,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub followed: bool,
    #[serde(default)]
    pub watched: bool,
    #[serde(default)]
    pub backlog: Option<String>,
    #[serde(default)]
    pub watermark: f64,
}

/// What this device remembers about one key from its last successful sync:
/// the record as it was published (`merged`) and this device's own state right
/// after applying it (`observed`). A local change is "current state differs from
/// `observed`" — not "differs from `merged`" — so a remote value this device
/// could not apply yet (e.g. a watermark beyond the episodes scraped here) is
/// never mistaken for a local un-mark.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct BaseEntry {
    pub merged: ShowRecord,
    pub observed: ShowState,
}

/// One change the merge asks the local database to make.
#[derive(Debug, Clone, PartialEq)]
pub struct ApplyOp {
    pub key: String,
    pub anilist_id: Option<i64>,
    pub title: String,
    pub change: FieldChange,
}

#[derive(Debug, Clone, PartialEq)]
pub enum FieldChange {
    Followed(bool),
    Watched(bool),
    Backlog(Option<String>),
    Watermark(f64),
}

#[derive(Debug, Clone, PartialEq)]
pub enum SyncError {
    /// The remote document was written by a newer build.
    UnsupportedSchema(u32),
    /// The remote bytes are not a valid document (truncated, not JSON...).
    Corrupt(String),
}

impl std::fmt::Display for SyncError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SyncError::UnsupportedSchema(v) => write!(
                f,
                "sync document uses schema {v}, this version understands up to {SYNC_SCHEMA_VERSION}: update AnimeOnTrack"
            ),
            SyncError::Corrupt(e) => write!(f, "sync document is corrupt: {e}"),
        }
    }
}

/// Parse remote bytes. Empty (or whitespace-only) bytes are an empty document
/// (first sync); anything else must be a valid document with a supported schema.
pub fn parse_doc(bytes: &[u8]) -> Result<SyncDoc, SyncError> {
    if bytes.iter().all(|b| b.is_ascii_whitespace()) {
        return Ok(SyncDoc::default());
    }
    // Read the schema first so a newer document is reported as such, not as corrupt.
    #[derive(Deserialize)]
    struct Head {
        schema: u32,
    }
    let head: Head = serde_json::from_slice(bytes).map_err(|e| SyncError::Corrupt(e.to_string()))?;
    if head.schema > SYNC_SCHEMA_VERSION {
        return Err(SyncError::UnsupportedSchema(head.schema));
    }
    serde_json::from_slice(bytes).map_err(|e| SyncError::Corrupt(e.to_string()))
}

/// Serialize for upload (compact; records with no stamps are dropped).
pub fn to_bytes(doc: &SyncDoc) -> Vec<u8> {
    let mut doc = doc.clone();
    doc.shows.retain(|_, r| !r.is_empty());
    serde_json::to_vec(&doc).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stamp<T>(v: T, at: i64) -> Option<Stamp<T>> {
        Some(Stamp { v, at, dev: "pc".into() })
    }

    #[test]
    fn empty_bytes_are_an_empty_document() {
        assert_eq!(parse_doc(b"").unwrap(), SyncDoc::default());
        assert_eq!(parse_doc(b"  \n").unwrap(), SyncDoc::default());
    }

    #[test]
    fn round_trips_and_drops_empty_records() {
        let mut doc = SyncDoc { updated_at: 5, updated_by: "pc".into(), ..Default::default() };
        doc.shows.insert(
            "al:1".into(),
            ShowRecord {
                anilist_id: Some(1),
                title: "One".into(),
                followed: stamp(true, 10),
                backlog: stamp(None, 11),
                watermark: stamp(12.5, 12),
                ..Default::default()
            },
        );
        doc.shows.insert("al:2".into(), ShowRecord { anilist_id: Some(2), ..Default::default() });
        let back = parse_doc(&to_bytes(&doc)).unwrap();
        assert_eq!(back.shows.len(), 1);
        assert_eq!(back.shows["al:1"], doc.shows["al:1"]);
        // A cleared backlog tag is a stamp holding null, not a missing field.
        assert_eq!(back.shows["al:1"].backlog, stamp(None, 11));
    }

    #[test]
    fn newer_schema_is_refused_not_reported_corrupt() {
        let bytes = br#"{"schema":2,"shows":{},"future_field":[1,2]}"#;
        assert_eq!(parse_doc(bytes), Err(SyncError::UnsupportedSchema(2)));
    }

    #[test]
    fn truncated_json_is_corrupt() {
        assert!(matches!(parse_doc(br#"{"schema":1,"shows":{"al:1":"#), Err(SyncError::Corrupt(_))));
        assert!(matches!(parse_doc(b"not json"), Err(SyncError::Corrupt(_))));
    }
}
