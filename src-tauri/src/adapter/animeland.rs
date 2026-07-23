use super::SiteAdapter;
use crate::models::{Episode, FinishedCard, Series, SeriesDetail};
use anyhow::{anyhow, Result};
use scraper::{Html, Selector};

pub struct AnimelandAdapter;

// Selectors confirmed against real captured HTML (src-tauri/tests/fixtures)
// live 2026-07-23 against https://w7.animeland.tv (animeland.tv redirects
// there) — see docs/superpowers/specs/2026-07-23-animeland-adapter-design.md.
//
// A different WordPress "video post" theme, not DooPlay — `.new_added`,
// `.gallery`, `.anime-col`, `.video_wrapper_container`.
//
// - Poster image `src` is relative (`/Thumbs/{name}.jpg`, `/ac/{name}.jpg`)
//   while series/episode links are already absolute — same situation and
//   same fix as `tioanime.rs`: resolve against the fixed `BASE` constant
//   below (the default mirror), not whatever mirror actually served the
//   page. Same accepted limitation: a second animeland mirror, if ever
//   configured, would still resolve images against `w7`.
// - Airing listing is the homepage (`{base}/`); `.new_added` cards
//   ("recently updated series") are confirmed to have zero overlap with the
//   unrelated `.sidebar_right` A-Z catalog list appended to every page on
//   this site (89 `/category/` links there alone on a real capture — a trap
//   for any selector broader than `.new_added`). No episode-count or
//   release-date signal on these cards at all.
// - Series page episodes are `.anime-col li.play a` (confirmed scoped away
//   from the same sidebar) — text "Episode N", digit-extracted. No
//   per-episode title or date exist on this site.
// - Synopsis is `.video_wrapper_container`'s text. Do NOT widen this to the
//   parent `.entry-content` — that element also contains an inline `<style>`
//   block whose raw CSS text (`.video-block{margin-right:15px!important;...}`)
//   would leak into the "synopsis", confirmed live.
// - No genre or kind/type data exists anywhere on this site (checked
//   thoroughly) — `genres`/`kind` are always `vec![]`/`None`, an honest site
//   limitation, not a missed selector.
// - Search zero-hit contract DIFFERS from every other adapter: `.gallery`
//   is entirely ABSENT (not present-but-empty) for a genuine zero-hit
//   search, confirmed live. The wrong-mirror sanity check therefore keys off
//   `.entry-content` (WP boilerplate present on every real page, hit or
//   empty) instead of the results container itself.
// - No genre-archive concept confirmed on this site — the five
//   genre-archive trait methods are left at their default (no-op)
//   implementations, same as tioanime/jkanime/gogoanime.
const BASE: &str = "https://w7.animeland.tv";
const AIRING_CARD: &str = ".new_added";

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

/// Trailing run of ASCII digits in `s`, e.g. "Episode 12" -> "12". Falls
/// back to the whole trimmed string if there are no digits at all, mirroring
/// the never-panic discipline the other adapters' own digit helpers use.
fn digits_in(s: &str) -> String {
    let digits: String = s.chars().filter(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        s.trim().to_string()
    } else {
        digits
    }
}

impl SiteAdapter for AnimelandAdapter {
    fn airing_url(&self, base_url: &str) -> String {
        format!("{}/", base_url.trim_end_matches('/'))
    }

    fn parse_airing(&self, html: &str) -> Result<Vec<Series>> {
        let doc = Html::parse_document(html);
        let card_sel = Selector::parse(AIRING_CARD).unwrap();
        let a_sel = Selector::parse("a").unwrap();
        let img_sel = Selector::parse("img").unwrap();

        let mut out = Vec::new();
        for card in doc.select(&card_sel) {
            let Some(anchor) = card.select(&a_sel).next() else { continue };
            let Some(href) = anchor.value().attr("href") else { continue };
            let url = abs(href);
            let title = anchor.text().collect::<String>().trim().to_string();
            let title = if title.is_empty() { slug_from_url(&url) } else { title };
            let cover_url = card.select(&img_sel).next().and_then(|i| i.value().attr("src")).map(abs);
            out.push(Series {
                id: 0,
                slug: slug_from_url(&url),
                title,
                url,
                cover_url,
                is_airing: true,
                followed: false,
                // No episode-count/release-date signal on this site's
                // homepage cards at all — see the module doc comment.
                next_episode_at: None,
                site_episode_count: None,
            });
        }
        Ok(out)
    }

    fn parse_series(&self, html: &str) -> Result<Vec<Episode>> {
        let doc = Html::parse_document(html);
        let item_sel = Selector::parse(".anime-col li.play").unwrap();
        let a_sel = Selector::parse("a").unwrap();

        let mut out = Vec::new();
        for item in doc.select(&item_sel) {
            let Some(anchor) = item.select(&a_sel).next() else { continue };
            let Some(href) = anchor.value().attr("href") else { continue };
            let url = abs(href);
            let number = digits_in(&anchor.text().collect::<String>());
            let number = if number.is_empty() { slug_from_url(&url) } else { number };
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
        // No genre or kind/type data exists anywhere on this site — see the
        // module doc comment. Synopsis: `.video_wrapper_container`'s text,
        // deliberately NOT the parent `.entry-content` (that pulls in an
        // inline <style> block's raw CSS text).
        let synopsis_sel = Selector::parse(".video_wrapper_container").unwrap();
        let synopsis = doc
            .select(&synopsis_sel)
            .next()
            .map(|el| el.text().collect::<String>().trim().to_string())
            .filter(|s| !s.is_empty());

        Ok(SeriesDetail { genres: vec![], kind: None, synopsis })
    }

    fn search_url(&self, base_url: &str, query: &str) -> String {
        let encoded: String = url::form_urlencoded::byte_serialize(query.as_bytes()).collect();
        format!("{}/?s={}", base_url.trim_end_matches('/'), encoded)
    }

    fn parse_search_results(&self, html: &str) -> Result<Vec<FinishedCard>> {
        let doc = Html::parse_document(html);
        // `.entry-content` is WP boilerplate present on every real page of
        // this site, hit or empty — used purely as the "is this even an
        // animeland page" sanity check. Unlike every other adapter, the
        // actual results container (`.gallery`) is legitimately ABSENT
        // (not present-but-empty) on a genuine zero-hit search, confirmed
        // live — so its absence must NOT be treated as a wrong-mirror error.
        let sanity_sel = Selector::parse(".entry-content").unwrap();
        if doc.select(&sanity_sel).next().is_none() {
            return Err(anyhow!("no .entry-content found; not a recognizable animeland page"));
        }

        let card_sel = Selector::parse(".gallery").unwrap();
        let title_sel = Selector::parse(".title a").unwrap();
        let img_sel = Selector::parse("img").unwrap();

        let mut out = Vec::new();
        for card in doc.select(&card_sel) {
            let Some(anchor) = card.select(&title_sel).next() else { continue };
            let Some(href) = anchor.value().attr("href") else { continue };
            let url = abs(href);
            let title = anchor.text().collect::<String>().trim().to_string();
            let title = if title.is_empty() { slug_from_url(&url) } else { title };
            let poster_url = card.select(&img_sel).next().and_then(|i| i.value().attr("src")).map(abs);
            // No type/kind badge on this site's search cards.
            out.push(FinishedCard { title, url, poster_url, kind: String::new(), matched_genre: None });
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn airing_url_is_the_homepage() {
        let a = AnimelandAdapter;
        assert_eq!(a.airing_url("https://w7.animeland.tv/"), "https://w7.animeland.tv/");
        assert_eq!(a.airing_url("https://w7.animeland.tv"), "https://w7.animeland.tv/");
    }

    #[test]
    fn search_url_is_urlencoded_query_string() {
        let a = AnimelandAdapter;
        assert_eq!(a.search_url("https://w7.animeland.tv", "naruto"), "https://w7.animeland.tv/?s=naruto");
        assert_eq!(
            a.search_url("https://w7.animeland.tv/", "Shingeki no Kyojin"),
            "https://w7.animeland.tv/?s=Shingeki+no+Kyojin"
        );
    }

    #[test]
    fn parses_airing_fixture() {
        let html = include_str!("../../tests/fixtures/animeland_airing.html");
        let out = AnimelandAdapter.parse_airing(html).unwrap();
        assert_eq!(out.len(), 3);
        for s in &out {
            assert!(!s.url.is_empty());
            assert!(s.url.starts_with("https://w7.animeland.tv/category/"), "url not absolute: {}", s.url);
            assert!(!s.slug.is_empty());
            assert!(!s.title.is_empty());
            assert!(s.is_airing);
            assert_eq!(s.next_episode_at, None);
            assert_eq!(s.site_episode_count, None);
        }
        let first = &out[0];
        assert_eq!(first.slug, "re-monster");
        assert_eq!(first.title, "Re Monster");
        assert_eq!(first.cover_url.as_deref(), Some("https://w7.animeland.tv/Thumbs/Re-Monster.jpg?xxx"));
    }

    #[test]
    fn parses_series_fixture() {
        let html = include_str!("../../tests/fixtures/animeland_series.html");
        let out = AnimelandAdapter.parse_series(html).unwrap();
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].number, "2");
        assert_eq!(out[0].url, "https://w7.animeland.tv/re-monster-episode-2-dubbed");
        assert_eq!(out[0].title, None, "this site has no per-episode title");
        assert_eq!(out[0].released_at, None, "this site shows no release date");
        assert_eq!(out[1].number, "1");
    }

    #[test]
    fn parses_series_detail_fixture() {
        let html = include_str!("../../tests/fixtures/animeland_series.html");
        let d = AnimelandAdapter.parse_series_detail(html).unwrap();
        assert_eq!(d.genres, Vec::<String>::new(), "no genre concept on this site");
        assert_eq!(d.kind, None, "no kind/type concept on this site");
        let synopsis = d.synopsis.unwrap();
        assert!(synopsis.contains("Tomokui Kanata"));
        assert!(!synopsis.contains("video-block"), "the sibling <style> block's CSS text must not leak in");
    }

    #[test]
    fn parses_search_hits_fixture() {
        let html = include_str!("../../tests/fixtures/animeland_search_hits.html");
        let out = AnimelandAdapter.parse_search_results(html).unwrap();
        assert_eq!(out.len(), 3);
        assert!(out.iter().any(|c| c.title == "Naruto Shippuuden Movie 1"));
        for c in &out {
            assert!(c.url.starts_with("https://w7.animeland.tv/category/"));
        }
    }

    #[test]
    fn parses_search_empty_fixture_as_zero_results_not_an_error() {
        // Confirmed live: `.gallery` is entirely absent (not just empty) for
        // a genuine zero-hit search on this site — this must still be
        // Ok(vec![]), not Err, despite the absence.
        let html = include_str!("../../tests/fixtures/animeland_search_empty.html");
        let out = AnimelandAdapter.parse_search_results(html).unwrap();
        assert!(out.is_empty());
    }

    #[test]
    fn parse_search_results_errs_on_unrecognizable_page() {
        let err = AnimelandAdapter.parse_search_results("<html><body>not this site</body></html>");
        assert!(err.is_err(), "a page with no .entry-content at all must be treated as a broken/wrong mirror");
    }

    #[test]
    fn genre_archive_methods_use_the_trait_defaults() {
        let a = AnimelandAdapter;
        assert_eq!(a.genre_list_url("https://w7.animeland.tv"), "");
        assert_eq!(a.parse_genre_list("<html></html>").unwrap(), Vec::<(String, String)>::new());
        assert_eq!(a.genre_page_url("https://w7.animeland.tv", "action", 1), "");
        assert_eq!(a.parse_finished_page("<html></html>").unwrap(), Vec::<FinishedCard>::new());
        assert_eq!(a.parse_pagination_last_page("<html></html>"), 1);
    }
}
