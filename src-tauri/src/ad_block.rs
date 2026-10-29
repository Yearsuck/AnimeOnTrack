//! Ad blocking for the windows where the user browses third-party pages (the
//! episode player and the AniList page): requests to known ad / tracking /
//! pop-up networks are cancelled before they leave the machine.
//!
//! Two complementary layers protect the viewer windows:
//! 1. A short, conservative host list (`BLOCKED_HOSTS` / `is_blocked_host`)
//!    matching registrable-host suffixes (e.g. `x.doubleclick.net` is blocked
//!    but `notdoubleclick.net` is not). Hits on this list are always blocked.
//! 2. Open-source filter lists (EasyList, EasyList Spanish, EasyPrivacy,
//!    uBlock filters/annoyances) parsed and evaluated via Brave's `adblock`
//!    engine using rules bundled at compile time (`filters/lists.txt.gz`).
//!
//! An explicit `ALLOWED_HOSTS` registry protects streaming sites, catalog APIs,
//! and known video CDNs so the filter engine can never break media playback or
//! navigation.
//! The scraper's hidden windows deliberately do NOT use this: they must see
//! the page exactly as a browser would for Cloudflare to pass.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

use adblock::{
    lists::{FilterSet, ParseOptions},
    request::Request,
    Engine,
};

static ENGINE: OnceLock<Mutex<Engine>> = OnceLock::new();
static ENGINE_ENABLED: AtomicBool = AtomicBool::new(true);

/// Globally enables or disables the adblock filter engine.
pub fn set_engine_enabled(enabled: bool) {
    ENGINE_ENABLED.store(enabled, Ordering::SeqCst);
}

/// Hosts (and all their subdomains) that only serve ads, trackers or pop-unders.
const BLOCKED_HOSTS: &[&str] = &[
    "doubleclick.net",
    "googlesyndication.com",
    "googleadservices.com",
    "adservice.google.com",
    "google-analytics.com",
    "googletagservices.com",
    "adsterra.com",
    "adsterratools.com",
    "highperformanceformat.com",
    "profitablecpmrate.com",
    "profitablegatecpm.com",
    "effectivegatecpm.com",
    "popads.net",
    "popcash.net",
    "propellerads.com",
    "propellerclick.com",
    "onclickads.net",
    "clickadu.com",
    "exoclick.com",
    "exosrv.com",
    "exdynsrv.com",
    "juicyads.com",
    "trafficjunky.net",
    "trafficstars.com",
    "hilltopads.net",
    "adcash.com",
    "a-ads.com",
    "ad-maven.com",
    "admaven.com",
    "yllix.com",
    "mgid.com",
    "taboola.com",
    "outbrain.com",
    "adnxs.com",
    "rubiconproject.com",
    "pubmatic.com",
    "criteo.com",
    "criteo.net",
    "amazon-adsystem.com",
    "moatads.com",
    "scorecardresearch.com",
    "adform.net",
    "smartadserver.com",
    "casalemedia.com",
    "bidvertiser.com",
    "revcontent.com",
    "richpush.co",
    "pushwoosh.com",
    "notix.io",
    "vidoomy.com",
    "yadro.ru",
    "mc.yandex.ru",
    "ads.yahoo.com",
    // animeyt (ad and pop-under scripts)
    "adsco.re",
    "visariomedia.com",
    // animeflv (ad tag script)
    "llvpn.com",
    // jkanime (click/pop tracker and pop-under scripts/landing)
    "agl003.com",
    "cln9vhvfo2.top",
    "driverhugoverblown.com",
    // monoschinos (banner and pop-under scripts)
    "numbestkarree.com",
    "browsepantartful.com",
    // mixdrop embed (pop-under redirect)
    "greatdexchange.com",
    // mp4upload embed (ad scripts)
    "dopattaoutcant.com",
    "talpiddullity.com",
    "sxlzbz.com",
    // yourupload / tioanime (ad script)
    "cpmstar.com",
    // voe embed (pop-under redirect and ad script)
    "evasivewashoutsociety.com",
    "phruk.org",
    // trackers and widget analytics
    "sharethis.com",
    "mdstats.info",
    "whos.amung.us",
];

/// Registrable domains (and all their subdomains) that must NEVER be blocked by the engine.
pub const ALLOWED_HOSTS: &[&str] = &[
    "anilist.co",
    "animeyt.cc",
    "tioanime.com",
    "animeflv.or.at",
    "jkanime.net",
    "gogoanime.by",
    "animeland.tv",
    "aniwaves.ru",
    "monoschinos.st",
    "animefenix.com.co",
    "animoratv.com",
    "yourupload.com",
    "mp4upload.com",
    "mediafire.com",
    "streamtape.com",
    "ok.ru",
    "mixdrop.ag",
    "mixdrop.co",
    "mixdrop.to",
    "voe.sx",
    "vidhide.com",
    "mytsumi.com",
    "megaplay.su",
    "megavid.buzz",
    "youtube.com",
    "youtube-nocookie.com",
    "ytimg.com",
    "dailymotion.com",
    "jsdelivr.net",
    // Players wait on these loaders; keep them unblocked as the host list does.
    "googletagmanager.com",
    "imasdk.googleapis.com",
    // Supported embed/video CDN hosts proven in download_sources.rs:
    "vidcache.net",           // download_sources.rs:1649 (YourUpload video CDN)
    "mxcontent.net",          // download_sources.rs:2310, 2386 (Mixdrop video CDN)
    "mxdrop.click",           // download_sources.rs:2380 (Mixdrop embed host)
    "mixdrop.top",            // download_sources.rs:1845 (Mixdrop embed host)
    "vidhidevip.com",         // download_sources.rs:1849 (Vidhide embed host)
    "callistanise.com",       // download_sources.rs:2346 (Vidhide embed host)
    "acek-cdn.com",           // download_sources.rs:2364 (Vidhide video CDN)
    "okcdn.ru",               // download_sources.rs:2409 (Ok.ru video CDN)
    "megankiddifference.com", // download_sources.rs:2556 (VOE embed redirect target)
    "cloudwindow-route.com",  // download_sources.rs:2571 (VOE video CDN)
    "zilla-networks.com",     // download_sources.rs:2615 (AnimeFLV player stream)
];

/// Whether a request to `host` must be cancelled by the conservative host blocklist.
pub fn is_blocked_host(host: &str) -> bool {
    let host = host.trim().trim_end_matches('.').to_ascii_lowercase();
    if host.is_empty() {
        return false;
    }
    BLOCKED_HOSTS
        .iter()
        .any(|blocked| host == *blocked || host.ends_with(&format!(".{blocked}")))
}

/// Whether `host` is an allowed domain that must never be blocked by the adblock engine.
pub fn is_allowed_host(host: &str) -> bool {
    let host = host.trim().trim_end_matches('.').to_ascii_lowercase();
    if host.is_empty() {
        return false;
    }
    ALLOWED_HOSTS
        .iter()
        .any(|allowed| host == *allowed || host.ends_with(&format!(".{allowed}")))
}

/// Whether the request URL (as WebView2 reports it) must be cancelled by the host blocklist.
pub fn is_blocked_url(url: &str) -> bool {
    match url::Url::parse(url) {
        Ok(u) => u.host_str().map(is_blocked_host).unwrap_or(false),
        Err(_) => false,
    }
}

/// Maps WebView2 `COREWEBVIEW2_WEB_RESOURCE_CONTEXT` integer values to adblock request type strings.
pub fn request_type_for_context(ctx: i32) -> &'static str {
    match ctx {
        1 => "document",
        2 => "stylesheet",
        3 => "image",
        4 => "media",
        5 => "font",
        6 => "script",
        7 => "xhr",
        8 => "fetch",
        9 => "other",
        10 => "other",
        11 => "websocket",
        12 => "other",
        13 => "other",
        14 => "ping",
        15 => "other",
        _ => "other",
    }
}

/// Builds an adblock engine from line-separated rule strings.
pub fn build_engine(rules: &str) -> Engine {
    let mut set = FilterSet::new(false);
    set.add_filter_list(rules.to_string(), ParseOptions::default());
    Engine::new_with_filter_set(set)
}

/// Decompresses gzipped filter rules and builds an adblock engine. Returns None on error.
pub fn build_engine_from_gz_bytes(bytes: &[u8]) -> Option<Engine> {
    use flate2::read::GzDecoder;
    use std::io::Read;

    let mut decoder = GzDecoder::new(bytes);
    let mut decompressed = String::new();
    decoder.read_to_string(&mut decompressed).ok()?;
    Some(build_engine(&decompressed))
}

/// Builds the engine using the bundled gzipped ABP-syntax filter lists.
pub fn build_bundled_engine() -> Option<Engine> {
    const BUNDLED_GZ: &[u8] = include_bytes!("../filters/lists.txt.gz");
    build_engine_from_gz_bytes(BUNDLED_GZ)
}

/// Spawns a background thread that builds the bundled adblock engine and stores it in `ENGINE`.
pub fn init_engine_in_background() {
    let _ = std::thread::Builder::new()
        .name("adblock-engine".to_string())
        .spawn(|| {
            if let Some(engine) = build_bundled_engine() {
                let _ = ENGINE.set(Mutex::new(engine));
            }
        });
}

/// Checks if the adblock engine blocks the given network request.
pub fn engine_blocks(engine: &Engine, url: &str, source_url: &str, req_type: &str) -> bool {
    let Ok(req) = Request::new(url, source_url, req_type, "GET") else {
        return false;
    };
    engine.check_network_request(&req).should_block()
}

/// Pure decision logic combining host blocklist, allowlist, and engine match result.
pub fn decide(host: &str, engine_result: bool) -> bool {
    if is_blocked_host(host) {
        return true;
    }
    if is_allowed_host(host) {
        return false;
    }
    engine_result
}

/// Request type handed to the engine. WebView2's DOCUMENT context covers the main
/// frame and iframes alike, while the filter lists only treat iframes as
/// `subdocument` (a common ad vector), so a document whose page differs from the
/// request is one.
fn effective_request_type(resource_context: i32, url: &str, source: &str) -> &'static str {
    match request_type_for_context(resource_context) {
        "document" if source != url => "subdocument",
        other => other,
    }
}

/// Whether a request should be blocked based on host lists and the adblock engine.
pub fn is_blocked_request(url: &str, source_url: Option<&str>, resource_context: i32) -> bool {
    let Ok(parsed) = url::Url::parse(url) else {
        return false;
    };
    let scheme = parsed.scheme();
    if scheme != "http" && scheme != "https" {
        return false;
    }
    let Some(host) = parsed.host_str() else {
        return false;
    };

    // Cheap exits first so the engine is only consulted when it can matter.
    if is_blocked_host(host) {
        return true;
    }
    if is_allowed_host(host) || !ENGINE_ENABLED.load(Ordering::Relaxed) {
        return false;
    }
    let Some(engine_lock) = ENGINE.get() else {
        return false;
    };
    // The handler runs on the UI thread: never wait for the lock.
    let Ok(engine) = engine_lock.try_lock() else {
        return false;
    };

    let source = source_url.unwrap_or(url);
    let req_type = effective_request_type(resource_context, url, source);
    // A panic inside the engine must not unwind through the COM callback.
    let blocked_by_engine = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        engine_blocks(&engine, url, source, req_type)
    }))
    .unwrap_or(false);

    decide(host, blocked_by_engine)
}

/// Starts cancelling blocked requests in this window's WebView2.
///
/// No-op off Windows (the player only has this engine there). Failures are
/// swallowed: an ad blocker that cannot install must never stop the page from
/// opening.
pub fn install(win: &tauri::WebviewWindow) {
    #[cfg(windows)]
    {
        let _ = win.with_webview(|platform| unsafe {
            use webview2_com::Microsoft::Web::WebView2::Win32::{
                ICoreWebView2_2, COREWEBVIEW2_WEB_RESOURCE_CONTEXT,
                COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL,
            };
            use webview2_com::WebResourceRequestedEventHandler;
            use windows::core::{w, Interface};

            let Ok(core) = platform.controller().CoreWebView2() else {
                return;
            };
            let Ok(core2) = core.cast::<ICoreWebView2_2>() else {
                return;
            };
            let Ok(env) = core2.Environment() else {
                return;
            };
            if core
                .AddWebResourceRequestedFilter(w!("*"), COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL)
                .is_err()
            {
                return;
            }
            let core_for_handler = core.clone();
            let handler = WebResourceRequestedEventHandler::create(Box::new(move |_, args| {
                let Some(args) = args else {
                    return Ok(());
                };
                let request = args.Request()?;
                let mut uri = windows::core::PWSTR::null();
                request.Uri(&mut uri)?;
                let uri = webview2_com::take_pwstr(uri);

                let source = {
                    let mut pwstr = windows::core::PWSTR::null();
                    if core_for_handler.Source(&mut pwstr).is_ok() && !pwstr.is_null() {
                        let s = webview2_com::take_pwstr(pwstr);
                        if s.is_empty() {
                            None
                        } else {
                            Some(s)
                        }
                    } else {
                        None
                    }
                };

                let mut raw_ctx = COREWEBVIEW2_WEB_RESOURCE_CONTEXT(0);
                let ctx = if args.ResourceContext(&mut raw_ctx).is_ok() {
                    raw_ctx.0
                } else {
                    0
                };

                if is_blocked_request(&uri, source.as_deref(), ctx) {
                    let response = env.CreateWebResourceResponse(None, 403, w!("Blocked"), w!(""))?;
                    args.SetResponse(&response)?;
                }
                Ok(())
            }));
            let mut token = 0i64;
            let _ = core.add_WebResourceRequested(&handler, &mut token);
        });
    }
    #[cfg(not(windows))]
    {
        let _ = win;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_newly_added_fixture_ad_hosts_and_subdomains() {
        let added_hosts = [
            "adsco.re",
            "visariomedia.com",
            "llvpn.com",
            "agl003.com",
            "cln9vhvfo2.top",
            "driverhugoverblown.com",
            "numbestkarree.com",
            "browsepantartful.com",
            "greatdexchange.com",
            "dopattaoutcant.com",
            "talpiddullity.com",
            "sxlzbz.com",
            "cpmstar.com",
            "evasivewashoutsociety.com",
            "phruk.org",
            "sharethis.com",
            "mdstats.info",
            "whos.amung.us",
        ];
        for host in added_hosts {
            assert!(is_blocked_host(host), "host {host} should be blocked");
            let sub = format!("x.{host}");
            assert!(is_blocked_host(&sub), "subdomain {sub} should be blocked");
        }
        assert!(is_blocked_host("x.adsco.re"));
    }

    #[test]
    fn blocks_listed_hosts_and_their_subdomains() {
        assert!(is_blocked_host("doubleclick.net"));
        assert!(is_blocked_host("ad.doubleclick.net"));
        assert!(is_blocked_host("A.B.PopAds.net"));
        assert!(is_blocked_host("exoclick.com."));
    }

    #[test]
    fn does_not_block_lookalikes_or_unrelated_hosts() {
        assert!(!is_blocked_host("notadsco.re"));
        assert!(!is_blocked_host("googletagmanager.com"));
        assert!(!is_blocked_host("imasdk.googleapis.com"));
        assert!(!is_blocked_host("megankiddifference.com"));
        assert!(!is_blocked_host("silverlinencreativeworks.com"));
        assert!(!is_blocked_host("silverlinencreativeworks.store"));
        assert!(!is_blocked_host("callistanise.com"));
        assert!(!is_blocked_host("notdoubleclick.net"));
        assert!(!is_blocked_host("doubleclick.net.example.org"));
        assert!(!is_blocked_host("anilist.co"));
        assert!(!is_blocked_host("s4.anilist.co"));
        assert!(!is_blocked_host("animeyt.cc"));
        assert!(!is_blocked_host("cdn.jsdelivr.net"));
        assert!(!is_blocked_host(""));
    }

    #[test]
    fn url_matching_uses_the_host_only() {
        assert!(is_blocked_url("https://pagead2.googlesyndication.com/pagead/js/adsbygoogle.js"));
        assert!(!is_blocked_url("https://animeyt.cc/tv/x/?ref=doubleclick.net"));
        assert!(!is_blocked_url("not a url"));
        assert!(!is_blocked_url("data:text/plain,hello"));
    }

    #[test]
    fn iframe_documents_are_typed_subdocument() {
        let page = "https://site.example/ver/x";
        assert_eq!(effective_request_type(1, page, page), "document");
        assert_eq!(effective_request_type(1, "https://ads.example/frame.html", page), "subdocument");
        assert_eq!(effective_request_type(6, "https://ads.example/a.js", page), "script");
    }

    #[test]
    fn request_type_for_context_maps_webview2_contexts() {
        assert_eq!(request_type_for_context(1), "document");
        assert_eq!(request_type_for_context(2), "stylesheet");
        assert_eq!(request_type_for_context(3), "image");
        assert_eq!(request_type_for_context(4), "media");
        assert_eq!(request_type_for_context(5), "font");
        assert_eq!(request_type_for_context(6), "script");
        assert_eq!(request_type_for_context(7), "xhr");
        assert_eq!(request_type_for_context(8), "fetch");
        assert_eq!(request_type_for_context(9), "other");
        assert_eq!(request_type_for_context(10), "other");
        assert_eq!(request_type_for_context(11), "websocket");
        assert_eq!(request_type_for_context(12), "other");
        assert_eq!(request_type_for_context(13), "other");
        assert_eq!(request_type_for_context(14), "ping");
        assert_eq!(request_type_for_context(15), "other");
        assert_eq!(request_type_for_context(16), "other");
        assert_eq!(request_type_for_context(99), "other");
        assert_eq!(request_type_for_context(-1), "other");
    }

    #[test]
    fn engine_from_rules_blocks_matching_and_respects_exceptions() {
        let rules = "||ads.example.com^\n@@||ads.example.com/ok.js^";
        let engine = build_engine(rules);
        assert!(engine_blocks(
            &engine,
            "https://ads.example.com/banner.js",
            "https://site.example/",
            "script"
        ));
        assert!(!engine_blocks(
            &engine,
            "https://ads.example.com/ok.js",
            "https://site.example/",
            "script"
        ));
        assert!(!engine_blocks(
            &engine,
            "https://good.example/app.js",
            "https://site.example/",
            "script"
        ));
    }

    #[test]
    fn allowed_hosts_and_subdomain_matching() {
        assert!(is_allowed_host("www.mp4upload.com"));
        assert!(is_allowed_host("mp4upload.com"));
        assert!(!is_allowed_host("notmp4upload.com"));
        assert!(!is_allowed_host("anilist.co.evil.example"));
        assert!(is_allowed_host("anilist.co"));
        assert!(is_allowed_host("s4.anilist.co"));
        assert!(is_allowed_host("animeyt.cc"));
        assert!(is_allowed_host("cdn.jsdelivr.net"));
        assert!(is_allowed_host("www.googletagmanager.com"));
        assert!(is_allowed_host("imasdk.googleapis.com"));
        assert!(!is_allowed_host(""));
    }

    #[test]
    fn allow_list_wins_over_engine() {
        let engine = build_engine("||mp4upload.com^");
        let blocked_by_engine = engine_blocks(
            &engine,
            "https://www.mp4upload.com/embed.js",
            "https://site.example/",
            "script",
        );
        assert!(blocked_by_engine, "engine rule matches mp4upload.com");
        assert!(
            !decide("www.mp4upload.com", blocked_by_engine),
            "decision must not block allowed host even when engine matched"
        );
        assert!(!decide("www.mp4upload.com", true));
    }

    #[test]
    fn existing_host_list_hits_stay_blocked() {
        assert!(is_blocked_host("doubleclick.net"));
        assert!(is_blocked_host("ad.doubleclick.net"));
        assert!(is_blocked_host("googlesyndication.com"));
        assert!(decide("doubleclick.net", false));
        assert!(decide("ad.doubleclick.net", false));
    }

    #[test]
    fn bundled_engine_blocks_known_ad_and_permits_allowed_embed() {
        let engine = build_bundled_engine().expect("bundled engine should load successfully");
        assert!(engine_blocks(
            &engine,
            "https://pagead2.googlesyndication.com/pagead/js/adsbygoogle.js",
            "https://some-site.example/",
            "script"
        ));
        assert!(!engine_blocks(
            &engine,
            "https://www.mp4upload.com/embed-x.html",
            "https://some-site.example/",
            "subdocument"
        ));
    }

    #[test]
    fn corrupt_gz_returns_none_without_panic() {
        assert!(build_engine_from_gz_bytes(&[0, 1, 2]).is_none());
        assert!(build_engine_from_gz_bytes(&[]).is_none());
    }

    #[test]
    fn is_blocked_request_rejects_non_http_schemes() {
        assert!(!is_blocked_request("data:text/plain,hello", None, 1));
        assert!(!is_blocked_request("javascript:alert(1)", None, 6));
        assert!(!is_blocked_request("not-a-valid-url", None, 1));
    }

    #[test]
    fn engine_toggle_enables_and_disables() {
        set_engine_enabled(false);
        assert!(!ENGINE_ENABLED.load(Ordering::Relaxed));
        set_engine_enabled(true);
        assert!(ENGINE_ENABLED.load(Ordering::Relaxed));
    }
}
