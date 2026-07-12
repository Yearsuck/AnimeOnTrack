use crate::models::Episode;
use anyhow::{anyhow, Result};
use std::sync::mpsc;
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_opener::OpenerExt;

/// Strategy for "watching" an episode (or opening an info page). Swappable so
/// a future EmbeddedPlayer can implement the same trait.
pub trait EpisodePlayer {
    fn open(&self, app: &AppHandle, episode: &Episode) -> Result<()>;
}

/// Opens the URL in the user's default system browser. Kept for the
/// swappable-trait design; no longer the one `open_episode` wires up (see
/// `AppWindowPlayer`).
#[allow(dead_code)]
pub struct BrowserPlayer;

impl EpisodePlayer for BrowserPlayer {
    fn open(&self, app: &AppHandle, episode: &Episode) -> Result<()> {
        app.opener().open_url(&episode.url, None::<&str>)?;
        Ok(())
    }
}

/// Opens the URL in a **visible, app-owned** WebView2 window — separate from
/// the user's browser (app WebView2 windows use the app's own user-data
/// folder, so a distinct cookie/login store). This is the user-viewing path,
/// deliberately NOT the scraper path: it is **visible**, acquires **no**
/// `SCRAPE_PERMITS`, and runs **no** host-side `eval()`/HTML extraction — so
/// it never competes with or interferes with `scraper_engine`'s hidden,
/// permit-gated scrape windows. The user solves any Cloudflare challenge
/// interactively here.
pub struct AppWindowPlayer;

impl EpisodePlayer for AppWindowPlayer {
    fn open(&self, app: &AppHandle, episode: &Episode) -> Result<()> {
        // Unique label per open → each episode/info link gets its own
        // independent window the user can arrange or close.
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let label = format!("viewer-{nanos}");
        let url_str = episode.url.clone();
        let app_main = app.clone();
        let app_build = app.clone();

        // Build the window on the MAIN thread. Tauri runs command handlers off
        // the main thread, and a WebView2 window created off it comes up blank
        // and tears itself down after a moment (verified: the same builder run
        // from the main thread loads and stays open fine). run_on_main_thread
        // dispatches the creation onto the UI loop where WebView2 needs it; we
        // hand the build Result back over a channel so callers still see errors.
        let (tx, rx) = mpsc::channel::<std::result::Result<(), String>>();
        app_main.run_on_main_thread(move || {
            let built = (|| -> std::result::Result<(), String> {
                let parsed = url_str.parse().map_err(|_| format!("bad url: {url_str}"))?;
                WebviewWindowBuilder::new(&app_build, &label, WebviewUrl::External(parsed))
                    .title("AnimeOnTrack")
                    .inner_size(1280.0, 800.0)
                    .visible(true)
                    .build()
                    .map_err(|e| e.to_string())?;
                Ok(())
            })();
            let _ = tx.send(built);
        })
        .map_err(|e| anyhow!("run_on_main_thread failed: {e}"))?;

        rx.recv()
            .map_err(|_| anyhow!("window build channel closed before a result"))?
            .map_err(|e| anyhow!("failed to open viewer window: {e}"))
    }
}
