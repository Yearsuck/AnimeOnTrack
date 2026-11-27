use super::*;

impl Db {
    /// Global minimum score floor for the Descubrir catalog deck.
    /// Absent key = `None` = unbounded ("all scores") — default behavior.
    pub fn get_deck_min_score(&self) -> Result<Option<i64>> {
        Ok(self.get_setting("deck_min_score")?.and_then(|v| v.parse().ok()))
    }

    pub fn set_deck_min_score(&self, value: Option<i64>) -> Result<()> {
        match value {
            Some(v) => self.set_setting("deck_min_score", &v.to_string()),
            None => self.delete_setting("deck_min_score"),
        }
    }

    /// A random synced entry carrying `genre`, subject to a quality floor —
    /// powers Descubrir's catalog-only swipe deck. The command layer
    /// (`discover_catalog_card`) owns the taste-weighted *genre* pick (via
    /// `get_genre_affinity` + `weighted_pick_index`, mirroring the site
    /// path); this just does the uniform-random pick *within* that genre,
    /// same division of labor `random_catalog_anime`'s callers already used.
    ///
    /// Quality floor: `format` restricted to the "real anime" formats
    /// (drops `MUSIC` and manga-side formats like `TV_SHORT` isn't excluded
    /// by name but is excluded implicitly by not being in this list), and
    /// `popularity >= 500` to cut the long tail of essentially-unknown
    /// entries. Already-decided titles (a `series` row with this
    /// `anilist_id`) are excluded so the deck never re-offers a decided
    /// card — see the `anilist_id` column comment in `init_schema`.
    ///
    /// `banned_formats` (from `get_banned_formats`) is subtracted from the
    /// default whitelist `['TV','MOVIE','OVA','ONA','SPECIAL']` to build the
    /// actual `IN (...)` clause dynamically — an unrecognized entry is
    /// harmlessly ignored (nothing in the whitelist matches it to remove).
    /// If every whitelisted format ends up banned, returns `Ok(None)`
    /// without querying — an empty SQL `IN ()` is invalid, and there's
    /// nothing left to offer anyway.
    ///
    /// `banned_genres` (from `get_banned_genres`) excludes any candidate that
    /// carries a banned genre *at all*, via its own `anilist_catalog_genres`
    /// rows — not just candidates whose one requested `genre` is banned. The
    /// command layer already keeps a banned genre from being picked as the
    /// round's outer genre (`filter_candidate_genres`), but that alone let a
    /// title tagged both "Accion" (allowed, and the genre this round sampled)
    /// and "Ecchi" (banned) through every filter and onto the deck — the ban
    /// list was doing half a job that the format ban list was already doing in
    /// full. Matching is `COLLATE NOCASE`, mirroring the case-insensitive
    /// comparison `filter_candidate_genres`/`banned_formats` use. An empty
    /// list adds no clause at all.
    /// `excluded_norm_titles` — normalized (`matching::normalize_title`)
    /// titles of series the user has already engaged with (followed,
    /// wanted, discarded, or marked watched-externally; see
    /// `engaged_series_titles`), scoped to the caller's source. Needed
    /// because the pre-existing `anilist_id NOT IN series` clause below only
    /// catches engaged series that got linked to a real AniList id — and
    /// followed *site*-scraped series almost never do (linking is on-demand,
    /// rarely triggered), so without this a followed show with the exact
    /// same title as a catalog entry keeps getting re-offered by the deck
    /// (see docs/superpowers/specs/2026-07-12-discover-exclude-followed-design.md).
    ///
    /// Rather than `ORDER BY RANDOM() LIMIT 1` and rejecting only that one
    /// row (which would silently starve the deck for any genre where the
    /// single random pick happens to be engaged), this pulls a
    /// `BATCH_SIZE`-row random batch and keeps every `survivor` — a
    /// candidate whose title/`title_romaji`/`title_english` (skipping NULLs)
    /// all normalize to something outside `excluded_norm_titles`. Kept as an
    /// explicit `survivors` vec so the taste-scoring pass
    /// (`recommend::pick_recommended`) can score the whole batch and pick the
    /// best-fitting one instead of just the first, without touching the
    /// batch-fetch or exclusion logic above it — see
    /// docs/superpowers/specs/2026-07-12-discover-recommendation-engine-design.md.
    /// Genres are backfilled per-survivor (not just for the final pick)
    /// because `score_candidate`'s secondary-genre-overlap term needs each
    /// candidate's full genre list — the batch query only joins on the one
    /// `genre` column being filtered on, so `row_to_catalog_anime` always
    /// returns an empty `genres` vec. `genre_affinity`/`format_affinity` come
    /// from the caller (`commands::discover_catalog_card`) — pass empty maps
    /// for the pre-recommendation-engine behavior (uniform pick over the
    /// batch, still exclusion/ban-correct).
    ///
    /// `recommended` selects the final-pick strategy, not the batch/ban/
    /// quality-floor/exclusion logic above it (that stays identical either
    /// way): `true` scores the survivors with `recommend::pick_recommended`
    /// (unchanged prior behavior); `false` bypasses scoring and returns
    /// `survivors.into_iter().next()` — the batch is already
    /// `ORDER BY RANDOM()`, so the first survivor is a uniform random pick.
    /// Passing empty affinity maps alone is NOT equivalent to `false`: an
    /// empty map still leaves `score_candidate`'s quality term active, which
    /// biases toward higher `average_score` rather than being genuinely
    /// uniform — see docs/superpowers/specs/2026-07-13-discover-recommendation-toggle-design.md.
    // The knobs are all independent recommendation inputs; bundling them into
    // a struct would just move the same nine fields elsewhere.
    /// Core implementation of `random_catalog_anime_in_genre_with_signals`, also accepting
    /// `excluded_ids` so batches can exclude cards already picked in the same refill without
    /// waiting for them to be saved to `series`.
    #[allow(clippy::too_many_arguments)]
    pub fn random_catalog_anime_in_genre_with_signals_and_excluded_ids(
        &self,
        genre: &str,
        banned_formats: &[String],
        banned_genres: &[String],
        excluded_norm_titles: &std::collections::HashSet<String>,
        excluded_ids: &std::collections::HashSet<i64>,
        genre_affinity: &std::collections::HashMap<String, f64>,
        format_affinity: &std::collections::HashMap<String, f64>,
        taste_signals: &crate::recommend::TasteSignals,
        recommended: bool,
        hide_upcoming: bool,
        min_start_date: Option<i64>,
        max_start_date: Option<i64>,
        min_score: Option<i64>,
    ) -> Result<Option<crate::anilist::CatalogAnime>> {
        const MIN_POPULARITY: i64 = 500;
        // Raised from 40: a random 40-title sample out of a genre that can
        // hold thousands of catalog rows too often missed the actual
        // best-scoring candidates before `recommend::pick_recommended` ever
        // saw them. 150 stays cheap (one local, indexed SQLite query) while
        // giving the scorer a meaningfully wider pool to rank.
        const BATCH_SIZE: i64 = 150;
        const DEFAULT_FORMATS: &[&str] = &["TV", "MOVIE", "OVA", "ONA", "SPECIAL"];
        let allowed: Vec<&str> = DEFAULT_FORMATS
            .iter()
            .copied()
            .filter(|f| !banned_formats.iter().any(|b| b.eq_ignore_ascii_case(f)))
            .collect();
        if allowed.is_empty() {
            return Ok(None);
        }
        let placeholders = allowed.iter().map(|_| "?").collect::<Vec<_>>().join(", ");
        // `status IS NULL` is deliberately included on the "keep" side: rows
        // synced before the `status` column existed (or an unsynced/partial
        // sync) stay eligible rather than vanishing from the deck the moment
        // this toggle is flipped on.
        let upcoming_clause =
            if hide_upcoming { "AND (c.status IS NULL OR c.status != 'NOT_YET_RELEASED')" } else { "" };
        // Same "don't punish unsynced data" policy as `upcoming_clause`: a
        // NULL `start_date` (unsynced, or synced before the column existed)
        // always passes, regardless of either bound. A non-NULL date must
        // satisfy BOTH bounds together (not either), so the min/max pair is
        // parenthesized as its own AND group, then OR'd with the NULL case.
        let mut date_clause = String::new();
        if min_start_date.is_some() || max_start_date.is_some() {
            date_clause.push_str("AND (c.start_date IS NULL OR (1=1");
            if min_start_date.is_some() {
                date_clause.push_str(" AND c.start_date >= ?");
            }
            if max_start_date.is_some() {
                date_clause.push_str(" AND c.start_date <= ?");
            }
            date_clause.push_str("))");
        }
        let mut score_clause = String::new();
        if min_score.is_some() {
            score_clause.push_str("AND c.average_score >= ?");
        }
        // A candidate carrying ANY banned genre is out, even when the genre
        // this round sampled is a perfectly allowed one — see the doc comment.
        // Blank entries are dropped so a stray empty line in the stored list
        // (`banned_genres` is a newline-joined setting) can't widen the ban.
        let banned_genres: Vec<&str> =
            banned_genres.iter().map(|g| g.trim()).filter(|g| !g.is_empty()).collect();
        let banned_genre_clause = if banned_genres.is_empty() {
            String::new()
        } else {
            let ph = banned_genres.iter().map(|_| "?").collect::<Vec<_>>().join(", ");
            format!(
                "AND c.id NOT IN (SELECT anilist_id FROM anilist_catalog_genres \
                 WHERE genre COLLATE NOCASE IN ({ph}))"
            )
        };
        let mut sorted_excluded_ids: Vec<i64> = excluded_ids.iter().copied().collect();
        sorted_excluded_ids.sort_unstable();
        let excluded_id_clause = if sorted_excluded_ids.is_empty() {
            String::new()
        } else {
            let ph = sorted_excluded_ids.iter().map(|_| "?").collect::<Vec<_>>().join(", ");
            format!("AND c.id NOT IN ({ph})")
        };
        let sql = format!(
            "SELECT c.id, c.title, c.title_romaji, c.title_english, c.cover_url, c.banner_url, c.format, c.episodes, c.average_score, c.popularity, c.url, c.status, c.duration, c.studio, c.start_date, c.next_airing_at, c.next_episode
             FROM anilist_catalog c
             JOIN anilist_catalog_genres g ON g.anilist_id = c.id
             WHERE g.genre = ?
               AND c.format IN ({placeholders})
               AND c.popularity >= ?
               AND c.id NOT IN (SELECT anilist_id FROM series WHERE anilist_id IS NOT NULL)
               {excluded_id_clause}
               {banned_genre_clause}
               {upcoming_clause}
               {date_clause}
               {score_clause}
             ORDER BY RANDOM() LIMIT {BATCH_SIZE}"
        );
        let mut params: Vec<Value> = vec![Value::Text(genre.to_string())];
        for f in &allowed {
            params.push(Value::Text((*f).to_string()));
        }
        params.push(Value::Integer(MIN_POPULARITY));
        for id in &sorted_excluded_ids {
            params.push(Value::Integer(*id));
        }
        // Bound in the same order the clauses appear in the SQL above: genre,
        // formats, popularity, excluded ids, banned genres, date bounds, then score floor.
        for g in &banned_genres {
            params.push(Value::Text((*g).to_string()));
        }
        if let Some(min) = min_start_date {
            params.push(Value::Integer(min));
        }
        if let Some(max) = max_start_date {
            params.push(Value::Integer(max));
        }
        if let Some(score) = min_score {
            params.push(Value::Integer(score));
        }
        let mut stmt = self.conn.prepare(&sql)?;
        let batch: Vec<crate::anilist::CatalogAnime> = stmt
            .query_map(rusqlite::params_from_iter(params.iter()), Self::row_to_catalog_anime)?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        let is_engaged_by_title = |anime: &crate::anilist::CatalogAnime| -> bool {
            [Some(&anime.title), anime.title_romaji.as_ref(), anime.title_english.as_ref()]
                .into_iter()
                .flatten()
                .any(|t| excluded_norm_titles.contains(&crate::matching::normalize_title(t)))
        };
        let mut survivors: Vec<crate::anilist::CatalogAnime> = batch
            .into_iter()
            .filter(|a| !excluded_ids.contains(&a.id) && !is_engaged_by_title(a))
            .collect();
        let survivor_ids: Vec<i64> = survivors.iter().map(|a| a.id).collect();
        let mut genres_map = self.list_catalog_genres_for(&survivor_ids)?;
        let mut tags_map = self.list_catalog_tags_for(&survivor_ids)?;
        for anime in &mut survivors {
            anime.genres = genres_map.remove(&anime.id).unwrap_or_default();
            if anime.tags.is_empty() {
                anime.tags = tags_map.remove(&anime.id).unwrap_or_default();
            }
        }

        if recommended {
            let now_unix = chrono::Utc::now().timestamp();
            Ok(crate::recommend::pick_recommended(
                survivors,
                genre_affinity,
                format_affinity,
                taste_signals,
                genre,
                now_unix,
            ))
        } else {
            Ok(survivors.into_iter().next())
        }
    }

    /// Same pick with nothing excluded by id. The app calls the `_and_excluded_ids` form
    /// (a deck batch excludes the cards it already picked); tests keep this shape.
    #[cfg(test)]
    #[allow(clippy::too_many_arguments)]
    pub fn random_catalog_anime_in_genre_with_signals(
        &self,
        genre: &str,
        banned_formats: &[String],
        banned_genres: &[String],
        excluded_norm_titles: &std::collections::HashSet<String>,
        genre_affinity: &std::collections::HashMap<String, f64>,
        format_affinity: &std::collections::HashMap<String, f64>,
        taste_signals: &crate::recommend::TasteSignals,
        recommended: bool,
        hide_upcoming: bool,
        min_start_date: Option<i64>,
        max_start_date: Option<i64>,
        min_score: Option<i64>,
    ) -> Result<Option<crate::anilist::CatalogAnime>> {
        self.random_catalog_anime_in_genre_with_signals_and_excluded_ids(
            genre,
            banned_formats,
            banned_genres,
            excluded_norm_titles,
            &std::collections::HashSet::new(),
            genre_affinity,
            format_affinity,
            taste_signals,
            recommended,
            hide_upcoming,
            min_start_date,
            max_start_date,
            min_score,
        )
    }

    /// Same pick without taste signals (tags/studios); the app calls `_with_signals`, tests keep this shape.
    #[cfg(test)]
    #[allow(clippy::too_many_arguments)]
    pub fn random_catalog_anime_in_genre(
        &self,
        genre: &str,
        banned_formats: &[String],
        banned_genres: &[String],
        excluded_norm_titles: &std::collections::HashSet<String>,
        genre_affinity: &std::collections::HashMap<String, f64>,
        format_affinity: &std::collections::HashMap<String, f64>,
        recommended: bool,
        hide_upcoming: bool,
        min_start_date: Option<i64>,
        max_start_date: Option<i64>,
        min_score: Option<i64>,
    ) -> Result<Option<crate::anilist::CatalogAnime>> {
        self.random_catalog_anime_in_genre_with_signals(
            genre,
            banned_formats,
            banned_genres,
            excluded_norm_titles,
            genre_affinity,
            format_affinity,
            &crate::recommend::TasteSignals::default(),
            recommended,
            hide_upcoming,
            min_start_date,
            max_start_date,
            min_score,
        )
    }

    /// Normalized-title exclusion set input: titles of `series` rows the
    /// user has already decided on — followed, "want", "discarded", or
    /// watched-externally, on ANY of the app's sites, not just the active
    /// one. See `random_catalog_anime_in_genre`'s doc comment for why this
    /// is needed alongside (not instead of) its `anilist_id NOT IN series`
    /// clause. Same site-scoping bug class already fixed for Estadísticas
    /// and cross-site progress sync — a title engaged with on site B must
    /// never keep getting re-offered by the Descubrir deck just because
    /// site A happens to be active. Returns raw titles (not yet normalized)
    /// — callers normalize via `matching::normalize_title` when building the
    /// exclusion set, keeping this function a pure DB read.
    pub fn engaged_series_titles(&self) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT title FROM series
             WHERE followed=1 OR watched_externally=1 OR backlog_status IN ('want','discarded')",
        )?;
        let titles = stmt.query_map([], |r| r.get::<_, String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(titles)
    }
}
