use super::*;

fn push_swipe_history(state: &State<'_, AppState>, sid: i64) {
    push_history(&mut state.swipe_history.lock().unwrap(), sid);
}

#[derive(serde::Deserialize)]
pub enum SwipeDecision {
    Seen,
    Want,
    Discard,
}

/// Pick a random cached-or-fresh (genre, page) pair, scrape it if not
/// already buffered this session, filter out anything already decided,
/// shuffle, and pop one card. `Ok(None)` means the freshly-scraped page (or
/// buffer) had nothing left after filtering — a normal "everything on this
/// page was already decided" case, not an error; the frontend just calls
/// again.
///
/// Not wired to the Section B genre/format bans (`get_banned_genres`/
/// `get_banned_formats`): Descubrir.tsx only calls the catalog-deck path
/// (`discover_catalog_card`/`decide_catalog_card`) — this site-scraped swipe
/// deck has no live caller in the current UI, so extending it here would be
/// speculative scope creep. See the design spec's "Out of scope".
#[tauri::command]
pub async fn discover_swipe_card(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Option<FinishedCard>, String> {
    let src = get_source_id(&state)?;
    let site_id = get_active_site_id(&state);
    let a = get_active_adapter(&state)?;
    let mirrors = {
        let db = state.db.lock().unwrap();
        load_mirrors(&db, &site_id)?
    };
    let genres = ensure_genre_list(&app, &mirrors, &state, a.as_ref(), &site_id).await?;
    if genres.is_empty() {
        return Err("no se encontraron géneros; reintenta el escaneo".into());
    }
    // Taste-weighted genre pick: bias toward genres this user actually
    // follows/wants over ones they've discarded, instead of picking
    // uniformly at random. Falls back to uniform whenever every genre nets
    // <= 0 (nothing decided yet), so a fresh profile still discovers freely.
    let affinity = {
        let db = state.db.lock().unwrap();
        db.get_genre_affinity(src).map_err(|e| e.to_string())?
    };
    let weights: Vec<f64> = genres
        .iter()
        .map(|(_, name)| *affinity.get(name).unwrap_or(&0.0))
        .collect();
    let pick = weighted_pick_index(&weights).unwrap();
    let (slug, name) = &genres[pick];

    let last_page = state.swipe_last_page.lock().unwrap().get(slug).copied().unwrap_or(1);
    let page = pick_index(last_page as usize).map(|i| i as u32 + 1).unwrap_or(1);

    let known = {
        let db = state.db.lock().unwrap();
        db.known_series_urls(src).map_err(|e| e.to_string())?
    };

    let buffered = state.swipe_buffer.lock().unwrap().remove(&(slug.clone(), page));
    let mut cards = match buffered {
        Some(cards) => undecided_cards(cards, &known),
        None => {
            let path = a.genre_page_url("", slug, page);
            let (scraped, raw_cards, _mirror) =
                scrape_via_mirrors(&app, &mirrors, &path, true, |html| a.parse_finished_page(html)).await?;
            state
                .swipe_last_page
                .lock()
                .unwrap()
                .insert(slug.clone(), a.parse_pagination_last_page(&scraped.html));
            let mut fresh = undecided_cards(raw_cards, &known);
            shuffle(&mut fresh);
            fresh
        }
    };

    let Some(mut card) = cards.pop() else {
        return Ok(None);
    };
    if !cards.is_empty() {
        state.swipe_buffer.lock().unwrap().insert((slug.clone(), page), cards);
    }
    card.matched_genre = Some(name.clone());
    state.swipe_served.lock().unwrap().insert(card.url.clone(), card.clone());
    Ok(Some(card))
}

#[tauri::command]
pub async fn decide_swipe(
    app: AppHandle,
    state: State<'_, AppState>,
    series_url: String,
    decision: SwipeDecision,
) -> Result<(), String> {
    let src = get_source_id(&state)?;
    let site_id = get_active_site_id(&state);
    let a = get_active_adapter(&state)?;
    let card = state
        .swipe_served
        .lock()
        .unwrap()
        .remove(&series_url)
        .ok_or_else(|| "unknown swipe card; call discover_swipe_card first".to_string())?;

    let series = Series {
        id: 0,
        slug: slug_from_url(&card.url),
        title: card.title.clone(),
        url: card.url.clone(),
        cover_url: card.poster_url.clone(),
        is_airing: false,
        followed: false, next_episode_at: None, site_episode_count: None,
    };

    let sid = match decision {
        SwipeDecision::Discard => {
            let db = state.db.lock().unwrap();
            let sid = db.upsert_series(src, &series).map_err(|e| e.to_string())?;
            db.set_kind(sid, &card.kind).map_err(|e| e.to_string())?;
            db.set_backlog_status(sid, Some("discarded")).map_err(|e| e.to_string())?;
            sid
        }
        SwipeDecision::Want => {
            let mirrors = {
                let db = state.db.lock().unwrap();
                load_mirrors(&db, &site_id)?
            };
            let detail = fetch_series_detail(&app, &mirrors, &card.url, a.as_ref()).await?;
            let db = state.db.lock().unwrap();
            let sid = db.upsert_series(src, &series).map_err(|e| e.to_string())?;
            db.set_kind(sid, detail.kind.as_deref().unwrap_or(&card.kind)).map_err(|e| e.to_string())?;
            db.insert_series_genres(sid, &detail.genres).map_err(|e| e.to_string())?;
            db.set_backlog_status(sid, Some("want")).map_err(|e| e.to_string())?;
            sid
        }
        SwipeDecision::Seen => {
            let mirrors = {
                let db = state.db.lock().unwrap();
                load_mirrors(&db, &site_id)?
            };
            let detail = fetch_series_detail(&app, &mirrors, &card.url, a.as_ref()).await?;
            let eps = fetch_episode_list_for(&app, &mirrors, &card.url, a.as_ref()).await?;
            let db = state.db.lock().unwrap();
            let sid = db.upsert_series(src, &series).map_err(|e| e.to_string())?;
            db.set_followed(sid, true).map_err(|e| e.to_string())?;
            db.set_kind(sid, detail.kind.as_deref().unwrap_or(&card.kind)).map_err(|e| e.to_string())?;
            db.insert_series_genres(sid, &detail.genres).map_err(|e| e.to_string())?;
            for mut e in eps {
                e.series_id = sid;
                e.seen = true;
                db.insert_episode(&e).map_err(|e| e.to_string())?;
            }
            sid
        }
    };
    push_swipe_history(&state, sid);
    Ok(())
}

/// Undo the most recent decide_swipe/decide_catalog_card call (Ctrl+Z) by
/// popping it off the front of `swipe_history` and hard-deleting the series
/// row it created. Calling this with nothing left to undo is a no-op, not an
/// error. For reaching further back than just the most recent, see
/// `undo_swipe_entry`.
#[tauri::command]
pub fn undo_last_swipe(state: State<'_, AppState>) -> Result<(), String> {
    let sid = state.swipe_history.lock().unwrap().pop_front();
    if let Some(sid) = sid {
        let db = state.db.lock().unwrap();
        db.delete_series(sid).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// One entry in the swipe-history strip — a still-live `series` row from
/// `swipe_history`, with its decision derived live from the row's current
/// classification flags (not stored separately, so a `reclassify_series`
/// call in between is reflected automatically on the next read).
#[derive(Serialize, Clone)]
pub struct SwipeHistoryItem {
    pub series_id: i64,
    pub title: String,
    pub poster_url: Option<String>,
    /// The row's `series.url` — the frontend uses this to clear the card
    /// from its client-side decided-set (`decidedUrlsRef`) when it
    /// legitimately returns to the deck via `returnToDeck`. See the design
    /// spec's Fix 1.
    pub url: String,
    /// "seen" | "want" | "discard" | "none", derived from the row's live
    /// `watched_externally`/`backlog_status` — see `decision_for_history_row`.
    pub decision: String,
}

fn decision_for_history_row(row: &crate::db::SwipeHistoryRow) -> String {
    if row.watched_externally {
        "seen".to_string()
    } else {
        match row.backlog_status.as_deref() {
            Some("want") => "want".to_string(),
            Some("discarded") => "discard".to_string(),
            _ => "none".to_string(),
        }
    }
}

/// Up to `SWIPE_HISTORY_CAP` most-recent swipe decisions still live in the
/// DB, most-recent first — the Descubrir history strip's data source. An id
/// in `swipe_history` whose row was already deleted (a prior undo/return-to-
/// deck) is silently skipped rather than erroring, so the deque self-heals
/// instead of requiring bookkeeping on every deletion path.
#[tauri::command]
pub fn list_swipe_history(state: State<'_, AppState>) -> Result<Vec<SwipeHistoryItem>, String> {
    let ids: Vec<i64> = state.swipe_history.lock().unwrap().iter().copied().collect();
    let db = state.db.lock().unwrap();
    let mut out = Vec::with_capacity(ids.len());
    for sid in ids {
        if let Some(row) = db.get_series_for_history(sid).map_err(|e| e.to_string())? {
            out.push(SwipeHistoryItem {
                series_id: sid,
                title: row.title.clone(),
                poster_url: row.poster_url.clone(),
                url: row.url.clone(),
                decision: decision_for_history_row(&row),
            });
        }
    }
    Ok(out)
}

/// Return a specific past card to the deck: hard-delete its series row (so
/// the catalog picker's `anilist_id NOT IN (...)` exclusion no longer
/// applies) and drop it from `swipe_history`. Unlike `undo_last_swipe`, this
/// targets an arbitrary entry in the history strip, not just the front —
/// "undo this one" rather than "undo my last action". A no-op (not an
/// error) if `series_id` isn't in the history or its row is already gone.
#[tauri::command]
pub fn undo_swipe_entry(state: State<'_, AppState>, series_id: i64) -> Result<(), String> {
    state.swipe_history.lock().unwrap().retain(|&id| id != series_id);
    let db = state.db.lock().unwrap();
    db.delete_series(series_id).map_err(|e| e.to_string())?;
    Ok(())
}

/// The Descubrir deck's user-configured genre/format bans (Section B) — see
/// `db::{get_banned_genres, get_banned_formats}`. Global (not per-site).
#[derive(Serialize, Clone)]
pub struct DeckBans {
    pub genres: Vec<String>,
    pub formats: Vec<String>,
}

/// Read the current deck bans for the Descubrir "Filtros" sub-view.
#[tauri::command]
pub fn get_deck_bans(state: State<'_, AppState>) -> Result<DeckBans, String> {
    let db = state.db.lock().unwrap();
    let genres = db.get_banned_genres().map_err(|e| e.to_string())?;
    let formats = db.get_banned_formats().map_err(|e| e.to_string())?;
    Ok(DeckBans { genres, formats })
}

/// Persist the deck bans. Takes effect on the very next `discover_catalog_card`
/// call — no cache to invalidate, `discover_catalog_card` reads the settings
/// table fresh every time it's called.
#[tauri::command]
pub fn set_deck_bans(state: State<'_, AppState>, genres: Vec<String>, formats: Vec<String>) -> Result<(), String> {
    let db = state.db.lock().unwrap();
    db.set_banned_genres(&genres).map_err(|e| e.to_string())?;
    db.set_banned_formats(&formats).map_err(|e| e.to_string())?;
    Ok(())
}

/// Genres never offered by the catalog swipe deck, regardless of taste
/// weighting — hardcoded, not a setting (see spec). `Hentai` alone is 1,652
/// of the ~22,400 synced rows (~7%); left in, a uniform-ish deck would show
/// it disproportionately often relative to how any real user wants to
/// browse. `Ecchi` is excluded alongside it for the same reason (adjacent
/// content policy, not a taste signal worth surfacing here).
const EXCLUDED_CATALOG_GENRES: &[&str] = &["Hentai", "Ecchi"];

/// Bounded retry count for `discover_catalog_card`'s genre pick: if the
/// weighted-picked genre turns out to have no undecided candidate left
/// (everything in it already decided), try the next-best genre instead of
/// immediately declaring the deck exhausted.
const MAX_GENRE_ATTEMPTS: usize = 5;

/// Genres eligible for the catalog deck: every synced genre except the
/// always-on baseline (`EXCLUDED_CATALOG_GENRES` — Hentai/Ecchi) and the
/// user's own banned-genre list (Section B, `get_banned_genres`), unioned.
/// The baseline can't be lifted by a user setting; bans are strictly
/// additive to it. Factored out of `discover_catalog_card` so the filter is
/// unit-testable without a `State<AppState>`.
fn filter_candidate_genres(all_genres: Vec<String>, banned_genres: &[String]) -> Vec<String> {
    all_genres
        .into_iter()
        .filter(|g| !EXCLUDED_CATALOG_GENRES.iter().any(|ex| ex.eq_ignore_ascii_case(g)))
        .filter(|g| !banned_genres.iter().any(|b| b.eq_ignore_ascii_case(g)))
        .collect()
}

/// Pick a taste-weighted genre (mirroring `discover_swipe_card`'s scheme:
/// `get_genre_affinity` + `weighted_pick_index`, uniform fallback when
/// nothing's been decided yet — but see the dampening note below) and ask
/// the DB for a taste-scored undecided, quality-floored catalog entry in it
/// (`recommend::pick_recommended`, see
/// docs/superpowers/specs/2026-07-12-discover-recommendation-engine-design.md).
/// Local + instant (no live AniList call per swipe, so no rate-limit
/// exposure from normal browsing). Catalog cards carry no episode data
/// (AniList is metadata-only), so they're decided through
/// `decide_catalog_card` rather than `decide_swipe`, which assumes a
/// scraped-site URL it can fetch an episode list from.
///
/// The outer genre-pick weights are run through
/// `recommend::dampen_genre_weight` (sub-linear, `w' = max(0,score)^0.6`)
/// before `weighted_pick_index` — raw affinity sums let one heavily-followed
/// genre swamp every other candidate; dampening compresses that lead without
/// flipping the order, so the deck still favors the user's top genre without
/// collapsing into showing only that genre. Cold start (nothing
/// followed/decided) still degrades to `weighted_pick_index`'s uniform
/// fallback: dampening never turns a non-positive score into a positive one.
///
/// `Ok(None)` means the deck is genuinely exhausted: every candidate genre
/// (after excluding Hentai/Ecchi) either has zero synced titles passing the
/// quality floor or every one of them has already been decided, and
/// `MAX_GENRE_ATTEMPTS` genre picks in a row all came up empty.
/// `recommended` selects the deck mode (see `DiscoverModeToggle` on the
/// frontend, persisted in localStorage `aot.discoverMode`): `true` is the
/// taste-weighted behavior documented above, unchanged. `false` ("Aleatorio")
/// builds **empty** affinity maps instead — which makes the outer genre pick
/// degrade to `weighted_pick_index`'s uniform fallback (all-dampened weights
/// are 0, same cold-start path) — and threads `recommended` into
/// `random_catalog_anime_in_genre` so the inner per-candidate pick bypasses
/// scoring too. Empty maps ALONE are not enough for the inner pick: with
/// `recommended=true` `score_candidate`'s quality term stays active even with
/// empty genre/format maps, still biasing toward high `average_score` — see
/// docs/superpowers/specs/2026-07-13-discover-recommendation-toggle-design.md.
#[tauri::command]
pub fn discover_catalog_card(
    state: State<'_, AppState>,
    recommended: bool,
) -> Result<Option<FinishedCard>, String> {
    let src = get_source_id(&state)?;
    let db = state.db.lock().unwrap();

    let affinity = if recommended {
        db.get_genre_affinity(src).map_err(|e| e.to_string())?
    } else {
        HashMap::new()
    };
    // Format-affinity map for the inner (per-candidate) score — built once
    // per call, not per genre attempt or per candidate (see
    // `recommend::format_affinity_from_type_stats`'s doc comment: empty for
    // a brand-new user with no follows, which reduces the format term to 0
    // for everyone, i.e. cold start behaves like the pre-recommendation-
    // engine build). Also empty in "Aleatorio" mode.
    let format_affinity = if recommended {
        crate::recommend::format_affinity_from_type_stats(&db.get_type_stats(src).map_err(|e| e.to_string())?)
    } else {
        HashMap::new()
    };
    // Candidate-genre filter is EXCLUDED_CATALOG_GENRES (always-on baseline:
    // Hentai/Ecchi) union the user's own banned-genre list (Section B) — the
    // baseline can't be lifted by a user setting, bans are additive to it.
    let banned_genres = db.get_banned_genres().map_err(|e| e.to_string())?;
    let banned_formats = db.get_banned_formats().map_err(|e| e.to_string())?;
    let candidates = filter_candidate_genres(
        db.distinct_catalog_genres().map_err(|e| e.to_string())?,
        &banned_genres,
    );
    if candidates.is_empty() {
        return Ok(None);
    }

    // Titles of series the user has already engaged with (followed/want/
    // discarded/watched-externally), normalized so a followed *site*-scraped
    // series (almost always anilist_id=NULL — see `engaged_series_titles`'s
    // doc comment) still blocks a same-titled catalog entry from being
    // re-offered by the deck. Built once per call, not per genre attempt.
    let excluded_norm_titles: HashSet<String> = db
        .engaged_series_titles(src)
        .map_err(|e| e.to_string())?
        .iter()
        .map(|t| crate::matching::normalize_title(t))
        .collect();

    // Pool of candidate-genre indices still worth trying this call; a genre
    // that yields no undecided candidate is removed from the pool so a
    // retry never re-picks the same exhausted genre.
    let mut pool: Vec<usize> = (0..candidates.len()).collect();
    for _ in 0..MAX_GENRE_ATTEMPTS.min(candidates.len()) {
        if pool.is_empty() {
            break;
        }
        let pool_weights: Vec<f64> = pool
            .iter()
            .map(|&i| crate::recommend::dampen_genre_weight(*affinity.get(&candidates[i]).unwrap_or(&0.0)))
            .collect();
        let Some(pick_in_pool) = weighted_pick_index(&pool_weights) else { break };
        let genre_idx = pool[pick_in_pool];
        let genre = &candidates[genre_idx];

        if let Some(anime) = db
            .random_catalog_anime_in_genre(
                genre,
                &banned_formats,
                &excluded_norm_titles,
                &affinity,
                &format_affinity,
                recommended,
            )
            .map_err(|e| e.to_string())?
        {
            return Ok(Some(FinishedCard {
                title: anime.title,
                url: anime.url,
                poster_url: anime.cover_url,
                kind: anime.format.unwrap_or_default(),
                matched_genre: Some(genre.clone()),
            }));
        }
        pool.remove(pick_in_pool);
    }
    Ok(None)
}

/// Decide on a catalog-sourced card: Discard, Want, or Seen. Stores a
/// `series` row keyed by a synthetic `anilist-{id}` slug so it coexists with
/// scraped-site rows without colliding, and records the real numeric
/// `anilist_id` too (see its column comment in `db::init_schema`) so the
/// catalog picker can exclude it directly. Pushes onto `swipe_history` the
/// same way `decide_swipe` does so `undo_last_swipe`/`list_swipe_history`/
/// `undo_swipe_entry` all work uniformly across both sources.
///
/// `Seen` means "I've watched this outside the app" — AniList has no
/// episode list to mark, so unlike `decide_swipe`'s `Seen` (which fetches
/// and marks every episode watched) this just sets `watched_externally=1`
/// and clears `followed`/`backlog_status`, so the title is excluded from
/// future decks without pretending we have real watch progress for it.
#[tauri::command]
pub fn decide_catalog_card(
    state: State<'_, AppState>,
    anilist_id: i64,
    title: String,
    url: String,
    poster_url: Option<String>,
    genres: Vec<String>,
    format: String,
    decision: SwipeDecision,
) -> Result<i64, String> {
    let src = get_source_id(&state)?;
    let db = state.db.lock().unwrap();
    let series = Series {
        id: 0,
        slug: format!("anilist-{anilist_id}"),
        title,
        url,
        cover_url: poster_url,
        is_airing: false,
        followed: false, next_episode_at: None, site_episode_count: None,
    };
    let sid = db.upsert_series(src, &series).map_err(|e| e.to_string())?;
    db.set_anilist_id(sid, anilist_id).map_err(|e| e.to_string())?;
    db.set_kind(sid, &format).map_err(|e| e.to_string())?;
    db.insert_series_genres(sid, &genres).map_err(|e| e.to_string())?;
    match decision {
        SwipeDecision::Discard => {
            db.set_backlog_status(sid, Some("discarded")).map_err(|e| e.to_string())?;
        }
        SwipeDecision::Want => {
            db.set_backlog_status(sid, Some("want")).map_err(|e| e.to_string())?;
        }
        SwipeDecision::Seen => {
            db.set_backlog_status(sid, None).map_err(|e| e.to_string())?;
            db.set_followed(sid, false).map_err(|e| e.to_string())?;
            db.set_watched_externally(sid, true).map_err(|e| e.to_string())?;
        }
    }
    push_swipe_history(&state, sid);
    Ok(sid)
}

pub fn to_candidates(cards: &[FinishedCard]) -> Vec<crate::matching::TitleCandidate<'_>> {
    cards.iter().map(|c| crate::matching::TitleCandidate { title: &c.title, url: &c.url }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_candidate_genres_drops_baseline_and_user_bans_case_insensitively() {
        let all = vec![
            "Action".to_string(),
            "Hentai".to_string(),  // always-on baseline
            "ecchi".to_string(),   // baseline, different case
            "Horror".to_string(),  // user-banned below
            "Drama".to_string(),
        ];
        let out = filter_candidate_genres(all, &["horror".to_string()]);
        assert_eq!(out, vec!["Action".to_string(), "Drama".to_string()]);
    }

    #[test]
    fn decision_for_history_row_derives_from_live_flags() {
        let seen = crate::db::SwipeHistoryRow {
            title: "S".into(), poster_url: None, url: "u".into(),
            backlog_status: None, watched_externally: true,
        };
        assert_eq!(decision_for_history_row(&seen), "seen");
        let want = crate::db::SwipeHistoryRow {
            title: "W".into(), poster_url: None, url: "u".into(),
            backlog_status: Some("want".into()), watched_externally: false,
        };
        assert_eq!(decision_for_history_row(&want), "want");
        let disc = crate::db::SwipeHistoryRow {
            title: "D".into(), poster_url: None, url: "u".into(),
            backlog_status: Some("discarded".into()), watched_externally: false,
        };
        assert_eq!(decision_for_history_row(&disc), "discard");
        let none = crate::db::SwipeHistoryRow {
            title: "N".into(), poster_url: None, url: "u".into(),
            backlog_status: None, watched_externally: false,
        };
        assert_eq!(decision_for_history_row(&none), "none");
    }
}
