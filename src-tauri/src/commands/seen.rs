use super::*;

/// Open an episode or info page in a visible, app-owned WebView2 window
/// (separate from the user's browser — see `AppWindowPlayer`). Does NOT mark
/// it seen — the user marks seen/unseen explicitly via `set_seen`.
///
/// `async` on purpose: creating a WebView2 window from a *synchronous* command
/// produced a blank window that closed itself (and an earlier main-thread-
/// dispatch attempt deadlocked the UI). Async commands run on Tauri's runtime,
/// the same context `scraper_engine` builds its windows from successfully.
#[tauri::command]
pub async fn open_episode(app: AppHandle, url: String) -> Result<(), String> {
    let ep = Episode {
        id: 0,
        series_id: 0,
        number: String::new(),
        title: None,
        url,
        released_at: None,
        seen: false,
    };
    AppWindowPlayer.open(&app, &ep).map_err(|e| e.to_string())
}

/// Mark an episode seen or unseen (persisted).
#[tauri::command]
pub fn set_seen(state: State<'_, AppState>, episode_id: i64, seen: bool) -> Result<(), String> {
    let db = state.db.lock().unwrap();
    db.set_seen(episode_id, seen).map_err(|e| e.to_string())
}

/// Mark an episode seen/unseen, cascading to keep watching gap-free: marking
/// seen also marks every earlier episode of the series seen; marking unseen
/// also marks every later one unseen.
#[tauri::command]
pub fn set_seen_cascade(
    state: State<'_, AppState>,
    series_id: i64,
    number: String,
    seen: bool,
) -> Result<(), String> {
    let db = state.db.lock().unwrap();
    db.set_seen_cascade(series_id, &number, seen).map_err(|e| e.to_string())
}

/// Hard-delete a series row outright — used for "Eliminar del todo" on
/// discarded backlog rows, which never have episodes so this is always
/// safe there.
#[tauri::command]
pub fn delete_series(state: State<'_, AppState>, series_id: i64) -> Result<(), String> {
    let db = state.db.lock().unwrap();
    db.delete_series(series_id).map_err(|e| e.to_string())
}
