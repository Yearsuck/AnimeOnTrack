use anyhow::{anyhow, Result};
use std::time::Duration;
use tauri::{AppHandle, Emitter, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

/// Result of scraping a page: just the rendered HTML. Cover images are NOT
/// fetched here (see `fetch_cover_image`) — doing it in bulk for every series
/// on the airing list (~150 images in one go) reads to Cloudflare as scraping
/// abuse and triggers rate limiting independent of having a valid session,
/// which is a server-side limit no client-side technique can fix. Covers are
/// instead fetched one at a time, only for series the user actually follows.
pub struct ScrapeResult {
    pub html: String,
}

fn emit_stage(app: &AppHandle, stage: &str) {
    let _ = app.emit("scrape-stage", stage);
}

/// Load `url` in a hidden webview, wait for Cloudflare/JS to settle, then
/// return the rendered HTML.
///
/// Extraction is driven host-side via WebView2's `ExecuteScript` (see `eval`),
/// NOT via page-side Tauri IPC. Tauri does not inject its IPC into external
/// remote pages (and exposing it to an untrusted scraped site would be a
/// security hole), so the host-driven approach is the only correct one here.
pub async fn fetch_html(app: &AppHandle, url: &str) -> Result<ScrapeResult> {
    emit_stage(app, "opening");
    let label = format!("scraper-{}", uuid_like());
    let window = WebviewWindowBuilder::new(
        app,
        &label,
        WebviewUrl::External(url.parse().map_err(|_| anyhow!("bad url: {url}"))?),
    )
    .title("AnimeOnTrack scraper")
    .inner_size(1000.0, 800.0)
    .visible(true) // temporarily visible: lets you see/solve an interactive
    // Cloudflare challenge if it ever escalates past the automatic JS one.
    .build()?;

    let result = extract_when_ready(app, &window).await;
    window.close().ok();
    result
}

/// Poll until the page is past any Cloudflare interstitial and has rendered
/// real content, then extract the full HTML. Condition-based waiting rather
/// than a fixed sleep, because the challenge clears in a variable amount of
/// time.
async fn extract_when_ready(app: &AppHandle, window: &WebviewWindow) -> Result<ScrapeResult> {
    const PROBE: &str = "JSON.stringify(document.readyState==='complete' \
&& !!document.body && document.body.innerHTML.length>3000 \
&& !/just a moment|un momento|checking your browser|verificando/i.test(document.title))";

    emit_stage(app, "verifying");
    let mut ready = false;
    for _ in 0..40 {
        tokio::time::sleep(Duration::from_secs(1)).await;
        if let Ok(json) = eval(window, PROBE, 10).await {
            let is_ready: bool = serde_json::from_str(&json).unwrap_or(false);
            if is_ready {
                ready = true;
                break;
            }
        }
    }
    if !ready {
        return Err(anyhow!(
            "page did not become ready within 40s (a Cloudflare challenge may require manual solving)"
        ));
    }

    emit_stage(app, "extracting");
    let json = eval(window, "document.documentElement.outerHTML", 15).await?;
    let html: String =
        serde_json::from_str(&json).map_err(|e| anyhow!("failed to decode page HTML: {e}"))?;

    Ok(ScrapeResult { html })
}

/// Fetch a single image as a base64 JPEG: open a small window pointed
/// directly at the image URL, wait for the browser's native image viewer to
/// finish loading it, then read the already-decoded pixels off a `<canvas>`
/// with `toDataURL()`. No fetch, no XHR, no promises.
///
/// Call this sparingly (once per followed series per refresh, not in bulk —
/// see `ScrapeResult`'s doc comment for why).
pub async fn fetch_cover_image(app: &AppHandle, image_url: &str) -> Result<String> {
    let label = format!("cover-{}", uuid_like());
    let window = WebviewWindowBuilder::new(
        app,
        &label,
        WebviewUrl::External(
            image_url.parse().map_err(|_| anyhow!("bad image url: {image_url}"))?,
        ),
    )
    .title("AnimeOnTrack cover")
    .inner_size(300.0, 450.0)
    .visible(false)
    .build()?;

    const READY_PROBE: &str = "JSON.stringify(!!document.images[0] \
&& document.images[0].complete && document.images[0].naturalWidth > 0)";
    let mut ready = false;
    for _ in 0..20 {
        tokio::time::sleep(Duration::from_millis(150)).await;
        if let Ok(json) = eval(&window, READY_PROBE, 5).await {
            if serde_json::from_str::<bool>(&json).unwrap_or(false) {
                ready = true;
                break;
            }
        }
    }

    let result = if !ready {
        Err(anyhow!("image did not finish loading in time"))
    } else {
        const EXTRACT_SCRIPT: &str = r#"(function(){
            var img = document.images[0];
            var canvas = document.createElement('canvas');
            canvas.width = img.naturalWidth;
            canvas.height = img.naturalHeight;
            canvas.getContext('2d').drawImage(img, 0, 0);
            return canvas.toDataURL('image/jpeg', 0.85);
        })()"#;
        eval(&window, EXTRACT_SCRIPT, 10)
            .await
            .and_then(|json| serde_json::from_str::<String>(&json).map_err(|e| anyhow!("decode failed: {e}")))
    };
    window.close().ok();
    result
}

/// Execute `script` in the webview via WebView2's `ExecuteScript` and return
/// its JSON-encoded result string. Works on external pages because it is
/// driven from the host, not from page JS. If `script`'s value is (or
/// resolves from) a promise, ExecuteScript awaits it before returning.
async fn eval(window: &WebviewWindow, script: &str, timeout_secs: u64) -> Result<String> {
    let (tx, rx) = tokio::sync::oneshot::channel::<std::result::Result<String, String>>();
    let script = script.to_string();

    window
        .with_webview(move |_platform| {
            #[cfg(windows)]
            {
                use webview2_com::ExecuteScriptCompletedHandler;
                use windows::core::{HSTRING, PCWSTR};

                let mut tx = Some(tx);
                unsafe {
                    let core = match _platform.controller().CoreWebView2() {
                        Ok(c) => c,
                        Err(e) => {
                            if let Some(tx) = tx.take() {
                                let _ = tx.send(Err(format!("no CoreWebView2: {e}")));
                            }
                            return;
                        }
                    };
                    let handler = ExecuteScriptCompletedHandler::create(Box::new(
                        move |_err: windows::core::Result<()>, result: String| {
                            if let Some(tx) = tx.take() {
                                let _ = tx.send(Ok(result));
                            }
                            Ok(())
                        },
                    ));
                    let hs = HSTRING::from(script.as_str());
                    // ExecuteScript copies the script synchronously; `hs` stays alive
                    // for the call. On error the handler is dropped (never invoked),
                    // which closes the channel and surfaces as an error to the caller.
                    let _ = core.ExecuteScript(PCWSTR(hs.as_ptr()), &handler);
                }
            }
            #[cfg(not(windows))]
            {
                let _ = &_platform;
                let _ = tx.send(Err("scraping is only implemented on Windows".into()));
            }
        })
        .map_err(|e| anyhow!("with_webview failed: {e}"))?;

    match tokio::time::timeout(Duration::from_secs(timeout_secs), rx).await {
        Ok(Ok(Ok(s))) => Ok(s),
        Ok(Ok(Err(e))) => Err(anyhow!("ExecuteScript error: {e}")),
        Ok(Err(_)) => Err(anyhow!("ExecuteScript channel closed before result")),
        Err(_) => Err(anyhow!("ExecuteScript timed out")),
    }
}

fn uuid_like() -> u128 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()
}
