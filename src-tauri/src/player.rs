use crate::models::Episode;
use anyhow::{anyhow, Result};
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_opener::OpenerExt;

/// Strategy for "watching" an episode (or opening an info page). Swappable so
/// a future EmbeddedPlayer can implement the same trait.
pub trait EpisodePlayer {
    #[allow(dead_code)]
    fn open(&self, app: &AppHandle, episode: &Episode) -> Result<()> {
        self.open_with_options(app, episode, None, false, false)
    }


    fn open_with_options(
        &self,
        app: &AppHandle,
        episode: &Episode,
        target_monitor: Option<&str>,
        fullscreen: bool,
        clean: bool,
    ) -> Result<()>;
}

/// Opens the URL in the user's default system browser. Kept for the
/// swappable-trait design; no longer the one `open_episode` wires up (see
/// `AppWindowPlayer`).
#[allow(dead_code)]
pub struct BrowserPlayer;

impl EpisodePlayer for BrowserPlayer {
    fn open_with_options(
        &self,
        app: &AppHandle,
        episode: &Episode,
        _target_monitor: Option<&str>,
        _fullscreen: bool,
        _clean: bool,
    ) -> Result<()> {
        app.opener().open_url(&episode.url, None::<&str>)?;
        Ok(())
    }
}

/// Resolves which monitor to target for opening episode windows based on user preference.
pub fn resolve_target_monitor(
    app: &AppHandle,
    monitors: &[tauri::Monitor],
    target_monitor: Option<&str>,
) -> Option<tauri::Monitor> {
    if monitors.is_empty() {
        return None;
    }

    if let Some(pref) = target_monitor {
        let trimmed = pref.trim();
        if !trimmed.is_empty() && trimmed != "default" && trimmed != "current" {
            // Check direct name / id match
            if let Some(found) = monitors.iter().find(|m| m.name().map(|n| n == trimmed).unwrap_or(false)) {
                return Some(found.clone());
            }
            // Check index match (e.g. "index-0" or "0")
            let idx_str = trimmed.strip_prefix("index-").unwrap_or(trimmed);
            if let Ok(idx) = idx_str.parse::<usize>() {
                if let Some(found) = monitors.get(idx) {
                    return Some(found.clone());
                }
            }
        }
    }

    // Default: try current monitor of main window
    if let Some(current) = app.get_webview_window("main").and_then(|w| w.current_monitor().ok().flatten()) {
        return Some(current);
    }

    // Fallback: primary monitor
    if let Some(primary) = app.primary_monitor().ok().flatten() {
        return Some(primary);
    }

    // Fallback: first monitor
    monitors.first().cloned()
}

/// Opens the URL in a **visible, app-owned** WebView2 window — separate from
/// the user's browser (app WebView2 windows use the app's own user-data
/// folder, so a distinct cookie/login store). This is the user-viewing path,
/// deliberately NOT the scraper path: it is **visible**, acquires **no**
/// `SCRAPE_PERMITS`, and runs no HTML extraction (the optional clean-player mode does run one
/// host-side style/layout script, see `clean_player`) — so
/// it never competes with or interferes with `scraper_engine`'s hidden,
/// permit-gated scrape windows. The user solves any Cloudflare challenge
/// interactively here.
///
/// Must be called from an **async** command (see `commands::open_episode`),
/// exactly like `scraper_engine`'s `fetch_html`/`fetch_cover_image` — building
/// a WebView2 window from a synchronous command's thread produced a blank
/// window that closed itself; the async-command context is the proven-working
/// one for window creation in this app.
pub struct AppWindowPlayer;

impl EpisodePlayer for AppWindowPlayer {
    fn open_with_options(
        &self,
        app: &AppHandle,
        episode: &Episode,
        target_monitor: Option<&str>,
        fullscreen: bool,
        clean: bool,
    ) -> Result<()> {
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

        let monitors = app.available_monitors().unwrap_or_default();
        let selected_monitor = resolve_target_monitor(app, &monitors, target_monitor);

        let win = WebviewWindowBuilder::new(app, &label, WebviewUrl::External(url))
            .title("AnimeOnTrack")
            .inner_size(1280.0, 800.0)
            .resizable(true)
            .visible(false)
            // Pop-ups and pop-unders are the worst of the ads on these sites:
            // nothing the user wants to watch needs a second window.
            .on_new_window(|_url, _features| tauri::webview::NewWindowResponse::Deny)
            // A navigation to a known ad host is cancelled instead of replacing
            // the video page (e.g. ad scripts attempting `location.href = ...`).
            .on_navigation(|url| !crate::ad_block::is_blocked_url(url.as_str()))
            .build()?;
        crate::ad_block::install(&win);

        if let Some(m) = selected_monitor {
            let pos = *m.position();
            let scale = m.scale_factor();

            if fullscreen {
                // Maximised, not true fullscreen: a fullscreen window hides the
                // title bar and its close button, so the episode window could not
                // be closed without Alt+F4. The viewer goes fullscreen themselves
                // once the episode plays.
                let _ = win.set_position(tauri::Position::Physical(pos));
                #[cfg(desktop)]
                let _ = win.maximize();
            } else {
                let work_area = m.work_area();
                let work: crate::window_fit::Rect = (
                    work_area.position.x,
                    work_area.position.y,
                    work_area.size.width,
                    work_area.size.height,
                );
                // The window was built at the DEFAULT monitor's scale. Move it
                // onto the target monitor first so a DPI change (e.g. a 125 %
                // laptop screen and a 100 % external one) is applied before
                // sizing, then size it from the target scale: 1280x800 logical
                // client area plus the frame.
                let _ = win.set_position(tauri::Position::Physical(tauri::PhysicalPosition::new(
                    work.0, work.1,
                )));
                let frame = match (win.outer_size(), win.inner_size()) {
                    (Ok(o), Ok(i)) => (
                        o.width.saturating_sub(i.width),
                        o.height.saturating_sub(i.height),
                    ),
                    _ => (0, 0),
                };
                let outer = tauri::PhysicalSize::new(
                    (1280.0 * scale) as u32 + frame.0,
                    (800.0 * scale) as u32 + frame.1,
                );
                let (new_x, new_y, new_w, new_h) =
                    crate::window_fit::centered_in((outer.width, outer.height), work);
                let _ = win.set_size(tauri::Size::Physical(tauri::PhysicalSize::new(
                    new_w.saturating_sub(frame.0).max(1),
                    new_h.saturating_sub(frame.1).max(1),
                )));
                let _ = win.set_position(tauri::Position::Physical(tauri::PhysicalPosition::new(new_x, new_y)));
            }
        } else if fullscreen {
            #[cfg(desktop)]
            let _ = win.maximize();
        }

        let _ = win.show();
        let _ = win.set_focus();
        if clean {
            // Async, off the main thread; leaves the page untouched on failure.
            crate::clean_player::spawn_injection(win.clone(), episode.url.clone());
        }
        Ok(())
    }
}
