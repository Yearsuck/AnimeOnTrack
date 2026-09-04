use super::*;
use crate::backup;
use tauri_plugin_opener::OpenerExt;

#[tauri::command]
pub fn backup_status(state: State<'_, AppState>) -> Result<BackupStatus, String> {
    let db = state.db.lock().unwrap();
    let refresh = db.get_setting("gdrive_refresh_token").ok().flatten();
    let last_at = db.get_setting("backup_last_at_iso").ok().flatten();
    let size = db
        .get_setting("backup_size_bytes")
        .ok()
        .flatten()
        .and_then(|s| s.parse::<i64>().ok());
    let last_error = db.get_setting("backup_last_error").ok().flatten();
    Ok(BackupStatus {
        configured: backup::configured_client(&db).is_some(),
        connected: refresh.is_some(),
        last_at,
        size_bytes: size,
        last_error,
    })
}

/// Store (or clear) the Google OAuth client the backup authenticates with, so
/// a user can set Drive up without rebuilding the app. Passing an empty string
/// for either field clears it, falling back to whatever the build was compiled
/// with.
///
/// Changing the client invalidates any existing connection: a refresh token is
/// issued to one specific client id and is meaningless to another, so the
/// stored token and file id are dropped and the user reconnects. Doing that
/// here beats leaving a token behind that fails confusingly on the next
/// backup.
#[tauri::command]
pub fn set_google_credentials(
    state: State<'_, AppState>,
    client_id: String,
    client_secret: String,
) -> Result<BackupStatus, String> {
    {
        let db = state.db.lock().unwrap();
        let previous = backup::configured_client(&db);
        db.set_setting("google_client_id", client_id.trim()).map_err(|e| e.to_string())?;
        db.set_setting("google_client_secret", &backup::secure_store::protect(client_secret.trim()))
            .map_err(|e| e.to_string())?;
        if backup::configured_client(&db) != previous {
            db.delete_setting("gdrive_refresh_token").map_err(|e| e.to_string())?;
            db.delete_setting("gdrive_file_id").map_err(|e| e.to_string())?;
        }
    }
    backup_status(state)
}

#[tauri::command]
pub async fn connect_drive(app: AppHandle, state: State<'_, AppState>) -> Result<BackupStatus, String> {
    let (cid, secret) = {
        let db = state.db.lock().unwrap();
        backup::configured_client(&db).ok_or("Google credentials not configured")?
    };
    let pkce = backup::oauth::pkce_pair();
    let app_for_open = app.clone();
    let (code, redirect_uri) = backup::oauth::run_loopback_and_get_code(&cid, &pkce.challenge, |url| {
        let _ = app_for_open.opener().open_url(url, None::<&str>);
    })
    .await?;
    let tokens = backup::oauth::exchange_code(&cid, &secret, &code, &pkce.verifier, &redirect_uri).await?;
    let refresh = tokens.refresh_token.ok_or("Google did not return a refresh token")?;
    {
        let db = state.db.lock().unwrap();
        db.set_setting("gdrive_refresh_token", &backup::secure_store::protect(&refresh))
            .map_err(|e| e.to_string())?;
    }
    backup_status(state)
}

#[tauri::command]
pub fn disconnect_drive(state: State<'_, AppState>) -> Result<BackupStatus, String> {
    {
        let db = state.db.lock().unwrap();
        db.delete_setting("gdrive_refresh_token").map_err(|e| e.to_string())?;
        db.delete_setting("gdrive_file_id").map_err(|e| e.to_string())?;
    }
    backup_status(state)
}

#[tauri::command]
pub async fn backup_now(app: AppHandle, state: State<'_, AppState>) -> Result<BackupStatus, String> {
    let dir = backup_dir(&app)?;
    let (client, refresh, bytes, sig) = {
        let db = state.db.lock().unwrap();
        let client = backup::configured_client(&db)
            .ok_or("Google credentials not configured")?;
        let refresh = db
            .get_setting("gdrive_refresh_token")
            .ok()
            .flatten()
            .map(|s| backup::secure_store::unprotect(&s))
            .filter(|s| !s.is_empty())
            .ok_or("Not connected to Google Drive")?;
        let bytes = backup::snapshot_bytes(&db, &dir)?;
        let sig = backup::signature_string(db.signature_counts().map_err(|e| e.to_string())?);
        (client, refresh, bytes, sig)
    };
    let token = backup::access_token(&client, &refresh).await?;
    let existing = {
        let db = state.db.lock().unwrap();
        db.get_setting("gdrive_file_id").ok().flatten()
    };
    // Same reason as `restore_latest`: without the by-name lookup, backing up
    // from a second machine (or after a restore, which replaces the settings
    // table with the snapshot's) would upload a *second* file into
    // appDataFolder and the two machines would silently stop converging.
    let existing = match existing {
        Some(id) => Some(id),
        None => backup::drive::find_backup_file(&token).await.ok().flatten(),
    };
    let file_id = match existing {
        Some(id) => {
            backup::drive::update_backup(&token, &id, bytes).await?;
            id
        }
        None => backup::drive::create_backup(&token, bytes).await?,
    };
    let meta = backup::drive::get_metadata(&token, &file_id).await.ok();
    {
        let db = state.db.lock().unwrap();
        db.set_setting("gdrive_file_id", &file_id).map_err(|e| e.to_string())?;
        db.set_setting("backup_last_at_unix", &backup_now_unix().to_string()).map_err(|e| e.to_string())?;
        db.set_setting("backup_last_at_iso", &backup_now_iso()).map_err(|e| e.to_string())?;
        db.set_setting("backup_signature", &sig).map_err(|e| e.to_string())?;
        if let Some(m) = meta.as_ref().and_then(|m| m.size.clone()) {
            db.set_setting("backup_size_bytes", &m).ok();
        }
        // A successful backup clears any previously-recorded failure — the
        // Settings warning banner is about the *current* state, not history.
        db.delete_setting("backup_last_error").ok();
    }
    backup_status(state)
}

fn backup_now_unix() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64
}

/// Cheap display timestamp; not meant to be parsed back, only shown in the UI.
fn backup_now_iso() -> String {
    chrono::Utc::now().to_rfc3339()
}

/// Throttled auto-backup: no-op unless credentials are configured, Drive is
/// connected, AND it's been >=24h since the last backup with a changed
/// signature (`backup::is_auto_backup_due`). Called from the startup spawn in
/// `lib.rs` and opportunistically at the end of `refresh()`. Errors are
/// swallowed by callers — this must never surface to or block the user.
#[tauri::command]
pub async fn auto_backup_if_due(app: AppHandle) -> Result<(), String> {
    let state = app.state::<AppState>();
    let (last_at, last_sig, cur_sig, connected) = {
        let db = state.db.lock().unwrap();
        let connected = backup::configured_client(&db).is_some()
            && db.get_setting("gdrive_refresh_token").ok().flatten().is_some();
        let last_at = db
            .get_setting("backup_last_at_unix")
            .ok()
            .flatten()
            .and_then(|s| s.parse::<i64>().ok());
        let last_sig = db.get_setting("backup_signature").ok().flatten().unwrap_or_default();
        let cur_sig = backup::signature_string(db.signature_counts().map_err(|e| e.to_string())?);
        (last_at, last_sig, cur_sig, connected)
    };
    if !connected {
        return Ok(());
    }
    if !backup::is_auto_backup_due(last_at, backup_now_unix(), &last_sig, &cur_sig) {
        return Ok(());
    }
    // Reuse the manual path. A failure here must never propagate to the
    // caller (this runs fire-and-forget after `refresh()`/at startup — see
    // `spawn`/`spawn_episode_backfill`'s doc comments for why those never
    // block their trigger on background work) — but it also must not just
    // vanish: record it so `backup_status`/Settings can tell the user
    // backups have stopped working instead of silently doing nothing on
    // every future cycle (expired OAuth grant, quota, a Drive API change).
    if let Err(e) = backup_now(app.clone(), app.state::<AppState>()).await {
        let state = app.state::<AppState>();
        let db = state.db.lock().unwrap();
        db.set_setting("backup_last_error", &e).ok();
    }
    Ok(())
}
