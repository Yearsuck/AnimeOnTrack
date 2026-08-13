use super::*;

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

/// Followed series with genres/kind/cover, for the 3D relationship graph.
/// The frontend builds the root/hub/link structure from this flat list.
#[tauri::command]
pub fn get_stats_graph(state: State<'_, AppState>) -> Result<Vec<SeriesGraphNode>, String> {
    let db = state.db.lock().unwrap();
    db.get_stats_graph_data().map_err(|e| e.to_string())
}
