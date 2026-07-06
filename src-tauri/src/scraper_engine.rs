use anyhow::{anyhow, Result};
use std::time::Duration;
use tauri::{AppHandle, Listener, WebviewUrl, WebviewWindowBuilder};

/// Load `url` in a hidden webview, wait for it to settle (letting Cloudflare
/// and JS run), then return the rendered outer HTML.
pub async fn fetch_html(app: &AppHandle, url: &str) -> Result<String> {
    let label = format!("scraper-{}", uuid_like());
    let window = WebviewWindowBuilder::new(
        app,
        &label,
        WebviewUrl::External(url.parse().map_err(|_| anyhow!("bad url: {url}"))?),
    )
    .visible(false)
    .build()?;

    // Give the page time to pass Cloudflare and render.
    tokio::time::sleep(Duration::from_secs(6)).await;

    // Stash the rendered HTML in a JS global.
    window.eval(r#"window.__ANIMEONTRACK_HTML__ = document.documentElement.outerHTML;"#)?;

    // Pull the stored HTML via the JS event bus round-trip.
    let html = read_global(&window).await?;
    window.close().ok();
    Ok(html)
}

async fn read_global(window: &tauri::WebviewWindow) -> Result<String> {
    // Tauri's `eval` is fire-and-forget, so the round-trip uses the JS event bus
    // (`window.__TAURI__.event.emit`) to hand the HTML back to Rust, where this
    // `listen` handler captures it.
    use std::sync::{Arc, Mutex};
    let slot: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let slot2 = slot.clone();
    let win = window.clone();

    win.listen("animeontrack://html", move |event| {
        if let Ok(s) = serde_json::from_str::<String>(event.payload()) {
            *slot2.lock().unwrap() = Some(s);
        }
    });
    window.eval(
        r#"window.__TAURI__.event.emit('animeontrack://html', window.__ANIMEONTRACK_HTML__);"#,
    )?;

    for _ in 0..50 {
        if let Some(h) = slot.lock().unwrap().take() {
            return Ok(h);
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Err(anyhow!("timed out reading page HTML"))
}

fn uuid_like() -> u128 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()
}
