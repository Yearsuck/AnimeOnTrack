use super::SiteAdapter;
use crate::models::{Episode, FinishedCard, Series, SeriesDetail};
use anyhow::Result;
use scraper::{Html, Selector};

pub struct GogoanimeAdapter;

// Selectors confirmed against real captured HTML (src-tauri/tests/fixtures)
// live 2026-07-23 against https://gogoanime.by — see
// docs/superpowers/specs/2026-07-23-gogoanime-adapter-design.md.
//
// Same DooPlay theme family as `animeytx` (`.bsx`/`.listupd`/`.genxed`/
// `.typez`/`.tt` all present and behave the same way), with real
// differences from it:
//
// - Airing listing is the 7-day weekly `/schedule/` grid, not a single
//   filtered list — cards link to `{base}/series/{slug}/` (the real series
//   page; the homepage's OWN separate `.bsx` grid links straight to
//   individual episode pages instead, so it is deliberately not used here).
//   `next_episode_at`/`site_episode_count` read from `.epx.cndwn[data-rlsdt]`
//   and `.sb` exactly like `animeytx` (absolute unix timestamp, empty string
//   when the episode already released; `.sb` not always numeric).
// - Episode list is `.episodes-container .ep-list .episode-item`, not
//   `.eplister` — each item carries a `data-episode-number` attribute
//   (cleaner than parsing "Episode N" text) and a single `<a href>`. No
//   per-episode title or release date exist on this site at all — `title`
//   and `released_at` are always `None`.
// - Synopsis is `.ninfo > p` (first paragraph) — `animeytx`'s
//   `.entry-content[itemprop="description"]` selector doesn't exist here.
// - No genre-archive concept confirmed distinct from `/schedule/` — the five
//   genre-archive trait methods are left at their default (no-op)
//   implementations, same as tioanime/jkanime.
const AIRING_CARD: &str = ".bsx";
const EPISODE_ITEM: &str = ".episodes-container .ep-list .episode-item";

fn slug_from_url(url: &str) -> String {
    url.trim_end_matches('/').rsplit('/').next().unwrap_or("").to_string()
}

fn text_of(el: scraper::ElementRef, sel: &Selector) -> Option<String> {
    el.select(sel)
        .next()
        .map(|n| n.text().collect::<String>().trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Trailing run of ASCII digits in `s`. Falls back to the whole trimmed
/// string if there are none, mirroring the never-panic discipline the other
/// adapters' own digit-extraction helpers use.
fn digits_in(s: &str) -> String {
    let digits: String = s.chars().filter(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        s.trim().to_string()
    } else {
        digits
    }
}

/// Extract (title, url, poster_url) from a `.bsx` card's anchor + img — same
/// shape `animeytx`'s own `card_basics` uses (this site is the same theme
/// family), duplicated here rather than shared since every adapter module in
/// this codebase keeps its own private helpers.
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
    let poster_url = card.select(img_sel).next().and_then(|i| i.value().attr("src")).map(String::from);
    Some((title, url, poster_url))
}

impl SiteAdapter for GogoanimeAdapter {
    fn airing_url(&self, base_url: &str) -> String {
        format!("{}/schedule/", base_url.trim_end_matches('/'))
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
            let Some((title, url, cover_url)) = card_basics(card, &a_sel, &tt_sel, &img_sel) else { continue };
            let next_episode_at = card
                .select(&cndwn_sel)
                .next()
                .and_then(|el| el.value().attr("data-rlsdt"))
                .and_then(|s| s.parse::<i64>().ok());
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
        let item_sel = Selector::parse(EPISODE_ITEM).unwrap();
        let a_sel = Selector::parse("a").unwrap();

        let mut out = Vec::new();
        for item in doc.select(&item_sel) {
            let Some(anchor) = item.select(&a_sel).next() else { continue };
            let Some(href) = anchor.value().attr("href") else { continue };
            let url = href.to_string();
            let number = item
                .value()
                .attr("data-episode-number")
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| digits_in(&anchor.text().collect::<String>()));
            out.push(Episode {
                id: 0,
                series_id: 0,
                number,
                // No per-episode title or release date on this site at all.
                title: None,
                url,
                released_at: None,
                seen: false,
            });
        }
        Ok(out)
    }

    fn parse_series_detail(&self, html: &str) -> Result<SeriesDetail> {
        let doc = Html::parse_document(html);
        let genxed_sel = Selector::parse(".genxed a").unwrap();
        let typez_sel = Selector::parse(".typez").unwrap();
        let synopsis_sel = Selector::parse(".ninfo > p").unwrap();

        let genres = doc
            .select(&genxed_sel)
            .map(|a| a.text().collect::<String>().trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        let kind = text_of(doc.root_element(), &typez_sel);
        let synopsis = doc
            .select(&synopsis_sel)
            .next()
            .map(|el| el.text().collect::<String>().trim().to_string())
            .filter(|s| !s.is_empty());

        Ok(SeriesDetail { genres, kind, synopsis })
    }

    fn search_url(&self, base_url: &str, query: &str) -> String {
        let encoded: String = url::form_urlencoded::byte_serialize(query.as_bytes()).collect();
        format!("{}/?s={}", base_url.trim_end_matches('/'), encoded)
    }

    fn parse_search_results(&self, html: &str) -> Result<Vec<FinishedCard>> {
        let doc = Html::parse_document(html);
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
            let Some((title, url, poster_url)) = card_basics(card, &a_sel, &tt_sel, &img_sel) else { continue };
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
    fn airing_url_is_the_schedule_page() {
        let a = GogoanimeAdapter;
        assert_eq!(a.airing_url("https://gogoanime.by/"), "https://gogoanime.by/schedule/");
        assert_eq!(a.airing_url("https://gogoanime.by"), "https://gogoanime.by/schedule/");
    }

    #[test]
    fn search_url_is_urlencoded_query_string() {
        let a = GogoanimeAdapter;
        assert_eq!(a.search_url("https://gogoanime.by", "naruto"), "https://gogoanime.by/?s=naruto");
        assert_eq!(
            a.search_url("https://gogoanime.by/", "Shingeki no Kyojin"),
            "https://gogoanime.by/?s=Shingeki+no+Kyojin"
        );
    }

    #[test]
    fn parses_airing_fixture() {
        let html = include_str!("../../tests/fixtures/gogoanime_airing.html");
        let out = GogoanimeAdapter.parse_airing(html).unwrap();
        assert_eq!(out.len(), 3);
        for s in &out {
            assert!(!s.url.is_empty());
            assert!(s.url.starts_with("https://gogoanime.by/series/"), "url not absolute: {}", s.url);
            assert!(!s.slug.is_empty());
            assert!(!s.title.is_empty());
            assert!(s.is_airing);
        }
        // First card has an empty data-rlsdt (already released today).
        assert_eq!(out[0].slug, "haibara-kun-no-tsuyokute-seishun-new-game");
        assert_eq!(out[0].next_episode_at, None);
        assert_eq!(out[0].site_episode_count, Some(13));
        // Second card has a real populated data-rlsdt.
        assert_eq!(out[1].slug, "xiao-lu-he-xiao-lan-5th-season");
        assert_eq!(out[1].next_episode_at, Some(1784779500));
        assert_eq!(out[1].site_episode_count, Some(10));
    }

    #[test]
    fn parses_series_fixture() {
        let html = include_str!("../../tests/fixtures/gogoanime_series.html");
        let out = GogoanimeAdapter.parse_series(html).unwrap();
        assert_eq!(out.len(), 6);
        assert_eq!(out[0].number, "12");
        assert_eq!(
            out[0].url,
            "https://gogoanime.by/haibara-kun-no-tsuyokute-seishun-new-game-episode-12-english-subbed/"
        );
        assert_eq!(out[0].title, None, "this site has no per-episode title");
        assert_eq!(out[0].released_at, None, "this site shows no release date on the episode list");
        assert_eq!(out.last().unwrap().number, "1");
    }

    #[test]
    fn parses_series_detail_fixture() {
        let html = include_str!("../../tests/fixtures/gogoanime_series.html");
        let d = GogoanimeAdapter.parse_series_detail(html).unwrap();
        assert_eq!(d.genres, vec!["Comedy".to_string(), "Romance".to_string()]);
        assert_eq!(d.kind.as_deref(), Some("TV Show"));
        assert!(d.synopsis.unwrap().contains("Haibara"));
    }

    #[test]
    fn parses_search_hits_fixture() {
        let html = include_str!("../../tests/fixtures/gogoanime_search_hits.html");
        let out = GogoanimeAdapter.parse_search_results(html).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].title, "Naruto: Shippuuden");
        assert_eq!(out[0].url, "https://gogoanime.by/series/naruto-shippuuden/");
        assert_eq!(out[0].kind, "TV Show");
    }

    #[test]
    fn parses_search_empty_fixture_as_zero_results_not_an_error() {
        let html = include_str!("../../tests/fixtures/gogoanime_search_empty.html");
        let out = GogoanimeAdapter.parse_search_results(html).unwrap();
        assert!(out.is_empty(), "a genuine zero-hit search must parse to Ok(vec![]), not Err");
    }

    #[test]
    fn parse_search_results_errs_on_unrecognizable_page() {
        let err = GogoanimeAdapter.parse_search_results("<html><body>not this site</body></html>");
        assert!(err.is_err(), "a page with no .listupd at all must be treated as a broken/wrong mirror");
    }

    #[test]
    fn genre_archive_methods_use_the_trait_defaults() {
        let a = GogoanimeAdapter;
        assert_eq!(a.genre_list_url("https://gogoanime.by"), "");
        assert_eq!(a.parse_genre_list("<html></html>").unwrap(), Vec::<(String, String)>::new());
        assert_eq!(a.genre_page_url("https://gogoanime.by", "action", 1), "");
        assert_eq!(a.parse_finished_page("<html></html>").unwrap(), Vec::<FinishedCard>::new());
        assert_eq!(a.parse_pagination_last_page("<html></html>"), 1);
    }
}
