use super::SiteAdapter;
use crate::models::{Episode, FinishedCard, Series, SeriesDetail};
use anyhow::Result;
use scraper::{Html, Selector};

pub struct TioanimeAdapter;

// Selectors confirmed against real captured HTML (src-tauri/tests/fixtures,
// captured live 2026-07-11 via scraper_engine::fetch_html against
// https://tioanime.com — NOT a DooPlay site like AnimeYT, `.bsx`/`.eplister`
// do not exist here).
//
// - Airing listing: `/directorio?status=1` (the "En emisión" filter on the
//   site's own directory browser — confirmed from the `<select id="status">`
//   options on `/directorio`; `status=2` is "Finalizado"). Cards are
//   `ul.animes li article.anime`, each a single `<a href="/anime/{slug}">`
//   wrapping a poster `<img src>` and an `<h3 class="title">`. No countdown
//   or episode-count badge of any kind is present on these cards — unlike
//   AnimeYT's `.bsx` (`data-rlsdt`/`.sb`), so `next_episode_at` and
//   `site_episode_count` are always `None` here; the airing-grid sort for
//   this site falls back to title order (see the design spec, task 8).
// - A series page (`/anime/{slug}`) carries BOTH the episode list AND the
//   genres/kind/synopsis, so `series.html`/`series_detail.html` are the same
//   captured fixture, matching the spec's explicit allowance for that case.
//   Episode rows are `.episodes-list li`; the episode number is parsed out
//   of the `p span` "Episodio N" text (no dedicated number-only element like
//   AnimeYT's `.epl-num`), and the release-relative-date is the *sibling*
//   `span` directly under `.flex-grow-1` (a CSS child combinator distinguishes
//   it from the nested "Episodio N" span — both are plain `<span>`s).
//   Per-episode titles don't exist on this site (the row repeats the SERIES
//   title, not an episode-specific one), so `Episode.title` is always `None`.
// - Genres: `.genres a` text. Kind: `.meta .anime-type-peli` text (e.g.
//   "TV") — confirmed only present on the *detail* page's `.meta` block;
//   listing/search cards carry no type badge at all (the "Pelicula" badge
//   seen on the homepage's movie rail is a different, unrelated section).
//   Synopsis: `.sinopsis` text.
// - Search: `/directorio?q={query}` reuses the exact same `ul.animes`
//   listing markup as the airing/status filter (confirmed live against both
//   a hit and a genuine zero-result query) — including on a zero-hit
//   search, where the `ul.animes` container is present but empty, same
//   "container present but empty = zero results, container absent entirely
//   = wrong/incompatible mirror" contract AnimeYT's `.listupd` uses.
// - No genre-archive concept confirmed on this site distinct from the plain
//   directory filter above (no separate finished-vs-ongoing marker at the
//   *listing* card level, only on the detail page's `.status` button, which
//   isn't reachable without an extra fetch per card) — the five genre-archive
//   trait methods are left at their default (no-op) implementations.
//
// KNOWN LIMITATION (not raised by the design spec, found while implementing):
// every `href`/`src` captured on this site is relative ("/anime/slug",
// "/uploads/portadas/x.jpg"), unlike AnimeYT's site markup which already
// emits absolute URLs. `SiteAdapter::parse_*` has no `base_url` parameter to
// resolve these against, so this adapter resolves them against the fixed
// `BASE` constant below (the default mirror) rather than whatever mirror
// actually served the page. In practice this only matters if a *second*
// tioanime mirror is ever configured and used instead of the default — until
// then `BASE` and the working mirror are the same domain. A real fix would
// thread the working mirror into every parse_* call; out of scope here
// (AnimeYT never hit this because its own markup happens to already be
// absolute).
const BASE: &str = "https://tioanime.com";
const CARD: &str = "ul.animes li article.anime";
const EP_ROW: &str = ".episodes-list li";

fn abs(path: &str) -> String {
    if path.starts_with("http://") || path.starts_with("https://") {
        path.to_string()
    } else if let Some(stripped) = path.strip_prefix('/') {
        format!("{BASE}/{stripped}")
    } else {
        format!("{BASE}/{path}")
    }
}

fn slug_from_url(url: &str) -> String {
    url.trim_end_matches('/').rsplit('/').next().unwrap_or("").to_string()
}

fn text_of(el: scraper::ElementRef, sel: &Selector) -> Option<String> {
    el.select(sel)
        .next()
        .map(|n| n.text().collect::<String>().trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Trailing run of ASCII digits in `s`, e.g. "Episodio 12" -> "12". Falls
/// back to the whole trimmed string if there are no digits at all (mirrors
/// AnimeYT's `slug_from_url` fallback discipline: never panic, never produce
/// an empty episode number silently).
fn digits_in(s: &str) -> String {
    let digits: String = s.chars().filter(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        s.trim().to_string()
    } else {
        digits
    }
}

impl SiteAdapter for TioanimeAdapter {
    fn airing_url(&self, base_url: &str) -> String {
        format!("{}/directorio?status=1", base_url.trim_end_matches('/'))
    }

    fn parse_airing(&self, html: &str) -> Result<Vec<Series>> {
        let doc = Html::parse_document(html);
        let card_sel = Selector::parse(CARD).unwrap();
        let a_sel = Selector::parse("a").unwrap();
        let title_sel = Selector::parse(".title").unwrap();
        let img_sel = Selector::parse("img").unwrap();

        let mut out = Vec::new();
        for card in doc.select(&card_sel) {
            let Some(anchor) = card.select(&a_sel).next() else { continue };
            let Some(href) = anchor.value().attr("href") else { continue };
            let url = abs(href);
            let title = text_of(card, &title_sel).unwrap_or_else(|| slug_from_url(&url));
            let cover_url = card
                .select(&img_sel)
                .next()
                .and_then(|i| i.value().attr("src"))
                .map(abs);
            out.push(Series {
                id: 0,
                slug: slug_from_url(&url),
                title,
                url,
                cover_url,
                is_airing: true,
                followed: false,
                // No countdown/episode-count signal on this site's listing
                // cards at all — see the module doc comment.
                next_episode_at: None,
                site_episode_count: None,
            });
        }
        Ok(out)
    }

    fn parse_series(&self, html: &str) -> Result<Vec<Episode>> {
        let doc = Html::parse_document(html);
        let row_sel = Selector::parse(EP_ROW).unwrap();
        let a_sel = Selector::parse("a").unwrap();
        let num_sel = Selector::parse("p span").unwrap();
        let date_sel = Selector::parse(".flex-grow-1 > span").unwrap();

        let mut out = Vec::new();
        for row in doc.select(&row_sel) {
            let Some(anchor) = row.select(&a_sel).next() else { continue };
            let Some(href) = anchor.value().attr("href") else { continue };
            let url = abs(href.trim());
            let number = text_of(row, &num_sel)
                .map(|s| digits_in(&s))
                .unwrap_or_else(|| slug_from_url(&url));
            let released_at = text_of(row, &date_sel);
            out.push(Episode {
                id: 0,
                series_id: 0,
                number,
                // No per-episode title on this site — the row repeats the
                // series title, not an episode-specific one.
                title: None,
                url,
                released_at,
                seen: false,
            });
        }
        Ok(out)
    }

    fn parse_series_detail(&self, html: &str) -> Result<SeriesDetail> {
        let doc = Html::parse_document(html);
        let genres_sel = Selector::parse(".genres a").unwrap();
        let kind_sel = Selector::parse(".meta .anime-type-peli").unwrap();
        let synopsis_sel = Selector::parse(".sinopsis").unwrap();

        let genres = doc
            .select(&genres_sel)
            .map(|a| a.text().collect::<String>().trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        let kind = doc
            .select(&kind_sel)
            .next()
            .map(|el| el.text().collect::<String>().trim().to_string())
            .filter(|s| !s.is_empty());
        let synopsis = doc
            .select(&synopsis_sel)
            .next()
            .map(|el| el.text().collect::<String>().trim().to_string())
            .filter(|s| !s.is_empty());

        Ok(SeriesDetail { genres, kind, synopsis })
    }

    fn search_url(&self, base_url: &str, query: &str) -> String {
        let encoded: String = url::form_urlencoded::byte_serialize(query.as_bytes()).collect();
        format!("{}/directorio?q={}", base_url.trim_end_matches('/'), encoded)
    }

    fn parse_search_results(&self, html: &str) -> Result<Vec<FinishedCard>> {
        let doc = Html::parse_document(html);
        // Same `ul.animes` container as the airing/status listing (confirmed
        // live against both a hit and a genuine zero-hit query) — present
        // but empty (no `.anime` article children) for a real zero-hit
        // search, absent entirely only for a page that doesn't look like
        // this site's directory layout at all (wrong/incompatible mirror).
        let container_sel = Selector::parse("ul.animes").unwrap();
        if doc.select(&container_sel).next().is_none() {
            return Err(anyhow::anyhow!(
                "no ul.animes container found; not a recognizable directory/search page"
            ));
        }
        let card_sel = Selector::parse(CARD).unwrap();
        let a_sel = Selector::parse("a").unwrap();
        let title_sel = Selector::parse(".title").unwrap();
        let img_sel = Selector::parse("img").unwrap();

        let mut out = Vec::new();
        for card in doc.select(&card_sel) {
            let Some(anchor) = card.select(&a_sel).next() else { continue };
            let Some(href) = anchor.value().attr("href") else { continue };
            let url = abs(href);
            let title = text_of(card, &title_sel).unwrap_or_else(|| slug_from_url(&url));
            let poster_url = card.select(&img_sel).next().and_then(|i| i.value().attr("src")).map(abs);
            // Search/directory cards carry no type badge at all on this site
            // (unlike the homepage's separate movie rail) — kind is always
            // empty here, same "unwrap_or_default" discipline AnimeYT uses
            // for its own occasionally-missing `.typez`.
            out.push(FinishedCard { title, url, poster_url, kind: String::new(), matched_genre: None });
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn airing_url_uses_the_status_filter() {
        let a = TioanimeAdapter;
        assert_eq!(a.airing_url("https://tioanime.com/"), "https://tioanime.com/directorio?status=1");
        assert_eq!(a.airing_url("https://tioanime.com"), "https://tioanime.com/directorio?status=1");
    }

    #[test]
    fn search_url_is_urlencoded_query_string() {
        let a = TioanimeAdapter;
        assert_eq!(a.search_url("https://tioanime.com", "naruto"), "https://tioanime.com/directorio?q=naruto");
        assert_eq!(
            a.search_url("https://tioanime.com/", "Shingeki no Kyojin"),
            "https://tioanime.com/directorio?q=Shingeki+no+Kyojin"
        );
    }

    #[test]
    fn parses_airing_fixture() {
        let html = include_str!("../../tests/fixtures/tioanime_airing.html");
        let out = TioanimeAdapter.parse_airing(html).unwrap();
        assert!(!out.is_empty(), "expected at least one series");
        for s in &out {
            assert!(!s.url.is_empty());
            assert!(s.url.starts_with("https://tioanime.com/anime/"), "url not absolute: {}", s.url);
            assert!(!s.slug.is_empty());
            assert!(!s.title.is_empty());
            assert!(s.is_airing);
            // Confirmed live: this site's listing cards carry no
            // countdown/episode-count signal at all.
            assert_eq!(s.next_episode_at, None);
            assert_eq!(s.site_episode_count, None);
        }
        let first = &out[0];
        assert_eq!(first.slug, "super-no-ura-de-yani-suu-futari");
        assert_eq!(first.title, "Super no Ura de Yani Suu Futari");
        assert!(first.cover_url.as_deref().unwrap().contains("uploads/portadas"));
    }

    #[test]
    fn parses_series_fixture() {
        let html = include_str!("../../tests/fixtures/tioanime_series.html");
        let out = TioanimeAdapter.parse_series(html).unwrap();
        assert!(!out.is_empty(), "expected at least one episode");
        let first = &out[0];
        assert_eq!(first.number, "1");
        assert_eq!(first.title, None, "this site has no per-episode title");
        assert_eq!(first.url, "https://tioanime.com/ver/youjo-senki-ii-1");
        assert_eq!(first.released_at.as_deref(), Some("Hace 2 dias"));
        for e in &out {
            assert!(!e.url.is_empty());
            assert!(!e.number.is_empty());
        }
    }

    #[test]
    fn parses_series_detail_fixture() {
        let html = include_str!("../../tests/fixtures/tioanime_series_detail.html");
        let d = TioanimeAdapter.parse_series_detail(html).unwrap();
        assert_eq!(d.genres, vec!["Acción".to_string(), "Fantasía".to_string(), "Militar".to_string()]);
        assert_eq!(d.kind.as_deref(), Some("TV"));
        assert!(d.synopsis.unwrap().contains("Segunda temporada"));
    }

    #[test]
    fn parses_search_hits_fixture() {
        let html = include_str!("../../tests/fixtures/tioanime_search_hits.html");
        let out = TioanimeAdapter.parse_search_results(html).unwrap();
        assert!(!out.is_empty(), "the 'naruto' search fixture must have hits");
        assert!(out.iter().any(|c| c.title == "Naruto"));
        for c in &out {
            assert!(!c.url.is_empty());
            assert!(c.url.starts_with("https://tioanime.com/anime/"));
        }
    }

    #[test]
    fn parses_search_empty_fixture_as_zero_results_not_an_error() {
        let html = include_str!("../../tests/fixtures/tioanime_search_empty.html");
        let out = TioanimeAdapter.parse_search_results(html).unwrap();
        assert!(out.is_empty(), "a genuine zero-hit search must parse to Ok(vec![]), not Err");
    }

    #[test]
    fn parse_search_results_errs_on_unrecognizable_page() {
        let err = TioanimeAdapter.parse_search_results("<html><body>not this site</body></html>");
        assert!(err.is_err(), "a page with no ul.animes at all must be treated as a broken/wrong mirror");
    }

    #[test]
    fn genre_archive_methods_use_the_trait_defaults() {
        // This site has no genre-archive concept distinct from the plain
        // directory filter — confirmed live, no separate finished/ongoing
        // marker at the listing-card level. The adapter deliberately does
        // NOT override these; this test locks that in.
        let a = TioanimeAdapter;
        assert_eq!(a.genre_list_url("https://tioanime.com"), "");
        assert_eq!(a.parse_genre_list("<html></html>").unwrap(), Vec::<(String, String)>::new());
        assert_eq!(a.genre_page_url("https://tioanime.com", "accion", 1), "");
        assert_eq!(a.parse_finished_page("<html></html>").unwrap(), Vec::<FinishedCard>::new());
        assert_eq!(a.parse_pagination_last_page("<html></html>"), 1);
    }
}
