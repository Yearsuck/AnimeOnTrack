use super::*;

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

impl Db {
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
            // `normalize_title`'s Unicode-aware folding).
            let norm = crate::matching::normalize_title(&s.title);
            let mut stmt = self.conn.prepare(
                "SELECT id, title FROM series WHERE source_id=?1 AND slug NOT LIKE 'anilist-%'",
            )?;
            let mut rows = stmt.query([source_id])?;
            let mut found = None;
            while let Some(row) = rows.next()? {
                let id: i64 = row.get(0)?;
                let title: String = row.get(1)?;
                if crate::matching::normalize_title(&title) == norm {
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
                    "INSERT INTO series(source_id, slug, title, url, cover_url, is_airing, next_episode_at, site_episode_count)
                     VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                    (
                        source_id, &s.slug, &s.title, &s.url,
                        &s.cover_url, s.is_airing as i64,
                        s.next_episode_at, s.site_episode_count,
                    ),
                )?;
                return Ok(self.conn.last_insert_rowid());
            }
        };

        self.conn.execute(
            "UPDATE series SET slug=?2, title=?3, url=?4, cover_url=?5, is_airing=?6,
                next_episode_at=?7, site_episode_count=?8
             WHERE id=?1",
            (
                target_id, &s.slug, &s.title, &s.url,
                &s.cover_url, s.is_airing as i64,
                s.next_episode_at, s.site_episode_count,
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
    pub fn merge_series_into(&self, existing_id: i64, synthetic_id: i64) -> Result<()> {
        let (followed, backlog_status, watched_externally): (i64, Option<String>, i64) = self.conn.query_row(
            "SELECT followed, backlog_status, watched_externally FROM series WHERE id=?1",
            [synthetic_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?;
        self.conn.execute(
            "UPDATE series SET
                followed = followed OR ?1,
                watched_externally = watched_externally OR ?2,
                backlog_status = COALESCE(backlog_status, ?3)
             WHERE id=?4",
            (followed, watched_externally, backlog_status, existing_id),
        )?;
        self.delete_series(synthetic_id)
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
            "UPDATE series SET slug=?1, url=?2, cover_url=?3, kind=?4 WHERE id=?5",
            (slug, url, cover_url, kind, series_id),
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
        if let Some(aid) = anilist_id {
            // Fast path: everything sharing the AniList id is the same show.
            let n = self
                .conn
                .execute("UPDATE series SET followed=?1 WHERE anilist_id=?2", (f, aid))?;
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
            changed += self
                .conn
                .execute("UPDATE series SET followed=?1 WHERE id=?2", (f, id))?;
        }
        Ok(changed)
    }

    /// Every followed series on a source OTHER than `exclude_source_id`, with
    /// its title and the highest *seen* episode number (0 when nothing's been
    /// watched). Non-numeric / recap episode numbers never inflate the
    /// watermark (`CAST(number AS INTEGER)` parses leading digits, "Recap"→0).
    /// Powers cross-site follow carry-over: matched by title against the newly
    /// scanned site's airing list. See the carry-over design doc.
    pub(crate) fn followed_titles_with_watermark(
        &self,
        exclude_source_id: i64,
    ) -> Result<Vec<(String, i64)>> {
        let mut stmt = self.conn.prepare(
            "SELECT s.title,
                    COALESCE(MAX(CASE WHEN e.seen=1 THEN CAST(e.number AS INTEGER) END), 0) AS watermark
             FROM series s
             LEFT JOIN episodes e ON e.series_id = s.id
             WHERE s.followed=1 AND s.source_id <> ?1
             GROUP BY s.id
             ORDER BY s.title",
        )?;
        let rows = stmt
            .query_map([exclude_source_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Carry a follow onto a new-site series row: set it followed and stash
    /// the watermark for `refresh()` to apply once. Guarded to `followed=0` so
    /// it never overrides an existing follow (or its real progress) — a series
    /// the user already follows on this site is left completely untouched.
    /// Returns true if a row was actually carried.
    pub fn carry_follow(&self, series_id: i64, watermark: i64) -> Result<bool> {
        let n = self.conn.execute(
            "UPDATE series SET followed=1, carried_seen_number=?2
             WHERE id=?1 AND followed=0",
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

    /// Replace a series' cover with a fetched base64 data URI.
    pub fn update_series_cover(&self, series_id: i64, cover_url: &str) -> Result<()> {
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
        self.conn
            .execute("UPDATE series SET anilist_id=?1 WHERE id=?2", (anilist_id, series_id))?;
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
            .prepare("SELECT genre FROM series_genres WHERE series_id=?1 ORDER BY genre")?;
        let rows = stmt
            .query_map([series_id], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// `true` if this series has no `series_genres` rows yet (needs a
    /// detail-page fetch to backfill). Split out as a plain sync check so the
    /// async fetch decision can be unit-tested without a scraper/AppHandle.
    pub fn series_needs_genre_backfill(&self, series_id: i64) -> Result<bool> {
        Ok(self.list_series_genres(series_id)?.is_empty())
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
        // and pick the active-site representative. cover falls back to the
        // catalog poster when the site row has none.
        let mut stmt = self.conn.prepare(
            "SELECT s.id, s.source_id, s.anilist_id, s.slug, s.title, s.url,
                    COALESCE(s.cover_url, c.cover_url) AS cover_url,
                    s.is_airing, s.followed, s.next_episode_at, s.site_episode_count,
                    s.watched_externally, s.kind,
                    COUNT(e.id) AS total,
                    SUM(CASE WHEN e.seen=1 THEN 1 ELSE 0 END) AS seen,
                    MAX(e.added_at) AS last_added,
                    MAX(e.seen_at) AS last_watched_at,
                    c.studio AS studio
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
        }
        let mut reps: Vec<Rep> = Vec::new();
        for key in &order {
            let mut group = groups.remove(key).unwrap();
            let total_episodes = group.iter().map(|m| m.total).max().unwrap_or(0);
            let seen_episodes = group.iter().map(|m| m.seen).max().unwrap_or(0);
            let watched_externally = group.iter().any(|m| m.watched_externally);
            let last_added = group.iter().filter_map(|m| m.last_added.clone()).max();
            let last_watched_at = group.iter().filter_map(|m| m.last_watched_at.clone()).max();
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
            reps.push(Rep { member, total_episodes, seen_episodes, watched_externally, last_added, last_watched_at });
        }

        // Bulk-fetch genres for the representative rows only (one statement, not
        // an N+1 per-series query — see the library-filters design doc).
        let ids: Vec<i64> = reps.iter().map(|r| r.member.series.id).collect();
        let mut genres_by_series: std::collections::HashMap<i64, Vec<String>> =
            std::collections::HashMap::new();
        if !ids.is_empty() {
            let placeholders = vec!["?"; ids.len()].join(",");
            let sql = format!(
                "SELECT series_id, genre FROM series_genres WHERE series_id IN ({}) ORDER BY genre",
                placeholders
            );
            let mut gstmt = self.conn.prepare(&sql)?;
            let params = rusqlite::params_from_iter(ids.iter());
            let grows = gstmt
                .query_map(params, |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            for (series_id, genre) in grows {
                genres_by_series.entry(series_id).or_default().push(genre);
            }
        }

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
            });
        }
        out.sort_by_key(|item| item.series.title.to_lowercase());
        Ok(out)
    }

    /// Series with the given `backlog_status` ('want' or 'discarded'), for
    /// the swipe mode's "Listas" sub-view.
    pub fn list_backlog(&self, source_id: i64, status: &str) -> Result<Vec<crate::models::Series>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, slug, title, url, cover_url, is_airing, followed, next_episode_at, site_episode_count
             FROM series WHERE source_id=?1 AND backlog_status=?2 ORDER BY title",
        )?;
        let rows = stmt
            .query_map((source_id, status), Self::row_to_series)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Series with `watched_externally=1` ("Ya lo vi" catalog swipe), for
    /// the Listas view's "Ya vistas" sub-list — mirrors `list_backlog`.
    pub fn list_watched_externally(&self, source_id: i64) -> Result<Vec<crate::models::Series>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, slug, title, url, cover_url, is_airing, followed, next_episode_at, site_episode_count
             FROM series WHERE source_id=?1 AND watched_externally=1 ORDER BY title",
        )?;
        let rows = stmt
            .query_map([source_id], Self::row_to_series)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
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

    pub fn list_followed(&self, source_id: i64) -> Result<Vec<crate::models::Series>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, slug, title, url, cover_url, is_airing, followed, next_episode_at, site_episode_count
             FROM series WHERE source_id=?1 AND followed=1 ORDER BY title",
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
    pub fn followed_series_without_episodes(&self, source_id: i64) -> Result<Vec<crate::models::Series>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, slug, title, url, cover_url, is_airing, followed, next_episode_at, site_episode_count
             FROM series s
             WHERE s.source_id=?1 AND s.followed=1
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upsert_series_writes_and_updates_scan_owned_airing_metadata() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
        let mut s = mk_airing("x", "X", Some(1_783_350_140));
        s.site_episode_count = Some(2);
        let sid = db.upsert_series(src, &s).unwrap();
        let got = db.list_airing(src).unwrap().into_iter().find(|r| r.id == sid).unwrap();
        assert_eq!(got.next_episode_at, Some(1_783_350_140));
        assert_eq!(got.site_episode_count, Some(2));

        // A re-scan with fresh values overwrites both (scan-owned fields).
        let mut s2 = mk_airing("x", "X", Some(1_783_954_940));
        s2.site_episode_count = Some(3);
        db.upsert_series(src, &s2).unwrap();
        let got = db.list_airing(src).unwrap().into_iter().find(|r| r.id == sid).unwrap();
        assert_eq!(got.next_episode_at, Some(1_783_954_940));
        assert_eq!(got.site_episode_count, Some(3));
    }

    #[test]
    fn upsert_series_slug_change_updates_existing_row_and_keeps_seen_history() {
        // Reproduces the "Youjo Senki II" bug: the site restructures a
        // series' slug between scans. A slug-keyed upsert would treat that
        // as a brand new series, orphaning the seen episodes under the old
        // id. Matching on normalized title within the same source instead
        // should update the existing row in place.
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
        let sid = db.upsert_series(src, &mk_airing("youjo-senki-2", "Youjo Senki II", None)).unwrap();
        insert_eps_seen_up_to(&db, sid, 5, 5);

        // Site moves the slug (e.g. "-2" -> "-ii") on a later scan; title
        // (once normalized) is unchanged.
        let sid2 = db.upsert_series(src, &mk_airing("youjo-senki-ii", "Youjo Senki II", None)).unwrap();

        assert_eq!(sid, sid2, "slug drift on the same source/title must reuse the existing series id");
        let eps = db.list_series_episodes(sid).unwrap();
        assert_eq!(eps.len(), 5, "seen episodes must still be reachable under the same series id");
        assert!(eps.iter().all(|e| e.seen), "seen state must survive the slug change");
    }

    #[test]
    fn upsert_series_different_title_creates_new_row() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
        let sid = db.upsert_series(src, &mk_airing("one-piece", "One Piece", None)).unwrap();
        let sid2 = db.upsert_series(src, &mk_airing("naruto", "Naruto", None)).unwrap();
        assert_ne!(sid, sid2, "genuinely different series must not be merged");
    }

    #[test]
    fn carry_follow_only_touches_unfollowed_rows() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("A", "a", "animeytx").unwrap();

        let sid = db.upsert_series(src, &mk_airing("s", "S", None)).unwrap();
        assert!(db.carry_follow(sid, 4).unwrap(), "unfollowed row carries");
        assert_eq!(db.take_carried_seen_number(sid).unwrap(), Some(4));
        // Applied once: a second take yields None.
        assert_eq!(db.take_carried_seen_number(sid).unwrap(), None);

        // Already-followed row: carry is a no-op and leaves no watermark.
        let sid2 = db.upsert_series(src, &mk_airing("t", "T", None)).unwrap();
        db.set_followed(sid2, true).unwrap();
        assert!(!db.carry_follow(sid2, 9).unwrap(), "followed row is left untouched");
        assert_eq!(db.take_carried_seen_number(sid2).unwrap(), None);
    }

    #[test]
    fn carried_watermark_marks_episodes_seen_via_cascade() {
        // Simulates refresh()'s apply-once step: carry a follow, later fetch
        // the new site's episodes, then apply the stored watermark.
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("B", "b", "tioanime").unwrap();
        let sid = db.upsert_series(src, &mk_airing("x", "X", None)).unwrap();

        db.carry_follow(sid, 3).unwrap();
        // Episodes arrive on the new site (all unseen initially).
        insert_eps_seen_up_to(&db, sid, 6, 0);

        // refresh() step: read+clear the watermark, cascade-mark up to it.
        let n = db.take_carried_seen_number(sid).unwrap().expect("watermark present");
        db.set_seen_cascade(sid, &n.to_string(), true).unwrap();

        let eps = db.list_series_episodes(sid).unwrap();
        let seen: Vec<&str> = eps.iter().filter(|e| e.seen).map(|e| e.number.as_str()).collect();
        assert_eq!(seen, vec!["1", "2", "3"], "episodes 1..=3 seen, 4..=6 unseen");
        // Applied once — column cleared.
        assert_eq!(db.take_carried_seen_number(sid).unwrap(), None);
    }

    #[test]
    fn unfollowed_watched_externally_series_with_real_seen_episodes_credits_the_remainder() {
        // followed=0, watched_externally=1, 3 real seen episodes, ALSO linked
        // to a catalog row with 12 episodes. "Ya lo vi" means the whole show
        // was watched, so all 12 are credited — but the 3 real marks are not
        // counted twice: they stay in episodes_watched and only the 9-episode
        // remainder lands in episodes_watched_external.
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("A", "a", "animeytx").unwrap();

        db.upsert_catalog_anime(
            &crate::anilist::CatalogAnime {
                id: 300, title: "Ext".into(), title_romaji: None, title_english: None,
                cover_url: None, format: Some("TV".into()), genres: vec![],
                episodes: Some(12), average_score: None, popularity: None,
                url: "https://anilist.co/anime/300".into(), status: None, duration: None,
                studio: None, start_date: None,
            },
            0,
        ).unwrap();
        let sid = db.upsert_series(src, &mk_airing("ext1", "Ext", None)).unwrap();
        db.set_watched_externally(sid, true).unwrap();
        db.set_anilist_id(sid, 300).unwrap();
        db.set_kind(sid, "TV").unwrap();
        for n in 1..=3 {
            db.insert_episode(&crate::models::Episode {
                id: 0, series_id: sid, number: n.to_string(), title: None,
                url: format!("https://site/anime/ext1-capitulo-{n}/"),
                released_at: None, seen: true,
            }).unwrap();
        }

        let summary = db.get_watch_summary().unwrap();
        assert_eq!(summary.episodes_watched, 3, "the 3 real seen episodes count, even though the series isn't followed");
        assert_eq!(summary.episodes_watched_external, 9, "12 catalog episodes minus the 3 already marked");
        assert_eq!(
            summary.episodes_watched + summary.episodes_watched_external, 12,
            "the two figures add up to the catalog total, never past it"
        );

        let insights = db.get_watch_insights().unwrap();
        assert_eq!(insights.estimated_minutes_tracked, 72, "3 seen eps * 24 min (TV), tracked minutes aren't followed-only anymore");
        assert_eq!(insights.estimated_minutes_external, 9 * 24, "remainder minutes only");
    }

    #[test]
    fn watched_externally_series_without_episodes_counts_via_catalog_estimate() {
        // watched_externally=1, no scraped episodes at all: falls back to the
        // catalog's episode count for both the episode count and the minutes.
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("A", "a", "animeytx").unwrap();

        db.upsert_catalog_anime(
            &crate::anilist::CatalogAnime {
                id: 301, title: "Ext2".into(), title_romaji: None, title_english: None,
                cover_url: None, format: Some("TV".into()), genres: vec![],
                episodes: Some(12), average_score: None, popularity: None,
                url: "https://anilist.co/anime/301".into(), status: None, duration: None,
                studio: None, start_date: None,
            },
            0,
        ).unwrap();
        let sid = db.upsert_series(src, &mk_airing("ext2", "Ext2", None)).unwrap();
        db.set_watched_externally(sid, true).unwrap();
        db.set_anilist_id(sid, 301).unwrap();

        let summary = db.get_watch_summary().unwrap();
        assert_eq!(summary.episodes_watched, 0, "no real episodes for this series");
        assert_eq!(summary.episodes_watched_external, 12, "falls back to the catalog's episode count");

        let insights = db.get_watch_insights().unwrap();
        assert_eq!(insights.estimated_minutes_external, 12 * 24, "12 catalog eps * minutes_per_episode(TV)");
    }

    #[test]
    fn followed_series_with_no_seen_episodes_excluded_from_distinct_anime() {
        // followed=1 with zero seen episodes must NOT count toward
        // distinct_anime — it has no watch evidence. A watched-externally
        // series (with seen evidence) still counts.
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("A", "a", "animeytx").unwrap();

        // Followed, zero episodes seen — must be excluded.
        let unseen_id = db.upsert_series(src, &mk_airing("unseen1", "Unseen Show", None)).unwrap();
        db.set_followed(unseen_id, true).unwrap();
        db.insert_episode(&crate::models::Episode {
            id: 0, series_id: unseen_id, number: "1".into(), title: None,
            url: "https://site/anime/unseen1-capitulo-1/".into(),
            released_at: None, seen: false,
        }).unwrap();

        // Watched-externally, no scraped episodes — has watch evidence via
        // the flag itself, must count.
        let ext_id = db.upsert_series(src, &mk_airing("ext3", "Ext3", None)).unwrap();
        db.set_watched_externally(ext_id, true).unwrap();

        let summary = db.get_watch_summary().unwrap();
        assert_eq!(summary.distinct_anime, 1, "only the watched-externally show counts; the unwatched followed show is excluded");
    }

    #[test]
    fn followed_and_watched_externally_series_with_seen_episodes_counts_each_episode_once() {
        // followed=1 AND watched_externally=1, linked to a catalog row, with
        // real seen episodes: every episode contributes exactly once. The 3
        // real ones go to the tracked side and only the remaining 9 of the
        // catalog's 12 to the external side — never 12 on top of 3.
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("A", "a", "animeytx").unwrap();

        db.upsert_catalog_anime(
            &crate::anilist::CatalogAnime {
                id: 302, title: "Both".into(), title_romaji: None, title_english: None,
                cover_url: None, format: Some("TV".into()), genres: vec![],
                episodes: Some(12), average_score: None, popularity: None,
                url: "https://anilist.co/anime/302".into(), status: None, duration: None,
                studio: None, start_date: None,
            },
            0,
        ).unwrap();
        let sid = db.upsert_series(src, &mk_airing("both1", "Both", None)).unwrap();
        db.set_followed(sid, true).unwrap();
        db.set_watched_externally(sid, true).unwrap();
        db.set_anilist_id(sid, 302).unwrap();
        db.set_kind(sid, "TV").unwrap();
        for n in 1..=3 {
            db.insert_episode(&crate::models::Episode {
                id: 0, series_id: sid, number: n.to_string(), title: None,
                url: format!("https://site/anime/both1-capitulo-{n}/"),
                released_at: None, seen: true,
            }).unwrap();
        }

        let insights = db.get_watch_insights().unwrap();
        assert_eq!(insights.estimated_minutes_tracked, 72, "3 seen eps * 24 min (TV)");
        assert_eq!(insights.estimated_minutes_external, 9 * 24, "the other 9 catalog eps, counted once");

        // Criterion 4: episodes and hours cover the exact same universe —
        // episodes_watched + episodes_watched_external matches what the
        // minutes were computed from (3 real eps + 9 remainder eps).
        let summary = db.get_watch_summary().unwrap();
        assert_eq!(summary.episodes_watched, 3);
        assert_eq!(summary.episodes_watched_external, 9);
        assert_eq!(
            (summary.episodes_watched + summary.episodes_watched_external) * 24,
            insights.estimated_minutes_tracked + insights.estimated_minutes_external
        );
    }

    #[test]
    fn series_genres_insert_is_idempotent_and_lists_sorted() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
        let s = crate::models::Series {
            id: 0, slug: "x".into(), title: "X".into(),
            url: "u".into(), cover_url: None, is_airing: false, followed: false, next_episode_at: None, site_episode_count: None,
        };
        let sid = db.upsert_series(src, &s).unwrap();

        db.insert_series_genres(sid, &["Seinen".to_string(), "Drama".to_string()]).unwrap();
        // inserting the same genres again must not error or duplicate
        db.insert_series_genres(sid, &["Drama".to_string()]).unwrap();

        let genres = db.list_series_genres(sid).unwrap();
        assert_eq!(genres, vec!["Drama".to_string(), "Seinen".to_string()]);
    }

    #[test]
    fn backlog_status_and_kind_round_trip() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
        let s = crate::models::Series {
            id: 0, slug: "x".into(), title: "X".into(),
            url: "u".into(), cover_url: None, is_airing: false, followed: false, next_episode_at: None, site_episode_count: None,
        };
        let sid = db.upsert_series(src, &s).unwrap();

        assert_eq!(db.get_backlog_status(sid).unwrap(), None);
        db.set_backlog_status(sid, Some("want")).unwrap();
        assert_eq!(db.get_backlog_status(sid).unwrap(), Some("want".to_string()));
        db.set_kind(sid, "TV").unwrap();

        // start_watching's transition: 'want' -> followed, backlog_status cleared
        db.set_followed(sid, true).unwrap();
        db.set_backlog_status(sid, None).unwrap();
        assert_eq!(db.get_backlog_status(sid).unwrap(), None);
        assert!(db.list_followed(src).unwrap().iter().any(|f| f.id == sid));
    }

    #[test]
    fn list_backlog_filters_by_status() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
        let want = crate::models::Series {
            id: 0, slug: "want".into(), title: "Want".into(),
            url: "u1".into(), cover_url: None, is_airing: false, followed: false, next_episode_at: None, site_episode_count: None,
        };
        let sid_want = db.upsert_series(src, &want).unwrap();
        db.set_backlog_status(sid_want, Some("want")).unwrap();

        let discarded = crate::models::Series {
            id: 0, slug: "disc".into(), title: "Disc".into(),
            url: "u2".into(), cover_url: None, is_airing: false, followed: false, next_episode_at: None, site_episode_count: None,
        };
        let sid_disc = db.upsert_series(src, &discarded).unwrap();
        db.set_backlog_status(sid_disc, Some("discarded")).unwrap();

        let wants = db.list_backlog(src, "want").unwrap();
        assert_eq!(wants.len(), 1);
        assert_eq!(wants[0].id, sid_want);

        let discards = db.list_backlog(src, "discarded").unwrap();
        assert_eq!(discards.len(), 1);
        assert_eq!(discards[0].id, sid_disc);
    }

    #[test]
    fn list_watched_externally_filters_by_flag() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();

        let watched = crate::models::Series {
            id: 0, slug: "watched".into(), title: "Watched".into(),
            url: "u1".into(), cover_url: None, is_airing: false, followed: false, next_episode_at: None, site_episode_count: None,
        };
        let sid_watched = db.upsert_series(src, &watched).unwrap();
        db.set_watched_externally(sid_watched, true).unwrap();

        let other = crate::models::Series {
            id: 0, slug: "other".into(), title: "Other".into(),
            url: "u2".into(), cover_url: None, is_airing: false, followed: false, next_episode_at: None, site_episode_count: None,
        };
        let sid_other = db.upsert_series(src, &other).unwrap();
        db.set_backlog_status(sid_other, Some("want")).unwrap();

        let watched_rows = db.list_watched_externally(src).unwrap();
        assert_eq!(watched_rows.len(), 1);
        assert_eq!(watched_rows[0].id, sid_watched);
    }

    #[test]
    fn list_library_includes_watched_externally_rows_with_no_episodes() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();

        // Followed series with episodes — the existing, already-working path.
        let followed = crate::models::Series {
            id: 0, slug: "followed".into(), title: "Followed".into(),
            url: "u1".into(), cover_url: None, is_airing: true, followed: false, next_episode_at: None, site_episode_count: None,
        };
        let sid_followed = db.upsert_series(src, &followed).unwrap();
        db.set_followed(sid_followed, true).unwrap();
        db.insert_episode(&crate::models::Episode {
            id: 0, series_id: sid_followed, number: "1".into(), title: None,
            url: "ep1".into(), released_at: None, seen: true,
        }).unwrap();

        // "Ya lo vi" catalog row: watched_externally=1, followed=0, no episodes.
        let watched = crate::models::Series {
            id: 0, slug: "watched".into(), title: "Watched".into(),
            url: "u2".into(), cover_url: None, is_airing: false, followed: false, next_episode_at: None, site_episode_count: None,
        };
        let sid_watched = db.upsert_series(src, &watched).unwrap();
        db.set_watched_externally(sid_watched, true).unwrap();

        let items = db.list_library(src).unwrap();
        assert_eq!(items.len(), 2, "both the followed and the watched-externally-only rows must be returned");

        let watched_item = items.iter().find(|it| it.series.id == sid_watched)
            .expect("watched_externally=1, followed=0 row must be present in list_library");
        assert!(watched_item.watched_externally);
        assert_eq!(watched_item.total_episodes, 0);

        let followed_item = items.iter().find(|it| it.series.id == sid_followed).unwrap();
        assert!(!followed_item.watched_externally);
        assert_eq!(followed_item.total_episodes, 1);
    }

    #[test]
    fn list_library_returns_kind_and_genres_via_one_bulk_query() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();

        let s1 = crate::models::Series {
            id: 0, slug: "s1".into(), title: "S1".into(),
            url: "u1".into(), cover_url: None, is_airing: false, followed: false, next_episode_at: None, site_episode_count: None,
        };
        let sid1 = db.upsert_series(src, &s1).unwrap();
        db.set_followed(sid1, true).unwrap();
        db.set_kind(sid1, "TV").unwrap();
        db.insert_series_genres(sid1, &["Accion".to_string(), "Comedia".to_string()]).unwrap();

        // A second series with no kind/genres set at all — must come back
        // with kind=None and an empty genres vec, not an error or a missing row.
        let s2 = crate::models::Series {
            id: 0, slug: "s2".into(), title: "S2".into(),
            url: "u2".into(), cover_url: None, is_airing: false, followed: false, next_episode_at: None, site_episode_count: None,
        };
        let sid2 = db.upsert_series(src, &s2).unwrap();
        db.set_followed(sid2, true).unwrap();

        let items = db.list_library(src).unwrap();
        let item1 = items.iter().find(|it| it.series.id == sid1).unwrap();
        assert_eq!(item1.kind.as_deref(), Some("TV"));
        assert_eq!(item1.genres, vec!["Accion".to_string(), "Comedia".to_string()]);

        let item2 = items.iter().find(|it| it.series.id == sid2).unwrap();
        assert_eq!(item2.kind, None);
        assert!(item2.genres.is_empty());
    }

    #[test]
    fn list_library_returns_studio_only_for_linked_series() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();

        // A catalog row carrying a studio, linked to a followed series via
        // anilist_id.
        let mut linked_anime = catalog_anime_with_popularity(500, "Linked", &["Drama"], Some(1000));
        linked_anime.studio = Some("Studio Ghibli".into());
        db.upsert_catalog_anime(&linked_anime, 0).unwrap();
        let linked = crate::models::Series {
            id: 0, slug: "linked".into(), title: "Linked".into(),
            url: "u1".into(), cover_url: None, is_airing: false, followed: false, next_episode_at: None, site_episode_count: None,
        };
        let sid_linked = db.upsert_series(src, &linked).unwrap();
        db.set_followed(sid_linked, true).unwrap();
        db.set_anilist_id(sid_linked, 500).unwrap();

        // An unlinked followed series — must come back with studio: None,
        // not an error or a stray value from another row.
        let unlinked = crate::models::Series {
            id: 0, slug: "unlinked".into(), title: "Unlinked".into(),
            url: "u2".into(), cover_url: None, is_airing: false, followed: false, next_episode_at: None, site_episode_count: None,
        };
        let sid_unlinked = db.upsert_series(src, &unlinked).unwrap();
        db.set_followed(sid_unlinked, true).unwrap();

        let items = db.list_library(src).unwrap();
        let linked_item = items.iter().find(|it| it.series.id == sid_linked).unwrap();
        assert_eq!(linked_item.studio.as_deref(), Some("Studio Ghibli"));

        let unlinked_item = items.iter().find(|it| it.series.id == sid_unlinked).unwrap();
        assert_eq!(unlinked_item.studio, None);
    }

    #[test]
    fn delete_series_cascades_episodes_and_genres() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
        let s = crate::models::Series {
            id: 0, slug: "x".into(), title: "X".into(),
            url: "u".into(), cover_url: None, is_airing: false, followed: true, next_episode_at: None, site_episode_count: None,
        };
        let sid = db.upsert_series(src, &s).unwrap();
        db.insert_series_genres(sid, &["Seinen".to_string()]).unwrap();
        db.insert_episode(&crate::models::Episode {
            id: 0, series_id: sid, number: "1".into(), title: None,
            url: "e1".into(), released_at: None, seen: false,
        }).unwrap();

        db.delete_series(sid).unwrap();

        assert!(db.list_followed(src).unwrap().iter().all(|f| f.id != sid));
        assert!(db.list_series_genres(sid).unwrap().is_empty());
        assert!(db.list_series_episodes(sid).unwrap().is_empty());
    }

    #[test]
    fn get_series_for_link_reads_link_relevant_fields() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
        let s = crate::models::Series {
            id: 0, slug: "anilist-42".into(), title: "X".into(),
            url: "https://anilist.co/anime/42".into(), cover_url: None, is_airing: false, followed: false, next_episode_at: None, site_episode_count: None,
        };
        let sid = db.upsert_series(src, &s).unwrap();
        db.set_anilist_id(sid, 42).unwrap();
        db.set_backlog_status(sid, Some("want")).unwrap();

        let info = db.get_series_for_link(sid).unwrap().unwrap();
        assert_eq!(info.id, sid);
        assert_eq!(info.source_id, src);
        assert_eq!(info.slug, "anilist-42");
        assert_eq!(info.anilist_id, Some(42));
        assert_eq!(info.backlog_status.as_deref(), Some("want"));
        assert!(!info.followed);
        assert!(!info.watched_externally);
    }

    #[test]
    fn get_series_for_link_none_for_missing_row() {
        let db = Db::open(":memory:").unwrap();
        assert!(db.get_series_for_link(999).unwrap().is_none());
    }

    #[test]
    fn already_linked_to_site_covers_both_null_anilist_id_and_relinked_slug() {
        let plain_site_row = SeriesForLink {
            id: 1, source_id: 1, slug: "baki-dou".into(), anilist_id: None,
            followed: true, backlog_status: None, watched_externally: false,
        };
        assert!(plain_site_row.already_linked_to_site());

        let unlinked_catalog_row = SeriesForLink {
            id: 2, source_id: 1, slug: "anilist-42".into(), anilist_id: Some(42),
            followed: false, backlog_status: Some("want".into()), watched_externally: false,
        };
        assert!(!unlinked_catalog_row.already_linked_to_site());

        // The critical case: a catalog row that was already linked in a
        // previous call keeps its `anilist_id` (relink_series never clears
        // it) but its slug is now the real site slug — must still count as
        // already-linked, or a second call (e.g. a race between the Seen
        // swipe's fire-and-forget link and opening SeriesDetail) would
        // re-search and re-scrape.
        let already_relinked_row = SeriesForLink {
            id: 3, source_id: 1, slug: "baki-dou".into(), anilist_id: Some(42),
            followed: false, backlog_status: None, watched_externally: true,
        };
        assert!(already_relinked_row.already_linked_to_site());
    }

    #[test]
    fn find_series_id_by_slug_excludes_given_id_and_scopes_by_source() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
        let real = crate::models::Series {
            id: 0, slug: "baki-dou".into(), title: "Baki-dou".into(),
            url: "u1".into(), cover_url: None, is_airing: true, followed: false, next_episode_at: None, site_episode_count: None,
        };
        let real_id = db.upsert_series(src, &real).unwrap();
        let synthetic = crate::models::Series {
            id: 0, slug: "anilist-7".into(), title: "Baki-dou".into(),
            url: "https://anilist.co/anime/7".into(), cover_url: None, is_airing: false, followed: false, next_episode_at: None, site_episode_count: None,
        };
        let synthetic_id = db.upsert_series(src, &synthetic).unwrap();

        // Excluding the synthetic row's own id, "baki-dou" resolves to the real row.
        assert_eq!(
            db.find_series_id_by_slug(src, "baki-dou", synthetic_id).unwrap(),
            Some(real_id)
        );
        // No collision for a slug nothing owns.
        assert_eq!(db.find_series_id_by_slug(src, "no-such-slug", synthetic_id).unwrap(), None);
    }

    #[test]
    fn merge_series_into_transfers_flags_and_deletes_synthetic() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
        let existing = crate::models::Series {
            id: 0, slug: "baki-dou".into(), title: "Baki-dou".into(),
            url: "https://site/tv/baki-dou/".into(), cover_url: None, is_airing: true, followed: false, next_episode_at: None, site_episode_count: None,
        };
        let existing_id = db.upsert_series(src, &existing).unwrap();

        let synthetic = crate::models::Series {
            id: 0, slug: "anilist-7".into(), title: "Baki-dou".into(),
            url: "https://anilist.co/anime/7".into(), cover_url: None, is_airing: false, followed: false, next_episode_at: None, site_episode_count: None,
        };
        let synthetic_id = db.upsert_series(src, &synthetic).unwrap();
        db.set_anilist_id(synthetic_id, 7).unwrap();
        db.set_followed(synthetic_id, true).unwrap();
        db.set_backlog_status(synthetic_id, Some("want")).unwrap();

        db.merge_series_into(existing_id, synthetic_id).unwrap();

        // Synthetic row is gone.
        assert!(db.get_series_for_link(synthetic_id).unwrap().is_none());
        // Existing row picked up the synthetic row's flags.
        let merged = db.get_series_for_link(existing_id).unwrap().unwrap();
        assert!(merged.followed);
        assert_eq!(merged.backlog_status.as_deref(), Some("want"));
        // Existing row's own slug/url survive untouched (it stays canonical).
        assert_eq!(merged.slug, "baki-dou");
    }

    #[test]
    fn merge_series_into_keeps_existing_backlog_status_when_it_already_has_one() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
        let existing = crate::models::Series {
            id: 0, slug: "baki-dou".into(), title: "Baki-dou".into(),
            url: "https://site/tv/baki-dou/".into(), cover_url: None, is_airing: true, followed: false, next_episode_at: None, site_episode_count: None,
        };
        let existing_id = db.upsert_series(src, &existing).unwrap();
        db.set_backlog_status(existing_id, Some("discarded")).unwrap();

        let synthetic = crate::models::Series {
            id: 0, slug: "anilist-7".into(), title: "Baki-dou".into(),
            url: "https://anilist.co/anime/7".into(), cover_url: None, is_airing: false, followed: false, next_episode_at: None, site_episode_count: None,
        };
        let synthetic_id = db.upsert_series(src, &synthetic).unwrap();
        db.set_backlog_status(synthetic_id, Some("want")).unwrap();

        db.merge_series_into(existing_id, synthetic_id).unwrap();

        // Existing row's own (more specific/pre-existing) status wins.
        let merged = db.get_series_for_link(existing_id).unwrap().unwrap();
        assert_eq!(merged.backlog_status.as_deref(), Some("discarded"));
    }

    #[test]
    fn relink_series_updates_slug_url_cover_and_kind_in_place() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
        let synthetic = crate::models::Series {
            id: 0, slug: "anilist-7".into(), title: "Baki-dou".into(),
            url: "https://anilist.co/anime/7".into(), cover_url: Some("https://anilist-cdn/7.jpg".into()),
            is_airing: false, followed: true, next_episode_at: None, site_episode_count: None,
        };
        let sid = db.upsert_series(src, &synthetic).unwrap();
        db.set_anilist_id(sid, 7).unwrap();
        // upsert_series deliberately never writes `followed` (see its own
        // doc comment) — set it the same way the rest of the codebase does.
        db.set_followed(sid, true).unwrap();

        db.relink_series(sid, "baki-dou", "https://site/tv/baki-dou/", Some("https://site/cover.jpg"), "TV").unwrap();

        let url = db.get_series_url(sid).unwrap().unwrap();
        assert_eq!(url, "https://site/tv/baki-dou/");
        // followed/anilist_id survive — relink_series only touches slug/url/cover/kind.
        let info = db.get_series_for_link(sid).unwrap().unwrap();
        assert_eq!(info.slug, "baki-dou");
        assert!(info.followed);
        assert_eq!(info.anilist_id, Some(7));
    }

    #[test]
    fn replace_series_genres_drops_old_genres_instead_of_accumulating() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
        let s = crate::models::Series {
            id: 0, slug: "x".into(), title: "X".into(),
            url: "u".into(), cover_url: None, is_airing: false, followed: false, next_episode_at: None, site_episode_count: None,
        };
        let sid = db.upsert_series(src, &s).unwrap();
        db.insert_series_genres(sid, &["Action".to_string(), "Isekai".to_string()]).unwrap();

        db.replace_series_genres(sid, &["Drama".to_string()]).unwrap();

        assert_eq!(db.list_series_genres(sid).unwrap(), vec!["Drama".to_string()]);
    }

    #[test]
    fn series_needs_genre_backfill_reflects_series_genres_rows() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
        let s = crate::models::Series {
            id: 0, slug: "x".into(), title: "X".into(),
            url: "u".into(), cover_url: None, is_airing: false, followed: true, next_episode_at: None, site_episode_count: None,
        };
        let sid = db.upsert_series(src, &s).unwrap();

        assert!(db.series_needs_genre_backfill(sid).unwrap());
        db.insert_series_genres(sid, &["Seinen".to_string()]).unwrap();
        assert!(!db.series_needs_genre_backfill(sid).unwrap());
    }

    #[test]
    fn engaged_series_titles_covers_followed_want_discarded_and_watched_externally() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();

        let make = |slug: &str, title: &str| crate::models::Series {
            id: 0,
            slug: slug.into(),
            title: title.into(),
            url: format!("https://example.com/tv/{slug}"),
            cover_url: None,
            is_airing: false,
            followed: false,
            next_episode_at: None,
            site_episode_count: None,
        };

        let sid_followed = db.upsert_series(src, &make("followed-show", "Followed Show")).unwrap();
        db.set_followed(sid_followed, true).unwrap();

        let sid_want = db.upsert_series(src, &make("want-show", "Want Show")).unwrap();
        db.set_backlog_status(sid_want, Some("want")).unwrap();

        let sid_discarded = db.upsert_series(src, &make("discarded-show", "Discarded Show")).unwrap();
        db.set_backlog_status(sid_discarded, Some("discarded")).unwrap();

        let sid_watched = db.upsert_series(src, &make("watched-show", "Watched Show")).unwrap();
        db.set_watched_externally(sid_watched, true).unwrap();

        // Untouched row must NOT be engaged.
        db.upsert_series(src, &make("untouched-show", "Untouched Show")).unwrap();

        let titles: std::collections::HashSet<String> = db.engaged_series_titles().unwrap().into_iter().collect();
        assert!(titles.contains("Followed Show"));
        assert!(titles.contains("Want Show"));
        assert!(titles.contains("Discarded Show"));
        assert!(titles.contains("Watched Show"));
        assert!(!titles.contains("Untouched Show"));
    }

    #[test]
    fn engaged_series_titles_covers_engagement_on_any_site_not_just_one() {
        let db = Db::open(":memory:").unwrap();
        let a = db.upsert_source("AnimeYT", "a", "animeytx").unwrap();
        let b = db.upsert_source("TioAnime", "b", "tioanime").unwrap();

        let make = |slug: &str, title: &str| crate::models::Series {
            id: 0, slug: slug.into(), title: title.into(),
            url: format!("https://example.com/tv/{slug}"),
            cover_url: None, is_airing: false, followed: false,
            next_episode_at: None, site_episode_count: None,
        };

        // Followed on site B only — must still show up globally, so the
        // Descubrir deck (which reads this without a site filter) never
        // re-offers it while site A happens to be active.
        let sid = db.upsert_series(b, &make("overlord-iv", "Overlord IV")).unwrap();
        db.set_followed(sid, true).unwrap();
        // Untouched on site A.
        db.upsert_series(a, &make("other", "Other Show")).unwrap();

        let titles: std::collections::HashSet<String> = db.engaged_series_titles().unwrap().into_iter().collect();
        assert!(titles.contains("Overlord IV"), "engagement on a non-active site must still be visible");
        assert!(!titles.contains("Other Show"));
    }

    #[test]
    fn followed_series_without_episodes_finds_only_never_fetched_followed_rows() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "a", "animeytx").unwrap();
        let make = |slug: &str, title: &str| crate::models::Series {
            id: 0, slug: slug.into(), title: title.into(),
            url: format!("https://example.com/tv/{slug}"),
            cover_url: None, is_airing: true, followed: false,
            next_episode_at: None, site_episode_count: None,
        };

        // Followed, never fetched — the carry-over-onto-a-new-site shape.
        let empty = db.upsert_series(src, &make("empty", "Empty Show")).unwrap();
        db.set_followed(empty, true).unwrap();

        // Followed, already has episodes — must NOT show up again.
        let has_eps = db.upsert_series(src, &make("has-eps", "Has Episodes")).unwrap();
        db.set_followed(has_eps, true).unwrap();
        db.insert_episode(&crate::models::Episode {
            id: 0, series_id: has_eps, number: "1".into(), title: None,
            url: "https://example.com/has-eps-1".into(), released_at: None, seen: false,
        }).unwrap();

        // Never fetched but NOT followed — must not show up either.
        db.upsert_series(src, &make("not-followed", "Not Followed")).unwrap();

        let needing = db.followed_series_without_episodes(src).unwrap();
        let titles: Vec<&str> = needing.iter().map(|s| s.title.as_str()).collect();
        assert_eq!(titles, vec!["Empty Show"]);
    }

    #[test]
    fn get_series_for_history_returns_none_for_a_deleted_row() {
        let db = Db::open(":memory:").unwrap();
        assert!(db.get_series_for_history(999).unwrap().is_none());
    }

    #[test]
    fn get_series_for_history_includes_the_rows_url() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
        let s = crate::models::Series {
            id: 0, slug: "anilist-7".into(), title: "T".into(),
            url: "https://anilist.co/anime/7/t".into(), cover_url: None,
            is_airing: false, followed: false, next_episode_at: None, site_episode_count: None,
        };
        let sid = db.upsert_series(src, &s).unwrap();
        let row = db.get_series_for_history(sid).unwrap().unwrap();
        assert_eq!(row.url, "https://anilist.co/anime/7/t");
    }

    #[test]
    fn anilist_id_and_watched_externally_round_trip() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
        let s = crate::models::Series {
            id: 0, slug: "anilist-42".into(), title: "X".into(),
            url: "u".into(), cover_url: None, is_airing: false, followed: false, next_episode_at: None, site_episode_count: None,
        };
        let sid = db.upsert_series(src, &s).unwrap();

        db.set_anilist_id(sid, 42).unwrap();
        db.set_watched_externally(sid, true).unwrap();

        let (anilist_id, watched): (Option<i64>, i64) = db.conn.query_row(
            "SELECT anilist_id, watched_externally FROM series WHERE id=?1",
            [sid],
            |r| Ok((r.get(0)?, r.get(1)?)),
        ).unwrap();
        assert_eq!(anilist_id, Some(42));
        assert_eq!(watched, 1);
    }
}
