use anyhow::{anyhow, Result};
use std::time::Duration;
use tauri::{AppHandle, Emitter, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

/// During development, show the scraper webview so you can watch the scrape
/// (Cloudflare challenge, page load). Set to `false` for a headless/hidden scrape.
const DEV_VISIBLE: bool = true;

/// Emit a log line to the dev console AND to the app UI (event `scan-log`).
fn log(app: &AppHandle, msg: impl AsRef<str>) {
    let msg = msg.as_ref();
    eprintln!("[scraper] {msg}");
    let _ = app.emit("scan-log", msg.to_string());
}

/// Load `url` in a webview, wait for Cloudflare/JS to settle, then return the
/// rendered outer HTML.
///
/// Extraction is driven host-side via WebView2's `ExecuteScript` (see `eval`),
/// NOT via page-side Tauri IPC. Tauri does not inject its IPC into external
/// remote pages (and exposing it to an untrusted scraped site would be a
/// security hole), so the host-driven approach is the only correct one here.
pub async fn fetch_html(app: &AppHandle, url: &str) -> Result<String> {
    log(app, format!("opening {url}"));
    let label = format!("scraper-{}", uuid_like());
    let window = WebviewWindowBuilder::new(
        app,
        &label,
        WebviewUrl::External(url.parse().map_err(|_| anyhow!("bad url: {url}"))?),
    )
    .title("AnimeOnTrack scraper")
    .inner_size(1000.0, 800.0)
    .visible(DEV_VISIBLE)
    .build()?;

    let result = extract_when_ready(app, &window).await;
    match &result {
        Ok(html) => log(app, format!("extracted {} bytes of HTML", html.len())),
        Err(e) => log(app, format!("ERROR: {e}")),
    }
    window.close().ok();
    result
}

/// Poll until the page is past any Cloudflare interstitial and has rendered real
/// content, then extract the full HTML. Condition-based waiting rather than a
/// fixed sleep, because the challenge clears in a variable amount of time.
async fn extract_when_ready(app: &AppHandle, window: &WebviewWindow) -> Result<String> {
    // Returns a small JSON object so we can log the live page title each poll.
    const PROBE: &str = "JSON.stringify({\
ready: document.readyState==='complete' && !!document.body && document.body.innerHTML.length>3000 \
&& !/just a moment|un momento|checking your browser|verificando/i.test(document.title),\
title: document.title,\
len: document.body ? document.body.innerHTML.length : 0})";

    let mut ready = false;
    for i in 0..40 {
        tokio::time::sleep(Duration::from_secs(1)).await;
        match eval(window, PROBE).await {
            Ok(json) => {
                // `json` is a JSON string containing our JSON object -> decode twice.
                let inner: String = serde_json::from_str(&json).unwrap_or(json.clone());
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&inner) {
                    let title = v.get("title").and_then(|t| t.as_str()).unwrap_or("");
                    let len = v.get("len").and_then(|l| l.as_u64()).unwrap_or(0);
                    let is_ready = v.get("ready").and_then(|r| r.as_bool()).unwrap_or(false);
                    log(app, format!("poll {}s: ready={is_ready} len={len} title=\"{title}\"", i + 1));
                    if is_ready {
                        ready = true;
                        break;
                    }
                } else {
                    log(app, format!("poll {}s: probe result unparsable: {inner}", i + 1));
                }
            }
            Err(e) => log(app, format!("poll {}s: probe error: {e}", i + 1)),
        }
    }
    if !ready {
        return Err(anyhow!(
            "page did not become ready within 40s (a Cloudflare challenge may require manual solving)"
        ));
    }

    log(app, "page ready, extracting HTML");
    let json = eval(window, "document.documentElement.outerHTML").await?;
    let html: String =
        serde_json::from_str(&json).map_err(|e| anyhow!("failed to decode page HTML: {e}"))?;
    Ok(html)
}

/// Execute `script` in the webview via WebView2's `ExecuteScript` and return its
/// JSON-encoded result string. Works on external pages because it is driven from
/// the host, not from page JS.
async fn eval(window: &WebviewWindow, script: &str) -> Result<String> {
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

    match tokio::time::timeout(Duration::from_secs(20), rx).await {
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
