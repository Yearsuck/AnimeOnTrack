//! Device-sync Tauri commands.

use std::sync::atomic::{AtomicBool, Ordering};

use tauri::{AppHandle, Emitter, Manager, State};

use super::{get_source_id, AppState};
use crate::backup;
use crate::error::AppError;
use crate::commands::backup::{access_token_or_disconnect, connected_token};
use crate::models::{SyncReport, SyncStatus};
use crate::sync::drive_remote::DriveRemote;
use crate::sync::engine;
use crate::sync::remote::LocalFolderRemote;

/// One at a time: prevents overlapping sync cycles.
static SYNC_RUNNING: AtomicBool = AtomicBool::new(false);

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

fn read_sync_status(state: &AppState) -> Result<SyncStatus, AppError> {
    let mut status = state.with_db(|db| {
        let transport = db
            .get_setting("sync_transport")
            .ok()
            .flatten()
            .unwrap_or_else(|| "off".to_string());
        let folder = db.get_setting("sync_folder").ok().flatten();
        let drive_connected =
            backup::configured_client(db).is_some() && connected_token(db).is_some();
        let device_id = db.sync_device_id().unwrap_or_default();
        let last_at = db
            .get_setting("sync_last_at")
            .ok()
            .flatten()
            .and_then(|s| s.parse::<i64>().ok());
        let last_error = db.get_setting("sync_last_error").ok().flatten();
        let last_report: Option<SyncReport> = db
            .get_setting("sync_last_report")
            .ok()
            .flatten()
            .and_then(|s| serde_json::from_str(&s).ok());
        Ok(SyncStatus {
            transport,
            folder,
            drive_connected,
            device_id,
            last_at,
            last_error,
            last_report,
            running: false,
        })
    })?;
    status.running = SYNC_RUNNING.load(Ordering::SeqCst);
    Ok(status)
}

#[tauri::command]
pub async fn sync_status(state: State<'_, AppState>) -> Result<SyncStatus, String> {
    read_sync_status(&state).map_err(String::from)
}

#[tauri::command]
pub async fn set_sync_transport(
    state: State<'_, AppState>,
    transport: String,
    folder: Option<String>,
) -> Result<SyncStatus, String> {
    let transport = transport.trim().to_lowercase();
    if !["off", "folder", "drive"].contains(&transport.as_str()) {
        return Err(format!("Transporte no válido: {transport}"));
    }

    let trimmed_folder = folder
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());

    if transport == "folder" {
        let path = trimmed_folder
            .ok_or_else(|| "La carpeta no es válida: ruta vacía".to_string())?;
        let p = std::path::Path::new(path);
        if !p.is_absolute() {
            return Err(format!(
                "La carpeta no es válida: la ruta debe ser absoluta ({path})"
            ));
        }
        std::fs::create_dir_all(p)
            .map_err(|e| format!("La carpeta no es válida: {e}"))?;
    }

    if transport == "drive" {
        let ready = state.with_db(|db| {
            Ok(backup::configured_client(db).is_some() && connected_token(db).is_some())
        })?;
        if !ready {
            return Err("Conecta Google Drive primero (Copia de seguridad)".to_string());
        }
    }

    // Detect whether the transport or folder actually changed.
    let changed = state.with_db(|db| {
        let old_transport = db
            .get_setting("sync_transport")
            .ok()
            .flatten()
            .unwrap_or_else(|| "off".to_string());
        let old_folder = db.get_setting("sync_folder").ok().flatten();
        Ok(old_transport != transport
            || (transport == "folder" && old_folder.as_deref() != trimmed_folder))
    })?;

    state.with_db(|db| {
        db.set_setting("sync_transport", &transport)
            .map_err(|e| AppError::Other(e.to_string()))?;
        if transport == "folder" {
            if let Some(path) = trimmed_folder {
                db.set_setting("sync_folder", path)
                    .map_err(|e| AppError::Other(e.to_string()))?;
            }
        }
        if changed {
            // Clear the base so the first sync with the new remote is a union.
            db.save_sync_base(&std::collections::BTreeMap::new())
                .map_err(|e| AppError::Other(e.to_string()))?;
            db.delete_setting("sync_last_error").ok();
        }
        Ok(())
    })?;

    read_sync_status(&state).map_err(String::from)
}

fn read_transport(state: &AppState) -> Result<String, AppError> {
    state.with_db(|db| {
        Ok(db
            .get_setting("sync_transport")
            .ok()
            .flatten()
            .unwrap_or_else(|| "off".to_string()))
    })
}

/// Best-effort: store `sync_last_error`; a DB error or poisoned lock is ignored
/// (the caller already has the original error to return).
fn record_sync_error(state: &AppState, e: &str) {
    let _ = state.with_db(|db| {
        db.set_setting("sync_last_error", e).ok();
        Ok(())
    });
}

async fn run_sync(app: &AppHandle, state: &State<'_, AppState>) -> Result<SyncReport, String> {
    let source_id = match get_source_id(state) {
        Ok(id) => id,
        Err(e) => {
            record_sync_error(state, &e);
            return Err(e);
        }
    };
    let now = now_unix();

    let transport = read_transport(state)?;

    let result = match transport.as_str() {
        "folder" => {
            let folder = state
                .with_db(|db| {
                    Ok(db
                        .get_setting("sync_folder")
                        .ok()
                        .flatten()
                        .ok_or_else(|| "No hay carpeta configurada".to_string()))
                })
                .map_err(String::from)
                .and_then(|r| r);
            match folder {
                Ok(f) => {
                    let remote = LocalFolderRemote::new(f);
                    engine::sync_once(&state.db, source_id, &remote, now).await
                }
                Err(e) => Err(e),
            }
        }
        "drive" => {
            let creds = state
                .with_db(|db| {
                    let client = backup::configured_client(db)
                        .ok_or_else(|| "Google credentials not configured".to_string());
                    let refresh = connected_token(db)
                        .ok_or_else(|| "Not connected to Google Drive".to_string());
                    Ok(match (client, refresh) {
                        (Ok(c), Ok(r)) => Ok((c, r)),
                        (Err(e), _) | (_, Err(e)) => Err(e),
                    })
                })
                .map_err(String::from)
                .and_then(|r| r);
            match creds {
                Ok((client, refresh)) => {
                    match access_token_or_disconnect(state, &client, &refresh).await {
                        Ok(token) => {
                            let remote = DriveRemote::new(token);
                            engine::sync_once(&state.db, source_id, &remote, now).await
                        }
                        Err(e) => Err(e),
                    }
                }
                Err(e) => Err(e),
            }
        }
        _ => Err(format!("Transporte desconocido: {transport}")),
    };

    match result {
        Ok(report) => {
            // Best effort, as before: a failed bookkeeping write (or a
            // poisoned lock) must not fail a sync that already succeeded.
            let _ = state.with_db(|db| {
                db.set_setting("sync_last_at", &now.to_string()).ok();
                let report_json = serde_json::to_string(&report).unwrap_or_default();
                db.set_setting("sync_last_report", &report_json).ok();
                db.delete_setting("sync_last_error").ok();
                Ok(())
            });

            if report.materialized > 0 {
                crate::commands::follow::spawn_library_import(app.clone());
            }
            if report.applied > 0 {
                let _ = app.emit("sync-applied", &report);
            }

            Ok(report)
        }
        Err(e) => {
            record_sync_error(state, &e);
            Err(e)
        }
    }
}

#[tauri::command]
pub async fn sync_now(app: AppHandle, state: State<'_, AppState>) -> Result<SyncReport, String> {
    if SYNC_RUNNING.swap(true, Ordering::SeqCst) {
        return Err("Ya se está sincronizando".to_string());
    }
    let _clear = crate::commands::follow::RunningGuard(&SYNC_RUNNING);

    let transport = read_transport(&state)?;

    if transport == "off" {
        return Err("La sincronización está desactivada".to_string());
    }

    run_sync(&app, &state).await
}

/// Automatic background sync: no-op when off or already running. Errors are
/// recorded in `sync_last_error` and swallowed. Not a Tauri command.
pub async fn auto_sync(app: AppHandle) {
    let state = app.state::<AppState>();

    let transport = match read_transport(&state) {
        Ok(t) => t,
        Err(e) => {
            crate::file_log::warn(&format!("auto sync skipped: {e}"));
            return;
        }
    };
    if transport == "off" {
        return;
    }

    if SYNC_RUNNING.swap(true, Ordering::SeqCst) {
        return;
    }
    let _clear = crate::commands::follow::RunningGuard(&SYNC_RUNNING);

    let _ = run_sync(&app, &state).await;
}
