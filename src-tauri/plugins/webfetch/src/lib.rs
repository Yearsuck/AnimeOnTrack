//! Android-only Tauri plugin: load a URL in an off-screen `android.webkit.WebView`,
//! wait until the page has really rendered (same probe as the desktop scraper), and
//! hand the HTML back to Rust. AnimeOnTrack's `scraper_engine` uses it on Android in
//! place of the hidden WebView2 window it opens on Windows (Android has no hidden
//! windows: a `WebviewWindowBuilder` window covers the whole app there).
//!
//! The app depends on this crate only for `target_os = "android"`.

use serde::{Deserialize, Serialize};
use tauri::{
    plugin::{Builder, PluginHandle, TauriPlugin},
    Manager, Runtime,
};

/// One fetch request. Field names match `FetchArgs` in `WebFetchPlugin.kt`.
#[derive(Serialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct FetchRequest {
    pub url: String,
    /// How long to wait for the page to become ready.
    pub timeout_ms: u64,
    /// How long reading the ready page's HTML may take.
    pub read_timeout_ms: u64,
    /// Synchronous script run on the loaded page after its HTML is captured;
    /// its (string) value comes back in `FetchResponse::extra`.
    pub extra_script: Option<String>,
    pub extra_timeout_ms: u64,
    /// Attach the WebView as a 1x1 view (renders like a foreground tab) instead of detached.
    pub attach: bool,
}

/// What the Kotlin side resolves with. `ok == false` carries `error`
/// (`"timeout"`, `"html read timed out"`, a main-frame network/TLS error, a renderer
/// crash, ...). The Kotlin side always answers: one deadline is armed until it does.
#[derive(Deserialize, Debug, Clone, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct FetchResponse {
    pub ok: bool,
    pub error: String,
    pub html: String,
    pub has_extra: bool,
    pub extra: String,
    pub title: String,
    pub final_url: String,
    pub elapsed_ms: i64,
    pub ready_ms: i64,
    pub polls: i64,
}

pub struct WebFetch<R: Runtime>(PluginHandle<R>);

impl<R: Runtime> WebFetch<R> {
    /// Blocking: waits until the Kotlin side resolves. Call it from a worker
    /// thread (`spawn_blocking`), never from the main thread or an async task.
    pub fn fetch_rendered(&self, request: FetchRequest) -> Result<FetchResponse, String> {
        self.0
            .run_mobile_plugin("fetchRendered", request)
            .map_err(|e| e.to_string())
    }
}

pub fn init<R: Runtime>() -> TauriPlugin<R> {
    Builder::new("webfetch")
        .setup(|app, api| {
            let handle = api.register_android_plugin("com.animeontrack.webfetch", "WebFetchPlugin")?;
            app.manage(WebFetch(handle));
            Ok(())
        })
        .build()
}
