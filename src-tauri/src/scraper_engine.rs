use anyhow::{anyhow, Result};
use std::collections::HashMap;
use std::time::Duration;
use tauri::{AppHandle, Emitter, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

/// Result of scraping a page: the rendered HTML, plus any poster images found
/// on it, already fetched as base64 data URIs (keyed by their original remote
/// `src`). Empty on pages with no matching images (e.g. episode-list pages).
pub struct ScrapeResult {
    pub html: String,
    pub covers: HashMap<String, String>,
}

fn emit_stage(app: &AppHandle, stage: &str) {
    let _ = app.emit("scrape-stage", stage);
}

/// Load `url` in a hidden webview, wait for Cloudflare/JS to settle, then
/// return the rendered HTML and any poster images on it.
///
/// Extraction is driven host-side via WebView2's `ExecuteScript` (see `eval`),
/// NOT via page-side Tauri IPC. Tauri does not inject its IPC into external
/// remote pages (and exposing it to an untrusted scraped site would be a
/// security hole), so the host-driven approach is the only correct one here.
///
/// Poster images are fetched the same way and for the same reason: the site's
/// images sit behind the same Cloudflare check as its pages, so an `<img>` in
/// our own UI (a different, never-challenged origin) can't load them directly.
/// Fetching them as `fetch()` calls made *from inside* the already-challenged
/// page (same-origin, cookies included automatically) works, so we do that
/// once here and cache the result as a data URI instead of the remote URL.
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
    .visible(false)
    .build()?;

    let result = extract_when_ready(app, &window).await;
    window.close().ok();
    result
}

/// Poll until the page is past any Cloudflare interstitial and has rendered
/// real content, then extract the full HTML and any poster images. Condition-
/// based waiting rather than a fixed sleep, because the challenge clears in a
/// variable amount of time.
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

    emit_stage(app, "covers");
    let covers = match fetch_covers(window).await {
        Ok(m) => {
            eprintln!("[covers] fetched {} image(s)", m.len());
            m
        }
        Err(e) => {
            eprintln!("[covers] failed: {e}");
            HashMap::new()
        }
    };

    Ok(ScrapeResult { html, covers })
}

/// Fetch every poster (`.bsx img`) on the current page as a base64 data URI,
/// from inside the page itself (same-origin fetch, cookies included
/// automatically). Returns an empty map on pages with no such images — this
/// is a no-op on episode-list pages, so it's safe to call unconditionally.
///
/// WebView2's `ExecuteScript` does NOT await a returned promise for us (an
/// unresolved promise just serializes to `null`), so this can't be a single
/// "await the async work, return the result" script like the other `eval`
/// calls here. Instead it kicks the async fetch work off in the page (fire
/// and forget, no top-level `await`), then polls a JS global synchronously —
/// the same condition-based-waiting pattern already proven for the
/// Cloudflare-readiness check.
async fn fetch_covers(window: &WebviewWindow) -> Result<HashMap<String, String>> {
    const START: &str = r#"(function(){
        try {
            window.__aotCoversDone = false;
            window.__aotCoversErr = null;
            var imgs = Array.prototype.slice.call(document.querySelectorAll('.bsx img'));
            window.__aotCoversTotal = imgs.length;
            (async () => {
                const out = {};
                await Promise.all(imgs.map(async (img) => {
                    const src = img.src;
                    if (!src || out[src]) return;
                    try {
                        const res = await fetch(src);
                        const blob = await res.blob();
                        const dataUri = await new Promise((resolve, reject) => {
                            const r = new FileReader();
                            r.onload = () => resolve(r.result);
                            r.onerror = () => reject(new Error('read failed'));
                            r.readAsDataURL(blob);
                        });
                        out[src] = dataUri;
                    } catch (e) { /* leave this one out; caller keeps the remote url */ }
                }));
                window.__aotCovers = JSON.stringify(out);
                window.__aotCoversDone = true;
            })().catch((e) => {
                window.__aotCoversErr = String(e);
                window.__aotCoversDone = true;
            });
            return JSON.stringify({ started: true, imgCount: imgs.length });
        } catch (e) {
            return JSON.stringify({ started: false, error: String(e) });
        }
    })()"#;
    const IS_DONE: &str = "JSON.stringify(window.__aotCoversDone === true)";
    const READ: &str = "window.__aotCovers";

    let start_json = eval(window, START, 10).await?;
    let start_info: String = serde_json::from_str(&start_json)?;
    eprintln!("[covers] start: {start_info}");

    let mut done = false;
    for _ in 0..40 {
        tokio::time::sleep(Duration::from_millis(500)).await;
        if let Ok(json) = eval(window, IS_DONE, 5).await {
            if serde_json::from_str::<bool>(&json).unwrap_or(false) {
                done = true;
                break;
            }
        }
    }
    if !done {
        return Err(anyhow!("cover fetch did not finish within 20s"));
    }

    let json = eval(window, READ, 10).await?;
    let inner: String = serde_json::from_str(&json)?;
    if inner.is_empty() || inner == "null" {
        return Err(anyhow!("cover fetch produced no data (window.__aotCovers empty)"));
    }
    let map: HashMap<String, String> = serde_json::from_str(&inner)?;
    Ok(map)
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
