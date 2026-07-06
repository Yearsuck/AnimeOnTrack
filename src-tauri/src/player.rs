use crate::models::Episode;
use anyhow::Result;
use tauri::AppHandle;
use tauri_plugin_shell::ShellExt;

/// Strategy for "watching" an episode. v1 opens the browser; a future
/// EmbeddedPlayer can implement the same trait.
pub trait EpisodePlayer {
    fn open(&self, app: &AppHandle, episode: &Episode) -> Result<()>;
}

pub struct BrowserPlayer;

impl EpisodePlayer for BrowserPlayer {
    fn open(&self, app: &AppHandle, episode: &Episode) -> Result<()> {
        app.shell().open(&episode.url, None)?;
        Ok(())
    }
}
