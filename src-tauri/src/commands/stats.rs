use super::*;
use crate::models::DustyEntry;
use crate::models::PopularityBias;
use crate::models::GenreCard;
use crate::models::YearlyActivity;

// Every command below is intentionally **not** scoped to the active site —
// Estadísticas reflects the whole library across every site you use, not
// whichever one happens to be active (see `Db::get_watch_summary`'s doc
// comment). `get_genre_stats`/`get_type_stats` here are the Estadísticas
// versions; the swipe deck's own per-site `get_genre_affinity` is unrelated
// and untouched.

#[tauri::command]
pub fn get_genre_stats(state: State<'_, AppState>) -> Result<Vec<GenreStat>, String> {
    let db = state.db.lock().unwrap();
    db.get_genre_stats().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_dusty_watchlist(state: State<'_, AppState>) -> Result<Vec<DustyEntry>, String> {
    let db = state.db.lock().unwrap();
    db.get_dusty_watchlist().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_genre_cards(state: State<'_, AppState>) -> Result<Vec<GenreCard>, String> {
    let db = state.db.lock().unwrap();
    db.get_genre_cards().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_type_stats(state: State<'_, AppState>) -> Result<Vec<TypeStat>, String> {
    let db = state.db.lock().unwrap();
    db.get_type_stats().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_watch_summary(state: State<'_, AppState>) -> Result<WatchSummary, String> {
    let db = state.db.lock().unwrap();
    db.get_watch_summary().map_err(|e| e.to_string())
}

/// Local watch-insights block for Estadísticas (time watched, funnel, top
/// series, marks-by-day) — pure SQL, no network, see
/// `docs/superpowers/specs/2026-07-13-stats-new-metrics-design.md`.
#[tauri::command]
pub async fn get_watch_insights(state: State<'_, AppState>) -> Result<WatchInsights, String> {
    let db = state.db.lock().unwrap();
    db.get_watch_insights().map_err(|e| e.to_string())
}

/// Binge record — the single day the user marked the most episodes seen in one
/// sitting. Pure SQL, no network; see `Db::get_binge_record`.
#[tauri::command]
pub fn get_binge_record(state: State<'_, AppState>) -> Result<BingeRecord, String> {
    let db = state.db.lock().unwrap();
    db.get_binge_record().map_err(|e| e.to_string())
}

/// Average days to finish a franchise across finished franchises with real seen episodes.
/// Returns null when no qualifying franchises exist.
#[tauri::command]
pub fn get_avg_completion_days(state: State<'_, AppState>) -> Result<Option<f64>, String> {
    let db = state.db.lock().unwrap();
    db.get_avg_completion_days().map_err(|e| e.to_string())
}

/// Full-year activity heatmap for Estadísticas — GitHub-contributions-style
/// grid with year selector. New additive command; does not touch the existing
/// 30-day chart.
#[tauri::command]
pub async fn get_yearly_activity(state: State<'_, AppState>, year: i32) -> Result<YearlyActivity, String> {
    let db = state.db.lock().unwrap();
    db.get_yearly_activity(year).map_err(|e| e.to_string())
}

/// Distinct years (local time) that have at least one seen episode, descending.
/// Always includes the current year so the year selector is never empty.
#[tauri::command]
pub async fn get_activity_years(state: State<'_, AppState>) -> Result<Vec<i32>, String> {
    let db = state.db.lock().unwrap();
    db.get_activity_years().map_err(|e| e.to_string())
}

/// Followed series with genres/kind/cover, for the 3D relationship graph.
/// The frontend builds the root/hub/link structure from this flat list.
#[tauri::command]
pub fn get_stats_graph(state: State<'_, AppState>) -> Result<Vec<SeriesGraphNode>, String> {
    let db = state.db.lock().unwrap();
    db.get_stats_graph_data().map_err(|e| e.to_string())
}

/// 24-hour distribution of when episodes are marked seen (0-23), zero-filled.
/// Pure SQL, no network — see `Db::get_hourly_distribution`.
#[tauri::command]
pub fn get_hourly_distribution(state: State<'_, AppState>) -> Result<Vec<HourCount>, String> {
    let db = state.db.lock().unwrap();
    db.get_hourly_distribution().map_err(|e| e.to_string())
}

/// Mainstream vs underground taste score — average AniList popularity of
/// followed series linked to the catalog, normalized to 0-10. Cross-site
/// canonical dedup via `canon_key`. Returns `None` fields when fewer than
/// 3 linked followed series exist.
#[tauri::command]
pub fn get_popularity_bias(state: State<'_, AppState>) -> Result<PopularityBias, String> {
    let db = state.db.lock().unwrap();
    db.get_popularity_bias().map_err(|e| e.to_string())
}
