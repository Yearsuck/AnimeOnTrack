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

    for candidate in candidates.into_iter().take(3) {
        let mut found_episodes = false;

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

        let sites_to_try = next_sites_to_try(all_sites, &active_site_id, &site_misses);
        let mut outcomes = Vec::new();

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

                    let detail = match a.parse_series_detail(&scraped.html) {
                        Ok(d) => d,
                        Err(_) => return SiteOutcome::TransientError,
                    };

                    let kind = detail.kind.clone().unwrap_or(matched.kind.clone());
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
                            detail: &detail,
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
                found_episodes = true;
                break;
            }
        }

        if !found_episodes && decide_show_miss(&outcomes) {
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
    detail: &'a crate::models::SeriesDetail,
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

    let sid = db.upsert_series(site_source_id, &series)?;
    if let Some(al_id) = step.candidate.anilist_id {
        let _ = db.set_anilist_id(sid, al_id);
    }

    db.relink_series(sid, step.new_slug, &step.matched.url, step.matched.poster_url.as_deref(), step.kind)?;
    db.replace_series_genres(sid, &step.detail.genres)?;
    db.apply_episode_diff(sid, None, step.episodes)?;
    db.set_followed_canonical(sid, true)?;

    Ok((sid, true))
}

fn next_sites_to_try<'a>(
    all_sites: &'a [crate::adapter::SiteInfo],
    active_site_id: &str,
    misses: &[String],
) -> Vec<&'a crate::adapter::SiteInfo> {
    all_sites
        .iter()
        .filter(|s| s.id != active_site_id && !misses.iter().any(|m| m == s.id))
        .collect()
}

fn mirrors_or_default(loaded: Result<Vec<String>, String>, default_url: &str) -> Vec<String> {
    match loaded {
        Ok(v) if !v.is_empty() => v,
        _ => vec![default_url.to_string()],
    }
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
        let to_try = next_sites_to_try(&sites, "b", &["d".to_string()]);

        let ids: Vec<&str> = to_try.iter().map(|s| s.id).collect();
        assert_eq!(ids, vec!["a", "c"]);
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
        };

        let detail = crate::models::SeriesDetail { genres: vec!["Action".into()], kind: Some("TV".into()), synopsis: None };
        let episodes = vec![];

        let step = ResolverStep {
            site: &site,
            mirrors: &mirrors,
            matched: &matched,
            candidate: &candidate,
            detail: &detail,
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
    fn test_decide_show_miss_logic() {
        assert!(!decide_show_miss(&[]));
        assert!(decide_show_miss(&[SiteOutcome::DefinitiveMiss]));
        assert!(decide_show_miss(&[SiteOutcome::DefinitiveMiss, SiteOutcome::DefinitiveMiss]));
        assert!(!decide_show_miss(&[SiteOutcome::DefinitiveMiss, SiteOutcome::TransientError]));
        assert!(!decide_show_miss(&[SiteOutcome::TransientError]));
        assert!(!decide_show_miss(&[SiteOutcome::FoundEpisodes]));
        assert!(!decide_show_miss(&[SiteOutcome::FoundEpisodes, SiteOutcome::DefinitiveMiss]));
    }
}
