use super::SiteAdapter;
use crate::models::{Episode, FinishedCard, Series, SeriesDetail};
use anyhow::Result;
use scraper::{Html, Selector};

pub struct AnimeflvAdapter;

// Selectors confirmed against real captured HTML (src-tauri/tests/fixtures,
// captured live 2026-07-11 via scraper_engine::fetch_html against
// https://www4.animeflv.net — NOT a DooPlay site like AnimeYT, `.bsx`/
// `.eplister` do not exist here).
//
// - Airing listing: `/browse?status[]=1` (confirmed from the
//   `<select name="status[]" id="status_select">` options on `/browse`;
//   `status[]=2` is "Finalizado"). Cards are `ul.ListAnimes li article.Anime`,
//   each `<a href="/anime/{slug}">` wrapping a poster `<img src>`, an
//   `h3.Title`, and a `span.Type` badge whose TEXT (not its 2nd CSS class,
//   same AnimeYT `.typez` gotcha) reads "Anime" for a TV series, "OVA" for
//   an OVA, etc — genuinely different vocabulary from AnimeYT's "TV". No
//   countdown or episode-count signal at all on these cards, so
//   `next_episode_at`/`site_episode_count` are always `None` here; the
//   airing-grid sort for this site falls back to title order (see the
//   design spec, task 8).
// - A series page (`/anime/{slug}`) carries BOTH the episode list AND the
//   genres/kind/synopsis, so `series.html`/`series_detail.html` are the same
//   captured fixture, matching the spec's explicit allowance for that case.
//   The episode list (`#episodeList li`) is rendered server-side into the
//   DOM already on load (NOT fetched client-side after — confirmed by
//   inspecting the captured HTML directly, the `<li>` markup is present
//   verbatim even though a `<script>` on the page also re-renders it via a
//   `var episodes = [[num, id], ...]` array on interaction; the poll-based
//   readiness check in scraper_engine doesn't need to wait for that script,
//   since the fixture already has it rendered by the time DOMContentLoaded
//   fires). Row shape: `<li><a href="/ver/{slug}-N">...<h3 class="Title">
//   {series title, NOT an episode title}</h3><p>Episodio N</p></a>
//   <label>...<input class="mseen" data-number="N">...</label></li>`. The
//   episode number is read from `input.mseen`'s `data-number` attribute
//   (reliable, numeric, no regex needed) rather than parsed out of the
//   "Episodio N" text. The list also contains a "next episode" placeholder
//   row (`li.Next`, `href="#"`, no `input.mseen` at all) that must be
//   skipped, not just filtered by a static selector — this adapter does that
//   by requiring `input.mseen[data-number]` to be present, which the
//   placeholder never has. No per-episode title or release date exists on
//   this site (the row repeats the series title), so both `Episode.title`
//   and `Episode.released_at` are always `None`.
// - Kind: `.Ficha .Type` text (scoped to the top info box, since `.Type` is
//   also the class used by card badges elsewhere on the site). Genres:
//   `nav.Nvgnrs a` text. Synopsis: the sinopsis section's `.Description p`
//   text (`.Description` is reused by listing cards for their blurb, but
//   `parse_series_detail` only ever runs against a detail page, not a
//   listing page, so that reuse doesn't collide here).
// - Search: `/browse?q={query}` reuses the exact same `ul.ListAnimes`
//   listing markup as the airing/status filter (confirmed live against both
//   a hit and a genuine zero-hit query) — including on a zero-hit search,
//   where the `ul.ListAnimes` container is present but empty, same
//   "container present but empty = zero results, container absent entirely
//   = wrong/incompatible mirror" contract AnimeYT's `.listupd` uses.
// - No genre-archive concept confirmed on this site distinct from the plain
//   `/browse` filter above (no separate finished-vs-ongoing marker at the
//   listing-card level) — the five genre-archive trait methods are left at
//   their phase-1 defaults, not implemented.
//
// KNOWN LIMITATION (shared with the tioanime adapter, not raised by the
// design spec, found while implementing): `href`s on this site are relative
// ("/anime/slug", "/ver/slug-N"), unlike AnimeYT's markup which already
// emits absolute URLs, and `SiteAdapter::parse_*` has no `base_url`
// parameter to resolve these against. This adapter resolves them against the
// fixed `BASE` constant below (the default mirror) rather than whatever
// mirror actually served the page — only matters in practice if a second
// animeflv mirror is ever configured and used instead of the default. Poster
// (`.Image img[src]`) and episode-thumbnail (`figure img[data-src]`) URLs
// were already absolute in every capture (a separate CDN host,
// `animeflv.net`/`cdn.animeflv.net`, not the mirror domain) so `abs()` is a
// no-op for those — it only ever rewrites the relative page `href`s.
const BASE: &str = "https://www4.animeflv.net";
const CARD: &str = "ul.ListAnimes li article.Anime";
const EP_ROW: &str = "#episodeList li";

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

impl SiteAdapter for AnimeflvAdapter {
    fn airing_url(&self, base_url: &str) -> String {
        format!("{}/browse?status[]=1", base_url.trim_end_matches('/'))
    }

    /// AnimeFLV's `/browse` results paginate with `&page=N` (page 1 is the bare
    /// query). The airing set spans several pages, so the scan walks them until
    /// one is empty; a wrong guess degrades to single-page via the scan's
    /// "no new series" stop.
    fn airing_page_url(&self, base_url: &str, page: u32) -> Option<String> {
        let base = base_url.trim_end_matches('/');
        Some(if page <= 1 {
            format!("{base}/browse?status[]=1")
        } else {
            format!("{base}/browse?status[]=1&page={page}")
        })
    }

    fn parse_airing(&self, html: &str) -> Result<Vec<Series>> {
        let doc = Html::parse_document(html);
        let card_sel = Selector::parse(CARD).unwrap();
        let a_sel = Selector::parse("a").unwrap();
        let title_sel = Selector::parse(".Title").unwrap();
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
        let num_sel = Selector::parse("input.mseen").unwrap();

        let mut out = Vec::new();
        for row in doc.select(&row_sel) {
            // The "next episode" placeholder row has no input.mseen at all —
            // this is what excludes it, not a static :not() selector.
            let Some(number) = row
                .select(&num_sel)
                .next()
                .and_then(|i| i.value().attr("data-number"))
                .map(|s| s.to_string())
            else {
                continue;
            };
            let Some(href) = row.select(&a_sel).next().and_then(|a| a.value().attr("href")) else {
                continue;
            };
            out.push(Episode {
                id: 0,
                series_id: 0,
                number,
                // No per-episode title or release date on this site.
                title: None,
                url: abs(href),
                released_at: None,
                seen: false,
            });
        }
        Ok(out)
    }

    fn parse_series_detail(&self, html: &str) -> Result<SeriesDetail> {
        let doc = Html::parse_document(html);
        let genres_sel = Selector::parse("nav.Nvgnrs a").unwrap();
        let kind_sel = Selector::parse(".Ficha .Type").unwrap();
        let synopsis_sel = Selector::parse(".Description p").unwrap();

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
        format!("{}/browse?q={}", base_url.trim_end_matches('/'), encoded)
    }

    fn parse_search_results(&self, html: &str) -> Result<Vec<FinishedCard>> {
        let doc = Html::parse_document(html);
        // Same `ul.ListAnimes` container as the airing/status listing
        // (confirmed live against both a hit and a genuine zero-hit query)
        // — present but empty (no `.Anime` article children) for a real
        // zero-hit search, absent entirely only for a page that doesn't
        // look like this site's browse layout at all (wrong/incompatible
        // mirror).
        let container_sel = Selector::parse("ul.ListAnimes").unwrap();
        if doc.select(&container_sel).next().is_none() {
            return Err(anyhow::anyhow!(
                "no ul.ListAnimes container found; not a recognizable browse/search page"
            ));
        }
        let card_sel = Selector::parse(CARD).unwrap();
        let a_sel = Selector::parse("a").unwrap();
        let title_sel = Selector::parse(".Title").unwrap();
        let img_sel = Selector::parse("img").unwrap();
        let type_sel = Selector::parse(".Type").unwrap();

        let mut out = Vec::new();
        for card in doc.select(&card_sel) {
            let Some(anchor) = card.select(&a_sel).next() else { continue };
            let Some(href) = anchor.value().attr("href") else { continue };
            let url = abs(href);
            let title = text_of(card, &title_sel).unwrap_or_else(|| slug_from_url(&url));
            let poster_url = card.select(&img_sel).next().and_then(|i| i.value().attr("src")).map(abs);
            let kind = text_of(card, &type_sel).unwrap_or_default();
            out.push(FinishedCard { title, url, poster_url, kind, matched_genre: None });
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn airing_url_uses_the_status_filter() {
        let a = AnimeflvAdapter;
        assert_eq!(
            a.airing_url("https://www4.animeflv.net/"),
            "https://www4.animeflv.net/browse?status[]=1"
        );
        assert_eq!(
            a.airing_url("https://www4.animeflv.net"),
            "https://www4.animeflv.net/browse?status[]=1"
        );
    }

    #[test]
    fn search_url_is_urlencoded_query_string() {
        let a = AnimeflvAdapter;
        assert_eq!(a.search_url("https://www4.animeflv.net", "naruto"), "https://www4.animeflv.net/browse?q=naruto");
        assert_eq!(
            a.search_url("https://www4.animeflv.net/", "Shingeki no Kyojin"),
            "https://www4.animeflv.net/browse?q=Shingeki+no+Kyojin"
        );
    }

    #[test]
    fn parses_airing_fixture() {
        let html = include_str!("../../tests/fixtures/animeflv_airing.html");
        let out = AnimeflvAdapter.parse_airing(html).unwrap();
        assert!(!out.is_empty(), "expected at least one series");
        for s in &out {
            assert!(!s.url.is_empty());
            assert!(s.url.starts_with("https://www4.animeflv.net/anime/"), "url not absolute: {}", s.url);
            assert!(!s.slug.is_empty());
            assert!(!s.title.is_empty());
            assert!(s.is_airing);
            assert_eq!(s.next_episode_at, None);
            assert_eq!(s.site_episode_count, None);
        }
        let first = &out[0];
        assert_eq!(first.slug, "aishiteru-game-wo-owarasetai");
        assert_eq!(first.title, "Aishiteru Game wo Owarasetai");
        assert!(first.cover_url.as_deref().unwrap().contains("uploads/animes/covers"));
    }

    #[test]
    fn parses_series_fixture_skips_the_next_episode_placeholder() {
        let html = include_str!("../../tests/fixtures/animeflv_series.html");
        let out = AnimeflvAdapter.parse_series(html).unwrap();
        assert_eq!(out.len(), 12, "the Liar Game fixture has 12 real episode rows plus 1 placeholder");
        let first = &out[0];
        assert_eq!(first.number, "12");
        assert_eq!(first.title, None, "this site has no per-episode title");
        assert_eq!(first.url, "https://www4.animeflv.net/ver/liar-game-12");
        assert_eq!(first.released_at, None);
        for e in &out {
            assert!(!e.url.is_empty());
            assert!(!e.number.is_empty());
            assert!(e.number.chars().all(|c| c.is_ascii_digit()), "placeholder row leaked in: {}", e.number);
        }
    }

    #[test]
    fn parses_series_detail_fixture() {
        let html = include_str!("../../tests/fixtures/animeflv_series_detail.html");
        let d = AnimeflvAdapter.parse_series_detail(html).unwrap();
        assert_eq!(
            d.genres,
            vec!["Drama".to_string(), "Psicológico".to_string(), "Seinen".to_string(), "Suspenso".to_string()]
        );
        assert_eq!(d.kind.as_deref(), Some("Anime"), "this site's TV badge text is literally \"Anime\", not \"TV\"");
        assert!(d.synopsis.unwrap().contains("Nao Kanzaki"));
    }

    #[test]
    fn parses_search_hits_fixture() {
        let html = include_str!("../../tests/fixtures/animeflv_search_hits.html");
        let out = AnimeflvAdapter.parse_search_results(html).unwrap();
        assert!(!out.is_empty(), "the 'naruto' search fixture must have hits");
        assert!(out.iter().any(|c| c.title.to_lowercase().contains("naruto")));
        for c in &out {
            assert!(!c.url.is_empty());
            assert!(c.url.starts_with("https://www4.animeflv.net/anime/"));
        }
    }

    #[test]
    fn parses_search_empty_fixture_as_zero_results_not_an_error() {
        let html = include_str!("../../tests/fixtures/animeflv_search_empty.html");
        let out = AnimeflvAdapter.parse_search_results(html).unwrap();
        assert!(out.is_empty(), "a genuine zero-hit search must parse to Ok(vec![]), not Err");
    }

    #[test]
    fn parse_search_results_errs_on_unrecognizable_page() {
        let err = AnimeflvAdapter.parse_search_results("<html><body>not this site</body></html>");
        assert!(err.is_err(), "a page with no ul.ListAnimes at all must be treated as a broken/wrong mirror");
    }

    #[test]
    fn genre_archive_methods_use_the_trait_defaults() {
        let a = AnimeflvAdapter;
        assert_eq!(a.genre_list_url("https://www4.animeflv.net"), "");
        assert_eq!(a.parse_genre_list("<html></html>").unwrap(), Vec::<(String, String)>::new());
        assert_eq!(a.genre_page_url("https://www4.animeflv.net", "accion", 1), "");
        assert_eq!(a.parse_finished_page("<html></html>").unwrap(), Vec::<FinishedCard>::new());
        assert_eq!(a.parse_pagination_last_page("<html></html>"), 1);
    }
}
