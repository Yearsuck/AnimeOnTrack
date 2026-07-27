use super::*;
use std::collections::HashMap;

/// One entry in the **site-agnostic library** (see
/// docs/cross-site-library-investigation.md, option C). Identity is canonical —
/// an AniList id when known, a normalized title otherwise — so the same show
/// followed on two different sites is one library entry, and switching the
/// active site never strands it.
///
/// Progress is a single `seen_watermark` (highest episode number marked seen)
/// rather than a per-episode set: watching is gap-free (`set_seen_cascade`), so
/// a watermark captures it losslessly and — unlike per-episode rows, whose URLs
/// and even numbering differ between sites — projects cleanly onto whatever
/// site you're currently on.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct LibraryEntry {
    pub id: i64,
    pub canon_key: String,
    pub anilist_id: Option<i64>,
    pub display_title: String,
    pub followed: bool,
    pub backlog_status: Option<String>,
    pub watched_externally: bool,
    pub seen_watermark: i64,
}

/// The canonical identity key for a series: its AniList id when it has one,
/// else its normalized title. Two `series` rows (on any sites) that share this
/// key are the same show in the library.
pub(crate) fn canon_key(anilist_id: Option<i64>, title: &str) -> String {
    match anilist_id {
        Some(id) => format!("al:{id}"),
        None => format!("t:{}", crate::matching::normalize_title(title)),
    }
}

/// One `series` row's contribution to the canonical library, straight from SQL.
struct SeriesLibRow {
    anilist_id: Option<i64>,
    title: String,
    followed: bool,
    watched_externally: bool,
    backlog_status: Option<String>,
    seen_watermark: i64,
}

/// Precedence for a canonical entry's single backlog status when member rows
/// disagree: an actual follow (or "watched") outranks any backlog tag, and
/// `want` (intends to watch) outranks `discarded` (passed on).
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

impl Db {
    /// Rebuild the canonical `library` table as a projection of the per-site
    /// `series` state. Idempotent (full rebuild), so it can run on every open
    /// while `series` is still the source of truth (phases 1-3); a later phase
    /// flips the write path to make `library` authoritative.
    ///
    /// Only series with a *positive* library signal contribute — followed,
    /// watched externally, or a `want` backlog tag. Discarded swipe rows are
    /// deliberately excluded: they are shows you passed on, not part of your
    /// library, and there are thousands of them (they'd otherwise dwarf the
    /// real entries). Members sharing a `canon_key` union their state: followed /
    /// watched-externally are OR-ed, the watermark is the max across sites, the
    /// backlog status is merged by precedence, and the display title is the
    /// shortest member (ties alphabetically) so a season/arc-qualified name
    /// doesn't win over the base show name.
    pub fn sync_library_from_series(&self) -> Result<()> {
        let mut stmt = self.conn.prepare(
            "SELECT s.anilist_id, s.title, s.followed, s.watched_externally, s.backlog_status,
                    COALESCE(MAX(CASE WHEN e.seen=1 THEN CAST(e.number AS INTEGER) END), 0) AS watermark
             FROM series s
             LEFT JOIN episodes e ON e.series_id = s.id
             WHERE s.followed=1 OR s.watched_externally=1 OR s.backlog_status='want'
             GROUP BY s.id",
        )?;
        let rows: Vec<SeriesLibRow> = stmt
            .query_map([], |r| {
                Ok(SeriesLibRow {
                    anilist_id: r.get(0)?,
                    title: r.get(1)?,
                    followed: r.get::<_, i64>(2)? != 0,
                    watched_externally: r.get::<_, i64>(3)? != 0,
                    backlog_status: r.get(4)?,
                    seen_watermark: r.get(5)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        let mut grouped: HashMap<String, LibraryEntry> = HashMap::new();
        for row in rows {
            let key = canon_key(row.anilist_id, &row.title);
            let entry = grouped.entry(key.clone()).or_insert_with(|| LibraryEntry {
                id: 0,
                canon_key: key,
                anilist_id: row.anilist_id,
                display_title: row.title.clone(),
                followed: false,
                backlog_status: None,
                watched_externally: false,
                seen_watermark: 0,
            });
            entry.followed |= row.followed;
            entry.watched_externally |= row.watched_externally;
            entry.seen_watermark = entry.seen_watermark.max(row.seen_watermark);
            entry.backlog_status = merge_backlog(entry.backlog_status.take(), row.backlog_status);
            entry.anilist_id = entry.anilist_id.or(row.anilist_id);
            let shorter = row.title.chars().count() < entry.display_title.chars().count();
            let tied_earlier = row.title.chars().count() == entry.display_title.chars().count()
                && row.title < entry.display_title;
            if shorter || tied_earlier {
                entry.display_title = row.title;
            }
        }

        let tx = self.conn.unchecked_transaction()?;
        tx.execute("DELETE FROM library", [])?;
        {
            let mut ins = tx.prepare(
                "INSERT INTO library
                   (canon_key, anilist_id, display_title, followed, backlog_status, watched_externally, seen_watermark)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            )?;
            for e in grouped.into_values() {
                // A follow (or watch) clears any leftover backlog tag: you don't
                // "want to watch" what you already follow.
                let backlog = if e.followed || e.watched_externally { None } else { e.backlog_status };
                ins.execute((
                    &e.canon_key, e.anilist_id, &e.display_title,
                    e.followed as i64, &backlog, e.watched_externally as i64, e.seen_watermark,
                ))?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Propagate "completed" (`watched_externally`) status across sites. When a
    /// show is marked "Ya lo vi" on any site, its twin rows on other sites (same
    /// `anilist_id`, else normalized title) are reconciled so it doesn't look
    /// unwatched on a site you merely switched to — per row type:
    ///
    /// - **Finished** twin → set `watched_externally=1` (a flag only, never
    ///   touching episode rows). With `list_pending`'s `watched_externally=0`
    ///   filter it drops out of Pendientes and reads as completed everywhere.
    /// - **Airing** twin with **zero local progress** → mark the episodes aired
    ///   so far seen, so it shows in the airing grid as *caught up* rather than a
    ///   false pending pile. We don't flag an airing row `watched_externally`
    ///   (the airing view hides that); future episodes still arrive unseen.
    /// - **Airing** twin with **real partial/full progress** → left untouched.
    ///   This is the One Piece guard: 1079/1171 with a loose "Ya lo vi" on the
    ///   base entry must never be force-completed — that once wiped 90+ genuine
    ///   pending episodes. Existing per-episode progress is authoritative.
    ///
    /// Safe to run repeatedly: a steady state is a no-op (finished rows are
    /// already flagged; airing rows already have their aired episodes seen).
    /// Returns rows changed.
    pub fn reconcile_completion_across_sites(&self) -> Result<usize> {
        // Canonical keys that are completed on at least one site.
        let mut stmt = self
            .conn
            .prepare("SELECT anilist_id, title FROM series WHERE watched_externally=1")?;
        let completed: std::collections::HashSet<String> = stmt
            .query_map([], |r| {
                let anilist_id: Option<i64> = r.get(0)?;
                let title: String = r.get(1)?;
                Ok(canon_key(anilist_id, &title))
            })?
            .collect::<rusqlite::Result<std::collections::HashSet<_>>>()?;
        if completed.is_empty() {
            return Ok(0);
        }

        // Candidate rows: not yet completed here, but their canonical twin is.
        // Carry each row's airing flag and current seen/total so we can apply
        // the right (safe) action per row below.
        let mut cstmt = self.conn.prepare(
            "SELECT s.id, s.anilist_id, s.title, s.is_airing,
                    COUNT(e.id) AS total,
                    SUM(CASE WHEN e.seen=1 THEN 1 ELSE 0 END) AS seen
             FROM series s LEFT JOIN episodes e ON e.series_id = s.id
             WHERE s.watched_externally=0
             GROUP BY s.id",
        )?;
        struct Cand { id: i64, key: String, is_airing: bool, total: i64, seen: i64 }
        let candidates: Vec<Cand> = cstmt
            .query_map([], |r| {
                Ok(Cand {
                    id: r.get(0)?,
                    key: canon_key(r.get::<_, Option<i64>>(1)?, &r.get::<_, String>(2)?),
                    is_airing: r.get::<_, i64>(3)? != 0,
                    total: r.get::<_, i64>(4)?,
                    seen: r.get::<_, Option<i64>>(5)?.unwrap_or(0),
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?
            .into_iter()
            .filter(|c| completed.contains(&c.key))
            .collect();

        let tx = self.conn.unchecked_transaction()?;
        let mut changed = 0usize;
        for c in &candidates {
            if !c.is_airing {
                // Finished show completed elsewhere → flag it completed. A flag
                // only; episode seen-state is never mutated (non-destructive).
                tx.execute("UPDATE series SET watched_externally=1 WHERE id=?1", [c.id])?;
                changed += 1;
            } else if c.seen == 0 && c.total > 0 {
                // Airing show you watched elsewhere but never here (0 progress):
                // mark the episodes aired so far seen, so it shows in the airing
                // grid as caught-up instead of a false pending pile. We do NOT
                // set watched_externally on an airing row (the airing view hides
                // that), and future episodes arrive unseen as normal.
                tx.execute(
                    "UPDATE episodes SET seen=1, seen_at=COALESCE(seen_at, datetime('now'))
                     WHERE series_id=?1 AND seen=0",
                    [c.id],
                )?;
                changed += 1;
            }
            // Airing with real partial/full progress (e.g. One Piece 1079/1171)
            // is left completely alone — that progress is authoritative.
        }
        tx.commit()?;
        Ok(changed)
    }

    /// Canonical **followed** entries that are NOT yet present on `source_id`
    /// — i.e. the part of your library that switching to this site would
    /// strand. "Present" means the site has a `series` row that either shares
    /// the entry's `anilist_id` or matches its `canon_key` (title). These are
    /// exactly the entries a cross-site import must resolve+link on the target
    /// site. Ordered most-progressed first so a paced import brings back the
    /// shows you're deepest into soonest.
    pub fn library_entries_missing_on_site(&self, source_id: i64) -> Result<Vec<LibraryEntry>> {
        // What the target site already has, as canonical keys.
        let mut stmt = self
            .conn
            .prepare("SELECT anilist_id, title FROM series WHERE source_id=?1")?;
        let present: std::collections::HashSet<String> = stmt
            .query_map([source_id], |r| {
                let anilist_id: Option<i64> = r.get(0)?;
                let title: String = r.get(1)?;
                Ok(canon_key(anilist_id, &title))
            })?
            .collect::<rusqlite::Result<std::collections::HashSet<_>>>()?;

        Ok(self
            .list_library_entries()?
            .into_iter()
            .filter(|e| e.followed && !present.contains(&e.canon_key))
            .collect())
    }

    /// The whole canonical library, most-progressed first — site-independent.
    pub fn list_library_entries(&self) -> Result<Vec<LibraryEntry>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, canon_key, anilist_id, display_title, followed, backlog_status, watched_externally, seen_watermark
             FROM library ORDER BY followed DESC, seen_watermark DESC, display_title",
        )?;
        let rows = stmt
            .query_map([], |r| {
                Ok(LibraryEntry {
                    id: r.get(0)?,
                    canon_key: r.get(1)?,
                    anilist_id: r.get(2)?,
                    display_title: r.get(3)?,
                    followed: r.get::<_, i64>(4)? != 0,
                    backlog_status: r.get(5)?,
                    watched_externally: r.get::<_, i64>(6)? != 0,
                    seen_watermark: r.get(7)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_support::*;

    fn seed_followed(db: &Db, src: i64, slug: &str, title: &str, anilist_id: Option<i64>, seen_up_to: i64, total: i64) -> i64 {
        let sid = db.upsert_series(src, &mk_airing(slug, title, None)).unwrap();
        db.set_followed(sid, true).unwrap();
        if let Some(a) = anilist_id {
            db.set_anilist_id(sid, a).unwrap();
        }
        for n in 1..=total {
            db.insert_episode(&crate::models::Episode {
                id: 0, series_id: sid, number: n.to_string(), title: None,
                url: format!("https://site/{slug}-{n}"), released_at: None, seen: false,
            }).unwrap();
        }
        if seen_up_to > 0 {
            db.set_seen_cascade(sid, &seen_up_to.to_string(), true).unwrap();
        }
        sid
    }

    #[test]
    fn same_show_on_two_sites_is_one_library_entry_with_max_watermark() {
        let db = Db::open(":memory:").unwrap();
        let a = db.upsert_source("AnimeYT", "a", "animeytx").unwrap();
        let b = db.upsert_source("TioAnime", "b", "tioanime").unwrap();
        // Same AniList id on both sites, different progress.
        seed_followed(&db, a, "op", "One Piece", Some(21), 5, 10);
        seed_followed(&db, b, "one-piece", "ONE PIECE", Some(21), 8, 10);
        db.sync_library_from_series().unwrap();

        let lib = db.list_library_entries().unwrap();
        assert_eq!(lib.len(), 1, "one canonical entry across both sites");
        assert_eq!(lib[0].anilist_id, Some(21));
        assert!(lib[0].followed);
        assert_eq!(lib[0].seen_watermark, 8, "max progress across sites");
    }

    #[test]
    fn no_anilist_id_groups_by_normalized_title() {
        let db = Db::open(":memory:").unwrap();
        let a = db.upsert_source("AnimeYT", "a", "animeytx").unwrap();
        let b = db.upsert_source("TioAnime", "b", "tioanime").unwrap();
        seed_followed(&db, a, "x", "Some Show", None, 3, 5);
        seed_followed(&db, b, "x2", "some  show", None, 1, 5);
        db.sync_library_from_series().unwrap();
        let lib = db.list_library_entries().unwrap();
        assert_eq!(lib.len(), 1, "grouped by normalized title");
        assert_eq!(lib[0].seen_watermark, 3);
    }

    #[test]
    fn distinct_shows_stay_distinct_and_untouched_rows_excluded() {
        let db = Db::open(":memory:").unwrap();
        let a = db.upsert_source("AnimeYT", "a", "animeytx").unwrap();
        seed_followed(&db, a, "naruto", "Naruto", Some(20), 2, 5);
        seed_followed(&db, a, "bleach", "Bleach", Some(269), 0, 5);
        // An un-followed, un-touched scraped row must not enter the library.
        db.upsert_series(a, &mk_airing("filler", "Random Airing", None)).unwrap();
        db.sync_library_from_series().unwrap();
        let lib = db.list_library_entries().unwrap();
        assert_eq!(lib.len(), 2);
        assert!(lib.iter().all(|e| e.display_title == "Naruto" || e.display_title == "Bleach"));
    }

    #[test]
    fn rebuild_is_idempotent() {
        let db = Db::open(":memory:").unwrap();
        let a = db.upsert_source("AnimeYT", "a", "animeytx").unwrap();
        seed_followed(&db, a, "op", "One Piece", Some(21), 5, 10);
        db.sync_library_from_series().unwrap();
        db.sync_library_from_series().unwrap();
        assert_eq!(db.list_library_entries().unwrap().len(), 1, "no duplication on re-sync");
    }

    #[test]
    fn missing_on_site_lists_followed_entries_absent_from_that_site() {
        let db = Db::open(":memory:").unwrap();
        let a = db.upsert_source("AnimeYT", "a", "animeytx").unwrap();
        let b = db.upsert_source("TioAnime", "b", "tioanime").unwrap();
        // Followed on AnimeYT: one also present on TioAnime (by anilist id),
        // one only on AnimeYT (would be stranded when switching to TioAnime).
        seed_followed(&db, a, "op", "One Piece", Some(21), 5, 10);
        seed_followed(&db, a, "frieren", "Frieren", Some(154587), 3, 12);
        // TioAnime has One Piece (same anilist id) but not Frieren.
        db.upsert_series(b, &mk_airing("one-piece", "One Piece", None)).unwrap();
        let bop = db.get_source_id_for_site("tioanime").unwrap().unwrap();
        let _ = bop;
        let op_b = db.conn.query_row("SELECT id FROM series WHERE source_id=?1 AND slug='one-piece'", [b], |r| r.get::<_,i64>(0)).unwrap();
        db.set_anilist_id(op_b, 21).unwrap();
        db.sync_library_from_series().unwrap();

        let missing = db.library_entries_missing_on_site(b).unwrap();
        assert_eq!(missing.len(), 1, "only Frieren is missing on TioAnime");
        assert_eq!(missing[0].display_title, "Frieren");
        assert_eq!(missing[0].seen_watermark, 3);
    }

    #[test]
    fn reconcile_flags_completed_twin_and_clears_it_from_pending() {
        let db = Db::open(":memory:").unwrap();
        let a = db.upsert_source("AnimeYT", "a", "animeytx").unwrap();
        let b = db.upsert_source("TioAnime", "b", "tioanime").unwrap();
        // Completed on AnimeYT via "Ya lo vi" (no episodes), followed on
        // TioAnime with a fresh, all-unseen episode list — the flood case.
        let ya = db.upsert_series(a, &mk_airing("op", "Some Finished Show", None)).unwrap();
        db.set_anilist_id(ya, 555).unwrap();
        db.set_watched_externally(ya, true).unwrap();
        let yb = seed_followed(&db, b, "sfs", "Some Finished Show", Some(555), 0, 5);
        // A finished show (only finished rows are reconciled — airing shows the
        // user may be catching up on are protected).
        db.conn.execute("UPDATE series SET is_airing=0 WHERE id=?1", [yb]).unwrap();
        assert_eq!(db.list_pending(b, crate::db::PendingSort::RemainingAsc).unwrap().len(), 5, "starts flooded");

        let fixed = db.reconcile_completion_across_sites().unwrap();
        assert_eq!(fixed, 1);
        // Non-destructive: the flag is set, but episode seen-state is untouched…
        let eps = db.list_series_episodes(yb).unwrap();
        assert!(eps.iter().all(|e| !e.seen), "episodes NOT mutated (no data loss)");
        // …and the pending filter keeps the completed show out of the queue.
        assert!(db.list_pending(b, crate::db::PendingSort::RemainingAsc).unwrap().is_empty(), "no longer pending");
        // Idempotent: a second run changes nothing.
        assert_eq!(db.reconcile_completion_across_sites().unwrap(), 0);
    }

    #[test]
    fn reconcile_never_completes_an_airing_show_being_caught_up() {
        // The One Piece regression: base entry marked "Ya lo vi" on site A, but
        // actively watched (partial progress) on an AIRING row on site B. That
        // row must keep its unseen tail — completion must not wipe real progress.
        let db = Db::open(":memory:").unwrap();
        let a = db.upsert_source("AnimeYT", "a", "animeytx").unwrap();
        let b = db.upsert_source("TioAnime", "b", "tioanime").unwrap();
        let base = db.upsert_series(a, &mk_airing("op", "One Piece", None)).unwrap();
        db.set_anilist_id(base, 21).unwrap();
        db.set_watched_externally(base, true).unwrap();
        // Airing on B, 3 of 5 watched (mk_airing => is_airing=1).
        let ob = seed_followed(&db, b, "one-piece", "ONE PIECE", Some(21), 3, 5);

        assert_eq!(db.reconcile_completion_across_sites().unwrap(), 0, "airing row not reconciled");
        let seen = db.list_series_episodes(ob).unwrap().iter().filter(|e| e.seen).count();
        assert_eq!(seen, 3, "real progress preserved, 2 still pending");
        assert_eq!(db.list_pending(b, crate::db::PendingSort::RemainingAsc).unwrap().len(), 2);
    }

    #[test]
    fn reconcile_marks_airing_zero_progress_twin_caught_up() {
        // Watched elsewhere ("Ya lo vi"), airing here but never marked: mark the
        // aired episodes seen so it shows caught-up in the airing grid, and does
        // NOT get the watched_externally flag (which the airing view hides).
        let db = Db::open(":memory:").unwrap();
        let a = db.upsert_source("AnimeYT", "a", "animeytx").unwrap();
        let b = db.upsert_source("TioAnime", "b", "tioanime").unwrap();
        let base = db.upsert_series(a, &mk_airing("tanya", "Saga of Tanya S2", None)).unwrap();
        db.set_anilist_id(base, 135865).unwrap();
        db.set_watched_externally(base, true).unwrap();
        // Airing on B (mk_airing => is_airing=1), 0 of 3 seen.
        let ob = seed_followed(&db, b, "youjo-senki-ii", "Youjo Senki II", Some(135865), 0, 3);

        assert_eq!(db.reconcile_completion_across_sites().unwrap(), 1);
        let eps = db.list_series_episodes(ob).unwrap();
        assert!(eps.iter().all(|e| e.seen), "aired episodes marked caught-up");
        // Stays visible in the airing grid (not flagged watched_externally).
        let airing = db.list_airing(b).unwrap();
        assert!(airing.iter().any(|s| s.id == ob), "still in the airing grid");
        assert!(db.list_pending(b, crate::db::PendingSort::RemainingAsc).unwrap().is_empty());
    }

    #[test]
    fn reconcile_leaves_unrelated_shows_untouched() {
        let db = Db::open(":memory:").unwrap();
        let a = db.upsert_source("AnimeYT", "a", "animeytx").unwrap();
        // A followed, mid-watch show with no completed twin anywhere must keep
        // its unseen episodes (not everything gets marked seen).
        let sid = seed_followed(&db, a, "op", "One Piece", Some(21), 3, 10);
        assert_eq!(db.reconcile_completion_across_sites().unwrap(), 0);
        let eps = db.list_series_episodes(sid).unwrap();
        assert_eq!(eps.iter().filter(|e| e.seen).count(), 3, "watermark untouched");
    }

    #[test]
    fn follow_clears_backlog_tag_across_sites() {
        let db = Db::open(":memory:").unwrap();
        let a = db.upsert_source("AnimeYT", "a", "animeytx").unwrap();
        let b = db.upsert_source("TioAnime", "b", "tioanime").unwrap();
        // Followed on A, but only 'want' on B — the canonical entry is followed,
        // with no leftover backlog tag.
        seed_followed(&db, a, "op", "One Piece", Some(21), 3, 10);
        let bsid = db.upsert_series(b, &mk_airing("one-piece", "One Piece", None)).unwrap();
        db.set_anilist_id(bsid, 21).unwrap();
        db.set_backlog_status(bsid, Some("want")).unwrap();
        db.sync_library_from_series().unwrap();
        let lib = db.list_library_entries().unwrap();
        assert_eq!(lib.len(), 1);
        assert!(lib[0].followed);
        assert_eq!(lib[0].backlog_status, None, "a follow outranks a 'want' tag");
    }
}
