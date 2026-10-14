use anyhow::{anyhow, Result};
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::Manager;

static RUNNING: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MapEntry {
    pub episode: i64,
    pub season: i64,
    pub absolute: Option<i64>,
    pub air_at: Option<i64>,
}

/// Pure parser for ani.zip mappings response body.
/// Keeps only regular episodes (episodeNumber > 0 and seasonNumber >= 1).
/// Drops specials, season 0, missing numbers.
/// Parses airDateUtc (RFC 3339) into unix timestamp seconds, None if unparsable/absent.
/// Sorts by episode; ignores duplicate episode numbers (keeps the first).
/// Invalid JSON returns Err; empty or missing episodes key returns Ok(empty).
pub fn parse_mapping(body: &str) -> Result<Vec<MapEntry>> {
    let value: serde_json::Value = serde_json::from_str(body)?;
    let Some(episodes_obj) = value.get("episodes").and_then(|v| v.as_object()) else {
        return Ok(Vec::new());
    };

    let mut entries = Vec::new();
    for (_key, ep_val) in episodes_obj {
        let Some(ep_obj) = ep_val.as_object() else {
            continue;
        };
        let Some(ep_num) = ep_obj.get("episodeNumber").and_then(|v| v.as_i64()) else {
            continue;
        };
        if ep_num <= 0 {
            continue;
        }
        let Some(season_num) = ep_obj.get("seasonNumber").and_then(|v| v.as_i64()) else {
            continue;
        };
        if season_num < 1 {
            continue;
        }

        let absolute = ep_obj.get("absoluteEpisodeNumber").and_then(|v| v.as_i64());
        let air_at = ep_obj
            .get("airDateUtc")
            .and_then(|v| v.as_str())
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.timestamp());

        entries.push(MapEntry {
            episode: ep_num,
            season: season_num,
            absolute,
            air_at,
        });
    }

    // Stable sort by episode, then keep the first occurrence of each episode number.
    entries.sort_by_key(|e| e.episode);
    entries.dedup_by_key(|e| e.episode);

    Ok(entries)
}

/// Fetch ani.zip mapping for `anilist_id`.
/// 15 s timeout, User-Agent `AnimeOnTrack/<CARGO_PKG_VERSION>`.
/// HTTP 404 or empty body -> Ok(None).
/// Other HTTP non-success -> Err.
/// Success -> Ok(Some(entries)).
pub async fn fetch_map(anilist_id: i64) -> Result<Option<Vec<MapEntry>>> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .user_agent(format!("AnimeOnTrack/{}", env!("CARGO_PKG_VERSION")))
        .build()?;

    let url = format!("https://api.ani.zip/mappings?anilist_id={anilist_id}");
    let resp = client.get(&url).send().await?;
    let status = resp.status();
    if status == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !status.is_success() {
        return Err(anyhow!("ani.zip HTTP error {status} for anilist_id {anilist_id}"));
    }

    let body = resp.text().await?;
    if body.trim().is_empty() {
        return Ok(None);
    }

    let entries = parse_mapping(&body)?;
    Ok(Some(entries))
}

/// Spawn the background ani.zip synchronization task.
/// Fire-and-forget via `tauri::async_runtime::spawn`.
pub fn spawn_sync(app: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        run_sync(app).await;
    });
}

struct RunningGuard;
impl Drop for RunningGuard {
    fn drop(&mut self) {
        RUNNING.store(false, Ordering::SeqCst);
    }
}

/// Store one fetched map, holding the DB lock only for the write.
fn store_map(app: &tauri::AppHandle, id: i64, entries: &[MapEntry], now: i64) {
    let state = app.state::<crate::commands::AppState>();
    let result = match state.db.lock() {
        Ok(db) => db.replace_episode_map(id, entries, now),
        Err(_) => return,
    };
    if let Err(e) = result {
        eprintln!("[anizip] failed to store episode map for {id}: {e}");
    }
}

async fn run_sync(app: tauri::AppHandle) {
    if RUNNING.swap(true, Ordering::SeqCst) {
        return;
    }
    let _guard = RunningGuard;

    // Do not compete with startup.
    tokio::time::sleep(std::time::Duration::from_secs(30)).await;

    let mut attempted = std::collections::HashSet::new();
    loop {
        let now = chrono::Utc::now().timestamp();
        let stale_ids = {
            let state = app.state::<crate::commands::AppState>();
            let Ok(db) = state.db.lock() else {
                eprintln!("[anizip] DB lock poisoned");
                break;
            };
            match db.episode_map_stale_ids(now, 20) {
                Ok(ids) => ids,
                Err(e) => {
                    eprintln!("[anizip] failed to query stale ids: {e}");
                    break;
                }
            }
        };

        let unattempted: Vec<i64> = stale_ids.into_iter().filter(|id| !attempted.contains(id)).collect();
        if unattempted.is_empty() {
            break;
        }

        for id in unattempted {
            attempted.insert(id);
            match fetch_map(id).await {
                Ok(Some(entries)) => store_map(&app, id, &entries, now),
                Ok(None) => store_map(&app, id, &[], now),
                Err(e) => {
                    eprintln!("[anizip] error fetching map for {id}: {e}");
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_mapping_185756_excerpt() {
        let json = r#"{
            "episodes": {
                "1": {"seasonNumber": 3, "episodeNumber": 1, "absoluteEpisodeNumber": 25, "airDateUtc": "2026-09-27T14:30:00Z", "runtime": 24, "episode": "1", "title": {"en": "Headman"}},
                "2": {"seasonNumber": 3, "episodeNumber": 2, "absoluteEpisodeNumber": 26, "airDateUtc": "2026-10-04T14:30:00Z", "episode": "2"}
            },
            "mappings": {"anilist_id": 185756, "mal_id": 60601},
            "episodeCount": 8,
            "specialCount": 0
        }"#;

        let entries = parse_mapping(json).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(
            entries[0],
            MapEntry {
                episode: 1,
                season: 3,
                absolute: Some(25),
                air_at: Some(1790519400),
            }
        );
        assert_eq!(
            entries[1],
            MapEntry {
                episode: 2,
                season: 3,
                absolute: Some(26),
                air_at: Some(chrono::DateTime::parse_from_rfc3339("2026-10-04T14:30:00Z").unwrap().timestamp()),
            }
        );
    }

    #[test]
    fn test_parse_mapping_specials_season_zero_and_duplicates() {
        let json = r#"{
            "episodes": {
                "S1": {"episode": "S1", "airdate": "2024-05-26", "title": {"en": "Special"}},
                "S51": {"seasonNumber": 0, "episodeNumber": 51, "episode": "S51"},
                "1": {"seasonNumber": 1, "episodeNumber": 1, "absoluteEpisodeNumber": 1},
                "1_dup": {"seasonNumber": 1, "episodeNumber": 1, "absoluteEpisodeNumber": 999},
                "2": {"seasonNumber": 1, "episodeNumber": 2}
            }
        }"#;

        let entries = parse_mapping(json).unwrap();
        assert_eq!(entries.len(), 2);
        // Ep 1 keeps the first one (absolute 1, not 999)
        assert_eq!(
            entries[0],
            MapEntry {
                episode: 1,
                season: 1,
                absolute: Some(1),
                air_at: None,
            }
        );
        // Ep 2 has absolute: None
        assert_eq!(
            entries[1],
            MapEntry {
                episode: 2,
                season: 1,
                absolute: None,
                air_at: None,
            }
        );
    }

    #[test]
    fn test_parse_mapping_invalid_json() {
        assert!(parse_mapping("not json").is_err());
        assert!(parse_mapping("{").is_err());
    }

    #[test]
    fn test_parse_mapping_empty_object_or_no_episodes() {
        assert_eq!(parse_mapping("{}").unwrap(), Vec::<MapEntry>::new());
        assert_eq!(parse_mapping(r#"{"episodes": {}}"#).unwrap(), Vec::<MapEntry>::new());
        assert_eq!(parse_mapping(r#"{"other": 123}"#).unwrap(), Vec::<MapEntry>::new());
    }

    #[test]
    fn test_running_guard_clears_on_exit() {
        assert!(!RUNNING.load(Ordering::SeqCst));
        {
            let _guard = RunningGuard;
            RUNNING.store(true, Ordering::SeqCst);
            assert!(RUNNING.load(Ordering::SeqCst));
        }
        assert!(!RUNNING.load(Ordering::SeqCst));
    }
}
