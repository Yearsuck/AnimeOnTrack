use super::SiteAdapter;
use crate::models::{Episode, Series};
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
            let anchor = match card.select(&a_sel).next() {
                Some(a) => a,
                None => continue,
            };
            let url = match anchor.value().attr("href") {
                Some(h) => h.to_string(),
                None => continue,
            };
            // Prefer the anchor's `title` attribute; fall back to `.tt`, then slug.
            let title = anchor
                .value()
                .attr("title")
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .or_else(|| text_of(card, &tt_sel))
                .unwrap_or_else(|| slug_from_url(&url));
            let cover_url = card.select(&img_sel).next().and_then(|i| {
                i.value()
                    .attr("data-src")
                    .or_else(|| i.value().attr("src"))
                    .map(|s| s.to_string())
            });
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
}
