use super::*;

/// Followed series with episode counts, for the library view.
#[tauri::command]
pub async fn list_library(state: State<'_, AppState>) -> Result<Vec<crate::models::LibraryItem>, String> {
    let src = get_source_id(&state)?;
    Ok(state.with_db(|db| Ok(db.list_library(src)?))?)
}

/// Pending queue, ordered by episodes-remaining per series. `sort` is the
/// JS-side string `"remaining_asc"` | `"remaining_desc"`; anything else
/// (incl. `None`) defaults to ascending (fewest-left first).
#[tauri::command]
pub async fn list_pending(
    state: State<'_, AppState>,
    sort: Option<String>,
) -> Result<Vec<PendingItem>, String> {
    let sort = match sort.as_deref() {
        Some("remaining_desc") => crate::db::PendingSort::RemainingDesc,
        _ => crate::db::PendingSort::RemainingAsc,
    };
    let src = state.source_id.lock().unwrap().unwrap_or(0);
    let rows = state.with_db(|db| {
        let known: Vec<&str> = crate::adapter::all_sites().iter().map(|s| s.id).collect();
        let priority = db.get_site_priority(&known)?;
        Ok(db.list_pending(src, sort, &priority)?)
    })?;
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
pub async fn pending_count(state: State<'_, AppState>) -> Result<i64, String> {
    let src = state.source_id.lock().unwrap().unwrap_or(0);
    Ok(state.with_db(|db| {
        let known: Vec<&str> = crate::adapter::all_sites().iter().map(|s| s.id).collect();
        let priority = db.get_site_priority(&known)?;
        Ok(db.pending_count(src, &priority)?)
    })?)
}

/// Series with the given backlog status ('want' or 'discarded'), for the
/// swipe mode's "Listas" sub-view.
#[tauri::command]
pub async fn list_backlog(state: State<'_, AppState>, status: String) -> Result<Vec<Series>, String> {
    let src = get_source_id(&state)?;
    Ok(state.with_db(|db| Ok(db.list_backlog(src, &status)?))?)
}

/// Series marked "watched outside the app" (the catalog "Ya lo vi" swipe),
/// for the Listas view's "Ya vistas" sub-list — mirrors `list_backlog`.
#[tauri::command]
pub async fn list_watched_externally(state: State<'_, AppState>) -> Result<Vec<Series>, String> {
    let src = get_source_id(&state)?;
    Ok(state.with_db(|db| Ok(db.list_watched_externally(src)?))?)
}
