use super::*;

/// (franchise key, season) -> linked series (id, title, anilist_id).
type SiblingIndex = std::collections::HashMap<(String, i64), Vec<(i64, String, String, i64)>>;

/// One title the local catalog could not link, with every series row that
/// carries it (same normalized title across sites), for an AniList search.
#[derive(Debug, Clone)]
pub struct SearchTarget {
    pub norm_title: String,
    pub title: String,
    pub series_ids: Vec<i64>,
}

impl Db {
    /// Every catalog row's title variants, for building a
    /// `matching::CatalogIndex`. All three stored spellings are returned
    /// because the scraped site lists some shows under their romaji title and
    /// others under the English one, and the display `title` column is only
    /// ever one of the two.
    pub fn catalog_titles_for_index(&self) -> Result<Vec<crate::matching::CatalogTitleRow>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, title, title_romaji, title_english, popularity FROM anilist_catalog")?;
        let mut rows = stmt
            .query_map([], |r| {
                let id: i64 = r.get(0)?;
                let title: String = r.get(1)?;
                let romaji: Option<String> = r.get(2)?;
                let english: Option<String> = r.get(3)?;
                let popularity: Option<i64> = r.get(4)?;
                let mut titles = vec![title];
                titles.extend(romaji);
                titles.extend(english);
                Ok(crate::matching::CatalogTitleRow {
                    id,
                    titles,
                    synonyms: Vec::new(),
                    popularity: popularity.unwrap_or(0),
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        let mut syn_stmt = self.conn.prepare("SELECT anilist_id, synonym FROM anilist_catalog_synonyms")?;
        let mut syn_map: std::collections::HashMap<i64, Vec<String>> = std::collections::HashMap::new();
        let syn_rows = syn_stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?;
        for res in syn_rows {
            let (id, syn) = res?;
            syn_map.entry(id).or_default().push(syn);
        }

        for row in &mut rows {
            if let Some(syns) = syn_map.remove(&row.id) {
                for syn in syns {
                    let trimmed = syn.trim();
                    // Under 4 chars is too ambiguous to match on.
                    if trimmed.chars().count() >= 4
                        && !row.titles.iter().chain(&row.synonyms).any(|t| t.eq_ignore_ascii_case(trimmed))
                    {
                        row.synonyms.push(trimmed.to_string());
                    }
                }
            }
        }
        Ok(rows)
    }

    /// Unlinked series worth resolving to a catalog row: the ones the user has
    /// engaged with, plus every airing show. Scraped-but-untouched rows (the
    /// bulk of the table) are skipped — linking them buys nothing and multiplies
    /// the chance of a wrong automatic link.
    ///
    /// **Across all sites** (no source filter): the canonical airing/library
    /// unions dedup by `anilist_id`, so a show must be linked on *every* site
    /// that has it, not just the active one — otherwise the same show appears
    /// twice (linked on one site, title-only on another).
    pub fn series_needing_catalog_link(&self) -> Result<Vec<(i64, String, String)>> {
        let mut stmt = self.conn.prepare(
            "SELECT s.id, s.title, s.slug FROM series s
             WHERE s.anilist_id IS NULL
               AND (s.followed=1 OR s.watched_externally=1 OR s.is_airing=1
                    OR EXISTS (SELECT 1 FROM episodes e WHERE e.series_id=s.id AND e.seen=1))
             ORDER BY s.id",
        )?;
        let rows = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    pub fn unlinked_for_search(&self, limit: usize, now: i64) -> Result<Vec<SearchTarget>> {
        // Misses were decided by older matching rules: when the rules improve,
        // every title gets another chance instead of waiting out the 14 days.
        const SEARCH_RULES_VERSION: &str = "2";
        if self.get_setting("anilist_search_rules")?.as_deref() != Some(SEARCH_RULES_VERSION) {
            self.conn.execute("DELETE FROM anilist_search_misses", [])?;
            self.set_setting("anilist_search_rules", SEARCH_RULES_VERSION)?;
        }
        let mut by_norm: std::collections::HashMap<String, SearchTarget> = std::collections::HashMap::new();
        let mut order: Vec<String> = Vec::new();

        let mut stmt = self.conn.prepare(
            "SELECT s.id, s.title, (s.is_airing = 1 OR s.followed = 1) as priority
             FROM series s
             WHERE s.anilist_id IS NULL
               AND (s.followed=1 OR s.watched_externally=1 OR s.is_airing=1
                    OR EXISTS (SELECT 1 FROM episodes e WHERE e.series_id=s.id AND e.seen=1))
             ORDER BY priority DESC, s.id"
        )?;

        let rows = stmt.query_map([], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
        })?.collect::<rusqlite::Result<Vec<_>>>()?;

        let mut check_miss = self.conn.prepare("SELECT tried_at FROM anilist_search_misses WHERE norm_title=?1")?;

        for (id, title) in rows {
            let norm = crate::matching::normalize_title(&title);
            if norm.is_empty() { continue; }

            if !by_norm.contains_key(&norm) {
                let tried_at: Option<i64> = check_miss.query_row([&norm], |r| r.get(0)).optional()?;
                if let Some(t) = tried_at {
                    if now - t < 14 * 24 * 3600 {
                        continue;
                    }
                }
                by_norm.insert(norm.clone(), SearchTarget {
                    norm_title: norm.clone(),
                    title: title.clone(),
                    series_ids: Vec::new(),
                });
                order.push(norm.clone());
            }
            if let Some(entry) = by_norm.get_mut(&norm) {
                entry.series_ids.push(id);
            }
        }

        let mut result = Vec::new();
        for norm in order {
            if result.len() >= limit { break; }
            if let Some(target) = by_norm.remove(&norm) {
                result.push(target);
            }
        }

        Ok(result)
    }

    pub fn record_search_miss(&self, norm_title: &str, now: i64) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO anilist_search_misses(norm_title, tried_at) VALUES(?1, ?2)",
            rusqlite::params![norm_title, now],
        )?;
        Ok(())
    }

    /// Store the AniList entry found by a search (so the series has a catalog
    /// row to read metadata from) and link every row of the target to it.
    pub fn link_search_result(&self, series_ids: &[i64], anime: &crate::anilist::CatalogAnime) -> Result<()> {
        let sort_order = self.catalog_sort_order(anime.id)?;
        self.upsert_catalog_anime(anime, sort_order)?;
        for &id in series_ids {
            self.set_anilist_id(id, anime.id)?;
        }
        Ok(())
    }

    /// Linked series grouped by (franchise key, season), built once per run so
    /// the sibling step is a hash lookup instead of re-normalising every linked
    /// title for every candidate (that O(n*m) loop cost seconds at real size
    /// while the DB mutex was held).
    fn sibling_index(linked: &[(i64, String, String, i64)]) -> SiblingIndex {
        let mut map: SiblingIndex = std::collections::HashMap::new();
        for (id, title, slug, anilist_id) in linked {
    
            // Ambiguous seasons ("The Final Season") must not pool with season 1.
            let Some(season) = crate::matching::effective_season(title, slug) else { continue };
            map.entry((crate::matching::franchise_dedup_key(title), season as i64))
                .or_default()
                .push((*id, title.clone(), slug.clone(), *anilist_id));
        }
        map
    }

    /// A show long-running enough that the sites split it into arc rows
    /// ("One Piece: Arco de Elbaph"). Only those may inherit their parent's
    /// link from the colon-stripped base title; for a short show the colon
    /// tail is a season ("Kami no Tou: Ouji no Kikan" is Tower of God season
    /// 2, not season 1). AniList leaves `episodes` empty while a show airs,
    /// so a NULL episode count alone does not make a show a long runner: we
    /// require `next_episode > 60` or >60 rows in `anilist_episode_map`.
    pub(super) fn is_long_runner(&self, anilist_id: i64) -> Result<bool> {
        let row: Option<(Option<i64>, Option<i64>)> = self
            .conn
            .query_row(
                "SELECT episodes, next_episode FROM anilist_catalog WHERE id=?1",
                [anilist_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let Some((episodes, next_episode)) = row else {
            return Ok(false);
        };
        if let Some(n) = episodes {
            return Ok(n >= 60);
        }
        if next_episode.is_some_and(|ne| ne > 60) {
            return Ok(true);
        }
        let map_count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM anilist_episode_map WHERE anilist_id=?1",
            [anilist_id],
            |r| r.get(0),
        )?;
        Ok(map_count > 60)
    }

    /// The permissive counterpart of `is_long_runner`, for the repair pass that
    /// CLEARS existing links: there a missing total (or a missing catalog row)
    /// must keep the link. A long runner between airings or on hiatus has no
    /// countdown and may not have its episode map yet, and wiping a correct
    /// "One Piece: Arco de Elbaph" link on startup is worse than keeping a
    /// doubtful one. Creating new links uses the strict `is_long_runner`.
    fn may_be_long_runner(&self, anilist_id: i64) -> Result<bool> {
        let episodes: Option<i64> = self
            .conn
            .query_row("SELECT episodes FROM anilist_catalog WHERE id=?1", [anilist_id], |r| r.get(0))
            .optional()?
            .flatten();
        Ok(episodes.is_none_or(|n| n >= 60))
    }

    fn resolve_catalog_link(
        &self,

        title: &str,
        slug: &str,
        ignore_series_id: i64,
        index: &crate::matching::CatalogIndex,
        catalog_titles: &std::collections::HashMap<i64, Vec<String>>,
        siblings: &SiblingIndex,
    ) -> Result<Option<i64>> {
        // A live-action adaptation is a different work: it may only link to an
        // entry that is itself live action, never to the anime (fuzzy matching
        // would otherwise pick the anime again on every run).
        let live = crate::matching::is_live_action(title);
        let is_consistent = |id: i64| -> bool {
            if let Some(titles) = catalog_titles.get(&id) {
                if live && !titles.iter().any(|t| crate::matching::is_live_action(t)) {
                    return false;
                }
                let refs: Vec<&str> = titles.iter().map(String::as_str).collect();
                crate::matching::season_consistent_with_slug(title, slug, &refs)
            } else {
                false
            }
        };

        if let Some(id) = index.lookup(&[title]) {
            if is_consistent(id) { return Ok(Some(id)); }
        }
        if let Some(id) = index.fuzzy_lookup(&[title]) {
            if is_consistent(id) { return Ok(Some(id)); }
        }


        let s = crate::matching::effective_season(title, slug);
        // Only a clearly season-1 title may fall back to its base ("X (2011)",
        // "X Part 1"); an ambiguous one ("X The Final Season") must stay unlinked
        // rather than join season 1.
        if s == Some(1) && !live {
            let base = crate::db::stats::franchise_base_title(title);
            if base != title {
                let colon = title.contains(':');
                for id in [index.lookup(&[&base]), index.fuzzy_lookup(&[&base])].into_iter().flatten() {
                    if is_consistent(id) && (!colon || self.is_long_runner(id)?) {
                        return Ok(Some(id));
                    }
                }
            }
        }

        // Sites spell season 2 as "X Temporada 2" while AniList often lists
        // the same season as "X 2" (a synonym or the romaji): try that spelling.
        if let Some(n) = s.filter(|&n| n >= 2 && !live) {
            let tokens: Vec<&str> = title.split_whitespace().collect();
            let stem = crate::matching::strip_season_markers(tokens, crate::matching::normalize_title).join(" ");
            if !stem.is_empty() {
                let bare = format!("{stem} {n}"); let s_cand = format!("{stem} Season {n}");
                for id in [index.lookup(&[&bare, &s_cand]), index.fuzzy_lookup(&[&bare, &s_cand])].into_iter().flatten() {
                    if is_consistent(id) {
    






                        return Ok(Some(id));
                    }
                }
            }
        }

        let Some(season) = s.filter(|_| !live) else {
            return Ok(None);
        };
        let key = (crate::matching::franchise_dedup_key(title), season as i64);
        let mut candidate_ids = std::collections::HashSet::new();
        for (sib_id, sib_title, sib_slug, sib_anilist_id) in siblings.get(&key).into_iter().flatten() {
            if *sib_id == ignore_series_id { continue; }
            if let Some(titles) = catalog_titles.get(sib_anilist_id) {
                let refs: Vec<&str> = titles.iter().map(String::as_str).collect();
                if crate::matching::season_consistent_with_slug(sib_title, sib_slug, &refs) {
                    candidate_ids.insert(*sib_anilist_id);
                }
            }
        }
        if candidate_ids.len() == 1 {
            return Ok(Some(candidate_ids.into_iter().next().unwrap()));
        }

        Ok(None) // If siblings disagree, do not link. a wrong automatic link is worse than none.
    }

    /// Re-resolve every link that contradicts the series' own season (a
    /// "... Temporada 5" row linked to the plain season-1 entry, typically
    /// because the season-5 entry was not in the catalog yet when it linked and
    /// links were never revisited). Relinks to the right entry when one can be
    /// found, otherwise clears the link: a wrong link also makes the status sync
    /// trust the wrong season's AniList status. Idempotent. Returns how many
    /// rows changed. `link_series_to_catalog` runs this first (sharing its
    /// index); this standalone form exists for the tests.
    #[cfg(test)]
    pub fn repair_season_mislinks(&self) -> Result<usize> {
        let rows = self.catalog_titles_for_index()?;
        let index = crate::matching::CatalogIndex::build(&rows);
        let catalog_titles: std::collections::HashMap<i64, Vec<String>> =
            rows.into_iter().map(|row| (row.id, row.titles)).collect();
        self.repair_season_mislinks_with(&index, &catalog_titles)
    }

    /// Same as `repair_season_mislinks` but reuses an index the caller already
    /// built (building it over ~22k titles is the expensive part, and this
    /// runs inside a command that holds the DB mutex).
    fn repair_season_mislinks_with(
        &self,
        index: &crate::matching::CatalogIndex,
        catalog_titles: &std::collections::HashMap<i64, Vec<String>>,
    ) -> Result<usize> {
        let mut to_repair = Vec::new();
        let mut live_action_mislinks = Vec::new();

        let mut stmt = self.conn.prepare("SELECT id, title, slug, anilist_id FROM series WHERE anilist_id IS NOT NULL")?;
        let rows = stmt.query_map([], |r| {
            let id: i64 = r.get(0)?;
            let title: String = r.get(1)?;
            let slug: String = r.get(2)?;
            let anilist_id: i64 = r.get(3)?;
            Ok((id, title, slug, anilist_id))
        })?;

        let mut linked_series = Vec::new();
        for row in rows {
            let (id, title, slug, anilist_id) = row?;
            linked_series.push((id, title.clone(), slug.clone(), anilist_id));

            // A live-action adaptation linked to the anime is a different work:
            // clear it (never re-resolve, fuzzy matching would pick the anime again).
            if crate::matching::is_live_action(&title) {
                let entry_is_live = catalog_titles
                    .get(&anilist_id)
                    .is_some_and(|t| t.iter().any(|x| crate::matching::is_live_action(x)));
                if !entry_is_live {
                    live_action_mislinks.push(id);
                    continue;
                }
            }
            // The row's own season uses the strict reading (explicit markers, trailing
            // numerals): a numeral before a colon ("Lupin III: ...") names a work, not a
            // season, and this pass removes links, so it must not guess.
            let s = crate::matching::effective_season(&title, &slug);
            // A colon-qualified title linked to a short show can only have got
            // there through the base-title fallback (see `is_long_runner`): keep
            // it only when its own full title matches that entry.
            if s == Some(1) && title.contains(':') && crate::db::stats::franchise_base_title(&title) != title {
                // Cheapest first. The entry's own titles are authoritative: two
                // entries can share a normalized title (a show and its movie), and
                // the index resolves that to the more popular one, which may not
                // be the one this row was (rightly) linked to.
                let own_title_matches = catalog_titles.get(&anilist_id).is_some_and(|titles| {
                    let variants = crate::matching::title_variants_norm(&title);
                    titles.iter().any(|t| variants.contains(&crate::matching::normalize_title(t)))
                });
                let justified = self.may_be_long_runner(anilist_id)?
                    || own_title_matches
                    || index.lookup(&[&title]) == Some(anilist_id)
                    || index.fuzzy_lookup(&[&title]) == Some(anilist_id);
                if !justified {
                    live_action_mislinks.push(id);
                    continue;
                }
            }
            if s.unwrap_or(0) >= 2 {
                // Clear only on positive evidence. An absent catalog row is
                // unknown, and so is an entry with a different subtitle ("Kaguya-sama
                // Temporada 3" -> "...: Ultra Romantic", which carries no number).
                // Evidence = the entry is the plain-titled show (same franchise key,
                // so it is season 1) or it explicitly names another season.
                let contradicts = catalog_titles.get(&anilist_id).is_some_and(|titles| {
                    let refs: Vec<&str> = titles.iter().map(String::as_str).collect();
                    if crate::matching::season_consistent_with_slug(&title, &slug, &refs) {
                        return false;
                    }
                    let key = crate::matching::franchise_dedup_key(&title);
                    let base_key = crate::matching::franchise_dedup_key(
                        &crate::db::stats::franchise_base_title(&title),
                    );
                    let all_catalog_titles_plain = refs.iter().all(|t| {
                        crate::matching::extract_season_number_from_title(t).is_none_or(|n| n <= 1)
                    });
                    refs.iter().any(|t| {
                        let catalog_key = crate::matching::franchise_dedup_key(t);
                        catalog_key == key
                            || (all_catalog_titles_plain && catalog_key == base_key)
                            || crate::matching::extract_season_number_from_title(t).is_some_and(|n| n >= 2)
                    })
                });
                if contradicts {
                    to_repair.push((id, title, slug));
                }
            }
        }

        if to_repair.is_empty() && live_action_mislinks.is_empty() {
            return Ok(0);
        }

        let mut changed = 0;
        for id in live_action_mislinks {
            self.conn.execute("UPDATE series SET anilist_id = NULL WHERE id = ?1", [id])?;
            changed += 1;
        }
        let siblings = Self::sibling_index(&linked_series);
        for (id, title, slug) in to_repair {
            if let Some(new_id) = self.resolve_catalog_link(&title, &slug, id, index, catalog_titles, &siblings)? {
                self.set_anilist_id(id, new_id)?;
            } else {
                self.conn.execute("UPDATE series SET anilist_id = NULL WHERE id = ?1", [id])?;
            }
            changed += 1;
        }

        if changed > 0 {
            eprintln!("[link] repaired {} season mislinks", changed);
        }

        Ok(changed)
    }

    #[cfg(test)] // superseded in production; kept as a test oracle
    pub fn link_series_to_catalog(&self) -> Result<i64> {
        let rows = self.catalog_titles_for_index()?;
        let index = crate::matching::CatalogIndex::build(&rows);
        self.link_series_to_catalog_with(&index, rows)
    }

    /// The linking pass given an already-built index. Building the index over
    /// the whole catalog is the expensive CPU part and needs no DB access, so
    /// the startup command builds it *outside* the `state.db` mutex and only
    /// holds the lock for this step; holding it for the build as well froze
    /// every data query for several seconds after each launch.
    pub fn link_series_to_catalog_with(
        &self,
        index: &crate::matching::CatalogIndex,
        rows: Vec<crate::matching::CatalogTitleRow>,
    ) -> Result<i64> {
        let catalog_titles: std::collections::HashMap<i64, Vec<String>> =
            rows.into_iter().map(|row| (row.id, row.titles)).collect();

        self.repair_season_mislinks_with(index, &catalog_titles)?;

        // Snapshot AFTER the repair so the sibling step sees the corrected links.
        let mut stmt = self.conn.prepare("SELECT id, title, slug, anilist_id FROM series WHERE anilist_id IS NOT NULL")?;
        let linked_series: Vec<(i64, String, String, i64)> = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?.collect::<rusqlite::Result<_>>()?;

        let siblings = Self::sibling_index(&linked_series);
        let mut linked = 0i64;
        for (series_id, title, slug) in self.series_needing_catalog_link()? {
            if let Some(new_id) = self.resolve_catalog_link(&title, &slug, series_id, index, &catalog_titles, &siblings)? {
                self.set_anilist_id(series_id, new_id)?;
                linked += 1;
            }
        }
        Ok(linked)
    }
}
