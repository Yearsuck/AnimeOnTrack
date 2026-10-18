use super::{slug_from_url, text_of, SiteAdapter};
use crate::models::{Episode, FinishedCard, Series, SeriesDetail};
use anyhow::{anyhow, Result};
use scraper::{Html, Selector};

pub struct AniwavesAdapter;

// Selectors confirmed against real captured HTML in src-tauri/tests/fixtures/
// (aniwaves_updated.html, aniwaves_search_hits.html, aniwaves_search_empty.html,
// aniwaves_series.html, aniwaves_episodes.json).
//
// Site characteristics:
// - Domain: https://aniwaves.ru (no Cloudflare challenge observed).
// - Airing listing: `{base}/updated` ("Recently Updated Anime").
//   Cards sit in `#list-items .item`. Each item has an anchor `a.name.d-title`
//   holding `/watch/<slug>-<id>` href, with a `data-jp` attribute containing
//   the romaji title (preferred over the English link text).
//   Episode count badge: `.ep-status.total span` carries the total episode count
//   when numeric (e.g. "26" or "13"), or "?" when ongoing/unknown.
//   No countdown timestamp exists on listing cards (`next_episode_at: None`).
// - Series detail page (`/watch/<slug>-<id>`):
//   Does NOT contain the episode list in the static HTML.
//   Contains series metadata under `#w-info .binfo`:
//   - Genres: `<a href="/genre/<slug>">` links inside `#w-info .bmeta`.
//   - Kind/Type: `<a href="/type/<slug>">` link inside `#w-info .bmeta` (e.g. "TV").
//   - Synopsis: `#w-info .synopsis .content`.
// - Episode list (AJAX):
//   The client page loads episodes via `GET /ajax/episode/list/<numeric_id>`.
//   `episode_fetch_script()` provides a synchronous in-page JavaScript snippet
//   extracting the numeric id from `location.pathname` (regex `-(\d+)$`),
//   firing a synchronous `XMLHttpRequest` with header `X-Requested-With: XMLHttpRequest`,
//   and returning the JSON response string.
//   The JSON shape is `{"status": 200, "result": "<html..."}`.
//   Inside `result`, each episode is `<a href="/watch/<id>/ep-N" data-num="N" data-aired="...">`.
//   Order in source is ascending (ep 1 to ep 25).
//   `parse_series` deserializes this JSON and tolerates empty or error responses gracefully.
// - Search:
//   Endpoint: `{base}/filter?keyword=<query>` (form-urlencoded, spaces to `+`).
//   Uses the identical `#list-items .item` card shape as the updated/airing page.
//   Contract: `#list-items` container present with `.item` cards on hits,
//   `#list-items` container present with `#empty-notification` (and 0 cards) on empty search,
//   and `#list-items` absent on wrong/incompatible mirror pages (yielding Err).
//   Note: the sidebar `<aside class="sidebar">` also contains `<a class="item">` recommendations;
//   scoping queries to `#list-items .item` ensures sidebar links are never confused with search results.
//
// KNOWN LIMITATION:
// Root-relative hrefs ("/watch/...", "/assets/...") are resolved against the `BASE`
// constant below. As documented in tioanime.rs, any mirror divergence is handled
// later by `commands::scan::rebase_to_mirror`.
const BASE: &str = "https://aniwaves.ru";
const LIST_ITEMS_CONTAINER: &str = "#list-items";
const CARD: &str = "#list-items .item";

fn abs(path: &str) -> String {
    if path.starts_with("http://") || path.starts_with("https://") {
        path.to_string()
    } else if let Some(stripped) = path.strip_prefix('/') {
        format!("{BASE}/{stripped}")
    } else {
        format!("{BASE}/{path}")
    }
}

/// Extra JS run in-page once the page is interactive, returning the raw response
/// of `GET /ajax/episode/list/<id>` synchronously. ExecuteScript cannot await
/// promises host-side, so synchronous XHR is mandatory.
const EPISODE_FETCH_SCRIPT: &str = r#"(function(){
    try {
        var m = location.pathname.match(/-(\d+)$/);
        var id = m ? m[1] : null;
        if (!id) {
            var el = document.getElementById('watch-main');
            if (el) id = el.getAttribute('data-id');
        }
        if (!id) return JSON.stringify({status:0,result:""});
        var xhr = new XMLHttpRequest();
        xhr.open('GET', '/ajax/episode/list/' + id, false);
        xhr.setRequestHeader('X-Requested-With', 'XMLHttpRequest');
        xhr.send(null);
        if (xhr.status === 200) {
            return xhr.responseText;
        }
        return JSON.stringify({status:0,result:""});
    } catch (e) {
        return JSON.stringify({status:0,result:""});
    }
})()"#;

/// Extracts title (preferring `data-jp`), absolute URL, and slug from `a.name.d-title`.
fn extract_card_info(card: scraper::ElementRef, name_sel: &Selector) -> Option<(String, String, String)> {
    let anchor = card.select(name_sel).next()?;
    let href = anchor.value().attr("href")?;
    let url = abs(href.trim());
    let slug = slug_from_url(&url);
    if slug.is_empty() {
        return None;
    }
    let title = anchor
        .value()
        .attr("data-jp")
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
        .or_else(|| {
            let t = anchor.text().collect::<String>();
            let trimmed = t.trim();
            if !trimmed.is_empty() {
                Some(trimmed.to_string())
            } else {
                None
            }
        })
        .unwrap_or_else(|| slug.clone());
    Some((title, url, slug))
}

impl SiteAdapter for AniwavesAdapter {
    fn airing_url(&self, base_url: &str) -> String {
        format!("{}/updated", base_url.trim_end_matches('/'))
    }

    fn parse_airing(&self, html: &str) -> Result<Vec<Series>> {
        let doc = Html::parse_document(html);
        let card_sel = Selector::parse(CARD).unwrap();
        let name_sel = Selector::parse("a.name.d-title").unwrap();
        let img_sel = Selector::parse("img").unwrap();
        let total_sel = Selector::parse(".ep-status.total span").unwrap();

        let mut out = Vec::new();
        for card in doc.select(&card_sel) {
            let Some((title, url, slug)) = extract_card_info(card, &name_sel) else {
                continue;
            };

            let cover_url = card
                .select(&img_sel)
                .next()
                .and_then(|i| i.value().attr("src"))
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(abs);

            let site_episode_count = card
                .select(&total_sel)
                .next()
                .map(|el| el.text().collect::<String>())
                .and_then(|s| s.trim().parse::<i64>().ok());

            out.push(Series {
                id: 0,
                slug,
                title,
                url,
                cover_url,
                is_airing: true,
                followed: false,
                next_episode_at: None,
                site_episode_count,
            });
        }
        Ok(out)
    }

    fn parse_series(&self, json: &str) -> Result<Vec<Episode>> {
        let parsed: serde_json::Value = match serde_json::from_str(json) {
            Ok(v) => v,
            Err(_) => return Ok(vec![]),
        };

        if parsed.get("status").and_then(|s| s.as_i64()) != Some(200) {
            return Ok(vec![]);
        }

        let Some(result_html) = parsed.get("result").and_then(|r| r.as_str()) else {
            return Ok(vec![]);
        };

        if result_html.trim().is_empty() {
            return Ok(vec![]);
        }

        let doc = Html::parse_document(result_html);
        let ep_sel = Selector::parse("ul.ep-range li a").unwrap();

        let mut out = Vec::new();
        for a in doc.select(&ep_sel) {
            let Some(num_attr) = a.value().attr("data-num") else {
                continue;
            };
            let number = num_attr.trim().to_string();
            if number.is_empty() {
                continue;
            }

            let Some(href) = a.value().attr("href") else {
                continue;
            };
            let url = abs(href.trim());

            let released_at = a
                .value()
                .attr("data-aired")
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(String::from);

            out.push(Episode {
                id: 0,
                series_id: 0,
                number,
                title: None,
                url,
                released_at,
                seen: false,
                download_path: None,
                download_status: None,
            });
        }

        Ok(out)
    }

    fn parse_series_detail(&self, html: &str) -> Result<SeriesDetail> {
        let doc = Html::parse_document(html);
        let genre_sel = Selector::parse("#w-info .bmeta a[href*='/genre/']").unwrap();
        let type_sel = Selector::parse("#w-info .bmeta a[href*='/type/']").unwrap();
        let synopsis_sel = Selector::parse("#w-info .synopsis .content").unwrap();

        let mut genres = Vec::new();
        for a in doc.select(&genre_sel) {
            let name = a.text().collect::<String>().trim().to_string();
            if !name.is_empty() && !genres.contains(&name) {
                genres.push(name);
            }
        }

        let kind = doc
            .select(&type_sel)
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
        format!("{}/filter?keyword={encoded}", base_url.trim_end_matches('/'))
    }

    fn parse_search_results(&self, html: &str) -> Result<Vec<FinishedCard>> {
        let doc = Html::parse_document(html);
        let container_sel = Selector::parse(LIST_ITEMS_CONTAINER).unwrap();
        if doc.select(&container_sel).next().is_none() {
            return Err(anyhow!(
                "no #list-items container found; not a recognizable search page"
            ));
        }

        let card_sel = Selector::parse(CARD).unwrap();
        let name_sel = Selector::parse("a.name.d-title").unwrap();
        let img_sel = Selector::parse("img").unwrap();
        let kind_sel = Selector::parse(".meta .right").unwrap();

        let mut out = Vec::new();
        for card in doc.select(&card_sel) {
            let Some((title, url, _)) = extract_card_info(card, &name_sel) else {
                continue;
            };

            let poster_url = card
                .select(&img_sel)
                .next()
                .and_then(|i| i.value().attr("src"))
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(abs);

            let kind = text_of(card, &kind_sel).unwrap_or_default();

            out.push(FinishedCard {
                title,
                url,
                poster_url,
                kind,
                matched_genre: None,
            });
        }
        Ok(out)
    }

    fn episode_fetch_script(&self) -> Option<&'static str> {
        Some(EPISODE_FETCH_SCRIPT)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_url_encodes_query() {
        let a = AniwavesAdapter;
        assert_eq!(
            a.search_url("https://aniwaves.ru/", "Shingeki no Kyojin"),
            "https://aniwaves.ru/filter?keyword=Shingeki+no+Kyojin"
        );
        assert_eq!(
            a.search_url("https://aniwaves.ru", "naruto"),
            "https://aniwaves.ru/filter?keyword=naruto"
        );
    }

    #[test]
    fn airing_url_uses_updated() {
        let a = AniwavesAdapter;
        assert_eq!(a.airing_url("https://aniwaves.ru"), "https://aniwaves.ru/updated");
        assert_eq!(a.airing_url("https://aniwaves.ru/"), "https://aniwaves.ru/updated");
    }

    #[test]
    fn parses_search_hits_fixture() {
        let html = include_str!("../../tests/fixtures/aniwaves_search_hits.html");
        let out = AniwavesAdapter.parse_search_results(html).unwrap();
        // The fixture contains 13 search hit cards inside #list-items (matching the page's "13 Items" counter).
        assert_eq!(out.len(), 13, "the haikyuu search fixture contains 13 result cards in #list-items");

        // Prefer data-jp romaji title:
        let main = out.iter().find(|c| c.url == "https://aniwaves.ru/watch/haikyuu-78821");
        assert!(main.is_some(), "haikyuu-78821 must be present in search hits");
        assert_eq!(main.unwrap().title, "Haikyuu!!");
        assert_eq!(main.unwrap().kind, "TV");
        assert!(main.unwrap().poster_url.as_deref().unwrap().starts_with("https://static.aniwaves.ru/"));

        // A card whose url ends with /watch/haikyuu-to-the-top-part-2-82587:
        let top2 = out.iter().find(|c| c.url.ends_with("/watch/haikyuu-to-the-top-part-2-82587"));
        assert!(top2.is_some(), "haikyuu-to-the-top-part-2-82587 must be present");
        assert_eq!(top2.unwrap().title, "Haikyuu!! To the Top Part 2");

        for card in &out {
            assert!(
                card.url.starts_with("https://aniwaves.ru/watch/"),
                "url not absolute: {}",
                card.url
            );
            assert!(!card.title.is_empty());
        }
    }

    #[test]
    fn parses_search_empty_fixture_as_zero_results() {
        let html = include_str!("../../tests/fixtures/aniwaves_search_empty.html");
        let out = AniwavesAdapter.parse_search_results(html).unwrap();
        assert!(out.is_empty(), "empty search page should parse to Ok(vec![])");
    }

    #[test]
    fn parse_search_results_errs_on_unrecognizable_page() {
        let err = AniwavesAdapter.parse_search_results("<html><body>not this site</body></html>");
        assert!(err.is_err(), "page lacking #list-items must return Err");
    }

    #[test]
    fn parses_episodes_json_fixture() {
        let json = include_str!("../../tests/fixtures/aniwaves_episodes.json");
        let out = AniwavesAdapter.parse_series(json).unwrap();
        assert_eq!(out.len(), 25);

        let first = &out[0];
        assert_eq!(first.number, "1");
        assert_eq!(first.title, None);
        assert_eq!(first.url, "https://aniwaves.ru/watch/78821/ep-1");
        assert_eq!(first.released_at.as_deref(), Some("2014-04-06T00:00:00+09:00"));
        assert!(!first.seen);

        let last = &out[24];
        assert_eq!(last.number, "25");
        assert_eq!(last.url, "https://aniwaves.ru/watch/78821/ep-25");
    }

    #[test]
    fn parse_series_handles_malformed_and_error_json_gracefully() {
        let a = AniwavesAdapter;
        assert_eq!(a.parse_series("").unwrap(), vec![]);
        assert_eq!(a.parse_series("not json").unwrap(), vec![]);
        assert_eq!(a.parse_series("{\"status\":500}").unwrap(), vec![]);
        assert_eq!(a.parse_series("{\"status\":200,\"result\":\"\"}").unwrap(), vec![]);
        assert_eq!(a.parse_series("{\"status\":200,\"result\":\"<div>nothing</div>\"}").unwrap(), vec![]);
    }

    #[test]
    fn parse_series_keeps_fractional_episode_number() {
        let json = r#"{"status":200,"result":"<ul class=\"ep-range\"><li><a href=\"/watch/78821/ep-13.5\" data-num=\"13.5\" data-aired=\"2014-07-01\"></a></li></ul>"}"#;
        let out = AniwavesAdapter.parse_series(json).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].number, "13.5");
        assert_eq!(out[0].url, "https://aniwaves.ru/watch/78821/ep-13.5");
        assert_eq!(out[0].released_at.as_deref(), Some("2014-07-01"));
    }

    #[test]
    fn parses_airing_fixture() {
        let html = include_str!("../../tests/fixtures/aniwaves_updated.html");
        let out = AniwavesAdapter.parse_airing(html).unwrap();
        assert!(!out.is_empty(), "airing listing must not be empty");

        for s in &out {
            assert!(!s.url.is_empty());
            assert!(
                s.url.starts_with("https://aniwaves.ru/watch/"),
                "url not absolute: {}",
                s.url
            );
            assert!(!s.slug.is_empty());
            assert!(!s.title.is_empty());
            assert!(s.is_airing);
            assert_eq!(s.next_episode_at, None);
        }

        let first = &out[0];
        assert_eq!(first.slug, "umayuru-full-gate-82755");
        assert_eq!(first.title, "Umayuru: Full Gate!");
        assert!(first.cover_url.as_deref().unwrap().contains("4fe22f109a7430d87c16e4f7b39b2c7c.jpg"));
        assert_eq!(first.site_episode_count, Some(26));

        // Second series has "?" total badge, which must parse to None
        let second = &out[1];
        assert_eq!(second.slug, "yozakura-san-chi-no-daisakusen-2nd-season-part-2-82816");
        assert_eq!(second.title, "Yozakura-san Chi no Daisakusen 2nd Season Part 2");
        assert_eq!(second.site_episode_count, None);
    }

    #[test]
    fn parses_series_detail_fixture() {
        let html = include_str!("../../tests/fixtures/aniwaves_series.html");
        let d = AniwavesAdapter.parse_series_detail(html).unwrap();
        assert!(
            d.genres.contains(&"Sports".to_string()),
            "genres must contain Sports: {:?}",
            d.genres
        );
        assert_eq!(d.genres, vec!["Sports", "School", "Team Sports", "Shounen"]);
        assert_eq!(d.kind.as_deref(), Some("TV"));
        let synopsis = d.synopsis.expect("synopsis should be present");
        assert!(!synopsis.is_empty());
        assert!(synopsis.contains("Little Giant"));
    }

    #[test]
    fn episode_fetch_script_is_present_and_synchronous() {
        let script = AniwavesAdapter.episode_fetch_script();
        assert!(script.is_some());
        let s = script.unwrap();
        assert!(s.contains("ajax/episode/list/"));
        assert!(s.contains("false"), "XHR must be synchronous");
    }

    #[test]
    fn genre_archive_methods_use_the_trait_defaults() {
        let a = AniwavesAdapter;
        assert_eq!(a.genre_list_url("https://aniwaves.ru"), "");
        assert_eq!(a.parse_genre_list("<html></html>").unwrap(), Vec::<(String, String)>::new());
        assert_eq!(a.genre_page_url("https://aniwaves.ru", "action", 1), "");
        assert_eq!(a.parse_finished_page("<html></html>").unwrap(), Vec::<FinishedCard>::new());
        assert_eq!(a.parse_pagination_last_page("<html></html>"), 1);
    }
}
