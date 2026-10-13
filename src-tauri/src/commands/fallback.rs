use super::*;
use tauri::{AppHandle, Manager};
use std::sync::atomic::Ordering;
use crate::models::Series;
use crate::scraper_engine::fetch_html_with_script;
use crate::commands::follow::search_site;
use crate::commands::discover::to_candidates;
use rusqlite::OptionalExtension;

const FALLBACK_PACED: std::time::Duration = std::time::Duration::from_millis(2500);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SiteOutcome {
    FoundEpisodes,
    DefinitiveMiss,
    TransientError,
}

pub(crate) fn decide_show_miss(outcomes: &[SiteOutcome]) -> bool {
    if outcomes.is_empty() {
        return false;
    }
    outcomes.iter().all(|&o| o == SiteOutcome::DefinitiveMiss)
}

pub fn spawn_episode_fallback(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        if let Err(e) = run_episode_fallback(app).await {
            eprintln!("[fallback] episode fallback failed: {e}");
        }
    });
}

async fn run_episode_fallback(app: AppHandle) -> Result<(), String> {
    let state = app.state::<AppState>();
    // Bail out early if a backfill is running (only one scraper at a time).
    // Do not block it by swapping `episode_backfill_running`.
    if state.episode_backfill_running.load(Ordering::SeqCst) {
        eprintln!("[fallback] skipped: the episode backfill is running");
        return Ok(());
    }

    // Use our own lock to prevent parallel fallback runs.
    if state.episode_fallback_running.swap(true, Ordering::SeqCst) {
        return Ok(());
    }
    let _clear = crate::commands::follow::RunningGuard(&state.episode_fallback_running);

    let (_active_source_id, active_site_id, candidates) = {
        let db = state.db.lock().unwrap();
        let src = get_source_id(&state)?;
        let site_id = get_active_site_id(&state);

        let now = chrono::Utc::now().timestamp();
        let candidates = db.shows_needing_episode_source(src, now).map_err(|e| e.to_string())?;

        (src, site_id, candidates)
    };

    if candidates.is_empty() {
        eprintln!("[fallback] no followed show needs another episode source");
        return Ok(());
    }
    eprintln!("[fallback] {} show(s) need an episode source (resolving up to 3)", candidates.len());

    let all_sites = crate::adapter::all_sites();

    // A candidate whose last attempt was under ATTEMPT_COOLDOWN_SECS ago goes to the back of
    // the line: a show that keeps failing (dead domain, Cloudflare) must not starve the rest.
    let now_ts = chrono::Utc::now().timestamp();
    let mut fresh: Vec<_> = Vec::new();
    let mut recent: Vec<_> = Vec::new();
    {
        let db = state.db.lock().unwrap();
        for c in candidates {
            if db.recently_attempted_episode_fallback(&c.canon_key, now_ts).unwrap_or(false) {
                recent.push(c);
            } else {
                fresh.push(c);
            }
        }
    }
    fresh.extend(recent);
    for candidate in fresh.into_iter().take(3) {
        {
            let db = state.db.lock().unwrap();
            let _ = db.record_episode_fallback_attempt(&candidate.canon_key, now_ts);
        }
        let mut found_episodes = false;
        let mut had_refetch_error = false;

        let priority = {
            let db = state.db.lock().unwrap();
            let known: Vec<&str> = all_sites.iter().map(|s| s.id).collect();
            db.get_site_priority(&known).unwrap_or_default()
        };

        let expected = {
            let db = state.db.lock().unwrap();
            expected_episode_for_candidate(&db, &candidate).unwrap_or(0)
        };

        // 1. For candidate with known_rows, FIRST refetch each known row of a site
        // other than the already-fresh ones, in site_priority order.
        if !candidate.known_rows.is_empty() {
            let rows = rows_to_refresh(&candidate.known_rows, &priority, &active_site_id);
            for (series_id, site_id, url) in rows {
                let a = match crate::adapter::adapter_for(&site_id) {
                    Some(adapter) => adapter,
                    None => continue,
                };

                let scraped = match fetch_html_with_script(&app, &url, a.episode_fetch_script()).await {
                    Ok(s) => s,
                    Err(e) => {
                        eprintln!("[fallback] refetch failed for {} on {}: {e}", candidate.title, site_id);
                        had_refetch_error = true;
                        tokio::time::sleep(FALLBACK_PACED).await;
                        continue;
                    }
                };

                let episodes = match a.parse_series(scraped.extra.as_deref().unwrap_or(&scraped.html)) {
                    Ok(e) => e,
                    Err(e) => {
                        eprintln!("[fallback] parse failed for {} on {}: {e}", candidate.title, site_id);
                        had_refetch_error = true;
                        tokio::time::sleep(FALLBACK_PACED).await;
                        continue;
                    }
                };

                {
                    let db = state.db.lock().unwrap();
                    let _ = db.apply_episode_diff(series_id, None, &episodes);
                    let _ = db.sync_seen_progress_across_sites();
                }

                let current_max = {
                    let db = state.db.lock().unwrap();
                    max_episode_for_candidate(&db, &candidate).unwrap_or(0)
                };

                if expected > 0 && current_max >= expected {
                    found_episodes = true;
                    break;
                }

                tokio::time::sleep(FALLBACK_PACED).await;
            }
        }

        let mut outcomes = Vec::new();

        // 2. Only if still behind, continue with existing search over sites_to_try
        if !found_episodes {
            let (title, romaji, english) = if let Some(al_id) = candidate.anilist_id {
                let db = state.db.lock().unwrap();
                db.get_catalog_titles(al_id).map_err(|e| e.to_string())?.unwrap_or((candidate.title.clone(), None, None))
            } else {
                (candidate.title.clone(), None, None)
            };
            let primary_query = romaji.clone().unwrap_or_else(|| title.clone());

            // Get misses for this candidate
            let site_misses = {
                let db = state.db.lock().unwrap();
                let now = chrono::Utc::now().timestamp();
                let mut misses = Vec::new();
                for site in all_sites {
                    if db.has_recent_episode_fallback_site_miss(&candidate.canon_key, site.id, now).unwrap_or(false) {
                        misses.push(site.id.to_string());
                    }
                }
                misses
            };

            let sites_to_try = next_sites_to_try(all_sites, &active_site_id, &site_misses, &priority);
            let sites_to_try: Vec<&crate::adapter::SiteInfo> = sites_to_try
                .into_iter()
                .filter(|s| !candidate.known_rows.iter().any(|(_, site_id, _)| site_id == s.id))
                .collect();

            for site in sites_to_try {
                let a = match crate::adapter::adapter_for(site.id) {
                    Some(adapter) => adapter,
                    None => continue,
                };

                let mirrors = {
                    let db = state.db.lock().unwrap();
                    mirrors_or_default(crate::commands::mirrors::load_mirrors(&db, site.id).map_err(|e| e.to_string()), site.default_base_url)
                };

                let outcome = async {
                    let mut cards = match search_site(&app, &mirrors, &primary_query, a.as_ref()).await {
                        Ok(c) => c,
                        // Cloudflare, network, "no web configured": the site may well have it.
                        Err(_) => return SiteOutcome::TransientError,
                    };

                    let mut best = crate::matching::best_match(&[&primary_query], &to_candidates(&cards));
                    if best.is_none() {
                        if let Some(eng) = &english {
                            if !eng.eq_ignore_ascii_case(&primary_query) {
                                tokio::time::sleep(FALLBACK_PACED).await;
                                if let Ok(c) = search_site(&app, &mirrors, eng, a.as_ref()).await {
                                    cards = c;
                                    best = crate::matching::best_match(&[eng], &to_candidates(&cards));
                                }
                            }
                        }
                    }

                    if let Some(m) = best {
                        let matched = cards[m.index].clone();

                        let mut all_refs = vec![title.as_str()];
                        if let Some(r) = &romaji {
                            all_refs.push(r.as_str());
                        }
                        if let Some(e) = &english {
                            all_refs.push(e.as_str());
                        }

                        if !crate::matching::season_consistent(&matched.title, &all_refs) {
                            return SiteOutcome::DefinitiveMiss;
                        }

                        let is_live_action_db = crate::matching::is_live_action(title.as_str()) ||
                            romaji.as_ref().map(|r| crate::matching::is_live_action(r.as_str())).unwrap_or(false) ||
                            english.as_ref().map(|e| crate::matching::is_live_action(e.as_str())).unwrap_or(false);
                        if crate::matching::is_live_action(&matched.title) != is_live_action_db {
                            return SiteOutcome::DefinitiveMiss;
                        }

                        let matched_norm = crate::matching::normalize_title(&matched.title);
                        let is_distinct = crate::matching::distinct_extension(&crate::matching::normalize_title(title.as_str()), &matched_norm) ||
                            romaji.as_ref().map(|r| crate::matching::distinct_extension(&crate::matching::normalize_title(r.as_str()), &matched_norm)).unwrap_or(false) ||
                            english.as_ref().map(|e| crate::matching::distinct_extension(&crate::matching::normalize_title(e.as_str()), &matched_norm)).unwrap_or(false);

                        if is_distinct {
                            return SiteOutcome::DefinitiveMiss;
                        }

                        // Match found, fetch episodes
                        let scraped = match fetch_html_with_script(&app, &matched.url, a.episode_fetch_script()).await {
                            Ok(s) => s,
                            Err(_) => return SiteOutcome::TransientError,
                        };

                        let episodes = match a.parse_series(scraped.extra.as_deref().unwrap_or(&scraped.html)) {
                            Ok(e) => e,
                            Err(_) => return SiteOutcome::TransientError,
                        };

                        if episodes.is_empty() {
                            return SiteOutcome::DefinitiveMiss; // Skip if still 0 episodes
                        }

                        let (detail, kind) = if candidate.anilist_id.is_some() {
                            (None, matched.kind.clone())
                        } else {
                            let d = match a.parse_series_detail(&scraped.html) {
                                Ok(d) => d,
                                Err(_) => return SiteOutcome::TransientError,
                            };
                            let k = d.kind.clone().unwrap_or(matched.kind.clone());
                            (Some(d), k)
                        };

                        let new_slug = crate::commands::scan::slug_from_url(&matched.url);

                        let is_releasing = if let Some(al_id) = candidate.anilist_id {
                            let db = state.db.lock().unwrap();
                            let status: Option<String> = db.conn.query_row("SELECT status FROM anilist_catalog WHERE id=?1", [al_id], |r| r.get(0)).ok();
                            status.as_deref() == Some("RELEASING") || status.as_deref() == Some("HIATUS")
                        } else { false };

                        let (target_id, written) = {
                            let db = state.db.lock().unwrap();
                            let step = ResolverStep {
                                site,
                                mirrors: &mirrors,
                                matched: &matched,
                                candidate: &candidate,
                                detail: detail.as_ref(),
                                episodes: &episodes,
                                new_slug: &new_slug,
                                kind: &kind,
                                is_releasing,
                            };
                            match perform_resolver_db_step(&db, &step) {
                                Ok(res) => res,
                                Err(_) => return SiteOutcome::TransientError,
                            }
                        };

                        if written {
                            let db = state.db.lock().unwrap();
                            if let Some(n) = db.take_carried_seen_number(target_id).unwrap_or(None) {
                                let _ = db.set_seen_cascade(target_id, &n.to_string(), true);
                            }
                            let _ = db.sync_seen_progress_across_sites();
                            return SiteOutcome::FoundEpisodes;
                        }
                        SiteOutcome::TransientError
                    } else {
                        SiteOutcome::DefinitiveMiss
                    }
                }.await;

                outcomes.push(outcome);

                eprintln!("[fallback] '{}' on {}: {:?}", candidate.title, site.id, outcome);
                if outcome == SiteOutcome::DefinitiveMiss {
                    let db = state.db.lock().unwrap();
                    let now = chrono::Utc::now().timestamp();
                    let _ = db.record_episode_fallback_site_miss(&candidate.canon_key, site.id, now);
                }

                tokio::time::sleep(FALLBACK_PACED).await;

                if outcome == SiteOutcome::FoundEpisodes {
                    let db = state.db.lock().unwrap();
                    let current_max = max_episode_for_candidate(&db, &candidate).unwrap_or(0);
                    if expected <= 0 || current_max >= expected {
                        found_episodes = true;
                        break;
                    }
                }
            }
        }

        if !found_episodes && should_record_group_miss(had_refetch_error, &outcomes, !candidate.known_rows.is_empty()) {
            let db = state.db.lock().unwrap();
            let now = chrono::Utc::now().timestamp();
            let _ = db.record_episode_fallback_miss(&candidate.canon_key, now);
        }
    }

    Ok(())
}


struct ResolverStep<'a> {
    site: &'a crate::adapter::SiteInfo,
    mirrors: &'a [String],
    matched: &'a crate::models::FinishedCard,
    candidate: &'a crate::db::EpisodeSourceCandidate,
    detail: Option<&'a crate::models::SeriesDetail>,
    episodes: &'a [crate::models::Episode],
    new_slug: &'a str,
    kind: &'a str,
    is_releasing: bool,
}

fn perform_resolver_db_step(
    db: &crate::db::Db,
    step: &ResolverStep<'_>,
) -> anyhow::Result<(i64, bool)> {
    let site_source_id = match db.get_source_id_for_site(step.site.id)? {
        Some(id) => id,
        None => {
            let working = step.mirrors.iter().find(|m| step.matched.url.starts_with(*m)).cloned().unwrap_or_else(|| step.site.default_base_url.to_string());
            db.upsert_source(step.site.name, &working, step.site.id)?
        }
    };

    // To prevent clobbering, we'll fetch existing row if any.
    // upsert_series uses title fallback, so we just use upsert_series but with the existing is_airing/next_episode_at/site_episode_count.
    // Wait, SQLite doesn't export the title fallback easily, let's just do query_row.
    let existing_id: Option<i64> = db.conn.query_row(
        "SELECT id FROM series WHERE source_id=?1 AND slug=?2",
        (site_source_id, step.new_slug),
        |r| r.get(0),
    ).optional()?;

    let (is_airing, next_episode_at, site_episode_count) = if let Some(id) = existing_id {
        db.conn.query_row(
            "SELECT is_airing, next_episode_at, site_episode_count FROM series WHERE id=?1",
            [id],
            |r| Ok((r.get::<_, i64>(0)? != 0, r.get::<_, Option<i64>>(1)?, r.get::<_, Option<i64>>(2)?)),
        )?
    } else {
        (step.is_releasing, None, None)
    };

    let series = Series {
        id: 0,
        slug: step.new_slug.to_string(),
        title: step.matched.title.clone(),
        url: step.matched.url.clone(),
        cover_url: step.matched.poster_url.clone(),
        is_airing,
        followed: true, // we force followed since it's canonical
        next_episode_at,
        site_episode_count,
    };

    // The fallback re-upserts a row the site's airing listing did not necessarily show; keep
    // `last_listed_at` as the last real scan left it so the status sync's grace window is honest.
    let prev_listed: Option<i64> = match existing_id {
        Some(id) => db.conn.query_row("SELECT last_listed_at FROM series WHERE id=?1", [id], |r| r.get(0))?,
        None => None,
    };
    let sid = db.upsert_series(site_source_id, &series)?;
    db.conn.execute("UPDATE series SET last_listed_at=?1 WHERE id=?2", (prev_listed, sid))?;
    if let Some(al_id) = step.candidate.anilist_id {
        let _ = db.set_anilist_id(sid, al_id);
    }

    let kind_to_write = if let Some(al_id) = step.candidate.anilist_id {
        db.conn
            .query_row("SELECT format FROM anilist_catalog WHERE id=?1", [al_id], |r| r.get(0))
            .ok()
            .flatten()
            .unwrap_or_else(|| {
                db.conn
                    .query_row("SELECT COALESCE(kind, '') FROM series WHERE id=?1", [sid], |r| r.get(0))
                    .unwrap_or_default()
            })
    } else {
        step.kind.to_string()
    };

    db.relink_series(sid, step.new_slug, &step.matched.url, step.matched.poster_url.as_deref(), &kind_to_write)?;
    if step.candidate.anilist_id.is_none() {
        if let Some(detail) = step.detail {
            db.replace_series_genres(sid, &detail.genres)?;
        }
    }
    db.apply_episode_diff(sid, None, step.episodes)?;
    db.set_followed_canonical(sid, true)?;

    Ok((sid, true))
}

fn next_sites_to_try<'a>(
    all_sites: &'a [crate::adapter::SiteInfo],
    active_site_id: &str,
    misses: &[String],
    priority: &[String],
) -> Vec<&'a crate::adapter::SiteInfo> {
    let mut sites: Vec<&'a crate::adapter::SiteInfo> = all_sites
        .iter()
        .filter(|s| s.id != active_site_id && !misses.iter().any(|m| m == s.id))
        .collect();
    sites.sort_by_key(|s| {
        priority
            .iter()
            .position(|p| p == s.id)
            .unwrap_or(usize::MAX)
    });
    sites
}

fn mirrors_or_default(loaded: Result<Vec<String>, String>, default_url: &str) -> Vec<String> {
    match loaded {
        Ok(v) if !v.is_empty() => v,
        _ => vec![default_url.to_string()],
    }
}

pub(crate) fn rows_to_refresh(
    known_rows: &[(i64, String, String)],
    priority: &[String],
    active_site: &str,
) -> Vec<(i64, String, String)> {
    let mut seen_sites = std::collections::HashSet::new();
    let mut candidate_rows = Vec::new();

    // Highest id first: split-arc shows keep several rows per site and the newest arc is the
    // one that can hold the missing episode.
    let mut newest_first: Vec<&(i64, String, String)> = known_rows.iter().collect();
    newest_first.sort_by_key(|row| std::cmp::Reverse(row.0));
    for row in newest_first {
        if row.1 == active_site {
            continue;
        }
        if seen_sites.insert(row.1.clone()) {
            candidate_rows.push(row.clone());
        }
    }

    candidate_rows.sort_by_key(|row| {
        priority
            .iter()
            .position(|p| p == &row.1)
            .unwrap_or(usize::MAX)
    });

    candidate_rows
}

pub(crate) fn should_record_group_miss(
    had_refetch_error: bool,
    outcomes: &[SiteOutcome],
    has_known_rows: bool,
) -> bool {
    if had_refetch_error {
        return false;
    }
    if outcomes.is_empty() {
        return has_known_rows;
    }
    decide_show_miss(outcomes)
}

fn max_episode_for_candidate(
    db: &crate::db::Db,
    candidate: &crate::db::EpisodeSourceCandidate,
) -> rusqlite::Result<i64> {
    let mut max_ep: i64 = 0;
    if let Some(al_id) = candidate.anilist_id {
        let ep: i64 = db.conn.query_row(
            "SELECT COALESCE(MAX(CAST(e.number AS INTEGER)), 0)
             FROM episodes e
             JOIN series s ON s.id = e.series_id
             WHERE s.anilist_id = ?1",
            [al_id],
            |r| r.get(0),
        )?;
        max_ep = max_ep.max(ep);
    }
    for (sid, _, _) in &candidate.known_rows {
        let ep: i64 = db.conn.query_row(
            "SELECT COALESCE(MAX(CAST(number AS INTEGER)), 0) FROM episodes WHERE series_id = ?1",
            [sid],
            |r| r.get(0),
        )?;
        max_ep = max_ep.max(ep);
    }
    Ok(max_ep)
}

fn expected_episode_for_candidate(
    db: &crate::db::Db,
    candidate: &crate::db::EpisodeSourceCandidate,
) -> Option<i64> {
    let al_id = candidate.anilist_id?;
    let next_ep: Option<i64> = db.conn.query_row(
        "SELECT next_episode FROM anilist_catalog WHERE id = ?1",
        [al_id],
        |r| r.get(0),
    ).ok().flatten();
    next_ep.map(|n| n - 1).filter(|&exp| exp > 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::SiteInfo;

    #[test]
    fn test_next_sites_to_try_skips_active_and_misses() {
        let sites = vec![
            SiteInfo { id: "a", name: "A", default_base_url: "" },
            SiteInfo { id: "b", name: "B", default_base_url: "" },
            SiteInfo { id: "c", name: "C", default_base_url: "" },
            SiteInfo { id: "d", name: "D", default_base_url: "" },
        ];

        // Active is "b", misses is ["d"]
        let to_try = next_sites_to_try(&sites, "b", &["d".to_string()], &[]);

        let ids: Vec<&str> = to_try.iter().map(|s| s.id).collect();
        assert_eq!(ids, vec!["a", "c"]);
    }

    #[test]
    fn test_next_sites_to_try_orders_by_priority() {
        let sites = vec![
            SiteInfo { id: "a", name: "A", default_base_url: "" },
            SiteInfo { id: "b", name: "B", default_base_url: "" },
            SiteInfo { id: "c", name: "C", default_base_url: "" },
            SiteInfo { id: "d", name: "D", default_base_url: "" },
        ];
        let priority = vec!["d".to_string(), "c".to_string(), "a".to_string(), "b".to_string()];
        let to_try = next_sites_to_try(&sites, "b", &["d".to_string()], &priority);
        let ids: Vec<&str> = to_try.iter().map(|s| s.id).collect();
        assert_eq!(ids, vec!["c", "a"]);
    }

    #[test]
    fn test_next_sites_to_try_empty_priority_uses_registry_order() {
        let sites = vec![
            SiteInfo { id: "a", name: "A", default_base_url: "" },
            SiteInfo { id: "b", name: "B", default_base_url: "" },
            SiteInfo { id: "c", name: "C", default_base_url: "" },
            SiteInfo { id: "d", name: "D", default_base_url: "" },
        ];
        let to_try = next_sites_to_try(&sites, "b", &["d".to_string()], &[]);
        let ids: Vec<&str> = to_try.iter().map(|s| s.id).collect();
        assert_eq!(ids, vec!["a", "c"]);
    }

    #[test]
    fn test_next_sites_to_try_site_missing_from_priority_goes_last() {
        let sites = vec![
            SiteInfo { id: "a", name: "A", default_base_url: "" },
            SiteInfo { id: "b", name: "B", default_base_url: "" },
            SiteInfo { id: "c", name: "C", default_base_url: "" },
            SiteInfo { id: "d", name: "D", default_base_url: "" },
        ];
        let priority = vec!["c".to_string()];
        let to_try = next_sites_to_try(&sites, "b", &[], &priority);
        let ids: Vec<&str> = to_try.iter().map(|s| s.id).collect();
        assert_eq!(ids, vec!["c", "a", "d"]);
    }

    #[test]
    fn test_next_sites_to_try_never_returns_active_or_misses_even_with_high_priority() {
        let sites = vec![
            SiteInfo { id: "a", name: "A", default_base_url: "" },
            SiteInfo { id: "b", name: "B", default_base_url: "" },
            SiteInfo { id: "c", name: "C", default_base_url: "" },
            SiteInfo { id: "d", name: "D", default_base_url: "" },
        ];
        let priority = vec!["b".to_string(), "c".to_string(), "d".to_string(), "a".to_string()];
        let to_try = next_sites_to_try(&sites, "b", &["c".to_string()], &priority);
        let ids: Vec<&str> = to_try.iter().map(|s| s.id).collect();
        assert_eq!(ids, vec!["d", "a"]);
    }

    #[test]
    fn test_mirrors_or_default() {
        assert_eq!(mirrors_or_default(Ok(vec!["a".to_string(), "b".to_string()]), "def"), vec!["a", "b"]);
        assert_eq!(mirrors_or_default(Ok(vec![]), "def"), vec!["def"]);
        assert_eq!(mirrors_or_default(Err("err".to_string()), "def"), vec!["def"]);
    }

    #[test]
    fn test_perform_resolver_db_step_reuses_source_and_does_not_clobber() {
        let db = Db::open(":memory:").unwrap();

        let site = SiteInfo { id: "testsite", name: "TestSite", default_base_url: "https://test.com" };
        let mirrors = vec!["https://custom.test.com".to_string(), "https://test.com".to_string()];

        // Setup existing source with custom URL
        db.upsert_source("TestSite", "https://custom.test.com", "testsite").unwrap();

        // Setup existing series with some scan-owned fields
        let src_id = db.get_source_id_for_site("testsite").unwrap().unwrap();
        let existing = Series {
            id: 0,
            slug: "test-show".into(),
            title: "Test Show".into(),
            url: "https://custom.test.com/test-show".into(),
            cover_url: None,
            is_airing: true, // Will test that this is NOT overwritten by is_releasing = false
            followed: false,
            next_episode_at: Some(12345), // Should not be clobbered
            site_episode_count: Some(10), // Should not be clobbered
        };
        db.upsert_series(src_id, &existing).unwrap();

        let matched = crate::models::FinishedCard {
            title: "Test Show".into(),
            url: "https://custom.test.com/test-show".into(),
            poster_url: None,
            kind: "TV".into(),
            matched_genre: None,
        };

        let candidate = crate::db::EpisodeSourceCandidate {
            canon_key: "test-show".into(),
            anilist_id: Some(999),
            title: "Test Show".into(),
            known_rows: vec![],
        };

        let detail = crate::models::SeriesDetail { genres: vec!["Action".into()], kind: Some("TV".into()), synopsis: None };
        let episodes = vec![];

        let step = ResolverStep {
            site: &site,
            mirrors: &mirrors,
            matched: &matched,
            candidate: &candidate,
            detail: Some(&detail),
            episodes: &episodes,
            new_slug: "test-show",
            kind: "TV",
            is_releasing: false,
        };
        let (sid, written) = perform_resolver_db_step(&db, &step).unwrap();

        assert!(written);

        // Check source wasn't duplicated (only one source row should exist)
        let source_count: i64 = db.conn.query_row("SELECT count(*) FROM sources", [], |r| r.get(0)).unwrap();
        assert_eq!(source_count, 1);

        // Linked candidate: site genres are not written (AniList source of truth)
        let site_genres_count: i64 = db.conn.query_row("SELECT count(*) FROM series_genres WHERE series_id=?1", [sid], |r| r.get(0)).unwrap();
        assert_eq!(site_genres_count, 0, "linked series must not have site genres written");

        // Check scan-owned fields weren't clobbered
        let (is_airing, next, count): (i64, Option<i64>, Option<i64>) = db.conn.query_row(
            "SELECT is_airing, next_episode_at, site_episode_count FROM series WHERE id=?1",
            [sid],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        ).unwrap();

        assert_eq!(is_airing, 1);
        assert_eq!(next, Some(12345));
        assert_eq!(count, Some(10));
    }

    #[test]
    fn test_perform_resolver_db_step_writes_genres_for_unlinked_candidate() {
        let db = Db::open(":memory:").unwrap();

        let site = SiteInfo { id: "testsite", name: "TestSite", default_base_url: "https://test.com" };
        let mirrors = vec!["https://test.com".to_string()];

        let matched = crate::models::FinishedCard {
            title: "Unlinked Show".into(),
            url: "https://test.com/unlinked-show".into(),
            poster_url: None,
            kind: "OVA".into(),
            matched_genre: None,
        };

        let candidate = crate::db::EpisodeSourceCandidate {
            canon_key: "unlinked-show".into(),
            anilist_id: None,
            title: "Unlinked Show".into(),
            known_rows: vec![],
        };

        let detail = crate::models::SeriesDetail { genres: vec!["Sci-Fi".into()], kind: Some("OVA".into()), synopsis: None };
        let episodes = vec![];

        let step = ResolverStep {
            site: &site,
            mirrors: &mirrors,
            matched: &matched,
            candidate: &candidate,
            detail: Some(&detail),
            episodes: &episodes,
            new_slug: "unlinked-show",
            kind: "OVA",
            is_releasing: false,
        };
        let (sid, written) = perform_resolver_db_step(&db, &step).unwrap();

        assert!(written);
        assert_eq!(db.list_series_genres(sid).unwrap(), vec!["Sci-Fi".to_string()]);
        let kind: String = db.conn.query_row("SELECT kind FROM series WHERE id=?1", [sid], |r| r.get(0)).unwrap();
        assert_eq!(kind, "OVA");
    }

    #[test]
    fn test_decide_show_miss_logic() {
        assert!(!decide_show_miss(&[]));
        assert!(decide_show_miss(&[SiteOutcome::DefinitiveMiss]));
        assert!(decide_show_miss(&[SiteOutcome::DefinitiveMiss, SiteOutcome::DefinitiveMiss]));
        assert!(!decide_show_miss(&[SiteOutcome::DefinitiveMiss, SiteOutcome::TransientError]));
        assert!(!decide_show_miss(&[SiteOutcome::TransientError]));
        assert!(!decide_show_miss(&[SiteOutcome::FoundEpisodes]));
        assert!(!decide_show_miss(&[SiteOutcome::FoundEpisodes, SiteOutcome::DefinitiveMiss]));
    }

    #[test]
    fn test_rows_to_refresh_orders_by_priority_and_dedups_sites_and_skips_active() {
        let known_rows = vec![
            (1, "active_site".to_string(), "https://active/show".to_string()),
            (2, "site_b".to_string(), "https://b/show".to_string()),
            (3, "site_c".to_string(), "https://c/show".to_string()),
            (4, "site_b".to_string(), "https://b/show_duplicate".to_string()),
        ];
        let priority = vec!["site_c".to_string(), "site_b".to_string()];
        let out = rows_to_refresh(&known_rows, &priority, "active_site");

        assert_eq!(out.len(), 2, "active site skipped and duplicate site_b deduped to its newest row");
        assert_eq!(out[0], (3, "site_c".to_string(), "https://c/show".to_string()));
        assert_eq!(out[1], (4, "site_b".to_string(), "https://b/show_duplicate".to_string()));
    }

    #[test]
    fn test_rows_to_refresh_empty_priority_and_missing_priority() {
        let known_rows = vec![
            (10, "site_x".to_string(), "https://x".to_string()),
            (20, "site_y".to_string(), "https://y".to_string()),
            (30, "site_z".to_string(), "https://z".to_string()),
        ];
        // Priority only mentions site_y
        let priority = vec!["site_y".to_string()];
        let out = rows_to_refresh(&known_rows, &priority, "other");
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].1, "site_y", "prioritized site comes first");
        // The rest follow the newest row first
        assert_eq!(out[1].1, "site_z");
        assert_eq!(out[2].1, "site_x");

        // Empty priority: newest row (highest id) first
        let out_empty = rows_to_refresh(&known_rows, &[], "other");
        assert_eq!(out_empty.len(), 3);
        assert_eq!(out_empty[0].1, "site_z");
        assert_eq!(out_empty[1].1, "site_y");
        assert_eq!(out_empty[2].1, "site_x");
    }

    #[test]
    fn test_refetch_failure_never_writes_miss_marker() {
        // Must NOT happen: A refetch failure writes a miss marker.
        assert!(!should_record_group_miss(true, &[SiteOutcome::DefinitiveMiss], true));
        assert!(!should_record_group_miss(true, &[], true));

        // When all refetches succeeded without errors, and all remaining sites were DefinitiveMiss (or no other sites):
        assert!(should_record_group_miss(false, &[SiteOutcome::DefinitiveMiss], true));
        assert!(should_record_group_miss(false, &[], true));

        // Transient error on search:
        assert!(!should_record_group_miss(false, &[SiteOutcome::TransientError], true));
    }
}
