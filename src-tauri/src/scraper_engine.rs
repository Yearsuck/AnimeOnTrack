use anyhow::{anyhow, Result};
use std::collections::HashMap;
use std::time::Duration;
use tauri::{AppHandle, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

/// Result of scraping a page: the rendered HTML, plus any poster images found
/// on it, already fetched as base64 data URIs (keyed by their original remote
/// `src`). Empty on pages with no matching images (e.g. episode-list pages).
pub struct ScrapeResult {
    pub html: String,
    pub covers: HashMap<String, String>,
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

    let result = extract_when_ready(&window).await;
    window.close().ok();
    result
}

/// Poll until the page is past any Cloudflare interstitial and has rendered
/// real content, then extract the full HTML and any poster images. Condition-
/// based waiting rather than a fixed sleep, because the challenge clears in a
/// variable amount of time.
async fn extract_when_ready(window: &WebviewWindow) -> Result<ScrapeResult> {
    const PROBE: &str = "JSON.stringify(document.readyState==='complete' \
&& !!document.body && document.body.innerHTML.length>3000 \
&& !/just a moment|un momento|checking your browser|verificando/i.test(document.title))";

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

    let json = eval(window, "document.documentElement.outerHTML", 15).await?;
    let html: String =
        serde_json::from_str(&json).map_err(|e| anyhow!("failed to decode page HTML: {e}"))?;

    let covers = fetch_covers(window).await.unwrap_or_default();

    Ok(ScrapeResult { html, covers })
}

/// Fetch every poster (`.bsx img`) on the current page as a base64 data URI,
/// from inside the page itself (same-origin fetch, cookies included
/// automatically). Returns an empty map on pages with no such images — this
/// is a no-op on episode-list pages, so it's safe to call unconditionally.
async fn fetch_covers(window: &WebviewWindow) -> Result<HashMap<String, String>> {
    const SCRIPT: &str = r#"JSON.stringify(await (async () => {
        const imgs = [...document.querySelectorAll('.bsx img')];
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
        return out;
    })())"#;

    let json = eval(window, SCRIPT, 30).await?;
    let inner: String = serde_json::from_str(&json)?;
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
