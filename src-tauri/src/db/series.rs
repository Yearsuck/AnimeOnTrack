use super::*;

/// Unrelated shows (distinct franchises) sharing one small cover byte for byte means a placeholder.
const SHARED_COVER_MIN_SERIES: usize = 3;

/// The show a title belongs to, for "do these rows share a cover legitimately":
/// arcs ("Big Show: Arc One") and seasons collapse onto the show itself.
fn cover_franchise_key(title: &str) -> String {
    crate::matching::franchise_dedup_key(&crate::db::stats::franchise_base_title(title))
}

#[cfg(test)]
use crate::db::test_support::*;

/// The subset of a `series` row's fields the swipe-history strip needs —
/// see `Db::get_series_for_history` and `commands::list_swipe_history`.
#[derive(Debug, Clone, PartialEq)]
pub struct SwipeHistoryRow {
    pub title: String,
    pub poster_url: Option<String>,
    pub backlog_status: Option<String>,
    pub watched_externally: bool,
    /// The row's `series.url` — lets the frontend clear this card from its
    /// client-side decided-set when it legitimately returns to the deck
    /// (undo / return-to-deck). See the design spec's Fix 1.
    pub url: String,
}

/// The subset of a `series` row's fields `link_catalog_series` needs to
/// decide how (or whether) to link it — see `Db::get_series_for_link` and
/// `commands::link_catalog_series`.
#[derive(Debug, Clone, PartialEq)]
pub struct SeriesForLink {
    pub id: i64,
    pub source_id: i64,
    pub slug: String,
    pub anilist_id: Option<i64>,
    pub followed: bool,
    pub backlog_status: Option<String>,
    pub watched_externally: bool,
}

/// Are two slugs on the same source "clearly related" — one a drifted spelling
/// of the other, rather than two separate listing cards?
///
/// Related means: identical once punctuation/case is normalized, or one is a
/// strict **token**-prefix of the other whose extra tail is not purely a noise
/// token. That last clause is the whole point: "naruto" and "naruto-latino" are
/// a prefix pair, but the only thing distinguishing them is exactly the noise
/// `normalize_title` strips, which is precisely the coincidence that makes the
/// two cards look like one show in the first place. Treating that as "related"
/// would rubber-stamp the very merge this guard exists to block.
fn slugs_are_related(a: &str, b: &str) -> bool {
    let na = crate::matching::normalize_title_strict(a);
    let nb = crate::matching::normalize_title_strict(b);
    if na.is_empty() || nb.is_empty() {
        return false;
    }
    if na == nb {
        return true;
    }
    let (short, long) = if na.len() <= nb.len() { (&na, &nb) } else { (&nb, &na) };
    let Some(tail) = long.strip_prefix(short.as_str()) else { return false };
    // A token boundary, not a mid-word coincidence ("naruto" vs "narutox").
    if !tail.starts_with(' ') {
        return false;
    }
    !crate::matching::is_all_noise(tail)
}

/// May `upsert_series`'s normalized-title fallback fold the incoming card onto
/// an existing row whose title normalizes the same way?
///
/// The fallback exists for one thing: a re-scraped card whose *slug* the site
/// restructured (the "Youjo Senki II" bug — `-2` became `-ii`), which a
/// slug-keyed upsert would otherwise treat as a brand-new series, orphaning all
/// its seen history. Matching on the normalized title alone is too loose for
/// that job, because `matching::normalize_title` deliberately strips noise
/// suffixes ("latino", "sub espanol", "hd", "online"): two genuinely separate
/// cards on the same listing that differ only by such a suffix — "Naruto" and
/// "Naruto Latino" — normalize identically, and the second upsert overwrote the
/// first row's slug/url/cover_url, silently collapsing two shows into one.
///
/// So the fallback fires only when the two cards are related by something the
/// noise list did *not* invent:
/// - their titles are equal under the **strict** normalization (no noise
///   stripping) — pure slug drift, the case this fallback was written for; or
/// - their slugs are clearly related (see `slugs_are_related`) — the title
///   moved, but the URL path says it's the same card.
///
/// Anything else stays two rows. Erring toward two rows is the cheap direction:
/// a spurious extra row is a duplicate card in the grid, while a spurious merge
/// destroys a row's identity and its scraped URL.
fn title_fallback_is_safe(
    new_slug: &str,
    new_title_strict: &str,
    existing_slug: &str,
    existing_title: &str,
) -> bool {
    new_title_strict == crate::matching::normalize_title_strict(existing_title)
        || slugs_are_related(new_slug, existing_slug)
}

impl SeriesForLink {
    /// Idempotent early-out for `commands::link_series_core`: a row is
    /// "already linked" either because it was never a catalog row to begin
    /// with (`anilist_id` NULL — a plain site row, e.g. from the airing
    /// scan) *or* because a previous `link_catalog_series` call already
    /// rewrote it onto a real site slug (`relink_series` deliberately keeps
    /// `anilist_id` set after a successful link — only slug/url/cover/kind
    /// change — so `anilist_id.is_some()` alone is NOT a reliable "still
    /// needs linking" signal). Checking the slug too closes that gap: the
    /// three trigger call sites (Seen swipe, "Empezar a ver", opening
    /// SeriesDetail) can race on the same freshly-linked row, and without
    /// this each would re-search and re-scrape it. Mirrors the frontend's
    /// `isUnlinkedCatalogRow` (`src/lib/catalogLink.ts`) — keep both in sync.
    pub fn already_linked_to_site(&self) -> bool {
        self.anilist_id.is_none() || !self.slug.starts_with("anilist-")
    }
}

/// `(id, title, cover_url, source_id, series_url)` of an airing row whose cover
/// still has to be fetched (`cover_url` is None when the listing had none).
pub type AiringCoverRow = (i64, String, Option<String>, i64, String);

/// What `Db::persist_listing` did, for logging and tests.
#[derive(Debug, Default, PartialEq)]
pub struct ListingOutcome {
    pub upserted: usize,
    pub covers_healed: usize,
    pub placeholders_cleared: usize,
    pub unlisted: usize,
}

impl Db {
    /// Persist a scraped airing listing in ONE transaction: every row lands or
    /// none does (a failure on row N used to leave rows 1..N-1 committed).
    /// Same steps and order `refresh` ran inline: heal dead cached covers
    /// (only for series in THIS listing), mark unlisted shows not airing (only
    /// if `complete`), clear placeholder covers, then upsert every row. The
    /// housekeeping steps stay non-fatal (logged); an upsert error rolls back
    /// everything. Callees that open their own transaction reuse this one.
    pub fn persist_listing(
        &self,
        source_id: i64,
        series: &[crate::models::Series],
        complete: bool,
    ) -> Result<ListingOutcome> {
        let tx = self.conn.unchecked_transaction()?;
        let listed: std::collections::HashSet<&str> = series.iter().map(|s| s.slug.as_str()).collect();
        let mut out = ListingOutcome::default();
        match self.heal_missing_cached_covers(source_id, &listed) {
            Ok(n) => {
                out.covers_healed = n;
                if n > 0 {
                    eprintln!("[cover] cleared {n} cached cover(s) whose file is missing");
                }
            }
            Err(e) => eprintln!("[cover] heal_missing_cached_covers failed: {e}"),
        }
        if complete {
            match self.mark_unlisted_not_airing(source_id, &listed) {
                Ok(n) => {
                    out.unlisted = n;
                    if n > 0 {
                        eprintln!("[scan] {n} unlinked show(s) no longer listed by the site: not airing");
                    }
                }
                Err(e) => eprintln!("[scan] mark_unlisted_not_airing failed: {e}"),
            }
        }
        match self.clear_placeholder_covers() {
            Ok(n) => {
                out.placeholders_cleared = n;
                if n > 0 {
                    eprintln!("[cover] cleared {n} placeholder data uri(s) across all sites");
                }
            }
            Err(e) => eprintln!("[cover] clear_placeholder_covers failed: {e}"),
        }
        for s in series {
            self.upsert_series(source_id, s)?;
            out.upserted += 1;
        }
        tx.commit()?;
        Ok(out)
    }

    /// `next_episode_at`/`site_episode_count` are written here just like the
    /// rest of the scanned fields (unlike `followed`, which is deliberately
    /// excluded from the UPDATE so re-scanning never un-follows anything) —
    /// see the scraper-performance design doc. Every real caller besides the
    /// airing scan (`decide_swipe`, `decide_catalog_card`) always passes
    /// `None` for both, and never collides with an already-scanned airing
    /// row's `(source_id, slug)` in practice (the swipe/catalog decks only
    /// ever offer titles not already known — see `known_series_urls`), so
    /// this can't silently clobber real scanned data in the normal flow.
    pub fn upsert_series(&self, source_id: i64, s: &crate::models::Series) -> Result<i64> {
        // A slug is not a stable identity: the scraped site sometimes
        // restructures a series' URL path (seen on sequels/"2nd Season"
        // titles in particular). Matching on (source_id, slug) alone then
        // treats that as a brand new series — a fresh row with no seen
        // history, while the real history sits orphaned under the old slug.
        // This is the same class of bug already fixed for episodes (see
        // `existing_episode_numbers`'s doc comment): identity drifts, so
        // fall back to the title's canonical normalized form (the same one
        // `canon_key` uses) to find the row this really is before deciding
        // it's new.
        let existing_id: Option<i64> = self
            .conn
            .query_row(
                "SELECT id FROM series WHERE source_id=?1 AND slug=?2",
                (source_id, &s.slug),
                |r| r.get(0),
            )
            .optional()?;

        // Synthetic AniList-placeholder rows (slug `anilist-N`) are
        // deliberately kept as their own row even when they share a title
        // with a real scraped row — `merge_series_into` / `relink_series` /
        // `find_series_id_by_slug` reconcile the two on purpose, later, once
        // linking actually happens. Folding them together here on title
        // match alone would short-circuit that whole mechanism, so the
        // title fallback below only ever applies among "real" (non-
        // `anilist-`) rows, and only when the incoming row is itself real.
        let is_synthetic = s.slug.starts_with("anilist-");

        let by_title = if existing_id.is_none() && !is_synthetic {
            // Scan this source's other non-synthetic rows and compare
            // normalized titles in Rust (SQLite has no access to
            // `normalize_title`'s Unicode-aware folding). A normalized-title
            // hit is necessary but NOT sufficient — see
            // `title_fallback_is_safe` for why the slug/strict-title guard
            // has to run before two rows are folded into one.
            let norm = crate::matching::normalize_title(&s.title);
            let strict = crate::matching::normalize_title_strict(&s.title);
            let mut stmt = self.conn.prepare(
                "SELECT id, slug, title FROM series WHERE source_id=?1 AND slug NOT LIKE 'anilist-%'",
            )?;
            let mut rows = stmt.query([source_id])?;
            let mut found = None;
            while let Some(row) = rows.next()? {
                let id: i64 = row.get(0)?;
                let existing_slug: String = row.get(1)?;
                let title: String = row.get(2)?;
                if crate::matching::normalize_title(&title) == norm
                    && title_fallback_is_safe(&s.slug, &strict, &existing_slug, &title)
                {
                    found = Some(id);
                    break;
                }
            }
            found
        } else {
            None
        };

        let target_id = match existing_id.or(by_title) {
            Some(id) => id,
            None => {
                self.conn.execute(
                    "INSERT INTO series(source_id, slug, title, url, cover_url, is_airing, next_episode_at, site_episode_count, last_listed_at)
                     VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, CASE WHEN ?6 != 0 THEN ?9 END)",
                    (
                        source_id, &s.slug, &s.title, &s.url,
                        &s.cover_url, s.is_airing as i64,
                        s.next_episode_at, s.site_episode_count,
                        chrono::Utc::now().timestamp(),
                    ),
                )?;
                return Ok(self.conn.last_insert_rowid());
            }
        };

        // `cover_url` is the one scanned field this UPDATE must NOT overwrite
        // unconditionally. A followed series' cover is downloaded once, by
        // `refresh()`'s cover step, and stored as a `data:` URI (see
        // CLAUDE.md — covers are fetched one at a time through a WebView2
        // window precisely because bulk-fetching them reads as scraping abuse
        // to Cloudflare, and the frontend's CSP only renders `data:`/`asset:`
        // images anyway). `refresh()` upserts every airing-listing card
        // *before* it reads `list_followed`, so overwriting here put the
        // remote thumbnail URL back on every followed airing series on every
        // single cycle — the cover step's `!remote.starts_with("data:")`
        // guard then saw a remote URL again and re-opened the fetch window
        // for that series, forever, flatly contradicting its own "drops out
        // of it for good the first time this succeeds" comment.
        //
        // So: an already-stored `data:` URI (or a `file:` path into the
        // on-disk cover cache, see `cover_cache`) wins over an incoming
        // non-local value (a remote thumbnail, or NULL from a caller that
        // has no cover at all). An incoming `data:`/`file:` value still
        // overwrites — that's a freshly fetched image, strictly newer than
        // what's stored. A `file:` whose file has gone missing is repaired
        // separately by `heal_missing_cached_covers`. The
        // dedicated `update_series_cover` remains the way to set one.
        self.conn.execute(
            "UPDATE series SET slug=?2, title=?3, url=?4,
                cover_url = CASE
                    WHEN substr(COALESCE(cover_url, ''), 1, 5) IN ('data:', 'file:')
                     AND substr(COALESCE(?5, ''), 1, 5) NOT IN ('data:', 'file:')
                    THEN cover_url ELSE ?5 END,
                is_airing=?6, next_episode_at=?7, site_episode_count=?8,
                last_listed_at = CASE WHEN ?6 != 0 THEN ?9 ELSE last_listed_at END
             WHERE id=?1",
            (
                target_id, &s.slug, &s.title, &s.url,
                &s.cover_url, s.is_airing as i64,
                s.next_episode_at, s.site_episode_count,
                chrono::Utc::now().timestamp(),
            ),
        )?;
        Ok(target_id)
    }

    pub fn get_series_url(&self, series_id: i64) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row("SELECT url FROM series WHERE id=?1", [series_id], |r| {
                r.get::<_, String>(0)
            })
            .ok())
    }

    /// The subset of a `series` row's fields `link_catalog_series` needs to
    /// decide how (or whether) to link it — see `commands::link_catalog_series`.
    pub fn get_series_for_link(&self, series_id: i64) -> Result<Option<SeriesForLink>> {
        Ok(self
            .conn
            .query_row(
                "SELECT id, source_id, slug, anilist_id, followed, backlog_status, watched_externally
                 FROM series WHERE id=?1",
                [series_id],
                |r| {
                    Ok(SeriesForLink {
                        id: r.get(0)?,
                        source_id: r.get(1)?,
                        slug: r.get(2)?,
                        anilist_id: r.get(3)?,
                        followed: r.get::<_, i64>(4)? != 0,
                        backlog_status: r.get(5)?,
                        watched_externally: r.get::<_, i64>(6)? != 0,
                    })
                },
            )
            .ok())
    }

    /// Does a *different* `series` row already own `(source_id, slug)`? Used
    /// by `link_catalog_series` to detect that the matched site series is
    /// already tracked (e.g. it's on the airing list) before ever attempting
    /// an insert/update that could violate `UNIQUE(source_id, slug)`.
    pub fn find_series_id_by_slug(&self, source_id: i64, slug: &str, exclude_id: i64) -> Result<Option<i64>> {
        Ok(self
            .conn
            .query_row(
                "SELECT id FROM series WHERE source_id=?1 AND slug=?2 AND id != ?3",
                (source_id, slug, exclude_id),
                |r| r.get(0),
            )
            .ok())
    }

    /// Fold a synthetic catalog-swipe row's decision flags onto an existing
    /// real site row that turns out to share its slug, then delete the
    /// synthetic row. `followed`/`watched_externally` are OR'd (either source
    /// wanting it means the merged row does); `backlog_status` is
    /// most-specific-wins — the existing row's own status survives if it
    /// already has one, otherwise the synthetic row's takes over. Slug/url/
    /// cover/kind/episodes are untouched: the existing row is canonical
    /// (already scraped through the normal path), not the synthetic one.
    /// Wrapped in one transaction so a crash between the flag merge and the
    /// synthetic row's deletion can't leave both rows alive with the same
    /// flags (a visible duplicate until the next merge attempt happens to
    /// fire again).
    ///
    /// The merge also writes a `series_merges` row recording which flags it
    /// actually *flipped* on the survivor, so `undo_swipe_decision` can undo a
    /// swipe whose synthetic row was merged away instead of deleted. Without
    /// it, undoing a "Ya lo vi" that happened to land on an already-tracked
    /// site row was a silent no-op that left a real series permanently marked
    /// watched. A flag the survivor already carried on its own is recorded as
    /// 0 and therefore never cleared by an undo — the undo reverses this
    /// merge's effect, not the user's own earlier decisions.
    pub fn merge_series_into(&self, existing_id: i64, synthetic_id: i64) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        let (followed, backlog_status, watched_externally, title, synthetic_cover, synthetic_anilist_id): (
            i64,
            Option<String>,
            i64,
            String,
            Option<String>,
            Option<i64>,
        ) = tx.query_row(
            "SELECT followed, backlog_status, watched_externally, title, cover_url, anilist_id FROM series WHERE id=?1",
            [synthetic_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
        )?;
        let (survivor_followed, survivor_backlog, survivor_watched): (i64, Option<String>, i64) = tx.query_row(
            "SELECT followed, backlog_status, watched_externally FROM series WHERE id=?1",
            [existing_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?;
        tx.execute(
            "UPDATE series SET
                followed = followed OR ?1,
                watched_externally = watched_externally OR ?2,
                backlog_status = COALESCE(backlog_status, ?3),
                cover_url = CASE
                    WHEN (cover_url IS NULL OR cover_url = '') THEN ?4
                    WHEN cover_url NOT LIKE 'https://%.anilist.co/%' AND ?4 LIKE 'https://%.anilist.co/%' AND cover_url NOT LIKE 'file:%' AND cover_url NOT LIKE 'data:%' THEN ?4
                    ELSE cover_url
                END,
                anilist_id = COALESCE(anilist_id, ?5)
             WHERE id=?6",
            (followed, watched_externally, &backlog_status, &synthetic_cover, synthetic_anilist_id, existing_id),
        )?;
        // What this merge changed, and only that. `OR`/`COALESCE` above mean a
        // field changed exactly when the survivor didn't already have it and
        // the synthetic row did.
        let set_followed = (survivor_followed == 0 && followed != 0) as i64;
        let set_watched = (survivor_watched == 0 && watched_externally != 0) as i64;
        let set_backlog = (survivor_backlog.is_none() && backlog_status.is_some()) as i64;
        // OR REPLACE: SQLite reuses rowids, so a long-lived database can hand
        // out a synthetic id that some far older merge already recorded.
        tx.execute(
            "INSERT OR REPLACE INTO series_merges
                (synthetic_id, survivor_id, title, set_followed, set_watched_externally, set_backlog_status)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6)",
            (synthetic_id, existing_id, &title, set_followed, set_watched, set_backlog),
        )?;
        // Same cleanup as `delete_series`, inlined so it runs inside this
        // same transaction rather than opening a second one against `self`.
        tx.execute("DELETE FROM episodes WHERE series_id=?1", [synthetic_id])?;
        tx.execute("DELETE FROM series_genres WHERE series_id=?1", [synthetic_id])?;
        tx.execute("DELETE FROM series WHERE id=?1", [synthetic_id])?;
        tx.commit()?;
        Ok(())
    }

    /// Reverse one swipe decision, whichever shape its row ended up in, and
    /// report the title of what was actually undone (`None` = nothing was).
    ///
    /// Two shapes, because a `Seen` catalog swipe fires a background site-link
    /// that can rewrite what it decided on:
    ///
    /// 1. The swiped row still exists — hard-delete it, exactly as
    ///    `undo_last_swipe`/`undo_swipe_entry` always did. The card returns to
    ///    the deck because nothing excludes it any more.
    /// 2. The row was merged into an existing site row by `merge_series_into`
    ///    and deleted. Deleting is then both impossible and wrong (the
    ///    survivor is a real, pre-existing, possibly-followed series), so
    ///    instead this reverts precisely the flags that merge set on the
    ///    survivor, per the `series_merges` trail. Before this existed, undo
    ///    found no row, deleted nothing, and returned success — leaving the
    ///    survivor stuck at `watched_externally=1` with no way back, while the
    ///    UI showed "Deshecho".
    ///
    /// `None` (nothing undone) is not an error — undoing with an empty history
    /// or a row someone already removed is a normal no-op. Callers use it to
    /// decide whether to claim anything happened.
    pub fn undo_swipe_decision(&self, series_id: i64) -> Result<Option<String>> {
        let title: Option<String> = self
            .conn
            .query_row("SELECT title FROM series WHERE id=?1", [series_id], |r| r.get(0))
            .optional()?;
        if let Some(title) = title {
            self.delete_series(series_id)?;
            // A stale trail row for a reused rowid must not outlive the row it
            // no longer describes.
            self.conn.execute("DELETE FROM series_merges WHERE synthetic_id=?1", [series_id])?;
            return Ok(Some(title));
        }

        let merge: Option<(i64, String, i64, i64, i64)> = self
            .conn
            .query_row(
                "SELECT survivor_id, title, set_followed, set_watched_externally, set_backlog_status
                 FROM series_merges WHERE synthetic_id=?1",
                [series_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .optional()?;
        let Some((survivor_id, title, set_followed, set_watched, set_backlog)) = merge else {
            return Ok(None);
        };
        // The survivor can itself have been deleted since (a later "return to
        // deck" on it, say) — then there's nothing to revert, but the stale
        // trail row still goes.
        let survivor_exists: bool = self
            .conn
            .query_row("SELECT 1 FROM series WHERE id=?1", [survivor_id], |_| Ok(()))
            .optional()?
            .is_some();
        if survivor_exists {
            if set_followed != 0 {
                self.conn.execute("UPDATE series SET followed=0 WHERE id=?1", [survivor_id])?;
            }
            if set_watched != 0 {
                self.conn.execute("UPDATE series SET watched_externally=0 WHERE id=?1", [survivor_id])?;
            }
            if set_backlog != 0 {
                self.conn.execute("UPDATE series SET backlog_status=NULL WHERE id=?1", [survivor_id])?;
            }
        }
        self.conn.execute("DELETE FROM series_merges WHERE synthetic_id=?1", [series_id])?;
        if !survivor_exists {
            return Ok(None);
        }
        Ok(Some(title))
    }

    /// Rewrite a synthetic row's slug/url/cover/kind in place after a
    /// successful site match — `id`/`followed`/`backlog_status`/
    /// `watched_externally`/`anilist_id` are deliberately untouched (see
    /// `commands::link_catalog_series`).
    pub fn relink_series(
        &self,
        series_id: i64,
        slug: &str,
        url: &str,
        cover_url: Option<&str>,
        kind: &str,
    ) -> Result<()> {
        self.conn.execute(
            "UPDATE series SET slug=?1, url=?2,
                cover_url = CASE
                    WHEN cover_url IS NOT NULL AND cover_url != '' THEN cover_url
                    WHEN ?3 IS NOT NULL AND ?3 != '' THEN ?3
                    ELSE (SELECT cover_url FROM anilist_catalog WHERE id = series.anilist_id)
                END,
                kind=?4
             WHERE id=?5",
            (slug, url, cover_url, kind, series_id),
        )?;
        Ok(())
    }

    /// Move a freshly matched catalog stub to the matched site: `source_id`,
    /// `slug` and `url` change in ONE statement.
    ///
    /// All three together on purpose. Moving only the source would leave the
    /// stub's `anilist-N` slug under the new source, which collides with a stub
    /// the same title left there earlier (`UNIQUE(source_id, slug)`); writing the
    /// real slug in the same UPDATE can only collide with a real series, and the
    /// caller already checked that with `find_series_id_by_slug`. A violation is
    /// still returned as `Err`, never a panic.
    pub fn set_series_source(&self, series_id: i64, source_id: i64, slug: &str, url: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE series SET source_id=?1, slug=?2, url=?3 WHERE id=?4",
            (source_id, slug, url, series_id),
        )?;
        Ok(())
    }

    /// Replace a series' genre set outright (unlike `insert_series_genres`,
    /// which is additive) — used when linking rewrites a synthetic row's
    /// AniList-sourced genres with the site's own.
    pub fn replace_series_genres(&self, series_id: i64, genres: &[String]) -> Result<()> {
        self.conn.execute("DELETE FROM series_genres WHERE series_id=?1", [series_id])?;
        self.insert_series_genres(series_id, genres)
    }

    pub fn set_followed(&self, series_id: i64, followed: bool) -> Result<()> {
        self.conn.execute(
            "UPDATE series SET followed=?1 WHERE id=?2",
            (followed as i64, series_id),
        )?;
        Ok(())
    }

    /// Follow/unfollow a show **canonically** — across every site that has it.
    /// Following is an AniList-level fact, not a per-site one: following One
    /// Piece on TioAnime marks the AnimeYT/AnimeFLV rows followed too, so your
    /// "seguidos" are identical whichever site is active (only the *pending
    /// episodes* differ, since those come from the active site). Members are the
    /// rows sharing this one's canonical identity — same `anilist_id`, or (for
    /// rows without one) the same normalized title. Returns rows changed.
    pub fn set_followed_canonical(&self, series_id: i64, followed: bool) -> Result<usize> {
        let (anilist_id, title): (Option<i64>, String) = self.conn.query_row(
            "SELECT anilist_id, title FROM series WHERE id=?1",
            [series_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        let f = followed as i64;
        // An explicit unfollow is remembered (`follow_declined_at` stamped)
        // so `carry_follow` won't silently re-follow this exact row from a
        // fuzzy-matched sibling on another site on the very next scan — see
        // that column's doc comment in db.rs. An explicit re-follow clears
        // it: the user changed their mind, so carry-over/auto-import are
        // welcome to treat this row normally again.
        let declined_sql = if followed { "NULL" } else { "datetime('now')" };
        if let Some(aid) = anilist_id {
            // Fast path: everything sharing the AniList id is the same show.
            let n = self.conn.execute(
                &format!("UPDATE series SET followed=?1, follow_declined_at={declined_sql} WHERE anilist_id=?2"),
                (f, aid),
            )?;
            return Ok(n);
        }
        // No AniList id: match by normalized title (computed in Rust — SQL can't
        // run `normalize_title`). Only rows that are *also* AniList-less are
        // candidates; a row with an id belongs to a different canonical bucket.
        let target = crate::matching::normalize_title(&title);
        let mut stmt = self
            .conn
            .prepare("SELECT id, title FROM series WHERE anilist_id IS NULL")?;
        let ids: Vec<i64> = stmt
            .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?
            .into_iter()
            .filter(|(_, t)| crate::matching::normalize_title(t) == target)
            .map(|(id, _)| id)
            .collect();
        let mut changed = 0;
        for id in ids {
            changed += self.conn.execute(
                &format!("UPDATE series SET followed=?1, follow_declined_at={declined_sql} WHERE id=?2"),
                (f, id),
            )?;
        }
        Ok(changed)
    }

    /// Every followed series on a source OTHER than `exclude_source_id`, with
    /// its title, highest *seen* episode number (0 when nothing's been
    /// watched), and its optional `anilist_id`. Non-numeric / recap episode
    /// numbers never inflate the watermark (`CAST(number AS INTEGER)` parses
    /// leading digits, "Recap"→0). Powers cross-site follow carry-over: matched
    /// by title against the newly scanned site's airing list, with a veto if
    /// both sides have an AniList id and they differ.
    pub(crate) fn followed_titles_with_watermark(
        &self,
        exclude_source_id: i64,
    ) -> Result<Vec<(String, i64, Option<i64>)>> {
        let mut stmt = self.conn.prepare(
            "SELECT s.title,
                    COALESCE(MAX(CASE WHEN e.seen=1 THEN CAST(e.number AS INTEGER) END), 0) AS watermark,
                    s.anilist_id
             FROM series s
             LEFT JOIN episodes e ON e.series_id = s.id
             WHERE s.followed=1 AND s.source_id <> ?1
             GROUP BY s.id
             ORDER BY s.title",
        )?;
        let rows = stmt
            .query_map([exclude_source_id], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, Option<i64>>(2)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Look up the optional `anilist_id` for each series id in order.
    pub(crate) fn anilist_ids_for_series(&self, series_ids: &[i64]) -> Result<Vec<Option<i64>>> {
        let mut stmt = self.conn.prepare("SELECT anilist_id FROM series WHERE id=?1")?;
        let mut out = Vec::with_capacity(series_ids.len());
        for &id in series_ids {
            let anilist_id: Option<i64> = stmt
                .query_row([id], |r| r.get(0))
                .optional()?
                .flatten();
            out.push(anilist_id);
        }
        Ok(out)
    }

    /// Carry a follow onto a new-site series row: set it followed and stash
    /// the watermark for `refresh()` to apply once. Guarded to `followed=0` so
    /// it never overrides an existing follow (or its real progress) — a series
    /// the user already follows on this site is left completely untouched.
    /// Returns true if a row was actually carried.
    pub fn carry_follow(&self, series_id: i64, watermark: i64) -> Result<bool> {
        // `follow_declined_at IS NULL`: never re-follow a row the user just
        // explicitly unfollowed — see that column's doc comment. Without
        // this, a fuzzy title match (score()'s season-blind same_franchise
        // floor treats every season of a show as interchangeable) against a
        // still-followed sibling on another site silently re-followed and
        // re-marked-seen the exact row the user un-followed, on the very
        // next airing rescan.
        let n = self.conn.execute(
            "UPDATE series SET followed=1, carried_seen_number=?2
             WHERE id=?1 AND followed=0 AND follow_declined_at IS NULL",
            (series_id, watermark),
        )?;
        Ok(n > 0)
    }

    /// Read and clear a series' pending carry-over watermark (apply-once). The
    /// clear happens whether or not the caller ends up cascading, so a series
    /// with no episodes yet doesn't get stuck re-applying every refresh.
    pub fn take_carried_seen_number(&self, series_id: i64) -> Result<Option<i64>> {
        let v: Option<i64> = self.conn.query_row(
            "SELECT carried_seen_number FROM series WHERE id=?1",
            [series_id],
            |r| r.get(0),
        )?;
        if v.is_some() {
            self.conn.execute(
                "UPDATE series SET carried_seen_number=NULL WHERE id=?1",
                [series_id],
            )?;
        }
        Ok(v)
    }

    /// Update a series' canonical URL (used when a mirror fallback succeeds on
    /// a different host than the one currently stored).
    pub fn update_series_url(&self, series_id: i64, url: &str) -> Result<()> {
        self.conn
            .execute("UPDATE series SET url=?1 WHERE id=?2", (url, series_id))?;
        Ok(())
    }

    /// Up to `limit` currently-airing series across **every** site whose cover
    /// has never been fetched: not AniList-linked (`list_airing` already shows
    /// those via the catalog's own `data:`-compatible-by-CSP cover instead of
    /// this column, see `db/airing.rs`) and not already a `data:`, `file:`, or
    /// `asset:` URI.
    ///
    /// Spans every site because the direct reqwest download
    /// (`cover_cache::cache_cover_image`) needs no site state — it speaks
    /// directly to the image CDN, not through the site's Cloudflare session.
    /// Only the WebView2 canvas fallback is site-sensitive (Cloudflare cookies
    /// live per domain and a fetch can burn up to 30 s), so callers must
    /// limit that path to the active site.
    ///
    /// This is `refresh()`'s only path to ever converting a cover for an
    /// airing series that ISN'T followed — the main fetch/backlog loops are
    /// both scoped to `list_followed`, so an unfollowed, unlinked airing
    /// series' cover was permanently stuck on the site's raw remote URL,
    /// which the app's CSP silently blocks. `limit` bounds this to a modest
    /// number of fetches per refresh cycle rather than bulk-fetching the whole
    /// airing listing's covers at once, which reads to Cloudflare as scraping
    /// abuse regardless of having a valid session — see CLAUDE.md.
    ///
    /// The window is random, not `ORDER BY id`: a cover that fails for good
    /// (404, non-image body, over the size cap) stays remote and so is
    /// selected again every cycle, and with a fixed order `limit` such rows
    /// would pin the window to the same ids forever and starve every later
    /// series. Shuffling lets the whole backlog drain over a few cycles.
    ///
    /// Returns `(id, title, cover_url, source_id, series_url)` so the caller can
    /// fetch a cover from the series page when the airing listing has no cover.
    ///
    /// Rows with no cover at all are only returned for `null_cover_source` (the
    /// active site, when its series pages can supply one): the caller has no way
    /// to fill them for other sites, and returning them would only take slots
    /// from the batch.
    pub fn airing_series_needing_cover_fetch(
        &self,
        limit: i64,
        null_cover_source: Option<i64>,
    ) -> Result<Vec<AiringCoverRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, title, cover_url, source_id, url FROM series
             WHERE is_airing=1 AND anilist_id IS NULL
               AND ((cover_url IS NULL AND source_id = ?2)
                 OR (cover_url NOT LIKE 'data:%'
                     AND cover_url NOT LIKE 'file:%'
                     AND cover_url NOT LIKE 'asset:%'))
               -- Back off a URL that already failed: 6 h, doubling per
               -- consecutive failure, capped at 7 days (`record_cover_failure`).
               AND (
                 cover_failed_url IS NULL OR cover_failed_at IS NULL
                 OR cover_failed_url <> COALESCE(cover_url, url)
                 OR strftime('%s','now') - strftime('%s', cover_failed_at)
                     >= MIN(21600 * (1 << MIN(MAX(cover_fail_count, 1) - 1, 5)), 604800)
               )
             ORDER BY RANDOM()
             LIMIT ?1",
        )?;
        let rows = stmt
            .query_map(rusqlite::params![limit, null_cover_source], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Store a cover URL found on the series page for a row that had none, so the
    /// regular download (and its failure back-off on that URL) takes over.
    pub fn set_missing_cover_url(&self, series_id: i64, url: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE series SET cover_url = ?2 WHERE id = ?1 AND cover_url IS NULL",
            rusqlite::params![series_id, url],
        )?;
        Ok(())
    }

    /// Remember that fetching `url` as this series' cover failed (direct
    /// download and, where tried, the WebView2 fallback). Consecutive
    /// failures of the same URL grow the backoff applied by
    /// `airing_series_needing_cover_fetch`; a different URL starts over.
    pub fn record_cover_failure(&self, series_id: i64, url: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE series SET
                cover_fail_count = CASE WHEN cover_failed_url = ?2 THEN cover_fail_count + 1 ELSE 1 END,
                cover_failed_url = ?2,
                cover_failed_at = datetime('now')
             WHERE id = ?1",
            rusqlite::params![series_id, url],
        )?;
        Ok(())
    }

    /// Clear `file:` covers of `source_id` whose cached file no longer exists (the user
    /// emptied `AppData\covers`, or the DB was restored onto another
    /// machine/user — the stored path is absolute and covers are not part
    /// of the Drive backup). `upsert_series` only protects a `file:` cover
    /// from a *remote* overwrite, so once cleared the next listing upsert
    /// puts the site's thumbnail back and the cover step re-downloads it.
    ///
    /// Only series named in `listed_slugs` (the slugs of a listing scan that
    /// just succeeded) are touched: a series absent from it — finished, or
    /// the scan failed — would not get its remote URL back and clearing its
    /// cover would lose it for good. Returns how many rows were cleared.
    pub fn heal_missing_cached_covers(
        &self,
        source_id: i64,
        listed_slugs: &std::collections::HashSet<&str>,
    ) -> Result<usize> {
        let mut stmt = self.conn.prepare(
            "SELECT id, slug, cover_url FROM series WHERE source_id=?1 AND cover_url LIKE 'file:%'",
        )?;
        let rows: Vec<(i64, String, String)> = stmt
            .query_map([source_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut cleared = 0;
        for (id, slug, url) in rows {
            if !listed_slugs.contains(slug.as_str()) {
                continue;
            }
            let path = url.strip_prefix("file:").unwrap_or(&url);
            if !std::path::Path::new(path).is_file() {
                self.conn.execute("UPDATE series SET cover_url=NULL WHERE id=?1", [id])?;
                cleared += 1;
            }
        }
        Ok(cleared)
    }

    /// Clear covers that are stored as data URIs but are actually tiny placeholder
    /// images (like the Edge error icon), returning how many were cleared.
    /// Runs a cheap length filter first to avoid loading huge data URIs.
    ///
    /// Also clears small covers that three or more series share byte for byte:
    /// a site's own "NO IMAGE" picture is a normal-sized poster that no
    /// dimension check catches, but a real cover is never identical across
    /// unrelated shows.
    pub fn clear_placeholder_covers(&self) -> Result<usize> {
        let mut stmt = self.conn.prepare(
            "SELECT id, cover_url FROM series
             WHERE cover_url LIKE 'data:image%' AND length(cover_url) < 20000"
        )?;
        let rows: Vec<(i64, String)> = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        // Distinct FRANCHISES per cover: arcs or seasons of one show on one site
        // legitimately share a poster; unrelated shows never do.
        let mut titles_by_cover: std::collections::HashMap<&str, std::collections::HashSet<String>> =
            std::collections::HashMap::new();
        {
            let mut tstmt = self.conn.prepare("SELECT title FROM series WHERE id=?1")?;
            for (id, url) in &rows {
                let title: String = tstmt.query_row([id], |r| r.get(0))?;
                titles_by_cover
                    .entry(url.as_str())
                    .or_default()
                    .insert(cover_franchise_key(&title));
            }
        }

        // Reuse an enclosing transaction (`persist_listing`); SQLite has no nested BEGIN.
        let tx = if self.conn.is_autocommit() { Some(self.conn.unchecked_transaction()?) } else { None };
        let mut cleared = 0;
        for (id, url) in &rows {
            let widely_shared =
                titles_by_cover.get(url.as_str()).map_or(0, |f| f.len()) >= SHARED_COVER_MIN_SERIES;
            if widely_shared || crate::cover_cache::is_placeholder_data_uri(url) {
                self.conn.execute("UPDATE series SET cover_url=NULL WHERE id=?1", [id])?;
                cleared += 1;
            }
        }
        if let Some(tx) = tx {
            tx.commit()?;
        }
        Ok(cleared)
    }

    /// A show that a COMPLETE airing listing of `source_id` no longer carries
    /// stops airing, unless AniList linked it (then AniList's status decides,
    /// `sync_status_from_catalog`) or the user follows it (its episodes still
    /// matter: it must not jump to the "finished" bucket because a site rotated
    /// it off a page). Without this a show the site dropped stayed "En emision"
    /// for ever: scans only ever upsert what is listed. The next listing that
    /// carries it again sets it back (adapters report listed shows as airing).
    /// The caller must only pass a listing whose page walk ended on its own
    /// (`scrape_airing_listing`'s `complete`). Two more guards against a broken
    /// parse or an incompatible mirror: fewer than 10 listed shows, or fewer than
    /// half as many as this source currently has airing and unlinked, change nothing.
    pub fn mark_unlisted_not_airing(
        &self,
        source_id: i64,
        listed_slugs: &std::collections::HashSet<&str>,
    ) -> Result<usize> {
        if listed_slugs.len() < 10 {
            return Ok(0);
        }
        let mut stmt = self.conn.prepare(
            "SELECT id, slug, followed FROM series WHERE source_id=?1 AND is_airing=1 AND anilist_id IS NULL",
        )?;
        let airing_unlinked: Vec<(i64, String, bool)> = stmt
            .query_map([source_id], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)? != 0)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        if listed_slugs.len() * 2 < airing_unlinked.len() {
            return Ok(0);
        }
        let stale: Vec<i64> = airing_unlinked
            .into_iter()
            .filter(|(_, slug, followed)| !followed && !listed_slugs.contains(slug.as_str()))
            .map(|(id, _, _)| id)
            .collect();
        // Reuse an enclosing transaction (`persist_listing`); SQLite has no nested BEGIN.
        let tx = if self.conn.is_autocommit() { Some(self.conn.unchecked_transaction()?) } else { None };
        for id in &stale {
            self.conn.execute("UPDATE series SET is_airing=0 WHERE id=?1", [id])?;
        }
        if let Some(tx) = tx {
            tx.commit()?;
        }
        Ok(stale.len())
    }

    /// Replace a series' cover with a fetched base64 data URI.
    ///
    /// A small image that two or more OTHER franchises already carry byte for byte
    /// is a site's "NO IMAGE" placeholder (see `clear_placeholder_covers`): store
    /// nothing. (The next refresh puts the remote URL back and fetches it once more
    /// per refresh; the fallback is capped, so the cost is bounded.)
    pub fn update_series_cover(&self, series_id: i64, cover_url: &str) -> Result<()> {
        if cover_url.starts_with("data:image") && cover_url.len() < 20_000 {
            let mut tstmt = self.conn.prepare("SELECT title FROM series WHERE cover_url=?1 AND id<>?2")?;
            let own_title: String = self.conn.query_row("SELECT title FROM series WHERE id=?1", [series_id], |r| r.get(0))?;
            let own_key = cover_franchise_key(&own_title);
            let other_franchises: std::collections::HashSet<String> = tstmt
                .query_map((cover_url, series_id), |r| r.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?
                .iter()
                .map(|t| cover_franchise_key(t))
                .filter(|k| *k != own_key)
                .collect();
            if other_franchises.len() >= SHARED_COVER_MIN_SERIES - 1 {
                self.conn.execute("UPDATE series SET cover_url=NULL WHERE id=?1", [series_id])?;
                return Ok(());
            }
        }
        self.conn.execute(
            "UPDATE series SET cover_url=?1 WHERE id=?2",
            (cover_url, series_id),
        )?;
        Ok(())
    }

    pub fn set_backlog_status(&self, series_id: i64, status: Option<&str>) -> Result<()> {
        self.conn
            .execute("UPDATE series SET backlog_status=?1 WHERE id=?2", (status, series_id))?;
        Ok(())
    }

    pub fn get_backlog_status(&self, series_id: i64) -> Result<Option<String>> {
        Ok(self.conn.query_row(
            "SELECT backlog_status FROM series WHERE id=?1",
            [series_id],
            |r| r.get::<_, Option<String>>(0),
        )?)
    }

    pub fn set_kind(&self, series_id: i64, kind: &str) -> Result<()> {
        self.conn
            .execute("UPDATE series SET kind=?1 WHERE id=?2", (kind, series_id))?;
        Ok(())
    }

    /// Set the real numeric AniList id on a `series` row created from a
    /// catalog swipe decision — see the `anilist_id` column comment in
    /// `init_schema`.
    pub fn set_anilist_id(&self, series_id: i64, anilist_id: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE series SET anilist_id=?1,
                cover_url = CASE
                    WHEN cover_url IS NULL OR cover_url = '' OR (cover_url NOT LIKE 'file:%' AND cover_url NOT LIKE 'data:%' AND cover_url NOT LIKE 'https://%anilist.co%')
                    THEN COALESCE((SELECT cover_url FROM anilist_catalog WHERE id=?1), cover_url)
                    ELSE cover_url
                END
             WHERE id=?2",
            (anilist_id, series_id),
        )?;
        Ok(())
    }

    /// Set (or clear) the "watched outside the app" flag — see
    /// `watched_externally`'s column comment in `init_schema`.
    pub fn set_watched_externally(&self, series_id: i64, watched: bool) -> Result<()> {
        self.conn.execute(
            "UPDATE series SET watched_externally=?1 WHERE id=?2",
            (watched as i64, series_id),
        )?;
        Ok(())
    }

    /// Insert genres for a series; already-present (series_id, genre) pairs
    /// are left alone (genres are static once fetched, no need to delete).
    pub fn insert_series_genres(&self, series_id: i64, genres: &[String]) -> Result<()> {
        for g in genres {
            self.conn.execute(
                "INSERT INTO series_genres(series_id, genre) VALUES(?1, ?2)
                 ON CONFLICT(series_id, genre) DO NOTHING",
                (series_id, g),
            )?;
        }
        Ok(())
    }

    pub fn list_series_genres(&self, series_id: i64) -> Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT genre FROM series_effective_genres WHERE series_id=?1 ORDER BY genre")?;
        let rows = stmt
            .query_map([series_id], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Bulk-fetch genres for multiple series ids in a single query.
    pub fn list_series_genres_batch(
        &self,
        series_ids: &[i64],
    ) -> Result<std::collections::HashMap<i64, Vec<String>>> {
        let mut map: std::collections::HashMap<i64, Vec<String>> = std::collections::HashMap::new();
        if series_ids.is_empty() {
            return Ok(map);
        }
        for &id in series_ids {
            map.entry(id).or_default();
        }
        let placeholders = vec!["?"; series_ids.len()].join(",");
        let sql = format!(
            "SELECT series_id, genre FROM series_effective_genres WHERE series_id IN ({}) ORDER BY genre",
            placeholders
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let params = rusqlite::params_from_iter(series_ids.iter());
        let rows = stmt
            .query_map(params, |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for (sid, g) in rows {
            map.entry(sid).or_default().push(g);
        }
        Ok(map)
    }


    /// `true` if this series has no `series_genres` rows yet (needs a
    /// detail-page fetch to backfill) and is NOT linked to AniList.
    /// Linked series take all metadata from AniList as the source of truth,
    /// so the site detail fetch is skipped.
    pub fn series_needs_genre_backfill(&self, series_id: i64) -> Result<bool> {
        let is_linked: bool = self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM series WHERE id=?1 AND anilist_id IS NOT NULL)",
            [series_id],
            |r| r.get(0),
        )?;
        if is_linked {
            return Ok(false);
        }
        let has_site_genres: bool = self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM series_genres WHERE series_id=?1)",
            [series_id],
            |r| r.get(0),
        )?;
        Ok(!has_site_genres)
    }

    pub(crate) fn row_to_series(r: &rusqlite::Row) -> rusqlite::Result<crate::models::Series> {
        Ok(crate::models::Series {
            id: r.get("id")?,
            slug: r.get("slug")?,
            title: r.get("title")?,
            url: r.get("url")?,
            cover_url: r.get("cover_url")?,
            is_airing: r.get::<_, i64>("is_airing")? != 0,
            followed: r.get::<_, i64>("followed")? != 0,
            next_episode_at: r.get("next_episode_at")?,
            site_episode_count: r.get("site_episode_count")?,
        })
    }

    /// Set of series ids for followed series on `source_id` that are already
    /// linked to an AniList catalog row (`anilist_id IS NOT NULL`). Used by
    /// `refresh()` to skip downloading site covers for shows whose display
    /// cover comes from AniList anyway.
    pub fn followed_linked_series_ids(&self, source_id: i64) -> Result<std::collections::HashSet<i64>> {
        let mut stmt = self.conn.prepare(
            "SELECT s.id FROM series s JOIN anilist_catalog c ON c.id = s.anilist_id
             WHERE s.source_id=?1 AND s.followed=1
               AND c.cover_url IS NOT NULL AND c.cover_url != ''",
        )?;
        let mut set = std::collections::HashSet::new();
        let rows = stmt.query_map([source_id], |r| r.get::<_, i64>(0))?;
        for id in rows {
            set.insert(id?);
        }
        Ok(set)
    }

    /// The **site-agnostic** library: every followed or watched-externally show
    /// across *all* sites, collapsed to one entry per canonical identity
    /// (`anilist_id`, else normalized title — see
    /// docs/cross-site-library-investigation.md, option C/D). This is why the
    /// Biblioteca no longer empties out when you switch sites: it shows your
    /// whole library regardless of the active site, and the active site only
    /// decides which member row is the clickable/scrapeable one.
    ///
    /// `active_source_id` is a *preference*, not a filter: when a canonical show
    /// exists on several sites the member on the active site is chosen as the
    /// representative (so opening an episode targets the current site), else the
    /// most-scraped member. Progress (total/seen/last-watched) is unioned across
    /// members so switching sites never loses your place. Series with zero
    /// scraped episodes still appear (`total=0`); `watched_externally` rows are
    /// classified "completed" on the frontend rather than "plan".
    pub fn list_library(&self, active_source_id: i64) -> Result<Vec<crate::models::LibraryItem>> {
        // Per-member row across every site (no source filter). anilist_id and
        // source_id are carried alongside the Series so we can group canonically
        // and pick the active-site representative. Title and cover prefer the
        // catalog row when linked (AniList is the source of truth).
        let mut stmt = self.conn.prepare(
            "SELECT s.id, s.source_id, s.anilist_id, s.slug,
                    COALESCE(c.title, s.title) AS title,
                    s.url,
                    COALESCE(c.cover_url, s.cover_url) AS cover_url,
                    s.is_airing, s.followed, s.next_episode_at, s.site_episode_count,
                    s.watched_externally, s.kind,
                    COUNT(e.id) AS total,
                    SUM(CASE WHEN e.seen=1 THEN 1 ELSE 0 END) AS seen,
                    MAX(e.added_at) AS last_added,
                    MAX(e.seen_at) AS last_watched_at,
                    c.studio AS studio,
                    c.episodes AS catalog_episodes,
                    c.status AS catalog_status
             FROM series s
             LEFT JOIN episodes e ON e.series_id = s.id
             LEFT JOIN anilist_catalog c ON c.id = s.anilist_id
             WHERE s.followed=1 OR s.watched_externally=1
             GROUP BY s.id",
        )?;
        struct Member {
            source_id: i64,
            anilist_id: Option<i64>,
            series: crate::models::Series,
            watched_externally: bool,
            kind: Option<String>,
            total: i64,
            seen: i64,
            last_added: Option<String>,
            last_watched_at: Option<String>,
            studio: Option<String>,
            catalog_episodes: Option<i64>,
            catalog_status: Option<String>,
        }
        let members = stmt
            .query_map([], |r| {
                Ok(Member {
                    source_id: r.get::<_, i64>("source_id")?,
                    anilist_id: r.get::<_, Option<i64>>("anilist_id")?,
                    series: Self::row_to_series(r)?,
                    watched_externally: r.get::<_, i64>("watched_externally")? != 0,
                    kind: r.get::<_, Option<String>>("kind")?,
                    total: r.get::<_, i64>("total")?,
                    seen: r.get::<_, Option<i64>>("seen")?.unwrap_or(0),
                    last_added: r.get::<_, Option<String>>("last_added")?,
                    last_watched_at: r.get::<_, Option<String>>("last_watched_at")?,
                    studio: r.get::<_, Option<String>>("studio")?,
                    catalog_episodes: r.get::<_, Option<i64>>("catalog_episodes")?,
                    catalog_status: r.get::<_, Option<String>>("catalog_status")?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        // Group members by canonical identity, preserving first-seen order.
        let mut order: Vec<String> = Vec::new();
        let mut groups: std::collections::HashMap<String, Vec<Member>> = std::collections::HashMap::new();
        for m in members {
            let key = super::library::canon_key(m.anilist_id, &m.series.title);
            if !groups.contains_key(&key) {
                order.push(key.clone());
            }
            groups.entry(key).or_default().push(m);
        }

        // Pick each group's representative: prefer the active-site member, then
        // the most-scraped, then one with a cover, then stable by id. Progress
        // fields are unioned across the group.
        struct Rep {
            member: Member,
            total_episodes: i64,
            seen_episodes: i64,
            watched_externally: bool,
            last_added: Option<String>,
            last_watched_at: Option<String>,
            catalog_episodes: Option<i64>,
            catalog_status: Option<String>,
        }
        let mut reps: Vec<Rep> = Vec::new();
        for key in &order {
            let mut group = groups.remove(key).unwrap();
            let total_episodes = group.iter().map(|m| m.total).max().unwrap_or(0);
            let seen_episodes = group.iter().map(|m| m.seen).max().unwrap_or(0);
            let watched_externally = group.iter().any(|m| m.watched_externally);
            let last_added = group.iter().filter_map(|m| m.last_added.clone()).max();
            let last_watched_at = group.iter().filter_map(|m| m.last_watched_at.clone()).max();
            // Both fields from the SAME catalog row, so a title-grouped show
            // never mixes one entry's total with another entry's status.
            let (catalog_episodes, catalog_status) = group
                .iter()
                .find(|m| m.catalog_episodes.is_some() || m.catalog_status.is_some())
                .map(|m| (m.catalog_episodes, m.catalog_status.clone()))
                .unwrap_or((None, None));
            group.sort_by(|a, b| {
                let a_active = (a.source_id == active_source_id) as u8;
                let b_active = (b.source_id == active_source_id) as u8;
                b_active
                    .cmp(&a_active)
                    .then(b.total.cmp(&a.total))
                    .then((b.series.cover_url.is_some()).cmp(&(a.series.cover_url.is_some())))
                    .then(a.series.id.cmp(&b.series.id))
            });
            let member = group.into_iter().next().unwrap();
            reps.push(Rep {
                member,
                total_episodes,
                seen_episodes,
                watched_externally,
                last_added,
                last_watched_at,
                catalog_episodes,
                catalog_status,
            });
        }

        // Bulk-fetch genres for the representative rows only (one statement, not
        // an N+1 per-series query — see the library-filters design doc). Real
        // genres only (AniList genres, or site labels mapped onto them), the
        // same vocabulary as the stats: `series_effective_genres` also carries
        // AniList tags and raw site labels, which made the genre filter a
        // ~360-chip mix of "Acción"/"Action"/"Age Gap"/"Achromatic".
        let ids: Vec<i64> = reps.iter().map(|r| r.member.series.id).collect();
        let mut genres_by_series = self.real_genres_batch(&ids)?;

        let mut out = Vec::with_capacity(reps.len());
        for r in reps {
            let next_episode = self.next_unseen_episode(r.member.series.id)?;
            let genres = genres_by_series.remove(&r.member.series.id).unwrap_or_default();
            out.push(crate::models::LibraryItem {
                series: r.member.series,
                total_episodes: r.total_episodes,
                seen_episodes: r.seen_episodes,
                last_added: r.last_added,
                next_episode,
                last_watched_at: r.last_watched_at,
                watched_externally: r.watched_externally,
                kind: r.member.kind,
                genres,
                studio: r.member.studio,
                catalog_episodes: r.catalog_episodes,
                catalog_status: r.catalog_status,
            });
        }
        out.sort_by_key(|item| item.series.title.to_lowercase());
        Ok(out)
    }

    /// Every row's `SELECT` list for the "Listas" queries below — the
    /// `row_to_series` columns plus the two grouping keys.
    const CLASSIFIED_COLUMNS: &str =
        "id, source_id, anilist_id, slug, title, url, cover_url, is_airing, followed,
         next_episode_at, site_episode_count";

    /// Collapse `series` rows drawn from **all** sites to one entry per
    /// canonical show (`library::canon_key`: AniList id, else normalized
    /// title), preferring the member on the active site so the row the UI
    /// acts on ("Empezar a ver", "Abrir") is the current site's whenever it
    /// has one. Ordered by title, case-insensitively.
    fn canonical_representatives(
        rows: Vec<(i64, Option<i64>, crate::models::Series)>,
        active_source_id: i64,
    ) -> Vec<crate::models::Series> {
        let mut order: Vec<String> = Vec::new();
        let mut groups: std::collections::HashMap<String, Vec<(i64, crate::models::Series)>> =
            std::collections::HashMap::new();
        for (source_id, anilist_id, s) in rows {
            let key = super::library::canon_key(anilist_id, &s.title);
            if !groups.contains_key(&key) {
                order.push(key.clone());
            }
            groups.entry(key).or_default().push((source_id, s));
        }
        let mut out = Vec::with_capacity(order.len());
        for key in &order {
            let mut group = groups.remove(key).unwrap();
            group.sort_by(|a, b| {
                let a_active = (a.0 == active_source_id) as u8;
                let b_active = (b.0 == active_source_id) as u8;
                b_active
                    .cmp(&a_active)
                    .then(b.1.cover_url.is_some().cmp(&a.1.cover_url.is_some()))
                    .then(a.1.id.cmp(&b.1.id))
            });
            out.push(group.into_iter().next().unwrap().1);
        }
        out.sort_by_key(|s| s.title.to_lowercase());
        out
    }

    /// Series with the given `backlog_status` ('want' or 'discarded'), for
    /// the swipe mode's "Listas" sub-view.
    ///
    /// **Site-agnostic**, like `list_library` and unlike the per-site query
    /// this replaced: a Want/Descartar decision is excluded from the Descubrir
    /// deck globally (`engaged_series_titles` has no source filter), so listing
    /// it per-site made an old decision invisible — and therefore impossible to
    /// review or reverse — the moment you switched sites, while it silently
    /// went on suppressing the title in the deck. Same per-site-scoping bug
    /// class already fixed for Estadísticas and the deck exclusion itself.
    ///
    /// `active_source_id` is a *preference*, not a filter — see
    /// `canonical_representatives`.
    pub fn list_backlog(
        &self,
        active_source_id: i64,
        status: &str,
    ) -> Result<Vec<crate::models::Series>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {} FROM series WHERE backlog_status=?1 ORDER BY title, id",
            Self::CLASSIFIED_COLUMNS
        ))?;
        let rows = stmt
            .query_map([status], |r| {
                Ok((
                    r.get::<_, i64>("source_id")?,
                    r.get::<_, Option<i64>>("anilist_id")?,
                    Self::row_to_series(r)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(Self::canonical_representatives(rows, active_source_id))
    }

    /// Series with `watched_externally=1` ("Ya lo vi" catalog swipe), for
    /// the Listas view's "Ya vistas" sub-list — mirrors `list_backlog`,
    /// site-agnostic for the same reason.
    pub fn list_watched_externally(
        &self,
        active_source_id: i64,
    ) -> Result<Vec<crate::models::Series>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {} FROM series WHERE watched_externally=1 ORDER BY title, id",
            Self::CLASSIFIED_COLUMNS
        ))?;
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, i64>("source_id")?,
                    r.get::<_, Option<i64>>("anilist_id")?,
                    Self::row_to_series(r)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(Self::canonical_representatives(rows, active_source_id))
    }

    /// Whether a `series` row with this id still exists.
    ///
    /// Exists because the schema declares **no** `ON DELETE CASCADE`: a write
    /// to `episodes` (or `series_genres`) for an id whose parent row is gone
    /// succeeds silently and leaves an orphan behind, and several stats
    /// queries read `episodes` without joining back to `series`, so orphans
    /// permanently inflate episode totals / the 30-day heatmap / the binge
    /// record. Any code path that scrapes first and writes episodes *after*
    /// (the background catalog link, `start_watching`) must re-check this
    /// under the `db` lock right before writing — the user can have undone
    /// the decision, and deleted the row, while the scrape was in flight.
    #[cfg(test)]
    pub fn series_exists(&self, series_id: i64) -> Result<bool> {
        Ok(self
            .conn
            .query_row("SELECT 1 FROM series WHERE id=?1", [series_id], |_| Ok(()))
            .optional()?
            .is_some())
    }

    /// Whether a `series` row with this id still exists AND still identifies
    /// the same anime it did when an async link/scrape operation started.
    ///
    /// Exists because SQLite's `INTEGER PRIMARY KEY` without `AUTOINCREMENT`
    /// reuses rowids from recently deleted rows. When a swipe decision
    /// creates a stub row X and kicks off a background link scrape, the user
    /// can undo the swipe (deleting row X) and swipe a completely different
    /// card (which gets rowid X again!). When the first scrape finishes, a bare
    /// `series_exists(X)` would pass and overwrite the new anime's metadata and
    /// episodes with the old anime's scrape.
    ///
    /// Callers must check this under the `db` lock right before writing.
    /// Identity check:
    /// - If `anilist_id` was `Some(aid)`, the row at `series_id` must have `anilist_id == Some(aid)`.
    /// - If `anilist_id` was `None`, the row at `series_id` must have `slug == expected_slug` (it may have been linked to AniList in the meantime).
    pub fn series_still_is(&self, series_id: i64, anilist_id: Option<i64>, slug: &str) -> Result<bool> {
        let row: Option<(Option<i64>, String)> = self
            .conn
            .query_row(
                "SELECT anilist_id, slug FROM series WHERE id=?1",
                [series_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let Some((row_anilist_id, row_slug)) = row else {
            return Ok(false);
        };
        match anilist_id {
            Some(expected_aid) => Ok(row_anilist_id == Some(expected_aid)),
            // (source_id, slug) is unique, so the slug alone identifies the row; the row may
            // legitimately have gained an anilist_id since (catalog linking) without being replaced.
            None => Ok(row_slug == slug),
        }
    }

    /// Hard-delete a series and everything referencing it (episodes,
    /// genres). No `ON DELETE CASCADE` is declared on the schema, so this
    /// deletes children first. Safe to call on any series — used both by
    /// swipe undo (fresh row, nothing else references it yet) and "Eliminar
    /// del todo" on discarded backlog rows (which never have episodes).
    pub fn delete_series(&self, series_id: i64) -> Result<()> {
        self.conn.execute("DELETE FROM episodes WHERE series_id=?1", [series_id])?;
        self.conn.execute("DELETE FROM series_genres WHERE series_id=?1", [series_id])?;
        self.conn.execute("DELETE FROM series WHERE id=?1", [series_id])?;
        Ok(())
    }

    /// Delete the synthetic `anilist-N` placeholder a cross-site import created
    /// when that import then failed to find the show on this site, and report
    /// whether anything was deleted.
    ///
    /// `run_library_import` writes the placeholder (plus `set_anilist_id`)
    /// *before* attempting the site link. On a `NoMatch`, a scrape error, or
    /// mirrors being down, the row survived: unreachable (its `url` points at
    /// anilist.co, and there is nothing to scrape there), invisible in the UI,
    /// and — because `library_entries_missing_on_site` matched it by
    /// `anilist_id` — proof to every later import that the entry was "already
    /// present on this site". One transient failure therefore blocked that
    /// series from ever being retried on that site again, while the dead rows
    /// piled up.
    ///
    /// Deliberately conservative, so the error path can call it blindly: only a
    /// row that is still an untouched placeholder goes — slug still
    /// `anilist-%`, no episodes, and none of the user-facing signals
    /// (`followed` / `watched_externally` / `backlog_status`) set. A successful
    /// link rewrites the slug (`relink_series`) or merges the row away
    /// (`merge_series_into`), and a catalog "want"/"Ya lo vi" row carries a
    /// signal — none of those can be hit by this.
    pub fn delete_orphan_synthetic_series(&self, series_id: i64) -> Result<bool> {
        let is_orphan: bool = self
            .conn
            .query_row(
                "SELECT 1 FROM series s
                  WHERE s.id=?1
                    AND s.slug LIKE 'anilist-%'
                    AND s.followed=0
                    AND COALESCE(s.watched_externally, 0)=0
                    AND s.backlog_status IS NULL
                    AND NOT EXISTS (SELECT 1 FROM episodes e WHERE e.series_id = s.id)",
                [series_id],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if !is_orphan {
            return Ok(false);
        }
        self.delete_series(series_id)?;
        Ok(true)
    }

    /// One-off repair of the damage `list_followed` used to do: a row still on
    /// its synthetic `anilist-<id>` slug was scraped as if the site had a page
    /// for it, so it holds other shows' episodes and a rewritten site url. A
    /// synthetic row never has real episodes of its own (linking rewrites the
    /// slug first), so they are all removed and the url goes back to
    /// `https://anilist.co/anime/<id>`.
    ///
    /// Deletes rows, so it first writes a consistent copy of the database to
    /// `backup_path` (when given) and aborts if that fails. Guarded by a
    /// settings key so it runs once; a database with nothing to repair just
    /// sets the key. Returns the number of episode rows removed.
    pub fn repair_scraped_synthetic_rows(&self, backup_path: Option<&str>) -> Result<usize> {
        const KEY: &str = "repair:synthetic_rows_v1";
        if self.get_setting(KEY)?.is_some() {
            return Ok(0);
        }
        let dirty: i64 = self.conn.query_row(
            "SELECT (SELECT COUNT(*) FROM episodes e JOIN series s ON s.id=e.series_id
                      WHERE s.slug LIKE 'anilist-%')
                  + (SELECT COUNT(*) FROM series
                      WHERE slug LIKE 'anilist-%' AND anilist_id IS NOT NULL
                        AND url NOT LIKE 'https://anilist.co/%')",
            [],
            |r| r.get(0),
        )?;
        if dirty == 0 {
            self.set_setting(KEY, "1")?;
            return Ok(0);
        }
        if let Some(path) = backup_path {
            self.snapshot_to(path)?;
        }
        let tx = self.conn.unchecked_transaction()?;
        let removed = self.conn.execute(
            "DELETE FROM episodes WHERE series_id IN (SELECT id FROM series WHERE slug LIKE 'anilist-%')",
            [],
        )?;
        self.conn.execute(
            "UPDATE series SET url='https://anilist.co/anime/' || anilist_id
             WHERE slug LIKE 'anilist-%' AND anilist_id IS NOT NULL AND url NOT LIKE 'https://anilist.co/%'",
            [],
        )?;
        self.set_setting(KEY, "1")?;
        tx.commit()?;
        Ok(removed)
    }

    /// Every caller scrapes the series page, so a row still on its synthetic
    /// `anilist-<id>` slug (its url is an anilist.co page, no site has it) is
    /// left out: fetching `/anime/<id>` on the active site returned a feed of
    /// other shows on animeyt.cc and filled the row with their episodes.
    pub fn list_followed(&self, source_id: i64) -> Result<Vec<crate::models::Series>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, slug, title, url, cover_url, is_airing, followed, next_episode_at, site_episode_count
             FROM series WHERE source_id=?1 AND followed=1 AND slug NOT LIKE 'anilist-%' ORDER BY title",
        )?;
        let rows = stmt
            .query_map([source_id], Self::row_to_series)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Followed series on `source_id` with NO episode rows at all yet —
    /// series that were carried over onto this site (`carry_follow`) or
    /// followed here directly, but whose episode list has never actually
    /// been fetched. Only `refresh()`'s per-series loop does that fetch, and
    /// (before the site-switch fix that calls this) `refresh()` only runs
    /// for whichever site is active at app startup or on a manual
    /// "Actualizar" — a site that only becomes active mid-session never got
    /// a chance, leaving its followed shows permanently at 0 episodes /
    /// 0 pending. See the pre-launch site-consistency investigation.
    /// Synthetic `anilist-%` rows are excluded for the same reason as `list_followed`.
    pub fn followed_series_without_episodes(&self, source_id: i64) -> Result<Vec<crate::models::Series>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, slug, title, url, cover_url, is_airing, followed, next_episode_at, site_episode_count
             FROM series s
             WHERE s.source_id=?1 AND s.followed=1 AND s.slug NOT LIKE 'anilist-%'
               AND NOT EXISTS (SELECT 1 FROM episodes e WHERE e.series_id=s.id)
             ORDER BY title",
        )?;
        let rows = stmt
            .query_map([source_id], Self::row_to_series)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// The subset of a `series` row's fields the swipe-history strip needs
    /// to render one entry (`commands::list_swipe_history`): title, poster,
    /// url (so the frontend can clear it from its decided-set on undo /
    /// return-to-deck), and the two decision signals a catalog-swipe row can
    /// carry (`backlog_status`, `watched_externally` — catalog rows are
    /// never `followed` by `decide_catalog_card`, so that flag isn't needed
    /// here). `None` when the row no longer exists (already deleted by an earlier
    /// undo/return-to-deck) — the caller skips it rather than erroring, so
    /// the history strip self-heals.
    pub fn get_series_for_history(&self, series_id: i64) -> Result<Option<SwipeHistoryRow>> {
        Ok(self
            .conn
            .query_row(
                "SELECT title, cover_url, backlog_status, watched_externally, url FROM series WHERE id=?1",
                [series_id],
                |r| {
                    Ok(SwipeHistoryRow {
                        title: r.get(0)?,
                        poster_url: r.get(1)?,
                        backlog_status: r.get(2)?,
                        watched_externally: r.get::<_, i64>(3)? != 0,
                        url: r.get(4)?,
                    })
                },
            )
            .ok())
    }

    /// Extended metadata for list entries (score, format, status, catalog cover, genres, etc.)
    /// joined with `anilist_catalog` when linked.
    pub fn list_entry_meta(&self, series_ids: &[i64]) -> Result<Vec<crate::models::ListEntryMeta>> {
        if series_ids.is_empty() {
            return Ok(Vec::new());
        }

        let mut results = Vec::with_capacity(series_ids.len());

        for chunk in series_ids.chunks(500) {
            let placeholders = vec!["?"; chunk.len()].join(",");
            let sql = format!(
                "SELECT s.id, s.anilist_id, c.average_score, c.format, c.status,
                        c.episodes, c.start_date, c.studio, c.popularity, c.cover_url
                 FROM series s
                 LEFT JOIN anilist_catalog c ON s.anilist_id = c.id
                 WHERE s.id IN ({placeholders})"
            );
            let mut stmt = self.conn.prepare(&sql)?;
            let params = rusqlite::params_from_iter(chunk.iter());

            struct RowData {
                series_id: i64,
                anilist_id: Option<i64>,
                score: Option<i64>,
                format: Option<String>,
                status: Option<String>,
                episodes: Option<i64>,
                start_date: Option<i64>,
                studio: Option<String>,
                popularity: Option<i64>,
                cover_url: Option<String>,
            }

            let rows = stmt
                .query_map(params, |r| {
                    Ok(RowData {
                        series_id: r.get(0)?,
                        anilist_id: r.get(1)?,
                        score: r.get(2)?,
                        format: r.get(3)?,
                        status: r.get(4)?,
                        episodes: r.get(5)?,
                        start_date: r.get(6)?,
                        studio: r.get(7)?,
                        popularity: r.get(8)?,
                        cover_url: r.get(9)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;

            let mut linked_anilist_ids: Vec<i64> = Vec::new();
            let mut unlinked_series_ids: Vec<i64> = Vec::new();

            for row in &rows {
                if let Some(aid) = row.anilist_id {
                    linked_anilist_ids.push(aid);
                } else {
                    unlinked_series_ids.push(row.series_id);
                }
            }

            linked_anilist_ids.sort_unstable();
            linked_anilist_ids.dedup();

            let mut catalog_genres: std::collections::HashMap<i64, Vec<String>> =
                std::collections::HashMap::new();
            if !linked_anilist_ids.is_empty() {
                let aid_placeholders = vec!["?"; linked_anilist_ids.len()].join(",");
                let sql = format!(
                    "SELECT anilist_id, genre FROM anilist_catalog_genres WHERE anilist_id IN ({aid_placeholders}) ORDER BY genre"
                );
                let mut gstmt = self.conn.prepare(&sql)?;
                let gparams = rusqlite::params_from_iter(linked_anilist_ids.iter());
                let grows = gstmt
                    .query_map(gparams, |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                for (aid, genre) in grows {
                    catalog_genres.entry(aid).or_default().push(genre);
                }
            }

            let mut fallback_series_ids = unlinked_series_ids;
            for row in &rows {
                if let Some(aid) = row.anilist_id {
                    if !catalog_genres.contains_key(&aid) {
                        fallback_series_ids.push(row.series_id);
                    }
                }
            }
            fallback_series_ids.sort_unstable();
            fallback_series_ids.dedup();

            let mut site_genres: std::collections::HashMap<i64, Vec<String>> =
                std::collections::HashMap::new();
            if !fallback_series_ids.is_empty() {
                let sid_placeholders = vec!["?"; fallback_series_ids.len()].join(",");
                let sql = format!(
                    "SELECT series_id, genre FROM series_genres WHERE series_id IN ({sid_placeholders}) ORDER BY genre"
                );
                let mut sstmt = self.conn.prepare(&sql)?;
                let sparams = rusqlite::params_from_iter(fallback_series_ids.iter());
                let srows = sstmt
                    .query_map(sparams, |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                for (sid, genre) in srows {
                    site_genres.entry(sid).or_default().push(genre);
                }
            }

            let mut by_id: std::collections::HashMap<i64, crate::models::ListEntryMeta> =
                std::collections::HashMap::with_capacity(rows.len());

            for row in rows {
                let genres = if let Some(aid) = row.anilist_id {
                    if let Some(g) = catalog_genres.get(&aid) {
                        g.clone()
                    } else {
                        site_genres.get(&row.series_id).cloned().unwrap_or_default()
                    }
                } else {
                    site_genres.get(&row.series_id).cloned().unwrap_or_default()
                };

                by_id.insert(
                    row.series_id,
                    crate::models::ListEntryMeta {
                        series_id: row.series_id,
                        anilist_id: row.anilist_id,
                        score: row.score,
                        format: row.format,
                        status: row.status,
                        episodes: row.episodes,
                        start_date: row.start_date,
                        studio: row.studio,
                        popularity: row.popularity,
                        genres,
                        cover_url: row.cover_url,
                    },
                );
            }

            for &id in chunk {
                if let Some(meta) = by_id.remove(&id) {
                    results.push(meta);
                }
            }
        }

        Ok(results)
    }
}


#[cfg(test)]
mod tests;
