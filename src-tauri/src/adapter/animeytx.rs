use super::SiteAdapter;
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

fn slug_from_url(url: &str) -> String {
    url.trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("")
        .to_string()
}

fn text_of(el: scraper::ElementRef, sel: &Selector) -> Option<String> {
    el.select(sel)
        .next()
        .map(|n| n.text().collect::<String>().trim().to_string())
        .filter(|s| !s.is_empty())
}

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

    fn parse_airing(&self, html: &str) -> Result<Vec<Series>> {
        let doc = Html::parse_document(html);
        let card_sel = Selector::parse(AIRING_CARD).unwrap();
        let a_sel = Selector::parse("a").unwrap();
        let tt_sel = Selector::parse(".tt").unwrap();
        let img_sel = Selector::parse("img").unwrap();

        let mut out = Vec::new();
        for card in doc.select(&card_sel) {
            let (title, url, cover_url) = match card_basics(card, &a_sel, &tt_sel, &img_sel) {
                Some(b) => b,
                None => continue,
            };
            out.push(Series {
                id: 0,
                slug: slug_from_url(&url),
                title,
                url,
                cover_url,
                is_airing: true,
                followed: false,
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
        for s in &out {
            assert!(!s.url.is_empty());
            assert!(!s.slug.is_empty());
            assert!(!s.title.is_empty());
        }
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
}
