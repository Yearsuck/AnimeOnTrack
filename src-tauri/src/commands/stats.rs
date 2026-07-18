use super::*;

#[tauri::command]
pub fn get_genre_stats(state: State<'_, AppState>) -> Result<Vec<GenreStat>, String> {
    let src = get_source_id(&state)?;
    let db = state.db.lock().unwrap();
    db.get_genre_stats(src).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_type_stats(state: State<'_, AppState>) -> Result<Vec<TypeStat>, String> {
    let src = get_source_id(&state)?;
    let db = state.db.lock().unwrap();
    db.get_type_stats(src).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_watch_summary(state: State<'_, AppState>) -> Result<WatchSummary, String> {
    let src = get_source_id(&state)?;
    let db = state.db.lock().unwrap();
    db.get_watch_summary(src).map_err(|e| e.to_string())
}

/// Local watch-insights block for Estadísticas (time watched, funnel, top
/// series, marks-by-day) — pure SQL, no network, see
/// `docs/superpowers/specs/2026-07-13-stats-new-metrics-design.md`.
#[tauri::command]
pub async fn get_watch_insights(state: State<'_, AppState>) -> Result<WatchInsights, String> {
    let src = get_source_id(&state)?;
    let db = state.db.lock().unwrap();
    db.get_watch_insights(src).map_err(|e| e.to_string())
}

/// Followed series with genres/kind/cover, for the 3D relationship graph.
/// The frontend builds the root/hub/link structure from this flat list.
#[tauri::command]
pub fn get_stats_graph(state: State<'_, AppState>) -> Result<Vec<SeriesGraphNode>, String> {
    let src = get_source_id(&state)?;
    let db = state.db.lock().unwrap();
    db.get_stats_graph_data(src).map_err(|e| e.to_string())
}
