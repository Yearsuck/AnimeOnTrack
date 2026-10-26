//! Ad blocking for the windows where the user browses third-party pages (the
//! episode player and the AniList page): requests to known ad / tracking /
//! pop-up networks are cancelled before they leave the machine.
//!
//! This is deliberately a short, conservative host list, not a general filter
//! engine: wrongly blocking a CDN that serves the video would break playback,
//! which is worse than letting an ad through. Matching is by registrable-host
//! suffix, so `x.doubleclick.net` is blocked but `notdoubleclick.net` is not.
//! The scraper's hidden windows do NOT use this: they must see the page exactly
//! as a browser would for Cloudflare to pass.

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

/// Whether a request to `host` must be cancelled.
pub fn is_blocked_host(host: &str) -> bool {
    let host = host.trim().trim_end_matches('.').to_ascii_lowercase();
    if host.is_empty() {
        return false;
    }
    BLOCKED_HOSTS
        .iter()
        .any(|blocked| host == *blocked || host.ends_with(&format!(".{blocked}")))
}

/// Whether the request URL (as WebView2 reports it) must be cancelled.
pub fn is_blocked_url(url: &str) -> bool {
    match url::Url::parse(url) {
        Ok(u) => u.host_str().map(is_blocked_host).unwrap_or(false),
        Err(_) => false,
    }
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
                ICoreWebView2_2, COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL,
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
            let handler = WebResourceRequestedEventHandler::create(Box::new(move |_, args| {
                let Some(args) = args else {
                    return Ok(());
                };
                let request = args.Request()?;
                let mut uri = windows::core::PWSTR::null();
                request.Uri(&mut uri)?;
                let uri = webview2_com::take_pwstr(uri);
                if is_blocked_url(&uri) {
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
}
