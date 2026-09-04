use super::{slug_from_url, text_of, SiteAdapter};
use crate::models::{Episode, FinishedCard, Series, SeriesDetail};
use anyhow::Result;
use scraper::{Html, Selector};

pub struct AnimeytxAdapter;

// Selectors confirmed against real captured HTML (src-tauri/tests/fixtures).
// The airing page (/anime-en-emision/) uses a DooPlay "schedule" layout: series
// cards are `.bsx`, each wrapping an <a> (with the clean title in its `title`
// attribute and in a `.tt` div) and a poster <img>.
const AIRING_CARD: &str = ".bsx";
const EP_ROW: &str = ".eplister ul li";


/// Extract (title, url, poster_url) from a `.bsx` card's anchor + img.
/// Shared by `parse_airing` and `parse_finished_page` — both cards carry the
/// clean title in the anchor's `title` attribute (with `.tt` as a fallback,
/// and finally the URL slug), and the poster in an `img`'s `data-src`/`src`.
/// Returns `None` if the card has no anchor `href` at all (nothing to link to).
fn card_basics(
    card: scraper::ElementRef,
    a_sel: &Selector,
    tt_sel: &Selector,
    img_sel: &Selector,
) -> Option<(String, String, Option<String>)> {
    let anchor = card.select(a_sel).next()?;
    let url = anchor.value().attr("href")?.to_string();
    let title = anchor
        .value()
        .attr("title")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| text_of(card, tt_sel))
        .unwrap_or_else(|| slug_from_url(&url));
    let poster_url = card.select(img_sel).next().and_then(|i| {
        i.value()
            .attr("data-src")
            .or_else(|| i.value().attr("src"))
            .map(|s| s.to_string())
    });
    Some((title, url, poster_url))
}

impl SiteAdapter for AnimeytxAdapter {
    fn airing_url(&self, base_url: &str) -> String {
        format!("{}/anime-en-emision/", base_url.trim_end_matches('/'))
    }

    /// DooPlay paginates its archive pages as `/anime-en-emision/page/N/`
    /// (page 1 is the bare path). The scan walks pages until one comes back
    /// empty, so an airing list longer than one page is captured in full
    /// instead of truncated at the first ~page. Safe if the theme happens not
    /// to paginate: page 2 then re-serves page 1 and the scan stops on the
    /// "no new series" guard.
    fn airing_page_url(&self, base_url: &str, page: u32) -> Option<String> {
        let base = base_url.trim_end_matches('/');
        Some(if page <= 1 {
            format!("{base}/anime-en-emision/")
        } else {
            format!("{base}/anime-en-emision/page/{page}/")
        })
    }

    fn parse_airing(&self, html: &str) -> Result<Vec<Series>> {
        let doc = Html::parse_document(html);
        let card_sel = Selector::parse(AIRING_CARD).unwrap();
        let a_sel = Selector::parse("a").unwrap();
        let tt_sel = Selector::parse(".tt").unwrap();
        let img_sel = Selector::parse("img").unwrap();
        let cndwn_sel = Selector::parse(".epx.cndwn").unwrap();
        let sb_sel = Selector::parse(".sb").unwrap();

        let mut out = Vec::new();
        for card in doc.select(&card_sel) {
            let (title, url, cover_url) = match card_basics(card, &a_sel, &tt_sel, &img_sel) {
                Some(b) => b,
                None => continue,
            };
            // data-rlsdt is the unix timestamp of the NEXT episode's release
            // (data-cndwn is the redundant seconds-remaining countdown, stale
            // the instant it's parsed — ignored). Missing on a card with no
            // countdown span at all.
            let next_episode_at = card
                .select(&cndwn_sel)
                .next()
                .and_then(|el| el.value().attr("data-rlsdt"))
                .and_then(|s| s.parse::<i64>().ok());
            // .sb is the site's reported episode count. Not always numeric —
            // observed live values include "2", "14", and "??" — so this
            // must parse to None rather than panic or coerce to 0.
            let site_episode_count = text_of(card, &sb_sel).and_then(|s| s.parse::<i64>().ok());
            out.push(Series {
                id: 0,
                slug: slug_from_url(&url),
                title,
                url,
                cover_url,
                is_airing: true,
                followed: false,
                next_episode_at,
                site_episode_count,
            });
        }
        Ok(out)
    }

    fn parse_series(&self, html: &str) -> Result<Vec<Episode>> {
        let doc = Html::parse_document(html);
        let row_sel = Selector::parse(EP_ROW).unwrap();
        let a_sel = Selector::parse("a").unwrap();
        let num_sel = Selector::parse(".epl-num").unwrap();
        let title_sel = Selector::parse(".epl-title").unwrap();
        let date_sel = Selector::parse(".epl-date").unwrap();

        let mut out = Vec::new();
        for row in doc.select(&row_sel) {
            let url = match row
                .select(&a_sel)
                .next()
                .and_then(|a| a.value().attr("href"))
            {
                Some(h) => h.to_string(),
                None => continue,
            };
            let number = text_of(row, &num_sel).unwrap_or_else(|| slug_from_url(&url));
            let title = text_of(row, &title_sel);
            let released_at = text_of(row, &date_sel);
            out.push(Episode {
                id: 0,
                series_id: 0,
                number,
                title,
                url,
                released_at,
                seen: false,
            });
        }
        Ok(out)
    }

    fn genre_list_url(&self, base_url: &str) -> String {
        format!("{}/", base_url.trim_end_matches('/'))
    }

    fn parse_genre_list(&self, html: &str) -> Result<Vec<(String, String)>> {
        let doc = Html::parse_document(html);
        let a_sel = Selector::parse(r#"a[href*="/genres/"]"#).unwrap();
        let mut seen = std::collections::HashSet::new();
        let mut out = Vec::new();
        for a in doc.select(&a_sel) {
            let href = match a.value().attr("href") {
                Some(h) => h,
                None => continue,
            };
            let slug = slug_from_url(href);
            let name = a.text().collect::<String>().trim().to_string();
            if slug.is_empty() || name.is_empty() || !seen.insert(slug.clone()) {
                continue;
            }
            out.push((slug, name));
        }
        Ok(out)
    }

    fn genre_page_url(&self, base_url: &str, genre_slug: &str, page: u32) -> String {
        let base = base_url.trim_end_matches('/');
        if page <= 1 {
            format!("{base}/genres/{genre_slug}/")
        } else {
            format!("{base}/genres/{genre_slug}/page/{page}/")
        }
    }

    fn parse_finished_page(&self, html: &str) -> Result<Vec<FinishedCard>> {
        let doc = Html::parse_document(html);
        let card_sel = Selector::parse(AIRING_CARD).unwrap();
        let a_sel = Selector::parse("a").unwrap();
        let tt_sel = Selector::parse(".tt").unwrap();
        let status_sel = Selector::parse(".status.Completed").unwrap();
        let typez_sel = Selector::parse(".typez").unwrap();
        let img_sel = Selector::parse("img").unwrap();

        let mut out = Vec::new();
        for card in doc.select(&card_sel) {
            if card.select(&status_sel).next().is_none() {
                continue; // no .status.Completed => not finished, skip
            }
            let (title, url, poster_url) = match card_basics(card, &a_sel, &tt_sel, &img_sel) {
                Some(b) => b,
                None => continue,
            };
            // .typez's text is the actual type badge; its 2nd CSS class does
            // NOT reliably match (e.g. class="typez Music" with text
            // "Donghua" observed live), so this must read text, never class.
            let kind = text_of(card, &typez_sel).unwrap_or_default();
            out.push(FinishedCard { title, url, poster_url, kind, matched_genre: None });
        }
        Ok(out)
    }

    fn parse_pagination_last_page(&self, html: &str) -> u32 {
        let doc = Html::parse_document(html);
        let sel = Selector::parse(".pagination a.page-numbers").unwrap();
        doc.select(&sel)
            // the "Siguiente »" next-link also carries class `page-numbers`,
            // so only text that parses as a plain number counts as a page.
            .filter_map(|a| a.text().collect::<String>().trim().parse::<u32>().ok())
            .max()
            .unwrap_or(1)
    }

    fn parse_series_detail(&self, html: &str) -> Result<SeriesDetail> {
        let doc = Html::parse_document(html);
        let genxed_sel = Selector::parse(".genxed a").unwrap();
        let spe_sel = Selector::parse(".spe > span").unwrap();
        let b_sel = Selector::parse("b").unwrap();
        let synopsis_sel = Selector::parse(r#".entry-content[itemprop="description"]"#).unwrap();

        let genres = doc
            .select(&genxed_sel)
            .map(|a| a.text().collect::<String>().trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();

        // .spe is a flat list of <span><b>Label:</b> value</span>; find the
        // span whose <b> reads "Tipo:" and take the rest of its text.
        let mut kind = None;
        for span in doc.select(&spe_sel) {
            let label = text_of(span, &b_sel).unwrap_or_default();
            if label == "Tipo:" {
                let full = span.text().collect::<String>();
                kind = full.strip_prefix(&label).map(|s| s.trim().to_string());
                break;
            }
        }

        let synopsis = doc
            .select(&synopsis_sel)
            .next()
            .map(|el| el.text().collect::<String>().trim().to_string())
            .filter(|s| !s.is_empty());

        Ok(SeriesDetail { genres, kind, synopsis })
    }

    fn search_url(&self, base_url: &str, query: &str) -> String {
        // DooPlay serves search at {base}/?s={urlencoded}, confirmed against
        // real captured search-results HTML (see tests/fixtures/animeytx_search_*.html).
        let encoded: String = url::form_urlencoded::byte_serialize(query.as_bytes()).collect();
        format!("{}/?s={}", base_url.trim_end_matches('/'), encoded)
    }

    fn parse_search_results(&self, html: &str) -> Result<Vec<FinishedCard>> {
        let doc = Html::parse_document(html);
        // `.listupd` wraps the results area on both search and genre-listing
        // pages (confirmed live): present-but-empty (just a "No se
        // encuentra" message, no `.bsx` cards) for a genuine zero-hit search,
        // and absent entirely only when the page isn't recognizable as this
        // site's layout at all (wrong/incompatible mirror) — that's the only
        // case this returns Err.
        let container_sel = Selector::parse(".listupd").unwrap();
        if doc.select(&container_sel).next().is_none() {
            return Err(anyhow::anyhow!(
                "no .listupd container found; not a recognizable search-results page"
            ));
        }
        let card_sel = Selector::parse(".listupd .bsx").unwrap();
        let a_sel = Selector::parse("a").unwrap();
        let tt_sel = Selector::parse(".tt").unwrap();
        let typez_sel = Selector::parse(".typez").unwrap();
        let img_sel = Selector::parse("img").unwrap();

        let mut out = Vec::new();
        for card in doc.select(&card_sel) {
            let (title, url, poster_url) = match card_basics(card, &a_sel, &tt_sel, &img_sel) {
                Some(b) => b,
                None => continue,
            };
            let kind = text_of(card, &typez_sel).unwrap_or_default();
            out.push(FinishedCard { title, url, poster_url, kind, matched_genre: None });
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn airing_url_is_built() {
        let a = AnimeytxAdapter;
        assert_eq!(
            a.airing_url("https://wwv.animeytx.net/"),
            "https://wwv.animeytx.net/anime-en-emision/"
        );
    }

    #[test]
    fn parses_airing_fixture() {
        let html = include_str!("../../tests/fixtures/airing.html");
        let out = AnimeytxAdapter.parse_airing(html).unwrap();
        assert!(!out.is_empty(), "expected at least one series");
        // First card is World Is Dancing.
        let first = &out[0];
        assert_eq!(first.slug, "world-is-dancing");
        assert_eq!(first.title, "World Is Dancing");
        assert_eq!(first.url, "https://wwv.animeytx.net/tv/world-is-dancing/");
        assert!(first.cover_url.as_deref().unwrap().contains("wp-content"));
        assert!(first.is_airing);
        // World Is Dancing's card: data-rlsdt="1783350140", <span class="sb Sub">2</span>.
        assert_eq!(first.next_episode_at, Some(1783350140));
        assert_eq!(first.site_episode_count, Some(2));
        for s in &out {
            assert!(!s.url.is_empty());
            assert!(!s.slug.is_empty());
            assert!(!s.title.is_empty());
        }
    }

    #[test]
    fn parses_airing_fixture_non_numeric_episode_count_is_none() {
        // Third card (Higeki no Genkyou...) has <span class="sb Sub">??</span>
        // — must parse to None, not panic or coerce to 0.
        let html = include_str!("../../tests/fixtures/airing.html");
        let out = AnimeytxAdapter.parse_airing(html).unwrap();
        let third = &out[2];
        assert_eq!(third.next_episode_at, Some(1783434000));
        assert_eq!(third.site_episode_count, None);
    }

    #[test]
    fn parses_airing_card_with_no_countdown_span_as_none() {
        let html = r#"<div class="bsx">
            <a href="https://wwv.animeytx.net/tv/no-countdown/" title="No Countdown">
                <div class="limit"><div class="bt"><span class="sb Sub">5</span></div>
                <img src="https://wwv.animeytx.net/wp-content/x.jpg"></div>
                <div class="tt">No Countdown</div>
            </a>
        </div>"#;
        let out = AnimeytxAdapter.parse_airing(html).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].next_episode_at, None);
        assert_eq!(out[0].site_episode_count, Some(5));
    }

    #[test]
    fn parses_series_fixture() {
        let html = include_str!("../../tests/fixtures/series.html");
        let out = AnimeytxAdapter.parse_series(html).unwrap();
        assert_eq!(out.len(), 5, "expected 5 episodes in fixture");
        let first = &out[0];
        assert_eq!(first.number, "13");
        assert_eq!(first.title.as_deref(), Some("Escisión"));
        assert_eq!(
            first.url,
            "https://wwv.animeytx.net/anime/liar-game-capitulo-13/"
        );
        assert_eq!(first.released_at.as_deref(), Some("junio 29, 2026"));
        for e in &out {
            assert!(!e.url.is_empty());
            assert!(!e.number.is_empty());
        }
    }

    #[test]
    fn genre_list_url_is_homepage() {
        let a = AnimeytxAdapter;
        assert_eq!(a.genre_list_url("https://wwv.animeytx.net/"), "https://wwv.animeytx.net/");
    }

    #[test]
    fn genre_page_url_omits_page_suffix_for_page_one() {
        let a = AnimeytxAdapter;
        assert_eq!(
            a.genre_page_url("https://wwv.animeytx.net", "aventura", 1),
            "https://wwv.animeytx.net/genres/aventura/"
        );
        assert_eq!(
            a.genre_page_url("https://wwv.animeytx.net", "aventura", 3),
            "https://wwv.animeytx.net/genres/aventura/page/3/"
        );
    }

    #[test]
    fn parses_genre_list_fixture_deduped() {
        let html = include_str!("../../tests/fixtures/homepage.html");
        let out = AnimeytxAdapter.parse_genre_list(html).unwrap();
        // "aventura" appears in the nav AND inside a .genxed card => must be deduped to 1
        assert_eq!(out.iter().filter(|(slug, _)| slug == "aventura").count(), 1);
        assert!(out.iter().any(|(slug, name)| slug == "fantasia" && name == "Fantasía"));
        assert_eq!(out.len(), 5, "aventura, drama, ecchi, fantasia, isekai — no new slugs from .genxed dupes");
    }

    #[test]
    fn parses_finished_page_fixture_skips_non_completed() {
        let html = include_str!("../../tests/fixtures/genre_listing.html");
        let out = AnimeytxAdapter.parse_finished_page(html).unwrap();
        assert_eq!(out.len(), 2, "the Hiatus card (no .status div) must be excluded");
        assert_eq!(out[0].kind, "Yuri");
        // Regression: .typez's 2nd CSS class ("Music") must NOT leak into kind —
        // the live site's class list disagrees with the badge text it displays.
        assert_eq!(out[1].kind, "Donghua");
        for c in &out {
            assert!(!c.url.is_empty());
            assert!(c.poster_url.is_some());
        }
    }

    #[test]
    fn parses_pagination_last_page_ignoring_next_link() {
        let html = include_str!("../../tests/fixtures/genre_listing.html");
        assert_eq!(AnimeytxAdapter.parse_pagination_last_page(html), 24);
    }

    #[test]
    fn parses_series_detail_fixture() {
        let html = include_str!("../../tests/fixtures/series_detail.html");
        let d = AnimeytxAdapter.parse_series_detail(html).unwrap();
        assert_eq!(
            d.genres,
            vec!["Drama".to_string(), "Juegos".to_string(), "Psicológico".to_string(), "Seinen".to_string(), "Suspenso".to_string()]
        );
        assert_eq!(d.kind.as_deref(), Some("TV"));
        assert!(d.synopsis.unwrap().contains("Nao Kanzaki"));
    }

    #[test]
    fn search_url_is_urlencoded_query_string() {
        let a = AnimeytxAdapter;
        assert_eq!(
            a.search_url("https://wwv.animeytx.net/", "naruto"),
            "https://wwv.animeytx.net/?s=naruto"
        );
        assert_eq!(
            a.search_url("https://wwv.animeytx.net", "Shingeki no Kyojin"),
            "https://wwv.animeytx.net/?s=Shingeki+no+Kyojin"
        );
    }

    // Both fixtures below are real HTML captured live via scraper_engine::fetch_html
    // against https://wwv.animeytx.net/?s=naruto and /?s=xyzzyqqqnonexistentanimetitle999
    // (2026-07-10) — not hand-written, per the design spec's requirement that
    // parse_search_results not be built against guessed markup.

    #[test]
    fn parses_search_hits_fixture() {
        let html = include_str!("../../tests/fixtures/animeytx_search_hits.html");
        let out = AnimeytxAdapter.parse_search_results(html).unwrap();
        assert_eq!(out.len(), 1, "the 'naruto' search fixture has exactly one .bsx result card");
        let first = &out[0];
        assert_eq!(first.title, "Boruto: Naruto Next Generations");
        assert_eq!(first.url, "https://wwv.animeytx.net/tv/boruto-naruto-next-generations/");
        assert_eq!(first.kind, "TV");
        assert!(first.poster_url.as_deref().unwrap().contains("wp-content"));
    }

    #[test]
    fn parses_search_empty_fixture_as_zero_results_not_an_error() {
        let html = include_str!("../../tests/fixtures/animeytx_search_empty.html");
        let out = AnimeytxAdapter.parse_search_results(html).unwrap();
        assert!(out.is_empty(), "a genuine zero-hit search must parse to Ok(vec![]), not Err");
    }

    #[test]
    fn parse_search_results_errs_on_unrecognizable_page() {
        let err = AnimeytxAdapter.parse_search_results("<html><body>not this site</body></html>");
        assert!(err.is_err(), "a page with no .listupd at all must be treated as a broken/wrong mirror");
    }
}
