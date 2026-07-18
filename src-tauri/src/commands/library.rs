use super::*;

/// Followed series with episode counts, for the library view.
#[tauri::command]
pub fn list_library(state: State<'_, AppState>) -> Result<Vec<crate::models::LibraryItem>, String> {
    let src = get_source_id(&state)?;
    let db = state.db.lock().unwrap();
    db.list_library(src).map_err(|e| e.to_string())
}

/// Pending queue, ordered by episodes-remaining per series. `sort` is the
/// JS-side string `"remaining_asc"` | `"remaining_desc"`; anything else
/// (incl. `None`) defaults to ascending (fewest-left first).
#[tauri::command]
pub fn list_pending(
    state: State<'_, AppState>,
    sort: Option<String>,
) -> Result<Vec<PendingItem>, String> {
    let sort = match sort.as_deref() {
        Some("remaining_desc") => crate::db::PendingSort::RemainingDesc,
        _ => crate::db::PendingSort::RemainingAsc,
    };
    let src = get_source_id(&state)?;
    let db = state.db.lock().unwrap();
    let rows = db.list_pending(src, sort).map_err(|e| e.to_string())?;
    Ok(rows
        .into_iter()
        .map(|(s, e)| PendingItem { series: s, episode: e })
        .collect())
}

#[derive(serde::Serialize)]
pub struct PendingItem {
    pub series: Series,
    pub episode: Episode,
}

#[tauri::command]
pub fn pending_count(state: State<'_, AppState>) -> Result<i64, String> {
    let src = get_source_id(&state)?;
    let db = state.db.lock().unwrap();
    db.pending_count(src).map_err(|e| e.to_string())
}

/// Series with the given backlog status ('want' or 'discarded'), for the
/// swipe mode's "Listas" sub-view.
#[tauri::command]
pub fn list_backlog(state: State<'_, AppState>, status: String) -> Result<Vec<Series>, String> {
    let src = get_source_id(&state)?;
    let db = state.db.lock().unwrap();
    db.list_backlog(src, &status).map_err(|e| e.to_string())
}

/// Series marked "watched outside the app" (the catalog "Ya lo vi" swipe),
/// for the Listas view's "Ya vistas" sub-list — mirrors `list_backlog`.
#[tauri::command]
pub fn list_watched_externally(state: State<'_, AppState>) -> Result<Vec<Series>, String> {
    let src = get_source_id(&state)?;
    let db = state.db.lock().unwrap();
    db.list_watched_externally(src).map_err(|e| e.to_string())
}
