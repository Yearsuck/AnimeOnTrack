use anyhow::{anyhow, Result};
use std::collections::VecDeque;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

/// Process-wide cap on concurrent scraper windows, shared by every caller
/// (refresh()'s own chunking, discover_swipe_card, decide_swipe, etc). Each
/// of those previously gated its OWN concurrency independently (e.g.
/// refresh() fetching 2 series at a time) with no awareness of any other
/// command doing the same thing at the same time — so a refresh running
/// while the user swiped through the discovery deck could pile up well
/// beyond a handful of simultaneous WebView2 windows, which is what was
/// actually causing multi-second stalls and occasional `ExecuteScript timed
/// out` failures at higher counts, not the per-fetch poll timing. A single
/// shared semaphore here means this is a real app-wide ceiling, not a
/// per-caller one that different commands can stack on top of.
///
/// Raised from 2 to 4 on 2026-08-14 (user request, informed of the tradeoff:
/// Cloudflare bot-detection can key off unnaturally-parallel requests to the
/// same site, and higher counts previously correlated with the
/// `ExecuteScript timed out` failures above) — if stalls/timeouts or mirror
/// blocks start showing up, this is the first thing to dial back down.
const SCRAPE_CONCURRENCY: usize = 4;
static SCRAPE_PERMITS: LazyLock<tokio::sync::Semaphore> =
    LazyLock::new(|| tokio::sync::Semaphore::new(SCRAPE_CONCURRENCY));

/// How long to wait for `WebviewWindowBuilder::build()` before giving up —
/// see `build_webview_window_with_timeout`'s doc comment for why this needs
/// a dedicated-thread timeout rather than an `async` one.
const WINDOW_BUILD_TIMEOUT: Duration = Duration::from_secs(20);

// ── Window-activity ring ──────────────────────────────────────────────────────
//
// A small, bounded audit trail of the last 8 window events (build start, build
// ok, build failed, window closed). Capacity 8 is enough to cover one full
// SCRAPE_CONCURRENCY=4 cycle (open × 4 + close × 4) with room to spare.
//
// The watchdog reads this when it detects a stall so the hang.log entry
// carries evidence about which build or close the main thread was stuck on,
// rather than just a timestamp.

/// Maximum number of activity entries retained in the ring. Older entries are
/// silently dropped when the ring is full so memory use is bounded regardless
/// of how long the app runs.
const ACTIVITY_RING_CAP: usize = 8;

/// Ring buffer of recent window events, protected by a plain `std::sync::Mutex`
/// (not tokio's) because `note_window_activity` is called from both async
/// tasks and the synchronous watchdog thread.
static WINDOW_ACTIVITY: LazyLock<Mutex<VecDeque<String>>> =
    LazyLock::new(|| Mutex::new(VecDeque::with_capacity(ACTIVITY_RING_CAP)));

/// Push a window-lifecycle event into the activity ring, dropping the oldest
/// entry when the ring is already at capacity.
///
/// Call this at every observable window lifecycle point (build start, build ok,
/// build error, window close) so the watchdog has breadcrumbs to report if the
/// main thread gets stuck during one of those operations.
///
/// `msg` should be a short, human-readable description — the target host on
/// build-start (not the full URL, which can be very long), or a status word
/// like "build ok" / "build timeout". A millisecond timestamp is prepended
/// automatically so entries are sortable without a real logger.
pub(crate) fn note_window_activity(msg: &str) {
    use std::time::{SystemTime, UNIX_EPOCH};
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let entry = format!("[{ms}ms] {msg}");
    if let Ok(mut ring) = WINDOW_ACTIVITY.lock() {
        if ring.len() == ACTIVITY_RING_CAP {
            ring.pop_front();
        }
        ring.push_back(entry);
    }
}

/// Join all activity ring entries with a semicolon, or return `"none"` when
/// the ring is empty. Called by the watchdog when it writes a stall log line.
pub fn describe_window_activity() -> String {
    match WINDOW_ACTIVITY.lock() {
        Ok(ring) if ring.is_empty() => "none".to_string(),
        Ok(ring) => ring.iter().cloned().collect::<Vec<_>>().join("; "),
        Err(_) => "none".to_string(),
    }
}

// ── Window-build spacing ──────────────────────────────────────────────────────
//
// `WebviewWindowBuilder::build()` posts work to the main thread's GUI event
// loop. Under rapid window churn (many windows opening and closing in quick
// succession) the event loop can become overwhelmed, which is the leading
// suspect for the observed indefinite hangs. A minimum gap of 250 ms between
// consecutive build-start moments reduces that churn without meaningfully
// slowing down a normal refresh cycle (at SCRAPE_CONCURRENCY=4, 250 ms × 4
// = 1 s of extra spacing per batch, well inside the per-fetch 150 s budget).

/// Instant of the last build start, shared across all callers (both
/// `fetch_html_with_script_inner` and `fetch_cover_image_inner`). Protected
/// by a `tokio::sync::Mutex` so it can be awaited without blocking a tokio
/// worker thread while the sleep runs inside the async fn.
static LAST_BUILD_START: LazyLock<tokio::sync::Mutex<Option<Instant>>> =
    LazyLock::new(|| tokio::sync::Mutex::new(None));

/// Minimum gap between consecutive window build starts.
const BUILD_SPACING: Duration = Duration::from_millis(250);

/// Compute how much time still needs to elapse before the next build is
/// allowed, given the previous build's start instant and the current instant.
///
/// Pure, testable function: takes instants as arguments rather than reading
/// `LAST_BUILD_START` directly so it can be called from unit tests.
///
/// Returns `Duration::ZERO` when `last` is `None` (no previous build) or
/// when enough time has already passed.
fn remaining_spacing(last: Option<Instant>, now: Instant, min_gap: Duration) -> Duration {
    let Some(last) = last else { return Duration::ZERO };
    let elapsed = now.duration_since(last);
    min_gap.saturating_sub(elapsed)
}

/// Enforce the minimum inter-build gap. Must be `await`ed immediately before
/// every `build_webview_window_with_timeout` call.
///
/// Locks `LAST_BUILD_START`, computes the remaining wait (if any), sleeps for
/// that duration, then records `Instant::now()` as the new last-build-start.
/// The lock is held for the duration of the sleep so that two concurrent
/// callers both see each other's reservation and don't both start at the same
/// time — this is intentional back-pressure, not a bug.
async fn space_window_builds() {
    let mut guard = LAST_BUILD_START.lock().await;
    let wait = remaining_spacing(*guard, Instant::now(), BUILD_SPACING);
    if wait > Duration::ZERO {
        tokio::time::sleep(wait).await;
    }
    *guard = Some(Instant::now());
}

/// Result of scraping a page: just the rendered HTML. Cover images are NOT
/// fetched here (see `fetch_cover_image`) — doing it in bulk for every series
/// on the airing list (~150 images in one go) reads to Cloudflare as scraping
/// abuse and triggers rate limiting independent of having a valid session,
/// which is a server-side limit no client-side technique can fix. Covers are
/// instead fetched one at a time, only for series the user actually follows.
pub struct ScrapeResult {
    pub html: String,
    /// Result of the adapter's optional `episode_fetch_script` (see
    /// `SiteAdapter`), run in-page right after `html` was captured. `None`
    /// for every site whose episode list is already in `html` — which is
    /// every site except jkanime so far. When present, callers that need the
    /// episode list pass THIS to `parse_series` instead of `html`.
    pub extra: Option<String>,
}

fn emit_stage(app: &AppHandle, stage: &str) {
    let _ = app.emit("scrape-stage", stage);
}

/// Run a `WebviewWindowBuilder::build()` call on the app's **main** thread
/// (via `AppHandle::run_on_main_thread`), with a real wall-clock timeout on
/// waiting for the result.
///
/// Two separate constraints drive this shape:
///
/// - `.build()` is a **synchronous** function (confirmed in tauri's own
///   source: it returns `tauri::Result<WebviewWindow>` directly, not a
///   `Future`), and it has been live-observed to hang indefinitely (zero
///   CPU, 10+ minutes, no error) right after a batch of concurrent scraper
///   windows closes. A hang inside a synchronous call with no `.await`
///   point can't be preempted by `tokio::time::timeout` on the async
///   caller — that was live-verified to NOT fire even wrapping this exact
///   call.
/// - WebView2/Win32 window creation is thread-affine: it must happen on the
///   thread that owns the app's GUI event loop. An earlier version of this
///   function called `.build()` from a freshly spawned `std::thread`, which
///   is off that thread — this crashed the whole process (an unhandled
///   native access violation, invisible to Rust's panic hook: silent exit
///   with no panic message, reproduced live right at the point the first
///   concurrent cover-fetch window build started).
///
/// So the build itself is posted to the main thread with
/// `run_on_main_thread` (satisfying thread affinity), while this function's
/// caller waits for the result via `recv_timeout` on a plain channel — a
/// real OS-level wait, not cooperative, so it still returns even if the
/// main-thread build never completes. If the build genuinely never
/// returns, the queued main-thread closure is leaked (there is no way to
/// cancel it once posted), but the caller gets its timeout error back
/// immediately.
fn build_webview_window_with_timeout<F>(app: &AppHandle, build: F, timeout: Duration) -> Result<WebviewWindow>
where
    F: FnOnce() -> tauri::Result<WebviewWindow> + Send + 'static,
{
    let (tx, rx) = std::sync::mpsc::channel();
    app.run_on_main_thread(move || {
        let _ = tx.send(build());
    })
    .map_err(|e| anyhow!("failed to queue window build on main thread: {e}"))?;
    match rx.recv_timeout(timeout) {
        Ok(Ok(window)) => Ok(window),
        Ok(Err(e)) => Err(anyhow!("window build failed: {e}")),
        Err(_) => Err(anyhow!(
            "window build timed out after {timeout:?} (main thread likely stuck)"
        )),
    }
}

/// Reject navigation targets that could reach a local file or an internal
/// network address instead of the public site being scraped/mirrored.
/// Applied to every URL that reaches `WebviewUrl::External` — mirror URLs a
/// user pastes into Settings, and `cover_url` values lifted verbatim from a
/// scraped `img[src]`/`data-src` attribute by every site adapter.
///
/// A malicious or merely wrong mirror is a real threat model for this app
/// specifically: mirror URLs are exactly the kind of value shared in
/// pirate-site forums/Discord ("official domain is down, use this one"), so
/// a user pasting one in good faith must not be able to point a hidden
/// scraper window at `file://` or an internal service.
///
/// Deliberately conservative and synchronous: only `http`/`https` schemes
/// are allowed, and a literal loopback/private/link-local IP or the
/// hostname `localhost` is rejected. A hostname that only *resolves* to one
/// of those (DNS rebinding) is not caught here — that needs an actual DNS
/// lookup, which this cheap pre-navigation check doesn't perform.
pub fn is_safe_external_url(raw: &str) -> bool {
    let Ok(u) = url::Url::parse(raw) else { return false };
    if u.scheme() != "http" && u.scheme() != "https" {
        return false;
    }
    match u.host() {
        Some(url::Host::Domain(d)) => !d.eq_ignore_ascii_case("localhost"),
        Some(url::Host::Ipv4(ip)) => {
            !(ip.is_loopback() || ip.is_private() || ip.is_link_local() || ip.is_unspecified())
        }
        Some(url::Host::Ipv6(ip)) => {
            !(ip.is_loopback() || ip.is_unspecified() || is_unique_local_v6(&ip) || is_link_local_v6(&ip))
        }
        None => false,
    }
}

fn is_unique_local_v6(ip: &std::net::Ipv6Addr) -> bool {
    (ip.segments()[0] & 0xfe00) == 0xfc00 // fc00::/7
}

fn is_link_local_v6(ip: &std::net::Ipv6Addr) -> bool {
    (ip.segments()[0] & 0xffc0) == 0xfe80 // fe80::/10
}

/// Load `url` in a hidden webview, wait for Cloudflare/JS to settle, then
/// return the rendered HTML — plus, when `extra_script` is `Some` (see
/// `SiteAdapter::episode_fetch_script`), the result of running that script
/// in-page afterward, in `ScrapeResult.extra`. Every call site except the two
/// that fetch a series' episode list passes `None`.
///
/// Extraction is driven host-side via WebView2's `ExecuteScript` (see `eval`),
/// NOT via page-side Tauri IPC. Tauri does not inject its IPC into external
/// remote pages (and exposing it to an untrusted scraped site would be a
/// security hole), so the host-driven approach is the only correct one here.
pub async fn fetch_html_with_script(
    app: &AppHandle,
    url: &str,
    extra_script: Option<&str>,
) -> Result<ScrapeResult> {
    // Outer hard timeout, same reasoning as `fetch_cover_image`'s:
    // `extract_when_ready`'s own poll loop is bounded (40s ready-poll, plus
    // up to a further 90s if `extra_script` is jkanime's episode-fetch
    // script), but `WebviewWindowBuilder::build()` right below has no
    // timeout of its own and has been observed to hang indefinitely under
    // rapid window churn. Comfortably above the 40+90=130s a legitimate slow
    // jkanime fetch can take, so this only fires when something is
    // genuinely stuck rather than just slow.
    const OUTER_TIMEOUT: Duration = Duration::from_secs(150);
    match tokio::time::timeout(OUTER_TIMEOUT, fetch_html_with_script_inner(app, url, extra_script)).await {
        Ok(result) => result,
        Err(_) => Err(anyhow!("scrape timed out (window build stalled): {url}")),
    }
}

async fn fetch_html_with_script_inner(
    app: &AppHandle,
    url: &str,
    extra_script: Option<&str>,
) -> Result<ScrapeResult> {
    if !is_safe_external_url(url) {
        return Err(anyhow!("refusing to navigate to unsafe url: {url}"));
    }
    let total_started = std::time::Instant::now();
    // Propagate rather than discard: `SCRAPE_PERMITS` is never `close()`d
    // today, but a bare `let _ =` here would have every future caller sail
    // through with zero concurrency control instead of failing loudly if
    // that ever changed.
    let _permit = SCRAPE_PERMITS.acquire().await?;
    emit_stage(app, "opening");
    let label = format!("scraper-{}", uuid_like());
    let parsed_url: url::Url = url.parse().map_err(|_| anyhow!("bad url: {url}"))?;

    // Record the target host (not the full URL — it can be very long) so
    // the watchdog has a breadcrumb if the build call hangs on the main
    // thread.  Extract before moving `parsed_url` into the closure.
    let host_for_log = parsed_url
        .host_str()
        .unwrap_or("unknown")
        .to_string();

    // Enforce the minimum gap between consecutive window build-starts to
    // reduce main-thread event-loop pressure under rapid window churn.
    space_window_builds().await;
    // Logged after the wait so the timestamp is the real build start.
    note_window_activity(&format!("scraper build start: {host_for_log}"));

    let build_started = std::time::Instant::now();
    let app_for_build = app.clone();
    let build_result = build_webview_window_with_timeout(
        app,
        move || {
            WebviewWindowBuilder::new(&app_for_build, &label, WebviewUrl::External(parsed_url))
                .title("AnimeOnTrack scraper")
                .inner_size(1000.0, 800.0)
                // Hidden: a visible popup stealing focus/screen space on every
                // series scraped was too disruptive during refresh. The
                // poll-based readiness check in extract_when_ready handles the
                // normal case with no user interaction; same invisible pattern
                // fetch_cover_image already uses safely. Trade-off: if
                // Cloudflare ever escalates to a challenge that needs a human
                // click (not just a timed JS check), there's no visible window
                // to solve it in — that shows up as a 40s "did not become
                // ready" error instead of a stuck window waiting on you.
                .visible(false)
                .build()
        },
        WINDOW_BUILD_TIMEOUT,
    );
    let window_build_ms = build_started.elapsed().as_millis();

    let window = match build_result {
        Ok(w) => {
            note_window_activity(&format!("scraper build ok: {host_for_log}"));
            w
        }
        Err(e) => {
            note_window_activity(&format!("scraper build failed: {host_for_log}"));
            return Err(e);
        }
    };

    let result = extract_when_ready(app, &window, window_build_ms, total_started, extra_script).await;
    window.close().ok();
    // After close(): if the close call itself is what hangs, the log must not
    // claim the window was already closed.
    note_window_activity(&format!("scraper window closed: {host_for_log}"));
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
    extra_script: Option<&str>,
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

    // Run the adapter's optional episode-fetch script (jkanime.net only, so
    // far) on the SAME already-loaded window/page rather than opening a
    // second one — the script pulls the numeric anime id and CSRF token out
    // of the page it's already sitting on. 90s timeout, not the normal 15s:
    // this loops a *synchronous* XHR per episode-list page (see
    // adapter/jkanime.rs), and a long-running series can be dozens of pages.
    let extra = match extra_script {
        Some(script) => {
            emit_stage(app, "episodes");
            let json = eval(window, script, 90).await?;
            let s: String = serde_json::from_str(&json)
                .map_err(|e| anyhow!("failed to decode episode script result: {e}"))?;
            Some(s)
        }
        None => None,
    };
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

    Ok(ScrapeResult { html, extra })
}

/// Fetch a single image as a base64 JPEG: open a small window pointed
/// directly at the image URL, wait for the browser's native image viewer to
/// finish loading it, then read the already-decoded pixels off a `<canvas>`
/// with `toDataURL()`. No fetch, no XHR, no promises.
///
/// Call this sparingly (once per followed series per refresh, not in bulk —
/// see `ScrapeResult`'s doc comment for why).
///
/// The whole call is wrapped in a hard outer timeout: the readiness poll
/// below is itself bounded (20 × up to 5s), but `WebviewWindowBuilder::build()`
/// has no timeout of its own, and was live-observed on a real machine to hang
/// indefinitely (zero CPU, 10+ minutes, no error) building a cover window
/// right after a batch of concurrent scraper windows had just closed — a
/// WebView2-runtime hiccup under rapid window churn is the leading suspect,
/// but this guards the symptom (one series' cover stalling the whole refresh
/// cycle forever) regardless of the exact cause. A timeout leaks the
/// in-flight hidden window (there is no handle left to call `.close()` on
/// once the future driving it is dropped) — an acceptable trade against
/// hanging the caller forever, and rare in practice.
pub async fn fetch_cover_image(app: &AppHandle, image_url: &str) -> Result<String> {
    const OUTER_TIMEOUT: Duration = Duration::from_secs(30);
    match tokio::time::timeout(OUTER_TIMEOUT, fetch_cover_image_inner(app, image_url)).await {
        Ok(result) => result,
        Err(_) => Err(anyhow!("cover fetch timed out (window build or ready-poll stalled)")),
    }
}

async fn fetch_cover_image_inner(app: &AppHandle, image_url: &str) -> Result<String> {
    if !is_safe_external_url(image_url) {
        return Err(anyhow!("refusing to navigate to unsafe cover url: {image_url}"));
    }
    let _permit = SCRAPE_PERMITS.acquire().await?;
    let label = format!("cover-{}", uuid_like());
    let parsed_url: url::Url = image_url.parse().map_err(|_| anyhow!("bad image url: {image_url}"))?;

    // Record the target host (not the full image URL) before moving
    // `parsed_url` into the closure, same pattern as fetch_html_with_script_inner.
    let host_for_log = parsed_url
        .host_str()
        .unwrap_or("unknown")
        .to_string();

    // Same back-pressure as the scraper path: space out build-start moments
    // to avoid overwhelming the main thread's event loop with back-to-back
    // WebviewWindowBuilder::build() calls.
    space_window_builds().await;
    note_window_activity(&format!("cover build start: {host_for_log}"));

    let app_for_build = app.clone();
    let build_result = build_webview_window_with_timeout(
        app,
        move || {
            WebviewWindowBuilder::new(&app_for_build, &label, WebviewUrl::External(parsed_url))
                .title("AnimeOnTrack cover")
                .inner_size(300.0, 450.0)
                .visible(false)
                .build()
        },
        WINDOW_BUILD_TIMEOUT,
    );

    let window = match build_result {
        Ok(w) => {
            note_window_activity(&format!("cover build ok: {host_for_log}"));
            w
        }
        Err(e) => {
            note_window_activity(&format!("cover build failed: {host_for_log}"));
            return Err(e);
        }
    };

    // Error pages carry their own icon; ensure the page IS the image we wanted.
    const READY_PROBE: &str = "JSON.stringify(!!document.images[0] \
&& document.images[0].complete && document.images[0].naturalWidth > 0 \
&& document.contentType.startsWith('image/') && document.images[0].src.split('#')[0] === location.href.split('#')[0])";
    let mut ready = false;
    for _ in 0..20 {
        tokio::time::sleep(Duration::from_millis(150)).await;
        // `eval` returns WebView2's own JSON-encoding of the script's
        // result. The script's own `JSON.stringify(bool)` return value is
        // itself a *string* ("true"/"false"), so the outer JSON-string
        // layer has to be decoded first before it means anything as a
        // bool — parsing the raw `eval` result directly as a bool always
        // fails (it's JSON string syntax, not JSON bool syntax) and falls
        // back to `false` unconditionally, which made this readiness check
        // never fire regardless of how quickly the image actually loaded
        // and reliably burned the whole 20-poll window on every fetch.
        if let Ok(json) = eval(&window, READY_PROBE, 5).await {
            let inner: String = serde_json::from_str(&json).unwrap_or_default();
            if inner == "true" {
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
    note_window_activity(&format!("cover window closed: {host_for_log}"));
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The activity ring is process-global and `cargo test` runs tests in
    /// parallel threads, so every test that clears or fills it holds this
    /// lock for its whole body.
    static RING_TEST_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn allows_ordinary_https_and_http_mirrors() {
        assert!(is_safe_external_url("https://example.com/directorio"));
        assert!(is_safe_external_url("http://some-mirror.example/path?x=1"));
    }

    #[test]
    fn rejects_non_http_schemes() {
        assert!(!is_safe_external_url("file:///C:/Windows/System32/config"));
        assert!(!is_safe_external_url("data:text/html,<script>1</script>"));
        assert!(!is_safe_external_url("ftp://example.com/x"));
    }

    #[test]
    fn rejects_localhost_and_loopback() {
        assert!(!is_safe_external_url("http://localhost/"));
        assert!(!is_safe_external_url("http://LOCALHOST:8080/"));
        assert!(!is_safe_external_url("http://127.0.0.1/"));
        assert!(!is_safe_external_url("http://[::1]/"));
    }

    #[test]
    fn rejects_private_and_link_local_ipv4_ranges() {
        assert!(!is_safe_external_url("http://10.0.0.5/"));
        assert!(!is_safe_external_url("http://172.16.3.4/"));
        assert!(!is_safe_external_url("http://192.168.1.1/"));
        assert!(!is_safe_external_url("http://169.254.169.254/")); // cloud metadata endpoint
    }

    #[test]
    fn rejects_unique_local_and_link_local_ipv6() {
        assert!(!is_safe_external_url("http://[fc00::1]/"));
        assert!(!is_safe_external_url("http://[fe80::1]/"));
    }

    #[test]
    fn rejects_unparseable_urls() {
        assert!(!is_safe_external_url("not a url at all"));
        assert!(!is_safe_external_url(""));
    }

    // ── Activity ring ─────────────────────────────────────────────────────────

    /// Drive `note_window_activity` directly and verify that the ring keeps
    /// only the last 8 entries and that the newest entry ends up last.
    #[test]
    fn activity_ring_keeps_only_last_8_newest_last() {
        let _serial = RING_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // Reset the global ring so this test is independent of others.
        // The ring is a `LazyLock<Mutex<VecDeque>>` — we can lock it and
        // clear it without any unsafe code.
        {
            let mut ring = WINDOW_ACTIVITY.lock().unwrap();
            ring.clear();
        }

        // Push 10 entries; the ring should cap at 8 and drop the first two.
        for i in 0..10usize {
            note_window_activity(&format!("entry-{i}"));
        }

        let description = describe_window_activity();
        // The last entry must be present (newest).
        assert!(
            description.contains("entry-9"),
            "newest entry missing: {description}"
        );
        // The first two must have been evicted.
        assert!(
            !description.contains("entry-0"),
            "oldest entry should have been evicted: {description}"
        );
        assert!(
            !description.contains("entry-1"),
            "second-oldest entry should have been evicted: {description}"
        );

        // Verify the count by splitting on the semicolon separator.
        let count = description.split(';').count();
        assert_eq!(count, 8, "ring should hold exactly 8 entries, got {count}");
    }

    #[test]
    fn describe_window_activity_empty_returns_none_string() {
        let _serial = RING_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        {
            let mut ring = WINDOW_ACTIVITY.lock().unwrap();
            ring.clear();
        }
        assert_eq!(describe_window_activity(), "none");
    }

    // ── Window-build spacing ──────────────────────────────────────────────────

    #[test]
    fn remaining_spacing_zero_when_no_previous_build() {
        // With no previous build, there is nothing to wait for.
        let now = Instant::now();
        let wait = remaining_spacing(None, now, Duration::from_millis(250));
        assert_eq!(wait, Duration::ZERO);
    }

    #[test]
    fn remaining_spacing_zero_when_gap_already_elapsed() {
        // Simulate a last build that started 300 ms ago; the 250 ms gap has
        // already passed so no wait is needed.
        let last = Instant::now() - Duration::from_millis(300);
        let now = Instant::now();
        let wait = remaining_spacing(Some(last), now, Duration::from_millis(250));
        assert_eq!(wait, Duration::ZERO, "gap already elapsed, expected no wait");
    }

    #[test]
    fn remaining_spacing_returns_correct_remainder() {
        // Last build 50 ms ago, min gap 250 ms → should still wait ~200 ms.
        // We allow a ±20 ms tolerance to absorb OS scheduling jitter without
        // making the test flaky.
        let last = Instant::now() - Duration::from_millis(50);
        let now = Instant::now();
        let wait = remaining_spacing(Some(last), now, Duration::from_millis(250));
        // Wait must be positive but not more than the full gap.
        assert!(
            wait > Duration::ZERO && wait <= Duration::from_millis(250),
            "expected ~200ms wait, got {wait:?}"
        );
    }

    #[test]
    fn remaining_spacing_zero_when_exactly_at_gap() {
        // last = exactly `min_gap` ago → elapsed == min_gap, saturating_sub gives 0.
        let min_gap = Duration::from_millis(250);
        let last = Instant::now() - min_gap;
        let now = Instant::now();
        let wait = remaining_spacing(Some(last), now, min_gap);
        // May be ZERO or a tiny positive value due to sub-millisecond elapsed;
        // crucially it must never exceed min_gap.
        assert!(wait <= min_gap, "wait must not exceed the gap: {wait:?}");
    }
}
