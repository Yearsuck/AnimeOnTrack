//! Device sync: merge the user's decisions (follows, seen progress, "Ya lo vi",
//! backlog tags) between devices through a small shared JSON document.
//! See `docs/superpowers/plans/2026-11-25-android-sync.md`.
// Nothing calls into this module until the sync command lands; drop this then.
#![allow(dead_code)]
pub mod doc;
