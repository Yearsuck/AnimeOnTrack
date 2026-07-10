use anyhow::{anyhow, Result};
use std::sync::LazyLock;
use std::time::Duration;
use tauri::{AppHandle, Emitter, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

/// Process-wide cap on concurrent scraper windows, shared by every caller
/// (refresh()'s own chunking, discover_swipe_card, decide_swipe, etc). Each
/// of those previously gated its OWN concurrency independently (e.g.
/// refresh() fetching 2 series at a time) with no awareness of any other
/// command doing the same thing at the same time — so a refresh running
/// while the user swiped through the discovery deck could pile up well
/// beyond 2 simultaneous WebView2 windows, which is what was actually
/// causing the multi-second stalls and the occasional `ExecuteScript timed
/// out` failure, not the per-fetch poll timing. A single shared semaphore
/// here means "2 concurrent scraper windows" is a real app-wide ceiling,
/// not a per-caller one that different commands can stack on top of.
const SCRAPE_CONCURRENCY: usize = 2;
static SCRAPE_PERMITS: LazyLock<tokio::sync::Semaphore> =
    LazyLock::new(|| tokio::sync::Semaphore::new(SCRAPE_CONCURRENCY));

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
    let total_started = std::time::Instant::now();
    let _permit = SCRAPE_PERMITS.acquire().await;
    emit_stage(app, "opening");
    let label = format!("scraper-{}", uuid_like());
    let build_started = std::time::Instant::now();
    let window = WebviewWindowBuilder::new(
        app,
        &label,
        WebviewUrl::External(url.parse().map_err(|_| anyhow!("bad url: {url}"))?),
    )
    .title("AnimeOnTrack scraper")
    .inner_size(1000.0, 800.0)
    // Hidden: a visible popup stealing focus/screen space on every series
    // scraped was too disruptive during refresh. The poll-based readiness
    // check in extract_when_ready handles the normal case with no user
    // interaction; same invisible pattern fetch_cover_image already uses
    // safely. Trade-off: if Cloudflare ever escalates to a challenge that
    // needs a human click (not just a timed JS check), there's no visible
    // window to solve it in — that shows up as a 40s "did not become ready"
    // error instead of a stuck window waiting on you.
    .visible(false)
    .build()?;
    let window_build_ms = build_started.elapsed().as_millis();

    let result = extract_when_ready(app, &window, window_build_ms, total_started).await;
    window.close().ok();
    result
}

/// Poll until the page is past any Cloudflare interstitial and has rendered
/// real content, then extract the full HTML. Condition-based waiting rather
/// than a fixed sleep, because the challenge clears in a variable amount of
/// time.
async fn extract_when_ready(
    app: &AppHandle,
    window: &WebviewWindow,
    window_build_ms: u128,
    total_started: std::time::Instant,
) -> Result<ScrapeResult> {
    // NOTE: does NOT require readyState==='complete' — pages on this site
    // carry ad/tracker resources that can keep the load event from ever
    // firing, leaving readyState stuck at 'interactive' indefinitely even
    // though the real DOM content we need is already there. 'interactive'
    // (DOM parsed, DOMContentLoaded fired) plus a real body size is enough.
    // Also excludes transient gateway-error titles (observed live:
    // "animeytx.net | 502: Bad gateway") — those aren't a Cloudflare
    // challenge, but they're just as much "not really ready" and, unlike a
    // 404, often clear on their own within a few seconds. Accepting one as
    // "ready" previously meant extracting an error page as if it were real
    // content, which parses to zero results and cascades into trying every
    // configured mirror (even unrelated ones) for nothing.
    const PROBE: &str = "JSON.stringify({\
ready: (document.readyState==='interactive' || document.readyState==='complete') \
&& !!document.body && document.body.innerHTML.length>3000 \
&& !/just a moment|un momento|checking your browser|verificando|502|503|504|bad gateway|gateway time-out|service unavailable/i.test(document.title),\
readyState: document.readyState,\
title: document.title,\
len: document.body ? document.body.innerHTML.length : -1})";

    emit_stage(app, "verifying");
    // eval() introspects the already-loaded page's DOM via WebView2's script
    // host — it issues zero network requests, so polling frequency has no
    // bearing on how the site sees this client (unlike the fetch itself,
    // which stays paced by the caller's between-series delay). Most pages
    // are ready on the very first check, so start fast (150ms) to catch
    // that common case, then back off toward 1s for pages that genuinely
    // need longer (a real Cloudflare JS challenge to clear), capped at the
    // same ~40s ceiling as before.
    const MAX_WAIT: Duration = Duration::from_secs(40);
    const MIN_INTERVAL: Duration = Duration::from_millis(150);
    const MAX_INTERVAL: Duration = Duration::from_secs(1);
    let started = std::time::Instant::now();
    let mut interval = MIN_INTERVAL;
    let mut poll_num = 0u32;
    let mut ready = false;
    while started.elapsed() < MAX_WAIT {
        tokio::time::sleep(interval).await;
        poll_num += 1;
        match eval(window, PROBE, 10).await {
            Ok(json) => {
                let inner: String = serde_json::from_str(&json).unwrap_or_default();
                let is_ready = serde_json::from_str::<serde_json::Value>(&inner)
                    .ok()
                    .and_then(|v| v.get("ready").and_then(|r| r.as_bool()))
                    .unwrap_or(false);
                if is_ready {
                    eprintln!("[scrape] poll {poll_num} ({:?} elapsed): {inner}", started.elapsed());
                    ready = true;
                    break;
                }
                // Only log the slow-path polls (fast-path success above always logs).
                eprintln!("[scrape] poll {poll_num}: {inner}");
            }
            Err(e) => eprintln!("[scrape] poll {poll_num}: eval FAILED: {e}"),
        }
        interval = std::cmp::min(interval + interval / 2, MAX_INTERVAL);
    }
    if !ready {
        return Err(anyhow!(
            "page did not become ready within 40s (a Cloudflare challenge may require manual solving)"
        ));
    }
    let time_to_ready_ms = started.elapsed().as_millis();

    emit_stage(app, "extracting");
    let extract_started = std::time::Instant::now();
    let json = eval(window, "document.documentElement.outerHTML", 15).await?;
    let html: String =
        serde_json::from_str(&json).map_err(|e| anyhow!("failed to decode page HTML: {e}"))?;
    let extract_ms = extract_started.elapsed().as_millis();
    let total_ms = total_started.elapsed().as_millis();

    // Phase-0 measurement instrumentation (see
    // docs/superpowers/specs/2026-07-10-scraper-performance-design.md): one
    // line per fetch with the four numbers the design's "measure first"
    // method needs to decide whether optimization B (a hot-window pool) is
    // worth its risk. Kept permanently, same style as the existing
    // `[scrape] poll N` lines — cheap, and useful for diagnosing slow
    // refreshes after this ships too.
    eprintln!(
        "[scrape] fetch timing: window_build_ms={window_build_ms} time_to_ready_ms={time_to_ready_ms} (polls={poll_num}) extract_ms={extract_ms} total_ms={total_ms}"
    );

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
    let _permit = SCRAPE_PERMITS.acquire().await;
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
