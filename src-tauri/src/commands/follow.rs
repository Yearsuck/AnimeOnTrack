use super::*;
use super::discover::to_candidates;

/// One search-results page, wrapped so a genuine "zero hits" answer can be
/// told apart from "this mirror's page didn't parse at all". See
/// `search_site`'s doc comment for why this indirection exists.
struct SearchOutcome {
    cards: Vec<FinishedCard>,
}

/// Pure carry-over planner: for each newly-scanned series, find the best
/// title match among series followed on OTHER sites and, if it clears
/// `matching::MATCH_THRESHOLD`, return `(index into new_site_series, watermark)`
/// so the caller can carry the follow + progress onto that row. One best match
/// per new-site series; nothing below the threshold carries (a false match
/// would wrongly follow + mark a show seen). Already-followed rows are handled
/// by `db::carry_follow`'s `followed=0` guard, not here. Split out pure so the
/// matching behaviour is unit-testable without a DB/scrape.
pub fn plan_carryover(
    new_site_series: &[Series],
    followed_elsewhere: &[(String, i64)],
) -> Vec<(usize, i64)> {
    if followed_elsewhere.is_empty() {
        return Vec::new();
    }
    let candidates: Vec<crate::matching::TitleCandidate> = followed_elsewhere
        .iter()
        .map(|(title, _)| crate::matching::TitleCandidate { title, url: "" })
        .collect();
    let mut out = Vec::new();
    for (i, s) in new_site_series.iter().enumerate() {
        if let Some(m) = crate::matching::best_match(&[&s.title], &candidates) {
            out.push((i, followed_elsewhere[m.index].1));
        }
    }
    out
}

#[tauri::command]
pub fn set_followed(
    state: State<'_, AppState>,
    series_id: i64,
    followed: bool,
) -> Result<(), String> {
    let db = state.db.lock().unwrap();
    db.set_followed(series_id, followed).map_err(|e| e.to_string())
}

/// Move a discarded row back to 'want'. If it never had its detail page
/// fetched (the common case — discard never fetches), fetch it now so the
/// row has genres before it shows up as a normal "want" backlog item; if it
/// already has genres (e.g. a previously-"want" row that got discarded),
/// just flip the status.
#[tauri::command]
pub async fn promote_discarded(
    app: AppHandle,
    state: State<'_, AppState>,
    series_id: i64,
) -> Result<(), String> {
    let site_id = get_active_site_id(&state);
    let a = get_active_adapter(&state)?;
    let (needs_detail, series_url, mirrors) = {
        let db = state.db.lock().unwrap();
        let needs_detail = db.series_needs_genre_backfill(series_id).map_err(|e| e.to_string())?;
        let url = db
            .get_series_url(series_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "series not found".to_string())?;
        (needs_detail, url, load_mirrors(&db, &site_id)?)
    };
    if needs_detail {
        let detail = fetch_series_detail(&app, &mirrors, &series_url, a.as_ref()).await?;
        let db = state.db.lock().unwrap();
        db.insert_series_genres(series_id, &detail.genres).map_err(|e| e.to_string())?;
        if let Some(kind) = &detail.kind {
            db.set_kind(series_id, kind).map_err(|e| e.to_string())?;
        }
    }
    let db = state.db.lock().unwrap();
    db.set_backlog_status(series_id, Some("want")).map_err(|e| e.to_string())?;
    Ok(())
}

/// Promote a `backlog_status='want'` row to an ordinary followed series.
///
/// Two shapes of "want" row reach here:
///
/// - A **catalog** row (synthetic `anilist-{id}` slug, AniList URL, no
///   episodes): its `url` points at anilist.co, so there is nothing to scrape
///   there. It must be **linked to the real site first** — this is one of the
///   three (and only three) explicit link triggers in the design spec, fired
///   because the user pressed "Empezar a ver". If the matcher can't find it on
///   the site (`NoMatch`), the row is left untouched in the backlog and the
///   outcome is returned so the UI can show "No encontrado" instead of
///   silently creating a followed-but-untrackable series. Linking already
///   scrapes and inserts the episode list, so no separate episode fetch runs.
/// - A **site** row (real slug/URL from a `decide_swipe` Want, `anilist_id`
///   NULL): unchanged behaviour — fetch its episode list, follow, clear the
///   backlog status.
///
/// `refresh()` already scans all followed rows regardless of `is_airing`, so
/// no changes are needed there. Returns a `LinkOutcome` so the caller can
/// distinguish a successful start from a `NoMatch` that left the row as-is.
#[tauri::command]
pub async fn start_watching(
    app: AppHandle,
    state: State<'_, AppState>,
    series_id: i64,
) -> Result<LinkOutcome, String> {
    let info = {
        let db = state.db.lock().unwrap();
        let status = db.get_backlog_status(series_id).map_err(|e| e.to_string())?;
        if status.as_deref() != Some("want") {
            return Err("series is not in the 'want' backlog".into());
        }
        db.get_series_for_link(series_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "series not found".to_string())?
    };

    // Catalog row (anilist-backed) with no real site episodes yet: link it
    // before following. A NoMatch leaves the row exactly as it is.
    let has_episodes = {
        let db = state.db.lock().unwrap();
        !db.list_series_episodes(series_id).map_err(|e| e.to_string())?.is_empty()
    };
    if info.anilist_id.is_some() && !has_episodes {
        let (outcome, resulting_id) = link_series_core(&app, &state, series_id).await?;
        let Some(target_id) = resulting_id else {
            // NoMatch — do not follow; hand the outcome back so the UI can
            // surface "No encontrado" and keep the row in the backlog.
            return Ok(outcome);
        };
        let db = state.db.lock().unwrap();
        db.set_followed(target_id, true).map_err(|e| e.to_string())?;
        db.set_backlog_status(target_id, None).map_err(|e| e.to_string())?;
        return Ok(outcome);
    }

    // Site row: fetch its episode list (all unseen), follow, clear backlog.
    let site_id = get_active_site_id(&state);
    let a = get_active_adapter(&state)?;
    let (series_url, mirrors) = {
        let db = state.db.lock().unwrap();
        let url = db
            .get_series_url(series_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "series not found".to_string())?;
        (url, load_mirrors(&db, &site_id)?)
    };
    let eps = fetch_episode_list_for(&app, &mirrors, &series_url, a.as_ref()).await?;
    let db = state.db.lock().unwrap();
    let episode_count = eps.len() as i64;
    for mut e in eps {
        e.series_id = series_id;
        e.seen = false;
        db.insert_episode(&e).map_err(|e| e.to_string())?;
    }
    db.set_followed(series_id, true).map_err(|e| e.to_string())?;
    db.set_backlog_status(series_id, None).map_err(|e| e.to_string())?;
    Ok(LinkOutcome::Linked { url: series_url, episodes: episode_count })
}

/// Set (or clear, with `status: None`) a series' backlog status directly —
/// used by the Listas view's "Descartar" action on a "want" row, which
/// (unlike decide_swipe) is transitioning a series already in the DB, not
/// inserting a fresh one from a swipe card.
#[tauri::command]
pub fn set_backlog_status(
    state: State<'_, AppState>,
    series_id: i64,
    status: Option<String>,
) -> Result<(), String> {
    let db = state.db.lock().unwrap();
    db.set_backlog_status(series_id, status.as_deref()).map_err(|e| e.to_string())
}

/// The four non-scraping classification states a series can be moved
/// between — see the state table in
/// docs/superpowers/specs/2026-07-11-reversibility-classifications-design.md.
/// Plain Rust variant names on the wire (no serde rename), same convention
/// as `SwipeDecision`.
#[derive(serde::Deserialize)]
pub enum Classification {
    None,
    Want,
    Discarded,
    WatchedExternally,
}

/// The body of `reclassify_series`, factored out (in the `switch_site_core`
/// style) so it can be unit-tested against an in-memory `Db` without a
/// `State<AppState>`. Clears all three classification signals
/// (`followed`/`backlog_status`/`watched_externally`) then applies `to` —
/// reusing the existing `db.set_*` methods rather than raw SQL, so this
/// stays in sync with their semantics. Deliberately does **not** scrape and
/// does **not** touch `episodes`/seen rows: the one target this can't reach
/// (Want -> actively Watching for a catalog stub with no episodes) needs a
/// scrape and stays on `start_watching`, unchanged.
fn reclassify_series_core(db: &Db, series_id: i64, to: Classification) -> Result<(), String> {
    db.set_followed(series_id, false).map_err(|e| e.to_string())?;
    db.set_backlog_status(series_id, None).map_err(|e| e.to_string())?;
    db.set_watched_externally(series_id, false).map_err(|e| e.to_string())?;
    match to {
        Classification::None => {}
        Classification::Want => {
            db.set_backlog_status(series_id, Some("want")).map_err(|e| e.to_string())?
        }
        Classification::Discarded => {
            db.set_backlog_status(series_id, Some("discarded")).map_err(|e| e.to_string())?
        }
        Classification::WatchedExternally => {
            db.set_watched_externally(series_id, true).map_err(|e| e.to_string())?
        }
    }
    Ok(())
}

/// Move a series between the four non-scraping classification states in one
/// atomic step — the universal "de-classify / move between lists" inverse
/// (Library "Dejar de seguir"/"Mover a Quiero ver", SeriesDetail "Dejar de
/// seguir", Descubrir Listas "Quitar de la lista" and the "Ya vistas"
/// sub-list). Held under a single `state.db` lock so nothing else can
/// interleave a partial state. Never scrapes, never touches episodes/seen.
#[tauri::command]
pub fn reclassify_series(
    state: State<'_, AppState>,
    series_id: i64,
    to: Classification,
) -> Result<(), String> {
    let db = state.db.lock().unwrap();
    reclassify_series_core(&db, series_id, to)
}

/// Search the configured site (with mirror fallback) for `query`.
///
/// Deliberately does **not** reuse `scrape_via_mirrors`'s "parsed empty ->
/// try next mirror" semantics as-is: for every other scrape in this codebase,
/// an empty parse legitimately signals "wrong/incompatible mirror", because
/// there's always *something* to find on a real airing/genre/series page.
/// A search is different — zero results for a query is a completely normal,
/// correct answer, and treating it as mirror failure would make `search_site`
/// silently keep trying (and eventually exhaust) every configured mirror any
/// time a title genuinely isn't on the site, plus misreport a real failure as
/// "not found" instead of surfacing the actual error.
///
/// The fix: wrap the parsed cards in a one-element `Vec<SearchOutcome>` before
/// handing it to `scrape_via_mirrors`. That vec is non-empty (so
/// `scrape_via_mirrors` accepts the mirror as "worked") whether the page had
/// 0 or 50 result cards on it — only `parse_search_results` itself returning
/// `Err` (page doesn't look like this site's layout at all) makes
/// `scrape_via_mirrors` fall through to the next mirror.
async fn search_site(
    app: &AppHandle,
    mirrors: &[String],
    query: &str,
    a: &dyn SiteAdapter,
) -> Result<Vec<FinishedCard>, String> {
    let path = a.search_url("", query);
    let (_scraped, mut outcomes, _mirror) = scrape_via_mirrors(app, mirrors, &path, true, |scraped| {
        a.parse_search_results(&scraped.html).map(|cards| vec![SearchOutcome { cards }])
    })
    .await?;
    Ok(outcomes.pop().map(|o| o.cards).unwrap_or_default())
}

/// Result of `link_catalog_series` — serde-tagged so the frontend can
/// distinguish "linked, here's the episode count", "searched but nothing
/// cleared the match threshold", and "this row wasn't a synthetic catalog
/// row to begin with" without stringly-typed error parsing.
#[derive(Serialize)]
#[serde(tag = "type")]
pub enum LinkOutcome {
    Linked { url: String, episodes: i64 },
    NoMatch,
    AlreadyLinked,
}

/// Search the site for a synthetic catalog-swipe row's title and, on a
/// confident match, rewrite the row into a real, fully-tracked series (real
/// site URL/slug, scraped episode list, site genres/kind) — see the design
/// spec (`docs/superpowers/specs/2026-07-10-discover-site-link-search-design.md`)
/// for the full rationale. Politeness: at most 3 scrapes total (search,
/// optionally a second search with the English title, then one fetch of the
/// matched series page reused for both `parse_series` and
/// `parse_series_detail`) — no batching, no background sweep of the existing
/// backlog (out of scope, see the spec).
#[tauri::command]
pub async fn link_catalog_series(
    app: AppHandle,
    state: State<'_, AppState>,
    series_id: i64,
) -> Result<LinkOutcome, String> {
    let (outcome, _resulting_id) = link_series_core(&app, &state, series_id).await?;
    Ok(outcome)
}

/// The shared body behind both `link_catalog_series` (fire-and-forget from a
/// `Seen` swipe / manual retry) and `start_watching` (which must link an
/// unlinked catalog row *before* following it). Returns the outcome plus the
/// canonical row id the caller should act on afterwards — that's the same
/// `series_id` for an in-place relink or `AlreadyLinked`, the *existing*
/// row's id when a slug collision merged the synthetic row away (the
/// synthetic `series_id` no longer exists after that), and `None` for
/// `NoMatch` (nothing to follow).
async fn link_series_core(
    app: &AppHandle,
    state: &State<'_, AppState>,
    series_id: i64,
) -> Result<(LinkOutcome, Option<i64>), String> {
    let site_id = get_active_site_id(state);
    let a = get_active_adapter(state)?;
    let (mirrors, info) = {
        let db = state.db.lock().unwrap();
        let mirrors = load_mirrors(&db, &site_id)?;
        let info = db
            .get_series_for_link(series_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "series not found".to_string())?;
        (mirrors, info)
    };
    // See SeriesForLink::already_linked_to_site's doc comment for why this
    // checks more than just `anilist_id.is_some()`.
    if info.already_linked_to_site() {
        return Ok((LinkOutcome::AlreadyLinked, Some(series_id)));
    }
    let anilist_id = info.anilist_id.unwrap();
    let (title, romaji, english) = {
        let db = state.db.lock().unwrap();
        db.get_catalog_titles(anilist_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "catalog entry not found for this anilist_id".to_string())?
    };

    let primary_query = romaji.clone().unwrap_or_else(|| title.clone());
    let mut cards = search_site(app, &mirrors, &primary_query, a.as_ref()).await?;
    let mut best = crate::matching::best_match(&[&primary_query], &to_candidates(&cards));

    // Second search only on failure, only if an english title exists and
    // actually differs from what was just tried — one extra scrape, not two
    // searches every time.
    if best.is_none() {
        if let Some(english) = &english {
            if !english.eq_ignore_ascii_case(&primary_query) {
                cards = search_site(app, &mirrors, english, a.as_ref()).await?;
                best = crate::matching::best_match(&[english], &to_candidates(&cards));
            }
        }
    }

    let Some(m) = best else { return Ok((LinkOutcome::NoMatch, None)) };
    let matched = cards[m.index].clone();
    let new_slug = slug_from_url(&matched.url);

    // Slug collision: the matched site series may already be tracked (e.g.
    // it's on the airing list). Merge onto that existing row instead of
    // touching slug/url at all — no extra scrape needed, the existing row
    // already has real episodes/genres from its own normal scrape.
    if let Some(existing_id) = {
        let db = state.db.lock().unwrap();
        db.find_series_id_by_slug(info.source_id, &new_slug, series_id).map_err(|e| e.to_string())?
    } {
        let db = state.db.lock().unwrap();
        db.merge_series_into(existing_id, series_id).map_err(|e| e.to_string())?;
        let episodes = db.list_series_episodes(existing_id).map_err(|e| e.to_string())?.len() as i64;
        let url = db
            .get_series_url(existing_id)
            .map_err(|e| e.to_string())?
            .unwrap_or(matched.url.clone());
        return Ok((LinkOutcome::Linked { url, episodes }, Some(existing_id)));
    }

    // No collision: fetch the matched series page once, reused for both the
    // episode list and the detail (genres/kind) parse. For a site with an
    // episode_fetch_script (jkanime.net), `scraped.extra` carries the episode
    // JSON from that same page load; every other site leaves `extra: None`
    // and `parse_series` reads `scraped.html` exactly as before.
    let scraped =
        fetch_html_with_script(app, &matched.url, a.episode_fetch_script()).await.map_err(|e| e.to_string())?;
    let episodes = a
        .parse_series(scraped.extra.as_deref().unwrap_or(&scraped.html))
        .map_err(|e| e.to_string())?;
    let detail = a.parse_series_detail(&scraped.html).map_err(|e| e.to_string())?;
    let kind = detail.kind.unwrap_or(matched.kind.clone());

    let db = state.db.lock().unwrap();
    db.relink_series(series_id, &new_slug, &matched.url, matched.poster_url.as_deref(), &kind)
        .map_err(|e| e.to_string())?;
    db.replace_series_genres(series_id, &detail.genres).map_err(|e| e.to_string())?;
    let episode_count = episodes.len() as i64;
    for mut e in episodes {
        e.series_id = series_id;
        db.insert_episode(&e).map_err(|e| e.to_string())?;
    }
    if info.watched_externally {
        db.mark_all_episodes_seen(series_id).map_err(|e| e.to_string())?;
    }

    Ok((LinkOutcome::Linked { url: matched.url, episodes: episode_count }, Some(series_id)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn insert_test_series(db: &Db, src: i64, slug: &str) -> i64 {
        let s = crate::models::Series {
            id: 0, slug: slug.into(), title: slug.into(), url: format!("u-{slug}"),
            cover_url: None, is_airing: false, followed: false, next_episode_at: None, site_episode_count: None,
        };
        db.upsert_series(src, &s).unwrap()
    }

    /// Watching -> None ("Dejar de seguir"): `followed` clears and no
    /// backlog/watched-externally signal is left behind.
    #[test]
    fn reclassify_series_core_watching_to_none_clears_followed() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
        let sid = insert_test_series(&db, src, "a");
        db.set_followed(sid, true).unwrap();

        reclassify_series_core(&db, sid, Classification::None).unwrap();

        assert!(!db.list_followed(src).unwrap().iter().any(|f| f.id == sid));
        assert_eq!(db.get_backlog_status(sid).unwrap(), None);
    }

    /// Watching -> Want ("Mover a Quiero ver"): unfollows and lands in the
    /// 'want' backlog in the same atomic step.
    #[test]
    fn reclassify_series_core_watching_to_want() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
        let sid = insert_test_series(&db, src, "a");
        db.set_followed(sid, true).unwrap();

        reclassify_series_core(&db, sid, Classification::Want).unwrap();

        assert!(!db.list_followed(src).unwrap().iter().any(|f| f.id == sid));
        assert_eq!(db.get_backlog_status(sid).unwrap().as_deref(), Some("want"));
    }

    /// Want -> None ("Quitar de la lista"): clears backlog_status without
    /// discarding (distinct from Want -> Discarded).
    #[test]
    fn reclassify_series_core_want_to_none() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
        let sid = insert_test_series(&db, src, "a");
        db.set_backlog_status(sid, Some("want")).unwrap();

        reclassify_series_core(&db, sid, Classification::None).unwrap();

        assert_eq!(db.get_backlog_status(sid).unwrap(), None);
    }

    /// WatchedExternally -> None ("Ya no la he visto"): clears the flag so
    /// the row is unclassified again (reappears in future decks).
    #[test]
    fn reclassify_series_core_watched_externally_to_none() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
        let sid = insert_test_series(&db, src, "a");
        db.set_watched_externally(sid, true).unwrap();

        reclassify_series_core(&db, sid, Classification::None).unwrap();

        assert!(db.list_watched_externally(src).unwrap().iter().all(|s| s.id != sid));
    }

    /// WatchedExternally -> Want ("Mover a Quiero ver" from Ya vistas).
    #[test]
    fn reclassify_series_core_watched_externally_to_want() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
        let sid = insert_test_series(&db, src, "a");
        db.set_watched_externally(sid, true).unwrap();

        reclassify_series_core(&db, sid, Classification::Want).unwrap();

        assert!(db.list_watched_externally(src).unwrap().iter().all(|s| s.id != sid));
        assert_eq!(db.get_backlog_status(sid).unwrap().as_deref(), Some("want"));
    }

    /// Any -> WatchedExternally applies the flag and clears whatever else
    /// was set (defensive: reclassify always fully clears first).
    #[test]
    fn reclassify_series_core_want_to_watched_externally() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
        let sid = insert_test_series(&db, src, "a");
        db.set_backlog_status(sid, Some("want")).unwrap();

        reclassify_series_core(&db, sid, Classification::WatchedExternally).unwrap();

        assert_eq!(db.get_backlog_status(sid).unwrap(), None);
        assert!(db.list_watched_externally(src).unwrap().iter().any(|s| s.id == sid));
    }

    /// Reclassify is a pure local state move: it must never touch
    /// `episodes` rows (seen/unseen), even when unfollowing a series with
    /// watched history.
    #[test]
    fn reclassify_series_core_never_touches_episodes() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
        let sid = insert_test_series(&db, src, "a");
        db.set_followed(sid, true).unwrap();
        db.insert_episode(&crate::models::Episode {
            id: 0, series_id: sid, number: "1".into(), title: None,
            url: "e1".into(), released_at: None, seen: false,
        }).unwrap();
        db.set_seen_cascade(sid, "1", true).unwrap();

        reclassify_series_core(&db, sid, Classification::Want).unwrap();

        let eps = db.list_series_episodes(sid).unwrap();
        assert_eq!(eps.len(), 1);
        assert!(eps[0].seen, "episode seen state must survive a reclassify");
    }

    fn scanned(title: &str) -> Series {
        Series {
            id: 0,
            slug: title.to_lowercase().replace(' ', "-"),
            title: title.to_string(),
            url: format!("https://site.example/tv/{}/", title.to_lowercase().replace(' ', "-")),
            cover_url: None,
            is_airing: true,
            followed: false,
            next_episode_at: None,
            site_episode_count: None,
        }
    }

    #[test]
    fn plan_carryover_matches_exact_title_and_returns_its_watermark() {
        let new_site = vec![scanned("Frieren Sub Español"), scanned("Totally Unrelated Show")];
        let followed = vec![("Frieren".to_string(), 12i64), ("Some Other".to_string(), 3)];
        let out = plan_carryover(&new_site, &followed);
        // Only the Frieren card matches; it carries index 0 with watermark 12.
        assert_eq!(out, vec![(0usize, 12i64)]);
    }

    #[test]
    fn plan_carryover_does_not_carry_a_decoy_below_threshold() {
        let new_site = vec![scanned("Kaiju No. 8")];
        let followed = vec![("Monster Musume".to_string(), 5i64)];
        assert!(plan_carryover(&new_site, &followed).is_empty());
    }

    #[test]
    fn plan_carryover_picks_the_best_of_several_followed_titles() {
        let new_site = vec![scanned("Overlord IV")];
        // A near-miss decoy and the true match; best_match must choose the
        // exact-after-normalization one and return ITS watermark (7), not the
        // decoy's.
        let followed = vec![
            ("Overlord".to_string(), 39i64),      // shorter, lower score
            ("Overlord IV".to_string(), 7i64),    // exact normalized match
        ];
        let out = plan_carryover(&new_site, &followed);
        assert_eq!(out, vec![(0usize, 7i64)]);
    }

    #[test]
    fn plan_carryover_empty_when_nothing_followed_elsewhere() {
        let new_site = vec![scanned("Anything")];
        assert!(plan_carryover(&new_site, &[]).is_empty());
    }
}
