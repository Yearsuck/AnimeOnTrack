use crate::models::Episode;
use anyhow::{anyhow, Result};
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
        let url = episode
            .url
            .parse()
            .map_err(|_| anyhow!("bad url: {}", episode.url))?;
        WebviewWindowBuilder::new(app, &label, WebviewUrl::External(url))
            .title("AnimeOnTrack")
            .inner_size(1280.0, 800.0)
            .visible(true)
            .build()?;
        Ok(())
    }
}
