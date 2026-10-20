use super::*;
use std::collections::HashMap;

/// How long after the last scan that listed a series it still counts as "on the
/// site's airing listing" for `sync_status_from_catalog`.
const LISTED_GRACE_SECS: i64 = 3 * 86400;

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
    pub seen_watermark: f64,
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

/// `candidate` is `base` followed by a season marker ("X" -> "X Season 3", "X 2nd Season",
/// "X Part 2", "X Final Season", "X III"): a later season of the same show, not a spin-off
/// or a different series that merely shares a title prefix ("Love Live!" -> "Love Live! Superstar!!").
fn extends_with_season_marker(candidate: &str, base: &str) -> bool {
    let (c, b) = (candidate.to_lowercase(), base.to_lowercase());
    let Some(rest) = c.strip_prefix(&b) else { return false };
    let Some(tail) = rest.strip_prefix(' ') else { return false };
    let tail = tail.trim();
    let first = tail.split_whitespace().next().unwrap_or("");
    matches!(first, "season" | "part" | "cour" | "final" | "ii" | "iii" | "iv" | "v" | "vi")
        || tail.starts_with("the final")
        || first.chars().next().is_some_and(|ch| ch.is_ascii_digit())
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
        let group_keys = self.canonical_group_keys()?;
        let now = chrono::Utc::now().timestamp();

        let mut stmt = self.conn.prepare(
            "SELECT s.id, s.anilist_id, s.title, s.followed, s.watched_externally, s.backlog_status
             FROM series s
             WHERE s.followed=1 OR s.watched_externally=1 OR s.backlog_status='want'",
        )?;
        struct BaseRow {
            id: i64,
            anilist_id: Option<i64>,
            title: String,
            followed: bool,
            watched_externally: bool,
            backlog_status: Option<String>,
        }
        let rows: Vec<BaseRow> = stmt
            .query_map([], |r| {
                Ok(BaseRow {
                    id: r.get(0)?,
                    anilist_id: r.get(1)?,
                    title: r.get(2)?,
                    followed: r.get::<_, i64>(3)? != 0,
                    watched_externally: r.get::<_, i64>(4)? != 0,
                    backlog_status: r.get(5)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        // Fetch all seen episodes for contributing series in one pass
        let mut ep_stmt = self.conn.prepare(
            "SELECT e.series_id, e.number
             FROM episodes e
             JOIN series s ON s.id = e.series_id
             WHERE e.seen = 1
               AND (s.followed=1 OR s.watched_externally=1 OR s.backlog_status='want')",
        )?;
        let mut seen_numbers_by_series: HashMap<i64, Vec<String>> = HashMap::new();
        let ep_rows = ep_stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?;
        for ep in ep_rows {
            let (sid, num) = ep?;
            seen_numbers_by_series.entry(sid).or_default().push(num);
        }

        let mut grouped: HashMap<String, LibraryEntry> = HashMap::new();
        for row in rows {
            let align = self.row_align(row.id, now)?;
            let mut row_watermark = 0.0f64;
            if let Some(nums) = seen_numbers_by_series.get(&row.id) {
                for num_str in nums {
                    if let Some(parsed) = crate::db::episodes::parse_ep_number(num_str) {
                        if let Some(canon) = align.to_canonical(parsed) {
                            if canon > row_watermark {
                                row_watermark = canon;
                            }
                        }
                    }
                }
            }

            let key = group_keys
                .get(&row.id)
                .cloned()
                .unwrap_or_else(|| canon_key(row.anilist_id, &row.title));

            let entry = grouped.entry(key.clone()).or_insert_with(|| LibraryEntry {
                id: 0,
                canon_key: key,
                anilist_id: row.anilist_id,
                display_title: row.title.clone(),
                followed: false,
                backlog_status: None,
                watched_externally: false,
                seen_watermark: 0.0,
            });
            entry.followed |= row.followed;
            entry.watched_externally |= row.watched_externally;
            if row_watermark > entry.seen_watermark {
                entry.seen_watermark = row_watermark;
            }
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
             WHERE s.watched_externally=0 AND s.completion_reconciled=0
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
                // Self-exempting from now on via watched_externally=1 alone,
                // but completion_reconciled is set below too for uniformity.
                tx.execute("UPDATE series SET watched_externally=1 WHERE id=?1", [c.id])?;
                changed += 1;
            } else if c.total == 0 {
                // No episodes scraped in for this row yet — nothing to decide.
                // Leave it un-reconciled so a future refresh (once episodes
                // exist) still gets a first chance to catch it up.
                continue;
            } else if c.seen == 0 {
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
            //
            // Either way this row's completion state is now decided for good
            // (auto-caught-up, real progress respected, or flagged
            // completed) — mark it reconciled so it's never revisited. Without
            // this, a user who later un-marks episodes back down to 0 on
            // purpose looked identical to "never watched here" on the next
            // refresh and got silently re-completed.
            tx.execute("UPDATE series SET completion_reconciled=1 WHERE id=?1", [c.id])?;
        }
        tx.commit()?;
        Ok(changed)
    }

    /// Find the AniList id of a RELEASING/HIATUS sequel in the catalog.
    ///
    /// When relation rows are available for `own_id`, walks the breadth-first
    /// SEQUEL relation chain (depth <= 4) and picks the candidate with the latest
    /// `start_date` (ties: higher id) without requiring a season title marker.
    /// When the own entry has no relation rows at all (old rows), falls back
    /// to title prefix matching ("X" -> "X Season 3") with `start_date > own`.
    pub(crate) fn releasing_sequel_id(&self, titles: &[String], own_id: i64) -> Result<Option<i64>> {
        let own_start_date: Option<i64> = self
            .conn
            .query_row(
                "SELECT start_date FROM anilist_catalog WHERE id = ?1",
                [own_id],
                |r| r.get::<_, Option<i64>>(0),
            )
            .optional()?
            .flatten();

        let has_own_relations: bool = self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM anilist_catalog_relations WHERE anilist_id = ?1)",
            [own_id],
            |r| r.get(0),
        )?;

        if has_own_relations {
            let mut visited = std::collections::HashSet::new();
            visited.insert(own_id);
            let mut queue = std::collections::VecDeque::new();
            queue.push_back((own_id, 0usize));

            let mut rel_stmt = self.conn.prepare(
                "SELECT related_id FROM anilist_catalog_relations WHERE anilist_id = ?1 AND relation_type = 'SEQUEL'",
            )?;
            let mut cand_stmt = self.conn.prepare(
                "SELECT status, format, start_date FROM anilist_catalog WHERE id = ?1",
            )?;

            let mut best: Option<(i64, i64)> = None; // (start_date, id)

            while let Some((curr_id, depth)) = queue.pop_front() {
                if depth >= 4 {
                    continue;
                }
                let sequels: Vec<i64> = rel_stmt
                    .query_map([curr_id], |r| r.get(0))?
                    .collect::<rusqlite::Result<Vec<_>>>()?;

                for next_id in sequels {
                    if !visited.insert(next_id) {
                        continue;
                    }
                    queue.push_back((next_id, depth + 1));

                    let cand_info = cand_stmt
                        .query_row([next_id], |r| {
                            Ok((
                                r.get::<_, Option<String>>(0)?,
                                r.get::<_, Option<String>>(1)?,
                                r.get::<_, Option<i64>>(2)?,
                            ))
                        })
                        .optional()?;

                    if let Some((status, format, start_date)) = cand_info {
                        let status_ok = matches!(status.as_deref(), Some("RELEASING" | "HIATUS"));
                        let format_ok = format.as_deref().is_none_or(|f| f == "TV");
                        let date_ok = match (start_date, own_start_date) {
                            (Some(sd), Some(own)) => sd > own,
                            (Some(_), None) => true,
                            (None, _) => false,
                        };
                        if status_ok && format_ok && date_ok {
                            let sd = start_date.unwrap();
                            match best {
                                Some((best_date, best_id)) => {
                                    if sd > best_date || (sd == best_date && next_id > best_id) {
                                        best = Some((sd, next_id));
                                    }
                                }
                                None => {
                                    best = Some((sd, next_id));
                                }
                            }
                        }
                    }
                }
            }

            return Ok(best.map(|(_, id)| id));
        }

        let mut stmt = self.conn.prepare(
            "SELECT id, start_date, title, title_romaji, title_english FROM anilist_catalog
             WHERE status IN ('RELEASING', 'HIATUS') AND id != ?1
               AND start_date IS NOT NULL
               AND (format = 'TV' OR format IS NULL)
               AND (title LIKE ?2 ESCAPE '!' OR title_romaji LIKE ?2 ESCAPE '!' OR title_english LIKE ?2 ESCAPE '!')",
        )?;
        let mut best: Option<(i64, i64)> = None; // (start_date, id)
        for t in titles {
            let escaped = t.replace('!', "!!").replace('%', "!%").replace('_', "!_");
            let pattern = format!("{escaped} %");
            let mut rows = stmt.query((own_id, &pattern))?;
            while let Some(row) = rows.next()? {
                let id: i64 = row.get(0)?;
                let start_date: i64 = row.get(1)?;
                let cand_titles: Vec<String> = (2..5)
                    .filter_map(|i| row.get::<_, Option<String>>(i).transpose())
                    .collect::<rusqlite::Result<_>>()?;
                if !cand_titles.iter().any(|c| extends_with_season_marker(c, t)) {
                    continue;
                }
                if own_start_date.is_none_or(|own| start_date > own) {
                    match best {
                        Some((best_date, best_id)) => {
                            if start_date > best_date || (start_date == best_date && id > best_id) {
                                best = Some((start_date, id));
                            }
                        }
                        None => {
                            best = Some((start_date, id));
                        }
                    }
                }
            }
        }
        Ok(best.map(|(_, id)| id))
    }

    /// Synchronize a linked series' `is_airing` flag with AniList's catalog status.
    ///
    /// Every site adapter's airing-listing parser hardcodes `is_airing: true` on
    /// every row it returns. A show scraped as airing stays "airing" forever, even
    /// after it finishes, unless corrected. This uses `anilist_catalog.status` as
    /// the bidirectional authoritative signal for a linked show, but ONLY if the
    /// link is season-consistent (a mislinked row is never trusted):
    /// - `RELEASING` or `HIATUS` -> `is_airing = 1`
    /// - `FINISHED` or `CANCELLED` -> `is_airing = 0` (EXCEPT when the site
    ///   announced a future episode, i.e. `next_episode_at > current unix time`,
    ///   since concrete site evidence beats a stale catalog; or when listed
    ///   recently on the site and a later season is releasing in the catalog, in
    ///   which case the row is relinked to that sequel and kept airing)
    /// - `NOT_YET_RELEASED` or `NULL` -> untouched
    ///
    /// Unlinked series and mislinked series are left untouched. Returns the
    /// number of rows changed.
    pub fn sync_status_from_catalog(&self) -> Result<usize> {
        let now = chrono::Utc::now().timestamp();

        let mut stmt = self.conn.prepare(
            "SELECT s.id, s.title, s.anilist_id, c.status, s.last_listed_at, s.watched_externally
             FROM series s
             JOIN anilist_catalog c ON c.id = s.anilist_id
             WHERE
               (s.is_airing = 1 AND c.status IN ('FINISHED', 'CANCELLED') AND (s.next_episode_at IS NULL OR s.next_episode_at <= ?1))
               OR (s.is_airing = 0 AND c.status IN ('RELEASING', 'HIATUS'))",
        )?;
        let candidates = stmt
            .query_map([now], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, Option<i64>>(4)?,
                    r.get::<_, i64>(5)? != 0,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        // Only the handful of candidate entries need their titles (a status
        // sync runs on every refresh, under the DB mutex): never load the
        // whole ~22k-row catalog here.
        let mut title_stmt = self
            .conn
            .prepare("SELECT title, title_romaji, title_english FROM anilist_catalog WHERE id = ?1")?;
        let mut titles_of: std::collections::HashMap<i64, Vec<String>> = std::collections::HashMap::new();
        let mut to_update_0 = Vec::new();
        let mut to_update_1 = Vec::new();
        let mut to_relink = Vec::new();

        for (id, title, anilist_id, status, last_listed_at, watched_externally) in candidates {
            if let std::collections::hash_map::Entry::Vacant(e) = titles_of.entry(anilist_id) {
                let v = title_stmt.query_row([anilist_id], |r| {
                    let mut v = vec![r.get::<_, String>(0)?];
                    v.extend(r.get::<_, Option<String>>(1)?);
                    v.extend(r.get::<_, Option<String>>(2)?);
                    Ok(v)
                })?;
                e.insert(v);
            }
            let refs: Vec<&str> = titles_of[&anilist_id].iter().map(String::as_str).collect();
            if !crate::matching::season_consistent(&title, &refs) {
                // A link the title cannot vouch for (e.g. a row relinked to "X Season 3") is
                // not trusted to flip a show on, but once the site stopped listing it a
                // finished entry must still un-air it, or it would stay airing forever.
                let listed_recently = last_listed_at.is_some_and(|t| t >= now - LISTED_GRACE_SECS);
                if (status == "FINISHED" || status == "CANCELLED") && !listed_recently {
                    to_update_0.push(id);
                }
                continue;
            }
            if status == "FINISHED" || status == "CANCELLED" {
                // A site that keeps every season under one slug links to the
                // first (FINISHED) season while a later one airs: if listed
                // recently and the catalog has a releasing sequel with a later
                // premiere date, relink the row to the sequel so title, cover
                // and season match what is actually airing.
                let listed_recently = last_listed_at.is_some_and(|t| t >= now - LISTED_GRACE_SECS);
                if !watched_externally && listed_recently {
                    if let Some(seq) = self.releasing_sequel_id(&titles_of[&anilist_id], anilist_id)? {
                        to_relink.push((id, seq));
                        continue;
                    }
                }
                to_update_0.push(id);
            } else if status == "RELEASING" || status == "HIATUS" {
                to_update_1.push(id);
            }
        }

        // Chunked (far below SQLite's bound-variable limit) and in one
        // transaction so the DB mutex is held for a single commit.
        let tx = self.conn.unchecked_transaction()?;
        let mut changed = 0;
        for (value, ids) in [(0, &to_update_0), (1, &to_update_1)] {
            for chunk in ids.chunks(500) {
                let ph = chunk.iter().map(|_| "?").collect::<Vec<_>>().join(", ");
                let sql = format!("UPDATE series SET is_airing = {value} WHERE id IN ({ph})");
                changed += tx.execute(&sql, rusqlite::params_from_iter(chunk.iter()))?;
            }
        }
        for (id, seq) in to_relink {
            changed += tx.execute(
                "UPDATE series SET anilist_id = ?1, is_airing = 1 WHERE id = ?2",
                (seq, id),
            )?;
        }
        tx.commit()?;

        Ok(changed)
    }

    /// Bring every followed series' seen-episode watermark up to the highest
    /// mark any of its canonical twins (same show, any site) has reached.
    ///
    /// Complements `reconcile_completion_across_sites`, which only handles a
    /// show explicitly flagged `watched_externally` ("Ya lo vi"). This covers
    /// the far more common case: you watch a followed show's new episodes on
    /// site A, and a twin `series` row for the same show on site B — followed
    /// there too, e.g. via `carry_follow`'s one-time carry, or because you
    /// followed it independently on both — never hears about it, so it keeps
    /// showing those episodes as pending every time you switch to site B.
    ///
    /// Strictly additive, like the carry-over it extends: only ever calls
    /// `set_seen_cascade(id, watermark, true)`, which marks episodes numbered
    /// <= watermark seen and leaves everything above it — including on a row
    /// that already has MORE progress than the canonical watermark — exactly
    /// as it was. Never unmarks anything itself. Scoped to `followed=1`: an
    /// unfollowed or backlog-only twin is not touched. Returns how many
    /// series rows were actually advanced.
    ///
    /// A row lagging behind its twin is ambiguous on a live snapshot alone:
    /// "never caught up here" and "the user just manually un-marked episodes
    /// on purpose" look identical (both are just a lower `own_watermark`).
    /// `sync_watermark_applied` remembers what this function itself last
    /// set/observed for a row — a live watermark that has *dropped* below it
    /// means something un-marked episodes since, and that reset is respected
    /// rather than immediately overwritten back up to the twin's watermark.
    /// Real forward progress past that point resumes normal catch-up.
    pub fn sync_seen_progress_across_sites(&self) -> Result<usize> {
        let now = chrono::Utc::now().timestamp();
        self.sync_seen_progress_across_sites_at(now)
    }

    pub(crate) fn sync_seen_progress_across_sites_at(&self, now: i64) -> Result<usize> {
        let group_keys = self.canonical_group_keys()?;

        let mut stmt = self.conn.prepare(
            "SELECT s.id, s.anilist_id, s.title, s.sync_watermark_applied, s.sync_rollback_active
             FROM series s
             WHERE s.followed=1",
        )?;
        struct BaseFollowed {
            id: i64,
            anilist_id: Option<i64>,
            title: String,
            applied: Option<f64>,
            rollback_active: bool,
        }
        let base_rows: Vec<BaseFollowed> = stmt
            .query_map([], |r| {
                Ok(BaseFollowed {
                    id: r.get(0)?,
                    anilist_id: r.get(1)?,
                    title: r.get(2)?,
                    applied: r.get(3)?,
                    rollback_active: r.get::<_, i64>(4)? != 0,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        let mut ep_stmt = self.conn.prepare(
            "SELECT e.series_id, e.number
             FROM episodes e
             JOIN series s ON s.id = e.series_id
             WHERE s.followed=1 AND e.seen=1",
        )?;
        let mut seen_eps_by_series: HashMap<i64, Vec<String>> = HashMap::new();
        let ep_rows = ep_stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?;
        for ep in ep_rows {
            let (sid, num) = ep?;
            seen_eps_by_series.entry(sid).or_default().push(num);
        }

        struct Row {
            id: i64,
            key: String,
            applied: Option<f64>,
            rollback_active: bool,
            own_watermark: f64,
            align: super::episode_align::RowAlign,
            canon_watermark: f64,
        }
        let mut rows: Vec<Row> = Vec::with_capacity(base_rows.len());
        for base in base_rows {
            let key = group_keys
                .get(&base.id)
                .cloned()
                .unwrap_or_else(|| canon_key(base.anilist_id, &base.title));
            let align = self.row_align(base.id, now)?;
            let mut own_watermark = 0.0f64;
            let mut canon_watermark = 0.0f64;
            if let Some(nums) = seen_eps_by_series.get(&base.id) {
                for num_str in nums {
                    if let Some(parsed) = crate::db::episodes::parse_ep_number(num_str) {
                        if parsed > own_watermark {
                            own_watermark = parsed;
                        }
                        if let Some(canon) = align.to_canonical(parsed) {
                            if canon > canon_watermark {
                                canon_watermark = canon;
                            }
                        }
                    }
                }
            }
            rows.push(Row {
                id: base.id,
                key,
                applied: base.applied,
                rollback_active: base.rollback_active,
                own_watermark,
                align,
                canon_watermark,
            });
        }

        let mut canonical: HashMap<String, f64> = HashMap::new();
        for row in &rows {
            let entry = canonical.entry(row.key.clone()).or_insert(0.0);
            if row.canon_watermark > *entry {
                *entry = row.canon_watermark;
            }
        }

        let mut advanced = 0usize;
        for row in &rows {
            let ceiling = canonical[&row.key];

            if row.rollback_active {
                // Already flagged from an earlier cycle: stays suppressed
                // until the user's own watermark rises past where it stood
                // when the rollback was first detected (`applied`) — not
                // just until it reaches the ceiling again.
                if row.own_watermark > row.applied.unwrap_or(0.0) {
                    self.conn.execute(
                        "UPDATE series SET sync_rollback_active=0 WHERE id=?1",
                        [row.id],
                    )?;
                } else {
                    self.conn.execute(
                        "UPDATE series SET sync_watermark_applied=?2 WHERE id=?1",
                        (row.id, row.own_watermark),
                    )?;
                    continue;
                }
            } else if row.applied.is_some_and(|applied| row.own_watermark < applied) {
                // Freshly detected: latch the flag and the drop point, and
                // suppress this cycle's cascade — the drop is presumed
                // deliberate (a manual un-mark) until forward progress says
                // otherwise.
                self.conn.execute(
                    "UPDATE series SET sync_watermark_applied=?2, sync_rollback_active=1 WHERE id=?1",
                    (row.id, row.own_watermark),
                )?;
                continue;
            }

            if ceiling > row.canon_watermark {
                let target = row.align.to_row(ceiling);
                let target_str = super::episode_align::format_ep_number(target);
                self.set_seen_cascade(row.id, &target_str, true)?;
                // Re-read what actually got marked, rather than assuming the
                // cascade reached `ceiling` exactly
                let mut num_stmt = self.conn.prepare("SELECT number FROM episodes WHERE series_id=?1 AND seen=1")?;
                let mut real_watermark = 0.0f64;
                let nums = num_stmt.query_map([row.id], |r| r.get::<_, String>(0))?;
                for n_res in nums {
                    let n_str = n_res?;
                    if let Some(parsed) = crate::db::episodes::parse_ep_number(&n_str) {
                        if parsed > real_watermark {
                            real_watermark = parsed;
                        }
                    }
                }
                self.conn.execute(
                    "UPDATE series SET sync_watermark_applied=?2 WHERE id=?1",
                    (row.id, real_watermark),
                )?;
                advanced += 1;
            } else {
                // Already at/above the ceiling: resync bookkeeping to the
                // row's real current watermark so a future advance is
                // judged against what it actually is now.
                self.conn.execute(
                    "UPDATE series SET sync_watermark_applied=?2 WHERE id=?1",
                    (row.id, row.own_watermark),
                )?;
            }
        }
        Ok(advanced)
    }


    /// Canonical **followed** entries that are NOT yet present on `source_id`
    /// — i.e. the part of your library that switching to this site would
    /// strand. "Present" means the site has a `series` row that either shares
    /// the entry's `anilist_id` or matches its `canon_key` (title). These are
    /// exactly the entries a cross-site import must resolve+link on the target
    /// site. Ordered most-progressed first so a paced import brings back the
    /// shows you're deepest into soonest.
    ///
    /// A still-synthetic `anilist-N` row does **not** count as present. Such a
    /// row is a placeholder, not a link: its `url` points at anilist.co, it has
    /// no episodes, and nothing on this site can be watched through it. They
    /// arise two ways — a catalog swipe that hasn't been linked yet, and (the
    /// bug this clause fixes) a cross-site import that created the placeholder,
    /// failed to match the show on this site, and left the row behind. Counting
    /// those as "present" meant one transient `NoMatch`/scrape failure/mirror
    /// outage permanently disqualified that series from ever being retried on
    /// that site. `commands::run_library_import` now also deletes its own
    /// placeholder on failure (`delete_orphan_synthetic_series`); this clause is
    /// what un-blocks the ones already sitting in existing databases.
    pub fn library_entries_missing_on_site(&self, source_id: i64) -> Result<Vec<LibraryEntry>> {
        let group_keys = self.canonical_group_keys()?;
        // What the target site already has, as canonical keys.
        let mut stmt = self
            .conn
            .prepare("SELECT id, anilist_id, title FROM series WHERE source_id=?1 AND slug NOT LIKE 'anilist-%'")?;
        let present: std::collections::HashSet<String> = stmt
            .query_map([source_id], |r| {
                let id: i64 = r.get(0)?;
                let anilist_id: Option<i64> = r.get(1)?;
                let title: String = r.get(2)?;
                Ok(group_keys.get(&id).cloned().unwrap_or_else(|| canon_key(anilist_id, &title)))
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
                url: format!("https://site/{slug}-{n}"), released_at: None, seen: false, download_path: None, download_status: None,
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
        assert_eq!(lib[0].seen_watermark, 8.0, "max progress across sites");
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
        assert_eq!(lib[0].seen_watermark, 3.0);
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
        assert_eq!(missing[0].seen_watermark, 3.0);
    }

    /// A failed cross-site import must stay retryable. `run_library_import`
    /// writes the synthetic `anilist-N` placeholder (and its `anilist_id`)
    /// BEFORE it tries to link the series on the site; on a NoMatch / scrape
    /// error / mirrors-down the row survived, and because it carried the right
    /// `anilist_id`, this query reported the entry as already present on the
    /// site — so the import never tried again, for good.
    #[test]
    fn a_failed_import_leaves_the_entry_still_missing_and_retryable() {
        let db = Db::open(":memory:").unwrap();
        let a = db.upsert_source("AnimeYT", "a", "animeytx").unwrap();
        let b = db.upsert_source("TioAnime", "b", "tioanime").unwrap();
        seed_followed(&db, a, "frieren", "Frieren", Some(154587), 3, 12);
        db.sync_library_from_series().unwrap();
        assert_eq!(db.library_entries_missing_on_site(b).unwrap().len(), 1, "missing to start with");

        // Exactly what a failed import leaves behind on site B.
        let orphan = db
            .upsert_series(
                b,
                &crate::models::Series {
                    id: 0, slug: "anilist-154587".into(), title: "Frieren".into(),
                    url: "https://anilist.co/anime/154587".into(), cover_url: None,
                    is_airing: false, followed: false, next_episode_at: None, site_episode_count: None,
                },
            )
            .unwrap();
        db.set_anilist_id(orphan, 154587).unwrap();

        let missing = db.library_entries_missing_on_site(b).unwrap();
        assert_eq!(missing.len(), 1, "an unlinked placeholder is not 'present on the site'");
        assert_eq!(missing[0].display_title, "Frieren");

        // And the import's own cleanup removes the dead row, so they can't pile
        // up: after that the entry is still (correctly) missing.
        assert!(db.delete_orphan_synthetic_series(orphan).unwrap());
        assert_eq!(db.library_entries_missing_on_site(b).unwrap().len(), 1);

        // A REAL linked row on B does end the retries, as it always did.
        let linked = db.upsert_series(b, &mk_airing("frieren-tio", "Frieren", None)).unwrap();
        db.set_anilist_id(linked, 154587).unwrap();
        assert!(db.library_entries_missing_on_site(b).unwrap().is_empty());
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
        assert_eq!(db.list_pending(b, crate::db::PendingSort::RemainingAsc, &[]).unwrap().len(), 5, "starts flooded");

        let fixed = db.reconcile_completion_across_sites().unwrap();
        assert_eq!(fixed, 1);
        // Non-destructive: the flag is set, but episode seen-state is untouched…
        let eps = db.list_series_episodes(yb).unwrap();
        assert!(eps.iter().all(|e| !e.seen), "episodes NOT mutated (no data loss)");
        // …and the pending filter keeps the completed show out of the queue.
        assert!(db.list_pending(b, crate::db::PendingSort::RemainingAsc, &[]).unwrap().is_empty(), "no longer pending");
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
        assert_eq!(db.list_pending(b, crate::db::PendingSort::RemainingAsc, &[]).unwrap().len(), 2);
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
        assert!(db.list_pending(b, crate::db::PendingSort::RemainingAsc, &[]).unwrap().is_empty());
    }

    #[test]
    fn reconcile_does_not_re_complete_a_row_the_user_manually_reset_to_zero() {
        // The reported bug: refresh() auto-catches-up an airing twin with 0
        // progress (correct, once). The user then deliberately un-marks every
        // episode back to 0 (wants to rewatch from scratch). Before
        // completion_reconciled existed, the next refresh saw seen==0 again —
        // indistinguishable from "never watched here" — and silently
        // re-completed it, undoing the user's un-mark every single time.
        let db = Db::open(":memory:").unwrap();
        let a = db.upsert_source("AnimeYT", "a", "animeytx").unwrap();
        let b = db.upsert_source("TioAnime", "b", "tioanime").unwrap();
        let base = db.upsert_series(a, &mk_airing("tanya", "Saga of Tanya S2", None)).unwrap();
        db.set_anilist_id(base, 135865).unwrap();
        db.set_watched_externally(base, true).unwrap();
        let ob = seed_followed(&db, b, "youjo-senki-ii", "Youjo Senki II", Some(135865), 0, 3);

        // First refresh: legitimate auto-catch-up.
        assert_eq!(db.reconcile_completion_across_sites().unwrap(), 1);
        assert!(db.list_series_episodes(ob).unwrap().iter().all(|e| e.seen));

        // User manually resets every episode back to unseen.
        db.set_seen_cascade(ob, "1", false).unwrap();
        assert!(db.list_series_episodes(ob).unwrap().iter().all(|e| !e.seen));

        // Second refresh must leave the manual reset alone.
        assert_eq!(db.reconcile_completion_across_sites().unwrap(), 0, "already reconciled once — must not re-fire");
        assert!(
            db.list_series_episodes(ob).unwrap().iter().all(|e| !e.seen),
            "manual reset to zero must survive a refresh"
        );
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

    // ---- sync_status_from_catalog ----

    fn set_is_airing(db: &Db, sid: i64, airing: bool) {
        db.conn.execute("UPDATE series SET is_airing=?1 WHERE id=?2", (airing, sid)).unwrap();
    }
    fn get_is_airing(db: &Db, sid: i64) -> bool {
        db.conn.query_row("SELECT is_airing FROM series WHERE id=?1", [sid], |r| r.get(0)).unwrap()
    }
    fn get_anilist_id(db: &Db, sid: i64) -> Option<i64> {
        db.conn.query_row("SELECT anilist_id FROM series WHERE id=?1", [sid], |r| r.get(0)).unwrap()
    }

    #[test]
    fn sync_status_from_catalog_flips_is_airing_down_for_finished_and_cancelled() {
        let db = Db::open(":memory:").unwrap();
        let a = db.upsert_source("AnimeYT", "a", "animeytx").unwrap();
        let mut finished = crate::db::test_support::catalog_anime(21, "One Piece", &[]);
        finished.status = Some("FINISHED".into());
        db.upsert_catalog_anime(&finished, 0).unwrap();
        let sid1 = db.upsert_series(a, &mk_airing("op", "One Piece", None)).unwrap();
        db.set_anilist_id(sid1, 21).unwrap(); // is_airing defaults to 1 via mk_airing

        let mut cancelled = crate::db::test_support::catalog_anime(22, "Cancelled Show", &[]);
        cancelled.status = Some("CANCELLED".into());
        db.upsert_catalog_anime(&cancelled, 1).unwrap();
        let sid2 = db.upsert_series(a, &mk_airing("canc", "Cancelled Show", None)).unwrap();
        db.set_anilist_id(sid2, 22).unwrap();

        let changed = db.sync_status_from_catalog().unwrap();
        assert_eq!(changed, 2);
        assert!(!get_is_airing(&db, sid1), "FINISHED -> flips 1 to 0");
        assert!(!get_is_airing(&db, sid2), "CANCELLED -> flips 1 to 0");

        assert_eq!(db.sync_status_from_catalog().unwrap(), 0, "idempotent second call returns 0");
    }

    #[test]
    fn sync_status_from_catalog_flips_is_airing_up_for_releasing_and_hiatus() {
        let db = Db::open(":memory:").unwrap();
        let a = db.upsert_source("AnimeYT", "a", "animeytx").unwrap();
        
        let mut releasing = crate::db::test_support::catalog_anime(31, "Releasing Show", &[]);
        releasing.status = Some("RELEASING".into());
        db.upsert_catalog_anime(&releasing, 0).unwrap();
        let sid1 = db.upsert_series(a, &mk_airing("rel", "Releasing Show", None)).unwrap();
        db.set_anilist_id(sid1, 31).unwrap();
        set_is_airing(&db, sid1, false);

        let mut hiatus = crate::db::test_support::catalog_anime(32, "Hiatus Show", &[]);
        hiatus.status = Some("HIATUS".into());
        db.upsert_catalog_anime(&hiatus, 1).unwrap();
        let sid2 = db.upsert_series(a, &mk_airing("hia", "Hiatus Show", None)).unwrap();
        db.set_anilist_id(sid2, 32).unwrap();
        set_is_airing(&db, sid2, false);

        let changed = db.sync_status_from_catalog().unwrap();
        assert_eq!(changed, 2);
        assert!(get_is_airing(&db, sid1), "RELEASING -> flips 0 to 1");
        assert!(get_is_airing(&db, sid2), "HIATUS -> flips 0 to 1");
    }

    #[test]
    fn sync_status_from_catalog_leaves_finished_airing_if_future_episode_announced() {
        let db = Db::open(":memory:").unwrap();
        let a = db.upsert_source("AnimeYT", "a", "animeytx").unwrap();
        
        let mut finished = crate::db::test_support::catalog_anime(41, "Soon Finished Show", &[]);
        finished.status = Some("FINISHED".into());
        db.upsert_catalog_anime(&finished, 0).unwrap();
        let mut series = mk_airing("soon", "Soon Finished Show", None);
        // Concrete site evidence: future episode
        series.next_episode_at = Some(chrono::Utc::now().timestamp() + 86400); 
        let sid = db.upsert_series(a, &series).unwrap();
        db.set_anilist_id(sid, 41).unwrap();

        let changed = db.sync_status_from_catalog().unwrap();
        assert_eq!(changed, 0);
        assert!(get_is_airing(&db, sid), "FINISHED with next_episode_at in the future stays 1");
    }

    #[test]
    fn sync_status_from_catalog_leaves_unlinked_not_yet_released_and_mismatched_untouched() {
        let db = Db::open(":memory:").unwrap();
        let a = db.upsert_source("AnimeYT", "a", "animeytx").unwrap();
        
        // Unlinked
        let s_unlinked = db.upsert_series(a, &mk_airing("u", "Unlinked Show", None)).unwrap();

        // NOT_YET_RELEASED
        let mut nyr = crate::db::test_support::catalog_anime(51, "Upcoming Show", &[]);
        nyr.status = Some("NOT_YET_RELEASED".into());
        db.upsert_catalog_anime(&nyr, 0).unwrap();
        let s_nyr = db.upsert_series(a, &mk_airing("upc", "Upcoming Show", None)).unwrap();
        db.set_anilist_id(s_nyr, 51).unwrap();
        set_is_airing(&db, s_nyr, false); // should not be flipped to 1

        // Season-mismatched link
        let mut finished = crate::db::test_support::catalog_anime(61, "Base Show", &[]);
        finished.status = Some("FINISHED".into()); // would normally flip 1->0
        db.upsert_catalog_anime(&finished, 1).unwrap();
        let s_mismatch = db.upsert_series(a, &mk_airing("base5", "Base Show Temporada 5", None)).unwrap();
        db.set_anilist_id(s_mismatch, 61).unwrap(); // is_airing = 1

        // Another mismatch the other way (0->1 would normally happen for RELEASING)
        let mut releasing = crate::db::test_support::catalog_anime(62, "Base Show 2", &[]);
        releasing.status = Some("RELEASING".into()); 
        db.upsert_catalog_anime(&releasing, 2).unwrap();
        let s_mismatch2 = db.upsert_series(a, &mk_airing("base6", "Base Show 2 Temporada 5", None)).unwrap();
        db.set_anilist_id(s_mismatch2, 62).unwrap();
        set_is_airing(&db, s_mismatch2, false);

        let changed = db.sync_status_from_catalog().unwrap();
        assert_eq!(changed, 0);
        
        assert!(get_is_airing(&db, s_unlinked), "unlinked rows are untouched");
        assert!(!get_is_airing(&db, s_nyr), "NOT_YET_RELEASED untouched");
        assert!(get_is_airing(&db, s_mismatch), "season-mismatched FINISHED untouched (stays 1)");
        assert!(!get_is_airing(&db, s_mismatch2), "season-mismatched RELEASING untouched (stays 0)");
    }

    #[test]
    fn sync_status_keeps_listed_series_airing_when_a_later_season_is_releasing() {
        let db = Db::open(":memory:").unwrap();
        let a = db.upsert_source("TioAnime", "a", "tioanime").unwrap();
        let mut s1 = crate::db::test_support::catalog_anime(71, "Appraisal Skill", &[]);
        s1.status = Some("FINISHED".into());
        s1.start_date = Some(100);
        db.upsert_catalog_anime(&s1, 0).unwrap();
        let mut s3 = crate::db::test_support::catalog_anime(73, "Appraisal Skill Season 3", &[]);
        s3.status = Some("RELEASING".into());
        s3.start_date = Some(300);
        db.upsert_catalog_anime(&s3, 1).unwrap();
        let sid = db.upsert_series(a, &mk_airing("appraisal", "Appraisal Skill", None)).unwrap();
        db.set_anilist_id(sid, 71).unwrap();

        assert_eq!(db.sync_status_from_catalog().unwrap(), 1, "relinked 1 row");
        assert_eq!(get_anilist_id(&db, sid), Some(73), "relinked to season 3");
        assert!(get_is_airing(&db, sid), "stays airing");

        assert_eq!(db.sync_status_from_catalog().unwrap(), 0, "second call changes nothing");
    }

    #[test]
    fn sync_status_relinks_to_sequel_with_latest_start_date() {
        let db = Db::open(":memory:").unwrap();
        let a = db.upsert_source("TioAnime", "a", "tioanime").unwrap();
        let mut s1 = crate::db::test_support::catalog_anime(100, "My Hero", &[]);
        s1.status = Some("FINISHED".into());
        s1.start_date = Some(100);
        db.upsert_catalog_anime(&s1, 0).unwrap();

        let mut s2 = crate::db::test_support::catalog_anime(101, "My Hero Season 2", &[]);
        s2.status = Some("RELEASING".into());
        s2.start_date = Some(300);
        db.upsert_catalog_anime(&s2, 1).unwrap();

        let mut s3 = crate::db::test_support::catalog_anime(102, "My Hero Season 3", &[]);
        s3.status = Some("RELEASING".into());
        s3.start_date = Some(500);
        db.upsert_catalog_anime(&s3, 2).unwrap();

        let sid = db.upsert_series(a, &mk_airing("my-hero", "My Hero", None)).unwrap();
        db.set_anilist_id(sid, 100).unwrap();

        assert_eq!(db.sync_status_from_catalog().unwrap(), 1);
        assert_eq!(get_anilist_id(&db, sid), Some(102), "relinks to the sequel with latest start_date (500)");
        assert!(get_is_airing(&db, sid));
    }

    #[test]
    fn sync_status_does_not_relink_when_sequel_start_date_is_earlier_than_own() {
        let db = Db::open(":memory:").unwrap();
        let a = db.upsert_source("TioAnime", "a", "tioanime").unwrap();
        let mut s_own = crate::db::test_support::catalog_anime(200, "Prequel Story", &[]);
        s_own.status = Some("FINISHED".into());
        s_own.start_date = Some(500);
        db.upsert_catalog_anime(&s_own, 0).unwrap();

        let mut s_cand = crate::db::test_support::catalog_anime(201, "Prequel Story Zero", &[]);
        s_cand.status = Some("RELEASING".into());
        s_cand.start_date = Some(200); // earlier than own 500!
        db.upsert_catalog_anime(&s_cand, 1).unwrap();

        // 1) With last_listed_at recent and no valid sequel, row is un-aired
        let sid = db.upsert_series(a, &mk_airing("prequel-story", "Prequel Story", None)).unwrap();
        db.set_anilist_id(sid, 200).unwrap();

        assert_eq!(db.sync_status_from_catalog().unwrap(), 1, "flips airing 1 to 0");
        assert_eq!(get_anilist_id(&db, sid), Some(200), "not relinked");
        assert!(!get_is_airing(&db, sid), "un-aired");

        // 2) When not listed recently, row is also un-aired
        let sid2 = db.upsert_series(a, &mk_airing("prequel-story-2", "Prequel Story", None)).unwrap();
        db.set_anilist_id(sid2, 200).unwrap();
        db.conn.execute("UPDATE series SET last_listed_at=0 WHERE id=?1", [sid2]).unwrap();

        assert_eq!(db.sync_status_from_catalog().unwrap(), 1);
        assert_eq!(get_anilist_id(&db, sid2), Some(200), "not relinked");
        assert!(!get_is_airing(&db, sid2), "un-aired");
    }

    #[test]
    fn sync_status_does_not_relink_when_row_not_listed_recently() {
        let db = Db::open(":memory:").unwrap();
        let a = db.upsert_source("TioAnime", "a", "tioanime").unwrap();
        let mut s1 = crate::db::test_support::catalog_anime(300, "Old Anime", &[]);
        s1.status = Some("FINISHED".into());
        s1.start_date = Some(100);
        db.upsert_catalog_anime(&s1, 0).unwrap();

        let mut s2 = crate::db::test_support::catalog_anime(301, "Old Anime Season 2", &[]);
        s2.status = Some("RELEASING".into());
        s2.start_date = Some(300);
        db.upsert_catalog_anime(&s2, 1).unwrap();

        // last_listed_at = 0
        let sid = db.upsert_series(a, &mk_airing("old-anime-0", "Old Anime", None)).unwrap();
        db.set_anilist_id(sid, 300).unwrap();
        db.conn.execute("UPDATE series SET last_listed_at=0 WHERE id=?1", [sid]).unwrap();

        // last_listed_at = NULL
        let b = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
        let sid_null = db.upsert_series(b, &mk_airing("old-anime-null", "Old Anime", None)).unwrap();
        db.set_anilist_id(sid_null, 300).unwrap();
        db.conn.execute("UPDATE series SET last_listed_at=NULL WHERE id=?1", [sid_null]).unwrap();

        assert_eq!(db.sync_status_from_catalog().unwrap(), 2);
        assert_eq!(get_anilist_id(&db, sid), Some(300), "not relinked");
        assert!(!get_is_airing(&db, sid), "un-aired (FINISHED rule)");
        assert_eq!(get_anilist_id(&db, sid_null), Some(300), "not relinked");
        assert!(!get_is_airing(&db, sid_null), "un-aired (FINISHED rule)");
    }

    #[test]
    fn sync_status_never_relinks_watched_externally_row() {
        let db = Db::open(":memory:").unwrap();
        let a = db.upsert_source("TioAnime", "a", "tioanime").unwrap();
        let mut s1 = crate::db::test_support::catalog_anime(400, "Completed Franchise", &[]);
        s1.status = Some("FINISHED".into());
        s1.start_date = Some(100);
        db.upsert_catalog_anime(&s1, 0).unwrap();

        let mut s2 = crate::db::test_support::catalog_anime(401, "Completed Franchise Season 2", &[]);
        s2.status = Some("RELEASING".into());
        s2.start_date = Some(300);
        db.upsert_catalog_anime(&s2, 1).unwrap();

        let sid = db.upsert_series(a, &mk_airing("completed-franchise", "Completed Franchise", None)).unwrap();
        db.set_anilist_id(sid, 400).unwrap();
        db.set_watched_externally(sid, true).unwrap();

        assert_eq!(db.sync_status_from_catalog().unwrap(), 1, "flips airing to 0");
        assert_eq!(get_anilist_id(&db, sid), Some(400), "never relinked when watched_externally");
        assert!(!get_is_airing(&db, sid), "un-aired");
    }

    #[test]
    fn sync_status_never_relinks_a_row_whose_linked_entry_is_releasing() {
        let db = Db::open(":memory:").unwrap();
        let a = db.upsert_source("TioAnime", "a", "tioanime").unwrap();
        let mut s1 = crate::db::test_support::catalog_anime(500, "Ongoing Series", &[]);
        s1.status = Some("RELEASING".into());
        s1.start_date = Some(100);
        db.upsert_catalog_anime(&s1, 0).unwrap();

        let mut s2 = crate::db::test_support::catalog_anime(501, "Ongoing Series Season 2", &[]);
        s2.status = Some("RELEASING".into());
        s2.start_date = Some(300);
        db.upsert_catalog_anime(&s2, 1).unwrap();

        let sid = db.upsert_series(a, &mk_airing("ongoing-series", "Ongoing Series", None)).unwrap();
        db.set_anilist_id(sid, 500).unwrap();

        assert_eq!(db.sync_status_from_catalog().unwrap(), 0, "no changes");
        assert_eq!(get_anilist_id(&db, sid), Some(500), "releasing entry is never relinked");
        assert!(get_is_airing(&db, sid));
    }

    #[test]
    fn sync_status_does_not_treat_title_sharing_letters_without_space_as_sequel() {
        let db = Db::open(":memory:").unwrap();
        let a = db.upsert_source("TioAnime", "a", "tioanime").unwrap();
        let mut s1 = crate::db::test_support::catalog_anime(600, "Bleach", &[]);
        s1.status = Some("FINISHED".into());
        s1.start_date = Some(100);
        db.upsert_catalog_anime(&s1, 0).unwrap();

        // "Bleachers" shares letters with "Bleach", but is NOT "Bleach <tail>"!
        let mut s2 = crate::db::test_support::catalog_anime(601, "Bleachers", &[]);
        s2.status = Some("RELEASING".into());
        s2.start_date = Some(300);
        db.upsert_catalog_anime(&s2, 1).unwrap();

        let sid = db.upsert_series(a, &mk_airing("bleach", "Bleach", None)).unwrap();
        db.set_anilist_id(sid, 600).unwrap();

        // Helper check: releasing_sequel_id must return None
        let titles = vec!["Bleach".to_string()];
        assert_eq!(db.releasing_sequel_id(&titles, 600).unwrap(), None);

        // Status sync un-airs the row and does not relink
        assert_eq!(db.sync_status_from_catalog().unwrap(), 1);
        assert_eq!(get_anilist_id(&db, sid), Some(600), "not relinked");
        assert!(!get_is_airing(&db, sid), "un-aired");
    }

    #[test]
    fn releasing_sequel_id_handles_null_start_date_and_like_escaping() {
        let db = Db::open(":memory:").unwrap();

        // Case A: own start_date is None -> accepts candidate with non-null start_date
        let mut s1 = crate::db::test_support::catalog_anime(700, "Null Date Show", &[]);
        s1.status = Some("FINISHED".into());
        s1.start_date = None;
        db.upsert_catalog_anime(&s1, 0).unwrap();

        let mut s2 = crate::db::test_support::catalog_anime(701, "Null Date Show Season 2", &[]);
        s2.status = Some("RELEASING".into());
        s2.start_date = Some(200);
        db.upsert_catalog_anime(&s2, 1).unwrap();

        assert_eq!(
            db.releasing_sequel_id(&["Null Date Show".into()], 700).unwrap(),
            Some(701),
            "when own start_date is NULL, accepts sequel with non-NULL start_date"
        );

        // Case B: candidate start_date is None -> rejected
        let mut s_base = crate::db::test_support::catalog_anime(710, "Base Only", &[]);
        s_base.status = Some("FINISHED".into());
        s_base.start_date = Some(100);
        db.upsert_catalog_anime(&s_base, 2).unwrap();

        let mut s_cand_null = crate::db::test_support::catalog_anime(711, "Base Only Season 2", &[]);
        s_cand_null.status = Some("RELEASING".into());
        s_cand_null.start_date = None;
        db.upsert_catalog_anime(&s_cand_null, 3).unwrap();

        assert_eq!(
            db.releasing_sequel_id(&["Base Only".into()], 710).unwrap(),
            None,
            "candidate with NULL start_date is never accepted"
        );

        // Case C: LIKE escaping of special characters: '%', '_', '!'
        let mut s_esc = crate::db::test_support::catalog_anime(720, "100%_Pascal!", &[]);
        s_esc.status = Some("FINISHED".into());
        s_esc.start_date = Some(100);
        db.upsert_catalog_anime(&s_esc, 4).unwrap();

        let mut s_esc_seq = crate::db::test_support::catalog_anime(721, "100%_Pascal! Season 2", &[]);
        s_esc_seq.status = Some("RELEASING".into());
        s_esc_seq.start_date = Some(200);
        db.upsert_catalog_anime(&s_esc_seq, 5).unwrap();

        // A trick title that would match unescaped '%' or '_'
        let mut s_esc_fake = crate::db::test_support::catalog_anime(722, "10000XPascal! Season 2", &[]);
        s_esc_fake.status = Some("RELEASING".into());
        s_esc_fake.start_date = Some(300);
        db.upsert_catalog_anime(&s_esc_fake, 6).unwrap();

        assert_eq!(
            db.releasing_sequel_id(&["100%_Pascal!".into()], 720).unwrap(),
            Some(721),
            "special LIKE characters are escaped properly"
        );
    }

    #[test]
    fn releasing_sequel_id_follows_sequel_chain_without_season_marker() {
        let db = Db::open(":memory:").unwrap();
        let mut a = crate::db::test_support::catalog_anime(1001, "Original Title", &[]);
        a.status = Some("FINISHED".into());
        a.start_date = Some(100);
        a.relations = vec![crate::anilist::CatalogRelation { related_id: 1002, relation_type: "SEQUEL".into() }];
        db.upsert_catalog_anime(&a, 0).unwrap();

        let mut b = crate::db::test_support::catalog_anime(1002, "Second Season Middle", &[]);
        b.status = Some("FINISHED".into());
        b.start_date = Some(200);
        b.relations = vec![crate::anilist::CatalogRelation { related_id: 1003, relation_type: "SEQUEL".into() }];
        db.upsert_catalog_anime(&b, 1).unwrap();

        let mut c = crate::db::test_support::catalog_anime(1003, "Totally Different Name", &[]);
        c.status = Some("RELEASING".into());
        c.start_date = Some(300);
        db.upsert_catalog_anime(&c, 2).unwrap();

        let res = db.releasing_sequel_id(&["Original Title".into()], 1001).unwrap();
        assert_eq!(res, Some(1003), "chain finds releasing sequel even with totally different title");
    }

    #[test]
    fn releasing_sequel_id_picks_latest_releasing_sequel_in_chain() {
        let db = Db::open(":memory:").unwrap();
        let mut a = crate::db::test_support::catalog_anime(2001, "Franchise A", &[]);
        a.status = Some("FINISHED".into());
        a.start_date = Some(100);
        a.relations = vec![
            crate::anilist::CatalogRelation { related_id: 2002, relation_type: "SEQUEL".into() },
            crate::anilist::CatalogRelation { related_id: 2003, relation_type: "SEQUEL".into() },
        ];
        db.upsert_catalog_anime(&a, 0).unwrap();

        let mut b = crate::db::test_support::catalog_anime(2002, "Sequel Early", &[]);
        b.status = Some("RELEASING".into());
        b.start_date = Some(300);
        db.upsert_catalog_anime(&b, 1).unwrap();

        let mut c = crate::db::test_support::catalog_anime(2003, "Sequel Later", &[]);
        c.status = Some("RELEASING".into());
        c.start_date = Some(500);
        db.upsert_catalog_anime(&c, 2).unwrap();

        let res = db.releasing_sequel_id(&["Franchise A".into()], 2001).unwrap();
        assert_eq!(res, Some(2003), "latest start_date in chain wins");
    }

    #[test]
    fn releasing_sequel_id_ignores_ova_and_earlier_or_null_start_date_in_chain() {
        let db = Db::open(":memory:").unwrap();
        let mut a = crate::db::test_support::catalog_anime(3001, "Main Show", &[]);
        a.status = Some("FINISHED".into());
        a.start_date = Some(500);
        a.relations = vec![
            crate::anilist::CatalogRelation { related_id: 3002, relation_type: "SEQUEL".into() },
            crate::anilist::CatalogRelation { related_id: 3003, relation_type: "SEQUEL".into() },
            crate::anilist::CatalogRelation { related_id: 3004, relation_type: "SEQUEL".into() },
        ];
        db.upsert_catalog_anime(&a, 0).unwrap();

        // 3002 is OVA
        let mut ova = crate::db::test_support::catalog_anime(3002, "OVA Sequel", &[]);
        ova.status = Some("RELEASING".into());
        ova.format = Some("OVA".into());
        ova.start_date = Some(600);
        db.upsert_catalog_anime(&ova, 1).unwrap();

        // 3003 has earlier start_date than own
        let mut earlier = crate::db::test_support::catalog_anime(3003, "Earlier Sequel", &[]);
        earlier.status = Some("RELEASING".into());
        earlier.start_date = Some(400);
        db.upsert_catalog_anime(&earlier, 2).unwrap();

        // 3004 has NULL start_date
        let mut null_date = crate::db::test_support::catalog_anime(3004, "Null Date Sequel", &[]);
        null_date.status = Some("RELEASING".into());
        null_date.start_date = None;
        db.upsert_catalog_anime(&null_date, 3).unwrap();

        let res = db.releasing_sequel_id(&["Main Show".into()], 3001).unwrap();
        assert_eq!(res, None, "OVA, earlier start_date, and null start_date sequels are all ignored");
    }

    #[test]
    fn releasing_sequel_id_terminates_on_relation_cycle() {
        let db = Db::open(":memory:").unwrap();
        let mut a = crate::db::test_support::catalog_anime(4001, "Cycle A", &[]);
        a.status = Some("FINISHED".into());
        a.start_date = Some(100);
        a.relations = vec![crate::anilist::CatalogRelation { related_id: 4002, relation_type: "SEQUEL".into() }];
        db.upsert_catalog_anime(&a, 0).unwrap();

        let mut b = crate::db::test_support::catalog_anime(4002, "Cycle B", &[]);
        b.status = Some("FINISHED".into());
        b.start_date = Some(200);
        b.relations = vec![crate::anilist::CatalogRelation { related_id: 4001, relation_type: "SEQUEL".into() }];
        db.upsert_catalog_anime(&b, 1).unwrap();

        let res = db.releasing_sequel_id(&["Cycle A".into()], 4001).unwrap();
        assert_eq!(res, None, "cycle terminates without infinite loop");
    }

    #[test]
    fn releasing_sequel_id_ignores_prequel_and_non_sequel_edges() {
        let db = Db::open(":memory:").unwrap();
        let mut a = crate::db::test_support::catalog_anime(5001, "Current Season", &[]);
        a.status = Some("FINISHED".into());
        a.start_date = Some(300);
        // Only PREQUEL edge, no SEQUEL edges
        a.relations = vec![crate::anilist::CatalogRelation { related_id: 5002, relation_type: "PREQUEL".into() }];
        db.upsert_catalog_anime(&a, 0).unwrap();

        let mut pre = crate::db::test_support::catalog_anime(5002, "Prequel Show", &[]);
        pre.status = Some("RELEASING".into());
        pre.start_date = Some(400);
        db.upsert_catalog_anime(&pre, 1).unwrap();

        let res = db.releasing_sequel_id(&["Current Season".into()], 5001).unwrap();
        assert_eq!(res, None, "PREQUEL edge is never followed forward to return a releasing entry");
    }

    #[test]
    fn sequel_must_carry_a_season_marker_and_be_tv() {
        assert!(extends_with_season_marker("My Hero Season 3", "My Hero"));
        assert!(extends_with_season_marker("My Hero 2nd Season", "My Hero"));
        assert!(extends_with_season_marker("My Hero Part 2", "My Hero"));
        assert!(extends_with_season_marker("My Hero The Final Season", "My Hero"));
        assert!(extends_with_season_marker("My Hero III", "My Hero"));
        assert!(!extends_with_season_marker("Love Live! Superstar!!", "Love Live!"));
        assert!(!extends_with_season_marker("Yu-Gi-Oh! Go Rush!!", "Yu-Gi-Oh!"));
        assert!(!extends_with_season_marker("My Hero: The Movie", "My Hero"));
        assert!(!extends_with_season_marker("My Hero", "My Hero"));
    }

    #[test]
    fn sync_status_never_relinks_to_a_spin_off_or_a_non_tv_entry() {
        let db = Db::open(":memory:").unwrap();
        let a = db.upsert_source("TioAnime", "a", "tioanime").unwrap();
        let mut base = crate::db::test_support::catalog_anime(600, "Love Live!", &[]);
        base.status = Some("FINISHED".into());
        base.start_date = Some(100);
        db.upsert_catalog_anime(&base, 0).unwrap();
        let mut spin = crate::db::test_support::catalog_anime(601, "Love Live! Superstar!!", &[]);
        spin.status = Some("RELEASING".into());
        spin.start_date = Some(900);
        db.upsert_catalog_anime(&spin, 1).unwrap();
        let mut ova = crate::db::test_support::catalog_anime(602, "Love Live! Season 2", &[]);
        ova.status = Some("RELEASING".into());
        ova.start_date = Some(800);
        db.upsert_catalog_anime(&ova, 2).unwrap();
        db.conn.execute("UPDATE anilist_catalog SET format='OVA' WHERE id=602", []).unwrap();

        let sid = db.upsert_series(a, &mk_airing("love-live", "Love Live!", None)).unwrap();
        db.set_anilist_id(sid, 600).unwrap();

        db.sync_status_from_catalog().unwrap();
        assert_eq!(get_anilist_id(&db, sid), Some(600), "not relinked to a spin-off or an OVA");
        assert!(!get_is_airing(&db, sid));
    }

    #[test]
    fn a_relinked_row_is_un_aired_once_its_season_finishes_and_the_site_stops_listing_it() {
        let db = Db::open(":memory:").unwrap();
        let a = db.upsert_source("TioAnime", "a", "tioanime").unwrap();
        let mut s1 = crate::db::test_support::catalog_anime(700, "Show", &[]);
        s1.status = Some("FINISHED".into());
        s1.start_date = Some(100);
        db.upsert_catalog_anime(&s1, 0).unwrap();
        let mut s3 = crate::db::test_support::catalog_anime(702, "Show Season 3", &[]);
        s3.status = Some("RELEASING".into());
        s3.start_date = Some(300);
        db.upsert_catalog_anime(&s3, 1).unwrap();
        let sid = db.upsert_series(a, &mk_airing("show", "Show", None)).unwrap();
        db.set_anilist_id(sid, 700).unwrap();
        db.sync_status_from_catalog().unwrap();
        assert_eq!(get_anilist_id(&db, sid), Some(702));

        // Season 3 ends; the site still lists it for a while, then stops.
        db.conn.execute("UPDATE anilist_catalog SET status='FINISHED' WHERE id=702", []).unwrap();
        db.sync_status_from_catalog().unwrap();
        assert!(get_is_airing(&db, sid), "still listed recently -> stays");
        db.conn.execute("UPDATE series SET last_listed_at=0 WHERE id=?1", [sid]).unwrap();
        db.sync_status_from_catalog().unwrap();
        assert!(!get_is_airing(&db, sid), "not listed any more -> un-aired");
    }

    // ---- sync_seen_progress_across_sites (2026-08-13 site-switch bug) ----

    #[test]
    fn sync_seen_progress_advances_a_lagging_twin_followed_on_two_sites() {
        let db = Db::open(":memory:").unwrap();
        let a = db.upsert_source("AnimeYT", "a", "animeytx").unwrap();
        let b = db.upsert_source("TioAnime", "b", "tioanime").unwrap();
        // Followed on both, no anilist_id (matches by normalized title) — the
        // real-world shape: carry_follow only fires once, so this pairing is
        // never revisited by anything else.
        seed_followed(&db, a, "shared", "Shared Show", None, 5, 10);
        let ob = seed_followed(&db, b, "shared-b", "Shared Show", None, 0, 10);

        let advanced = db.sync_seen_progress_across_sites().unwrap();
        assert_eq!(advanced, 1, "only B needed to catch up");
        let seen_count = db.list_series_episodes(ob).unwrap().iter().filter(|e| e.seen).count();
        assert_eq!(seen_count, 5, "B is caught up to A's watermark");
        assert!(
            db.list_series_episodes(ob).unwrap().iter().any(|e| e.number == "6" && !e.seen),
            "episodes past the watermark stay pending"
        );
    }

    #[test]
    fn sync_seen_progress_never_regresses_the_side_with_more_progress() {
        let db = Db::open(":memory:").unwrap();
        let a = db.upsert_source("AnimeYT", "a", "animeytx").unwrap();
        let b = db.upsert_source("TioAnime", "b", "tioanime").unwrap();
        let oa = seed_followed(&db, a, "shared", "Shared Show", None, 2, 10);
        seed_followed(&db, b, "shared-b", "Shared Show", None, 7, 10);

        let advanced = db.sync_seen_progress_across_sites().unwrap();
        assert_eq!(advanced, 1, "only A (behind) is advanced, not B");
        let a_seen = db.list_series_episodes(oa).unwrap().iter().filter(|e| e.seen).count();
        assert_eq!(a_seen, 7, "A raised to B's watermark");
        // B, already ahead, is completely untouched — same watermark as before.
        let b_seen = db
            .list_series_episodes(db.conn.query_row(
                "SELECT id FROM series WHERE slug='shared-b'", [], |r| r.get::<_, i64>(0),
            ).unwrap())
            .unwrap()
            .iter()
            .filter(|e| e.seen)
            .count();
        assert_eq!(b_seen, 7);
    }

    #[test]
    fn sync_seen_progress_is_idempotent() {
        let db = Db::open(":memory:").unwrap();
        let a = db.upsert_source("AnimeYT", "a", "animeytx").unwrap();
        let b = db.upsert_source("TioAnime", "b", "tioanime").unwrap();
        seed_followed(&db, a, "shared", "Shared Show", None, 5, 10);
        seed_followed(&db, b, "shared-b", "Shared Show", None, 0, 10);

        assert_eq!(db.sync_seen_progress_across_sites().unwrap(), 1);
        assert_eq!(db.sync_seen_progress_across_sites().unwrap(), 0, "second run has nothing left to advance");
    }

    #[test]
    fn sync_seen_progress_ignores_an_unfollowed_twin() {
        let db = Db::open(":memory:").unwrap();
        let a = db.upsert_source("AnimeYT", "a", "animeytx").unwrap();
        let b = db.upsert_source("TioAnime", "b", "tioanime").unwrap();
        seed_followed(&db, a, "shared", "Shared Show", None, 5, 10);
        // Same show scraped on B but never followed there — must be left alone.
        let ob = db.upsert_series(b, &mk_airing("shared-b", "Shared Show", None)).unwrap();
        for n in 1..=10 {
            db.insert_episode(&crate::models::Episode {
                id: 0, series_id: ob, number: n.to_string(), title: None,
                url: format!("https://site/shared-b-{n}"), released_at: None, seen: false, download_path: None, download_status: None,
            }).unwrap();
        }

        assert_eq!(db.sync_seen_progress_across_sites().unwrap(), 0);
        assert_eq!(db.list_series_episodes(ob).unwrap().iter().filter(|e| e.seen).count(), 0);
    }

    #[test]
    fn sync_seen_progress_does_not_re_raise_a_manually_rolled_back_row() {
        // The reported bug: followed on two sites, both caught up to 10. The
        // user un-marks A back down to 3 on purpose (wants to rewatch). The
        // next refresh's sync saw A(3) behind B's still-10 watermark and
        // silently raised A right back to 10 — undoing the un-mark every time.
        let db = Db::open(":memory:").unwrap();
        let a = db.upsert_source("AnimeYT", "a", "animeytx").unwrap();
        let b = db.upsert_source("TioAnime", "b", "tioanime").unwrap();
        let oa = seed_followed(&db, a, "shared", "Shared Show", None, 10, 10);
        seed_followed(&db, b, "shared-b", "Shared Show", None, 10, 10);

        // Both already caught up — a first sync just records bookkeeping.
        assert_eq!(db.sync_seen_progress_across_sites().unwrap(), 0);

        // User manually un-marks A from episode 3 onward (un-marking cascades
        // to every later episode too, including the one clicked — 1 and 2
        // stay seen).
        db.set_seen_cascade(oa, "3", false).unwrap();
        assert_eq!(db.list_series_episodes(oa).unwrap().iter().filter(|e| e.seen).count(), 2);

        // Refresh (== another sync) must respect the rollback, not re-raise A.
        assert_eq!(db.sync_seen_progress_across_sites().unwrap(), 0, "must not re-raise a manual rollback");
        assert_eq!(
            db.list_series_episodes(oa).unwrap().iter().filter(|e| e.seen).count(),
            2,
            "A's manual rollback survives a refresh"
        );

        // A SECOND consecutive sync with no further progress is the actual
        // regression case: an earlier version reset `sync_watermark_applied`
        // down to the rolled-back value on the first sync above, which made
        // the rollback undetectable on this second call and re-raised A.
        assert_eq!(db.sync_seen_progress_across_sites().unwrap(), 0, "rollback must still be respected on a later sync too");
        assert_eq!(
            db.list_series_episodes(oa).unwrap().iter().filter(|e| e.seen).count(),
            2,
            "A's manual rollback survives a second refresh cycle"
        );

        // Genuine forward progress past the rollback point still resumes
        // normal cross-site catch-up.
        db.set_seen_cascade(oa, "4", true).unwrap();
        assert_eq!(db.sync_seen_progress_across_sites().unwrap(), 1, "real progress past the rollback resumes catch-up");
        assert_eq!(db.list_series_episodes(oa).unwrap().iter().filter(|e| e.seen).count(), 10);
    }

    #[test]
    fn sync_seen_progress_does_not_falsely_latch_a_rollback_when_a_site_simply_has_fewer_episodes() {
        // The reported bug: A confirmed live to affect 162 of 438 followed
        // rows in a real library. Site B has only scraped 5 episodes of a
        // show whose twin on site A already has 10 marked seen. The cascade
        // to B is real work (mark B's 5 seen) but can only ever reach 5 —
        // stamping `sync_watermark_applied=10` (the requested ceiling)
        // regardless made the *next* sync see B's real watermark (5) sitting
        // below `applied` (10) and latch a rollback that never happened,
        // permanently blocking B from ever catching up again once A gets
        // more episodes.
        let db = Db::open(":memory:").unwrap();
        let a = db.upsert_source("AnimeYT", "a", "animeytx").unwrap();
        let b = db.upsert_source("TioAnime", "b", "tioanime").unwrap();
        seed_followed(&db, a, "shared", "Shared Show", None, 10, 10);
        let ob = seed_followed(&db, b, "shared-b", "Shared Show", None, 0, 5);
        let rollback_active = |db: &Db, id: i64| -> bool {
            db.conn
                .query_row("SELECT sync_rollback_active FROM series WHERE id=?1", [id], |r| r.get::<_, i64>(0))
                .unwrap()
                != 0
        };

        db.sync_seen_progress_across_sites().unwrap();
        assert_eq!(
            db.list_series_episodes(ob).unwrap().iter().filter(|e| e.seen).count(),
            5,
            "B can only ever reach its own 5 episodes"
        );
        assert!(!rollback_active(&db, ob), "B is already at its own real ceiling — must not be flagged as rolled back");

        // The critical assertion: a second sync with nothing new on either
        // side (B's own watermark is still capped at 5 — nothing to roll
        // back from) must not falsely latch the rollback flag either. The
        // pre-fix bug stamped `sync_watermark_applied` with the requested
        // ceiling (10) instead of what B's cascade could actually reach (5),
        // so this exact second sync used to see own(5) < applied(10) and
        // latch a rollback that never happened.
        db.sync_seen_progress_across_sites().unwrap();
        assert!(!rollback_active(&db, ob), "still not rolled back after a second sync with no real change");

        // B's site finally scrapes 5 more episodes (6-10) — real forward
        // progress on B's own site, not a cross-site push. Confirms B is not
        // stuck suppressed by a false rollback latch: a fresh sync (as would
        // happen once B's episode list is re-fetched and its watermark
        // recomputed) must still be able to advance it.
        for n in 6..=10 {
            db.insert_episode(&crate::models::Episode {
                id: 0, series_id: ob, number: n.to_string(), title: None,
                url: format!("https://site-b/x-{n}/"), released_at: None, seen: false, download_path: None, download_status: None,
            }).unwrap();
        }
        assert_eq!(db.sync_seen_progress_across_sites().unwrap(), 1, "B must still be able to catch up once it has more episodes");
        assert_eq!(db.list_series_episodes(ob).unwrap().iter().filter(|e| e.seen).count(), 10);
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

    #[test]
    fn sync_seen_progress_canonical_alignment_tensei_fixture() {
        let db = Db::open(":memory:").unwrap();
        let now = chrono::Utc::now().timestamp();
        let src_a = db.upsert_source("AnimeYT", "https://animeyt.cc", "animeytx").unwrap();
        let src_b = db.upsert_source("TioAnime", "https://tioanime.com", "tioanime").unwrap();

        let entries = vec![
            crate::anizip::MapEntry { episode: 1, season: 3, absolute: Some(25), air_at: Some(now - 14 * 86400) },
            crate::anizip::MapEntry { episode: 2, season: 3, absolute: Some(26), air_at: Some(now - 7 * 86400) },
            crate::anizip::MapEntry { episode: 3, season: 3, absolute: Some(27), air_at: Some(now) },
            crate::anizip::MapEntry { episode: 4, season: 3, absolute: Some(28), air_at: Some(now + 7 * 86400) },
            crate::anizip::MapEntry { episode: 5, season: 3, absolute: Some(29), air_at: Some(now + 14 * 86400) },
            crate::anizip::MapEntry { episode: 6, season: 3, absolute: Some(30), air_at: Some(now + 21 * 86400) },
            crate::anizip::MapEntry { episode: 7, season: 3, absolute: Some(31), air_at: Some(now + 28 * 86400) },
            crate::anizip::MapEntry { episode: 8, season: 3, absolute: Some(32), air_at: Some(now + 35 * 86400) },
        ];
        db.replace_episode_map(185756, &entries, now).unwrap();

        let mk = |slug: &str, title: &str| crate::models::Series {
            id: 0, slug: slug.into(), title: title.into(), url: format!("https://site/{slug}"),
            cover_url: None, is_airing: true, followed: true, next_episode_at: None, site_episode_count: None,
        };

        let row_a = db.upsert_series(src_a, &mk("tensei-a", "Tensei")).unwrap();
        db.set_anilist_id(row_a, 185756).unwrap();
        db.set_followed(row_a, true).unwrap();
        for i in 1..=3 {
            db.insert_episode(&crate::models::Episode {
                id: 0, series_id: row_a, number: i.to_string(), title: None,
                url: format!("https://animeyt.cc/tv/tensei/{i}"), released_at: None, seen: false,
                download_path: None, download_status: None,
            }).unwrap();
        }

        let row_b = db.upsert_series(src_b, &mk("tensei-b", "Tensei")).unwrap();
        db.set_anilist_id(row_b, 185756).unwrap();
        db.set_followed(row_b, true).unwrap();
        for i in 1..=28 {
            db.insert_episode(&crate::models::Episode {
                id: 0, series_id: row_b, number: i.to_string(), title: None,
                url: format!("https://tioanime.com/ver/tensei-{i}"), released_at: None, seen: false,
                download_path: None, download_status: None,
            }).unwrap();
        }

        // Case 1: TioAnime watermark raw 26 and animeyt watermark raw 2 are in sync (no row advanced, returns 0)
        db.set_seen_cascade(row_b, "26", true).unwrap();
        db.set_seen_cascade(row_a, "2", true).unwrap();
        assert_eq!(db.sync_seen_progress_across_sites().unwrap(), 0, "raw 26 and raw 2 are both canonical 2: in sync");
        assert_eq!(db.list_series_episodes(row_a).unwrap().iter().filter(|e| e.seen).count(), 2);
        assert_eq!(db.list_series_episodes(row_b).unwrap().iter().filter(|e| e.seen).count(), 26);

        // Case 2: TioAnime raw 27 and animeyt raw 1 advances animeyt to 3 and nothing else.
        // First reset animeyt to raw 1 seen (and clear rollback state by setting sync_watermark_applied to 1)
        db.set_seen_cascade(row_a, "2", false).unwrap();
        db.conn.execute("UPDATE series SET sync_watermark_applied=1, sync_rollback_active=0 WHERE id=?1", [row_a]).unwrap();
        assert_eq!(db.list_series_episodes(row_a).unwrap().iter().filter(|e| e.seen).count(), 1);

        // Advance TioAnime to 27
        db.set_seen_cascade(row_b, "27", true).unwrap();
        db.conn.execute("UPDATE series SET sync_watermark_applied=27, sync_rollback_active=0 WHERE id=?1", [row_b]).unwrap();
        assert_eq!(db.list_series_episodes(row_b).unwrap().iter().filter(|e| e.seen).count(), 27);

        // Run sync: advances animeyt to 3 and nothing else
        let advanced = db.sync_seen_progress_across_sites().unwrap();
        assert_eq!(advanced, 1, "only animeyt advanced");
        // Assert the number of seen animeyt episodes is exactly 3 (without fix, it would have marked all seen up to 27)
        assert_eq!(db.list_series_episodes(row_a).unwrap().iter().filter(|e| e.seen).count(), 3, "animeyt must be marked seen up to 3");
        assert_eq!(db.list_series_episodes(row_b).unwrap().iter().filter(|e| e.seen).count(), 27, "tioanime stays at 27");
    }

    #[test]
    fn watermark_of_12_5_survives_sync_library_from_series() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "a", "animeytx").unwrap();
        let sid = db.upsert_series(src, &mk_airing("show-125", "Fractional Show", None)).unwrap();
        db.set_followed(sid, true).unwrap();

        for i in 1..=12 {
            db.insert_episode(&crate::models::Episode {
                id: 0, series_id: sid, number: i.to_string(), title: None,
                url: format!("https://site/ep-{i}"), released_at: None, seen: true,
                download_path: None, download_status: None,
            }).unwrap();
        }
        db.insert_episode(&crate::models::Episode {
            id: 0, series_id: sid, number: "12.5".into(), title: None,
            url: "https://site/ep-12-5".into(), released_at: None, seen: true,
            download_path: None, download_status: None,
        }).unwrap();

        db.sync_library_from_series().unwrap();
        let lib = db.list_library_entries().unwrap();
        assert_eq!(lib.len(), 1);
        assert_eq!(lib[0].seen_watermark, 12.5, "12.5 fractional watermark preserved without integer truncation");
    }

    #[test]
    fn linked_and_unlinked_same_title_rows_collapse_into_one_library_entry() {
        let db = Db::open(":memory:").unwrap();
        let src_a = db.upsert_source("AnimeYT", "a", "animeytx").unwrap();
        let src_b = db.upsert_source("TioAnime", "b", "tioanime").unwrap();

        let mk = |slug: &str, title: &str| crate::models::Series {
            id: 0, slug: slug.into(), title: title.into(), url: format!("https://site/{slug}"),
            cover_url: None, is_airing: true, followed: true, next_episode_at: None, site_episode_count: None,
        };

        let row_a = db.upsert_series(src_a, &mk("frieren-a", "Sousou no Frieren")).unwrap();
        db.set_anilist_id(row_a, 500).unwrap();
        db.set_followed(row_a, true).unwrap();

        let row_b = db.upsert_series(src_b, &mk("frieren-b", "Sousou no Frieren")).unwrap();
        db.set_followed(row_b, true).unwrap();

        db.sync_library_from_series().unwrap();
        let lib = db.list_library_entries().unwrap();
        assert_eq!(lib.len(), 1, "linked and unlinked same-title rows collapse into one library entry");
        assert_eq!(lib[0].canon_key, "al:500");
    }

    #[test]
    fn cumulative_offset_target_replays_aligned_watermark_not_raw() {
        let db = Db::open(":memory:").unwrap();
        let now = chrono::Utc::now().timestamp();
        let src_b = db.upsert_source("TioAnime", "https://tioanime.com", "tioanime").unwrap();

        let entries = vec![
            crate::anizip::MapEntry { episode: 1, season: 3, absolute: Some(25), air_at: Some(now - 14 * 86400) },
            crate::anizip::MapEntry { episode: 2, season: 3, absolute: Some(26), air_at: Some(now - 7 * 86400) },
            crate::anizip::MapEntry { episode: 3, season: 3, absolute: Some(27), air_at: Some(now) },
            crate::anizip::MapEntry { episode: 4, season: 3, absolute: Some(28), air_at: Some(now + 7 * 86400) },
        ];
        db.replace_episode_map(185756, &entries, now).unwrap();

        let row_b = db.upsert_series(src_b, &crate::models::Series {
            id: 0, slug: "tensei-b".into(), title: "Tensei".into(), url: "https://tioanime.com/ver/tensei".into(),
            cover_url: None, is_airing: true, followed: true, next_episode_at: None, site_episode_count: None,
        }).unwrap();
        db.set_anilist_id(row_b, 185756).unwrap();

        for i in 1..=28 {
            db.insert_episode(&crate::models::Episode {
                id: 0, series_id: row_b, number: i.to_string(), title: None,
                url: format!("https://tioanime.com/ver/tensei-{i}"), released_at: None, seen: false,
                download_path: None, download_status: None,
            }).unwrap();
        }

        // Suppose library entry has canonical seen_watermark 3.0
        let entry_watermark = 3.0f64;
        let align = db.row_align(row_b, now).unwrap();
        assert!(align.cumulative, "sanity: row_b is cumulative with offset 24");
        assert_eq!(align.offset, 24);

        let row_number = crate::db::episode_align::format_ep_number(align.to_row(entry_watermark));
        assert_eq!(row_number, "27", "canonical 3.0 aligned to cumulative 27");

        db.set_seen_cascade(row_b, &row_number, true).unwrap();
        let eps = db.list_series_episodes(row_b).unwrap();
        assert_eq!(eps.iter().filter(|e| e.seen).count(), 27, "episodes 1..=27 are marked seen");
        assert!(!eps.iter().find(|e| e.number == "28").unwrap().seen);
    }
}
