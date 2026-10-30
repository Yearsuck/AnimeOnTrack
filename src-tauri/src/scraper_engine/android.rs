//! Android backend of `fetch_html_with_script`.
//!
//! Android cannot open a hidden window: a `WebviewWindowBuilder` window there covers
//! the whole app with the scraped site, and WebView2's `ExecuteScript` (what `eval`
//! drives on Windows) does not exist. Instead the `webfetch` plugin
//! (`plugins/webfetch`) renders the page in an off-screen `android.webkit.WebView`,
//! waits for the same readiness probe as `extract_when_ready`, runs the adapter's
//! optional episode script on the same page, and returns the HTML. The WebView is
//! destroyed after every fetch.
//!
//! The mapping from the plugin's answer to a `ScrapeResult` is plain logic and is
//! compiled for host tests too; only the plugin call is Android-only.

use super::ScrapeResult;
use anyhow::{anyhow, Result};
use std::time::Duration;

/// Same ceilings as the desktop path: 40 s for the page to become ready, 15 s to read
/// its HTML, 90 s for jkanime's synchronous episode script. The plugin enforces each of
/// them itself (one deadline is always armed until it answers); `OUTER_TIMEOUT` is
/// only the Rust-side guard in case the plugin never answers at all.
pub(super) const READY_TIMEOUT: Duration = Duration::from_secs(40);
pub(super) const READ_TIMEOUT: Duration = Duration::from_secs(15);
pub(super) const EXTRA_TIMEOUT: Duration = Duration::from_secs(90);
const OUTER_TIMEOUT: Duration = Duration::from_secs(150);

/// The plugin's answer, copied into a host-buildable type (the plugin crate itself
/// only builds for Android).
#[derive(Debug, Default, Clone, PartialEq)]
pub(super) struct RenderedPage {
    pub ok: bool,
    /// Why `ok` is false: `"timeout"`, a main-frame network/TLS error, a renderer crash...
    pub error: String,
    pub html: String,
    /// Value of the episode script, when one was given and it returned a string.
    pub extra: Option<String>,
    pub final_url: String,
    pub elapsed_ms: i64,
    pub ready_ms: i64,
    pub polls: i64,
}

/// Turn the plugin's answer into what the callers of `fetch_html_with_script` expect.
/// Errors read like the desktop ones so logs and the UI stay the same.
pub(super) fn page_into_result(page: RenderedPage, wants_extra: bool) -> Result<ScrapeResult> {
    if !page.ok {
        if page.error == "timeout" {
            return Err(anyhow!(
                "page did not become ready within {}s (a Cloudflare challenge may require manual solving)",
                READY_TIMEOUT.as_secs()
            ));
        }
        let reason = if page.error.trim().is_empty() { "unknown error" } else { page.error.trim() };
        return Err(anyhow!("page load failed: {reason}"));
    }
    // The WebView may have been redirected anywhere; same rule as the URL it was sent to.
    if !super::is_safe_external_url(&page.final_url) {
        return Err(anyhow!("page ended on an unsafe url: {:?}", page.final_url));
    }
    if page.html.is_empty() {
        return Err(anyhow!("page load failed: empty HTML"));
    }
    let extra = if wants_extra {
        match page.extra {
            Some(s) => Some(s),
            None => return Err(anyhow!("episode script returned no result")),
        }
    } else {
        None
    };
    Ok(ScrapeResult { html: page.html, extra })
}

/// One log line per fetch, in the spirit of the desktop `[scrape] fetch timing:` line.
pub(super) fn timing_line(page: &RenderedPage) -> String {
    format!(
        "[scrape] android fetch: ok={} time_to_ready_ms={} (polls={}) total_ms={} html_len={} final_url={}{}",
        page.ok,
        page.ready_ms,
        page.polls,
        page.elapsed_ms,
        page.html.len(),
        page.final_url,
        if page.error.is_empty() { String::new() } else { format!(" error={}", page.error) },
    )
}

#[derive(Debug)]
pub(super) enum SerializedError {
    Closed,
    TimedOut,
    Worker(String),
}

impl std::fmt::Display for SerializedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SerializedError::Closed => write!(f, "fetch queue closed"),
            SerializedError::TimedOut => write!(f, "timed out"),
            SerializedError::Worker(e) => write!(f, "worker error: {e}"),
        }
    }
}

/// Run blocking `work` on a worker thread, one at a time per `permits`, giving up after
/// `timeout`. The permit belongs to the waiting caller, not to the worker: when the
/// timeout fires the permit is released with it, so a worker stuck forever (a plugin
/// call that never resolves) cannot block every later fetch. The abandoned worker may
/// still be finishing; the plugin's own deadlines make it end and clean up.
pub(super) async fn run_serialized<T, F>(
    permits: &tokio::sync::Semaphore,
    timeout: Duration,
    work: F,
) -> std::result::Result<T, SerializedError>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    let _permit = permits.acquire().await.map_err(|_| SerializedError::Closed)?;
    let worker = tauri::async_runtime::spawn_blocking(work);
    match tokio::time::timeout(timeout, worker).await {
        Ok(Ok(v)) => Ok(v),
        Ok(Err(e)) => Err(SerializedError::Worker(e.to_string())),
        Err(_) => Err(SerializedError::TimedOut),
    }
}

/// One fetch at a time: each one is a full WebView (renderer, network, memory) on a phone.
#[cfg(target_os = "android")]
static FETCH_PERMITS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);

#[cfg(target_os = "android")]
pub(super) async fn fetch_html_with_script(
    app: &tauri::AppHandle,
    url: &str,
    extra_script: Option<&str>,
) -> Result<ScrapeResult> {
    use tauri::Manager;
    use tauri_plugin_webfetch::{FetchRequest, WebFetch};

    if !super::is_safe_external_url(url) {
        return Err(anyhow!("refusing to navigate to unsafe url: {url}"));
    }
    if app.try_state::<WebFetch<tauri::Wry>>().is_none() {
        return Err(anyhow!("scraping is unavailable: the webfetch plugin is not registered"));
    }
    super::emit_stage(app, "opening");

    let request = FetchRequest {
        url: url.to_string(),
        timeout_ms: READY_TIMEOUT.as_millis() as u64,
        read_timeout_ms: READ_TIMEOUT.as_millis() as u64,
        extra_script: extra_script.map(str::to_string),
        extra_timeout_ms: EXTRA_TIMEOUT.as_millis() as u64,
        attach: true,
    };
    let handle = app.clone();
    let stage_app = app.clone();
    let work = move || {
        super::emit_stage(&stage_app, "verifying");
        handle.state::<WebFetch<tauri::Wry>>().fetch_rendered(request)
    };
    let response = match run_serialized(&FETCH_PERMITS, OUTER_TIMEOUT, work).await {
        Ok(Ok(r)) => r,
        Ok(Err(e)) => return Err(anyhow!("page load failed: {e}")),
        Err(SerializedError::TimedOut) => {
            return Err(anyhow!("scrape timed out after {OUTER_TIMEOUT:?}: {url}"))
        }
        Err(e) => return Err(anyhow!("page load failed: {e}")),
    };
    let page = RenderedPage {
        ok: response.ok,
        error: response.error,
        html: response.html,
        extra: response.has_extra.then_some(response.extra),
        final_url: response.final_url,
        elapsed_ms: response.elapsed_ms,
        ready_ms: response.ready_ms,
        polls: response.polls,
    };
    eprintln!("{}", timing_line(&page));
    if page.ok {
        super::emit_stage(app, "extracting");
    }
    page_into_result(page, extra_script.is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ready_page() -> RenderedPage {
        RenderedPage {
            ok: true,
            html: "<html><body>content</body></html>".into(),
            final_url: "https://example.com/".into(),
            elapsed_ms: 1200,
            ready_ms: 900,
            polls: 4,
            ..Default::default()
        }
    }

    fn ok_result(r: Result<ScrapeResult>) -> ScrapeResult {
        match r {
            Ok(r) => r,
            Err(e) => panic!("expected success, got: {e}"),
        }
    }

    fn err_text(r: Result<ScrapeResult>) -> String {
        match r {
            Ok(_) => panic!("expected an error"),
            Err(e) => e.to_string(),
        }
    }

    #[test]
    fn ready_page_without_script_returns_html_and_no_extra() {
        let mut page = ready_page();
        // An extra the caller did not ask for is never passed on.
        page.extra = Some("[]".into());
        let r = ok_result(page_into_result(page, false));
        assert_eq!(r.html, "<html><body>content</body></html>");
        assert!(r.extra.is_none());
    }

    #[test]
    fn ready_page_with_script_returns_its_value() {
        let mut page = ready_page();
        page.extra = Some("[{\"number\":1}]".into());
        let r = ok_result(page_into_result(page, true));
        assert_eq!(r.extra.as_deref(), Some("[{\"number\":1}]"));
    }

    #[test]
    fn missing_script_value_is_an_error() {
        let e = err_text(page_into_result(ready_page(), true));
        assert!(e.contains("episode script"), "{e}");
    }

    #[test]
    fn timeout_reads_like_the_desktop_error() {
        let page = RenderedPage { ok: false, error: "timeout".into(), ..Default::default() };
        let e = err_text(page_into_result(page, false));
        assert!(e.starts_with("page did not become ready within 40s"), "{e}");
    }

    #[test]
    fn network_error_is_passed_through() {
        let page = RenderedPage {
            ok: false,
            error: "main frame error -11 net::ERR_SSL_PROTOCOL_ERROR".into(),
            ..Default::default()
        };
        let e = err_text(page_into_result(page, false));
        assert_eq!(e, "page load failed: main frame error -11 net::ERR_SSL_PROTOCOL_ERROR");
    }

    #[test]
    fn failure_without_reason_and_empty_html_are_errors() {
        let e = err_text(page_into_result(RenderedPage::default(), false));
        assert_eq!(e, "page load failed: unknown error");
        let page = RenderedPage { ok: true, final_url: "https://example.com/".into(), ..Default::default() };
        let e = err_text(page_into_result(page, false));
        assert_eq!(e, "page load failed: empty HTML");
    }

    #[test]
    fn timing_line_carries_the_numbers_and_error() {
        let line = timing_line(&ready_page());
        assert!(line.contains("ok=true"));
        assert!(line.contains("time_to_ready_ms=900 (polls=4)"));
        assert!(line.contains("total_ms=1200"));
        assert!(!line.contains("error="));
        let failed = RenderedPage { ok: false, error: "timeout".into(), ..Default::default() };
        assert!(timing_line(&failed).ends_with(" error=timeout"));
    }

    #[test]
    fn timeouts_match_the_desktop_ceilings() {
        assert_eq!(READY_TIMEOUT, Duration::from_secs(40));
        assert_eq!(READ_TIMEOUT, Duration::from_secs(15));
        assert_eq!(EXTRA_TIMEOUT, Duration::from_secs(90));
        // The plugin's own deadlines must always fire before Rust gives up on it.
        assert!(OUTER_TIMEOUT > READY_TIMEOUT + READ_TIMEOUT + EXTRA_TIMEOUT);
    }

    #[test]
    fn page_that_ended_on_an_unsafe_url_is_rejected() {
        for bad in ["http://192.168.1.1/", "http://127.0.0.1:8080/x", "file:///etc/passwd", "http://[::1]/", ""] {
            let mut page = ready_page();
            page.final_url = bad.into();
            let e = err_text(page_into_result(page, false));
            assert!(e.contains("unsafe"), "{bad:?}: {e}");
        }
    }

    #[test]
    fn failed_page_reports_its_error_even_with_an_unsafe_final_url() {
        let page = RenderedPage { ok: false, error: "timeout".into(), final_url: "about:blank".into(), ..Default::default() };
        let e = err_text(page_into_result(page, false));
        assert!(e.starts_with("page did not become ready"), "{e}");
    }

    #[tokio::test]
    async fn stuck_worker_does_not_keep_the_permit() {
        let permits = tokio::sync::Semaphore::new(1);
        let stuck = run_serialized(&permits, Duration::from_millis(50), || {
            std::thread::sleep(Duration::from_millis(400));
            1
        })
        .await;
        assert!(matches!(stuck, Err(SerializedError::TimedOut)), "{stuck:?}");
        // The next fetch must start at once, not wait for the zombie worker.
        let next = tokio::time::timeout(
            Duration::from_millis(200),
            run_serialized(&permits, Duration::from_secs(1), || 2),
        )
        .await
        .expect("second call blocked on the permit");
        assert!(matches!(next, Ok(2)), "{next:?}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn serialized_calls_never_overlap() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;
        let permits = Arc::new(tokio::sync::Semaphore::new(1));
        let in_flight = Arc::new(AtomicUsize::new(0));
        let max_seen = Arc::new(AtomicUsize::new(0));
        let mut tasks = Vec::new();
        for _ in 0..4 {
            let (permits, in_flight, max_seen) = (permits.clone(), in_flight.clone(), max_seen.clone());
            tasks.push(tokio::spawn(async move {
                run_serialized(&permits, Duration::from_secs(5), move || {
                    let now = in_flight.fetch_add(1, Ordering::SeqCst) + 1;
                    max_seen.fetch_max(now, Ordering::SeqCst);
                    std::thread::sleep(Duration::from_millis(30));
                    in_flight.fetch_sub(1, Ordering::SeqCst);
                })
                .await
            }));
        }
        for t in tasks {
            assert!(t.await.unwrap().is_ok());
        }
        assert_eq!(max_seen.load(Ordering::SeqCst), 1);
    }
}
