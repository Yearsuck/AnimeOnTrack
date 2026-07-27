use super::*;

/// Launch the app's own uninstaller, then quit so the files it needs to remove
/// aren't locked by this running process.
///
/// The Tauri **NSIS** installer drops `uninstall.exe` in the install directory,
/// right next to the main executable — so we resolve it relative to
/// `current_exe()` rather than hunting the registry (which would need extra
/// `windows`-crate registry features we don't otherwise pull in). Running it in
/// place is correct: NSIS copies itself to `%TEMP%` and relaunches from there
/// automatically, so it can delete the original install directory once this
/// process exits.
///
/// This is deliberately **not** a silent uninstall — it hands off to the
/// standard uninstaller UI, which by our bundle config keeps the user's data
/// (`%APPDATA%\com.ernes.aot-scaffold`) in place. The frontend must confirm
/// with the user before calling this; the command itself performs no prompt.
///
/// Returns an error (without exiting) when no uninstaller is found — the normal
/// case for a `cargo run` / dev build or an unpacked binary, where there is
/// nothing to uninstall.
#[tauri::command]
#[cfg(windows)]
pub fn uninstall_app(app: AppHandle) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let dir = exe
        .parent()
        .ok_or_else(|| "no se pudo determinar el directorio de instalación".to_string())?;
    let uninstaller = dir.join("uninstall.exe");
    if !uninstaller.exists() {
        return Err(
            "No se encontró el desinstalador. Esto suele significar que la app se está \
             ejecutando en modo desarrollo o desde una copia no instalada."
                .to_string(),
        );
    }
    std::process::Command::new(&uninstaller)
        .current_dir(dir)
        .spawn()
        .map_err(|e| format!("no se pudo iniciar el desinstalador: {e}"))?;
    // Release our lock on the install directory so the uninstaller can remove
    // it. `app.exit` runs Tauri's normal shutdown; the uninstaller (already
    // relaunched from %TEMP%) continues independently.
    app.exit(0);
    Ok(())
}

#[tauri::command]
#[cfg(not(windows))]
pub fn uninstall_app(_app: AppHandle) -> Result<(), String> {
    Err("La desinstalación desde la app solo está disponible en Windows.".to_string())
}
