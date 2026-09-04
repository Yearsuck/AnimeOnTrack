use crate::models::{Episode, FinishedCard, Series, SeriesDetail};
use anyhow::Result;

pub mod animeflv;
pub mod animeland;
pub mod animeytx;
pub mod gogoanime;
pub mod jkanime;
pub mod tioanime;

/// Static metadata for one supported site — never the base URL (that changes
/// with every mirror); `id` is the stable slug stored in `sources.site_id`
/// and the per-site `mirror_urls:{id}` settings key.
pub struct SiteInfo {
    pub id: &'static str,
    pub name: &'static str,
    pub default_base_url: &'static str,
}

/// Every site the app knows about, in the order the Settings selector shows
/// them. Adding an entry here does NOT by itself make the site usable —
/// `adapter_for` must also return a real adapter for its `id`, otherwise it's
/// listed but inert.
pub fn all_sites() -> &'static [SiteInfo] {
    &[
        SiteInfo { id: "animeytx", name: "AnimeYT", default_base_url: "https://wwv.animeytx.net" },
        SiteInfo { id: "tioanime", name: "TioAnime", default_base_url: "https://tioanime.com" },
        SiteInfo { id: "animeflv", name: "AnimeFLV", default_base_url: "https://www4.animeflv.net" },
        SiteInfo { id: "jkanime", name: "JKanime", default_base_url: "https://jkanime.net" },
        SiteInfo { id: "gogoanime", name: "GogoAnime", default_base_url: "https://gogoanime.by" },
        SiteInfo { id: "animeland", name: "AnimeLand", default_base_url: "https://w7.animeland.tv" },
    ]
}

/// Look up the adapter for a stable site slug (`sources.site_id`). `None`
/// means either an unknown id or a site listed in `all_sites()` whose
/// adapter hasn't landed yet.
pub fn adapter_for(site_id: &str) -> Option<Box<dyn SiteAdapter>> {
    match site_id {
        "animeytx" => Some(Box::new(animeytx::AnimeytxAdapter)),
        "tioanime" => Some(Box::new(tioanime::TioanimeAdapter)),
        "animeflv" => Some(Box::new(animeflv::AnimeflvAdapter)),
        "jkanime" => Some(Box::new(jkanime::JkanimeAdapter)),
        "gogoanime" => Some(Box::new(gogoanime::GogoanimeAdapter)),
        "animeland" => Some(Box::new(animeland::AnimelandAdapter)),
        _ => None,
    }
}

/// A site-specific parser. HTML is fetched by the scraper engine; the adapter
/// only turns HTML into domain models.
pub trait SiteAdapter: Send + Sync {
    /// Absolute URL of the "currently airing" listing.
    fn airing_url(&self, base_url: &str) -> String;
    /// Absolute URL of page `page` (1-based) of the airing listing, or `None`
    /// when the site has no such page. The default is a **single page**: page 1
    /// is `airing_url`, and there is no page 2 — correct for sites whose airing
    /// listing fits one page. Sites whose airing directory paginates (e.g.
    /// TioAnime's `?p=N`) override this so the scan walks every page instead of
    /// only the first; `scan_airing` stops at the first `None` or first empty
    /// page. See `parse_airing` for how each page's HTML is parsed.
    fn airing_page_url(&self, base_url: &str, page: u32) -> Option<String> {
        (page <= 1).then(|| self.airing_url(base_url))
    }
    /// Parse the airing-list HTML into series (id/followed unset -> 0/false).
    fn parse_airing(&self, html: &str) -> Result<Vec<Series>>;
    /// Parse a series page HTML into its episodes (id/series_id/seen unset).
    fn parse_series(&self, html: &str) -> Result<Vec<Episode>>;

    /// Homepage URL — there is no dedicated "all genres" index page on this
    /// site, so genre discovery scrapes whatever links the homepage has.
    ///
    /// Genre-archive method (see the other four below): default `""` for
    /// sites with no such archive.
    fn genre_list_url(&self, _base_url: &str) -> String {
        String::new()
    }
    /// `(slug, display_name)` pairs from `a[href*="/genres/"]`, deduped by
    /// slug (the homepage links to a genre many times over: once per series
    /// card plus its own nav).
    ///
    /// Genre-archive method: default `Ok(vec![])` for sites with no genre
    /// archive at all — see the trait-level doc comment on why these five
    /// have default impls instead of being required.
    fn parse_genre_list(&self, _html: &str) -> Result<Vec<(String, String)>> {
        Ok(vec![])
    }
    /// `page=1` maps to the bare `/genres/{slug}/` (no `/page/1/` suffix) —
    /// confirmed that's how the site's own pagination links behave.
    ///
    /// Genre-archive method: default `""`.
    fn genre_page_url(&self, _base_url: &str, _genre_slug: &str, _page: u32) -> String {
        String::new()
    }
    /// Cards from a genre-listing page, filtered to only those carrying a
    /// `.status.Completed` div (the finished/not-finished signal — ongoing
    /// cards don't have a `.status` div at all, rather than a different
    /// class value).
    ///
    /// Genre-archive method: default `Ok(vec![])`.
    fn parse_finished_page(&self, _html: &str) -> Result<Vec<FinishedCard>> {
        Ok(vec![])
    }
    /// Highest page number found in `.pagination`, or 1 if there's no
    /// pagination element at all (a genre with a single page of results).
    ///
    /// Genre-archive method: default `1`.
    fn parse_pagination_last_page(&self, _html: &str) -> u32 {
        1
    }
    /// Full genre tag list, authoritative type, and synopsis from a series
    /// detail page (`/tv/{slug}/`) — this is the only place a series'
    /// *complete* genre set is available; listing cards only imply the
    /// genre archive they were found under.
    fn parse_series_detail(&self, html: &str) -> Result<SeriesDetail>;

    /// Search-results URL for a free-text query (URL-encoded by the impl).
    fn search_url(&self, base_url: &str, query: &str) -> String;
    /// Cards from a search-results page. Same card shape as a genre listing.
    /// `Ok(vec![])` is a legitimate "zero results for this query" answer,
    /// not a failure — only a page that doesn't look like this site's search
    /// results at all (wrong/incompatible mirror) is `Err`. See
    /// `commands::search_site` for why this distinction matters: unlike
    /// every other scrape in this codebase, a search's caller must not treat
    /// "parsed empty" as "this mirror failed".
    fn parse_search_results(&self, html: &str) -> Result<Vec<FinishedCard>>;

    /// Extra JS run in-page, once the normal readiness poll passes and the
    /// plain page HTML has already been captured, whose result becomes
    /// `ScrapeResult.extra` — what `parse_series` receives instead of `html`
    /// when present. Exists for jkanime.net: its episode list isn't in the
    /// series page's HTML at all, it's fetched client-side via a paginated
    /// JSON AJAX endpoint requiring a CSRF token (see `jkanime.rs`). The
    /// script pulls the numeric anime id and token straight out of the page
    /// it's already sitting on, so it needs no `base_url` of its own — every
    /// other site's episode list is already in the plain page HTML, so this
    /// defaults to `None` (no extra step, zero behavior change).
    fn episode_fetch_script(&self) -> Option<&'static str> {
        None
    }
}

// --- Shared parsing helpers ---------------------------------------------
//
// `slug_from_url`, `text_of` and `digits_in` used to be copy-pasted
// near-verbatim into every one of the six adapter files. That duplication is
// exactly what let the `digits_in` bug below (see its own doc comment) go
// unfixed in three places at once, and is the same root cause the
// URL-staleness bug in `commands::scan::rebase_to_mirror`'s doc comment
// describes for `abs()`. One copy here, used by every adapter via
// `use super::{digits_in, slug_from_url, text_of};`.

/// The last path segment of a URL, with any trailing slash removed — the
/// site's own slug for a series/episode.
pub(crate) fn slug_from_url(url: &str) -> String {
    url.trim_end_matches('/').rsplit('/').next().unwrap_or("").to_string()
}

/// First element matching `sel` under `el`, as trimmed text — `None` if
/// absent or empty after trimming.
pub(crate) fn text_of(el: scraper::ElementRef, sel: &scraper::Selector) -> Option<String> {
    el.select(sel)
        .next()
        .map(|n| n.text().collect::<String>().trim().to_string())
        .filter(|s| !s.is_empty())
}

/// The first maximal run of ASCII digits in `s`, e.g. "Episodio 12" -> "12".
/// Falls back to the whole trimmed string if there are no digits at all
/// (never panics, never produces an empty episode number silently).
///
/// Deliberately the *first contiguous run*, not every digit character in the
/// string concatenated together: the latter turned "Episode 12 (1080p)" into
/// "121080" and "1x12" into "112" — real-looking labels this could plausibly
/// see from a markup change — instead of the actual episode number.
pub(crate) fn digits_in(s: &str) -> String {
    let mut run = String::new();
    for c in s.chars() {
        if c.is_ascii_digit() {
            run.push(c);
        } else if !run.is_empty() {
            break;
        }
    }
    if run.is_empty() {
        s.trim().to_string()
    } else {
        run
    }
}

#[cfg(test)]
mod helper_tests {
    use super::*;

    #[test]
    fn digits_in_takes_the_first_run_not_every_digit_concatenated() {
        assert_eq!(digits_in("Episodio 12"), "12");
        assert_eq!(digits_in("Episode 12 (1080p)"), "12");
        assert_eq!(digits_in("1x12"), "1");
    }

    #[test]
    fn digits_in_falls_back_to_trimmed_input_with_no_digits() {
        assert_eq!(digits_in("  Especial  "), "Especial");
    }
}

// The five genre-archive methods above (`genre_list_url`, `parse_genre_list`,
// `genre_page_url`, `parse_finished_page`, `parse_pagination_last_page`)
// exist because AnimeYT has genre archives with a `.status.Completed`
// finished/not-finished marker on listing cards. They are ONLY consumed by
// `discover_swipe_card`/`decide_swipe` (commands.rs), which the catalog-swipe
// work already disconnected from the UI — the swipe deck now reads the local
// AniList catalog, not a live site scrape. A site with no equivalent
// "genre archive" concept (most sites: episode list + search only) should
// just not override these; contorting a site's real layout to fake a genre
// archive would be worse than admitting the concept doesn't apply there.
// (Removing these five from the trait outright, now that the swipe-from-site
// path is confirmed dead, is a follow-up cleanup — not done here to keep this
// change a pure additive refactor.)

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adapter_for_animeytx_resolves() {
        assert!(adapter_for("animeytx").is_some());
    }

    #[test]
    fn adapter_for_unknown_site_is_none() {
        assert!(adapter_for("not-a-real-site").is_none());
    }

    #[test]
    fn all_sites_ids_are_unique_and_every_id_is_a_valid_lookup_or_not_yet_landed() {
        let sites = all_sites();
        let mut ids: Vec<&str> = sites.iter().map(|s| s.id).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), sites.len(), "duplicate site id in all_sites()");
        assert!(!sites.is_empty());
    }

    /// A trait object built from the genre-archive-less default impls must
    /// behave like "no genre archive" everywhere those methods are consulted
    /// — a struct that implements only the required methods proves the
    /// defaults compile and return the documented values.
    struct BareAdapter;
    impl SiteAdapter for BareAdapter {
        fn airing_url(&self, base_url: &str) -> String {
            format!("{base_url}/airing")
        }
        fn parse_airing(&self, _html: &str) -> Result<Vec<Series>> {
            Ok(vec![])
        }
        fn parse_series(&self, _html: &str) -> Result<Vec<Episode>> {
            Ok(vec![])
        }
        fn parse_series_detail(&self, _html: &str) -> Result<SeriesDetail> {
            Ok(SeriesDetail { genres: vec![], kind: None, synopsis: None })
        }
        fn search_url(&self, base_url: &str, query: &str) -> String {
            format!("{base_url}/search?q={query}")
        }
        fn parse_search_results(&self, _html: &str) -> Result<Vec<FinishedCard>> {
            Ok(vec![])
        }
    }

    #[test]
    fn genre_archive_methods_default_to_empty_and_page_one() {
        let a = BareAdapter;
        assert_eq!(a.genre_list_url("https://x"), "");
        assert_eq!(a.parse_genre_list("<html></html>").unwrap(), Vec::<(String, String)>::new());
        assert_eq!(a.genre_page_url("https://x", "slug", 2), "");
        assert_eq!(a.parse_finished_page("<html></html>").unwrap(), Vec::<crate::models::FinishedCard>::new());
        assert_eq!(a.parse_pagination_last_page("<html></html>"), 1);
    }
}
