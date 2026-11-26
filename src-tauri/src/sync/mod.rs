//! Device sync: merge the user's decisions (follows, seen progress, "Ya lo vi",
//! backlog tags) between devices through a small shared JSON document.
//! See `docs/superpowers/plans/2026-11-25-android-sync.md`.
pub mod doc;
pub mod drive_remote;
pub mod engine;
pub mod merge;
pub mod remote;
