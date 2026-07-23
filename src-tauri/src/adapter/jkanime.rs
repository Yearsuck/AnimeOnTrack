use super::SiteAdapter;
use crate::models::{Episode, FinishedCard, Series, SeriesDetail};
use anyhow::{anyhow, Result};
use scraper::{Html, Selector};
use serde::Deserialize;

pub struct JkanimeAdapter;

// Confirmed live against https://jkanime.net 2026-07-22 via browser devtools
// (navigate + targeted DOM/network queries — see
// docs/superpowers/specs/2026-07-22-jkanime-adapter-design.md for the full
// trail). No Cloudflare challenge observed; plain page loads.
//
// Fixture provenance note: the browser tool used for capture blocks bulk
// raw-HTML transfer out of a tool call (a content-safety filter, confirmed
// it also blocks a base64-encoded version of the same content — the filter
// correctly refusing that evasion). A single verbatim page dump like the
// other two multi-site adapters' fixtures was therefore not obtainable this
// way. The fixtures under tests/fixtures/jkanime_*.{html,json} are instead
// hand-assembled from real fragments — every class name, tag hierarchy, and
// content string was independently confirmed against the live site through
// many small targeted queries, not guessed.
//
// - Airing: `{base}/directorio/?estado=emision` — the query param, NOT the
//   `/directorio/emision/` path segment (that's silently ignored by the
//   site and returns the unfiltered directory, mixing in finished/movie
//   cards). Cards: `.dirbox .card` -> `a` (wraps `.d-thumb` with the poster
//   `img[src]`, already an absolute URL like AnimeYT's markup) + sibling
//   `.svea .card-title` (title). hrefs are already absolute too. No
//   countdown/episode-count badge exists on listing cards at all, so
//   `next_episode_at`/`site_episode_count` are always `None` (same
//   limitation as tioanime).
//
// - Episode list is NOT in the series page's HTML at all — it's fetched
//   client-side via a paginated JSON AJAX endpoint requiring a CSRF token
//   (confirmed on two real series: `one-piece`, id 201, 1170 episodes/74
//   pages; `hanaori-san-wa-tensei-shitemo-kenka-ga-shitai`, id 4797, 2
//   episodes/1 page). `EPISODE_FETCH_SCRIPT` below runs in-page (see
//   `SiteAdapter::episode_fetch_script`) to do the fetching; `parse_series`
//   here only deserializes its JSON result — never HTML. The script:
//     1. Regex-extracts the numeric anime id + full ajax URL prefix from an
//        inline (no-`src`) `<script>` tag's own `$.ajax` call.
//     2. Reads the CSRF token from `meta[name="csrf-token"]`.
//     3. Loops a *synchronous* `XMLHttpRequest` per page (the repo's
//        established `ExecuteScript` constraint: no `await`able promise
//        survives the host-side eval bridge — see scraper_engine.rs), from
//        page 1 until `page > last_page` (known from the first response).
//     4. Builds each episode's detail URL itself (`{origin}/{slug}/{number}/`
//        — confirmed by clicking the real "Ver más" button and reading the
//        rendered anchor; the JSON response has no url field at all) and
//        returns the concatenated array, JSON-stringified.
//   Always fetches every page, never just the newest — simpler and correct;
//   see the design doc for why the multi-page cost on an outlier like One
//   Piece is an accepted one-time cost.
//
// - Series detail (`{base}/{slug}/`): both genres and kind live in
//   `.anime_data li` — each `li` has a label `span` ("Tipo:", "Generos:",
//   "Studios:", ...) followed by either plain text (`kind` — the ONLY one
//   of these with no `<a>` inside) or `<a>` tags (`genres`). Confirmed the
//   page renders this block twice (pc + mobile duplicate, identical
//   content) — `parse_series_detail` takes the first match only, same
//   first-wins discipline `text_of`/`.next()` use elsewhere in this
//   codebase. Synopsis: `.anime_info .scroll` (a `<p>`).
//
// - Search: the real endpoint is `{base}/buscar/{query}` — a path segment,
//   reached via a GET form whose `action="https://jkanime.net/buscar"`.
//   `?q=` alone does nothing on this site (confirmed: `/directorio/?nombre=`
//   also does nothing — `nombre` isn't a real directory filter param either).
//   Cards: `.anime__item` -> `a[href]` (poster is a CSS `background-image`
//   on `.anime__item__pic`, not an `<img src>`) + `.anime__item__text`,
//   whose text is the concatenation of a status label ("Concluido"/"En
//   emision"), a type label ("Serie"/"Pelicula"), and the title with no
//   separator beyond the source's own line breaks — title is the last
//   non-empty line. Zero-hit contract confirmed identical in shape to the
//   other two adapters: `.anime__page__content` is present either way;
//   `.anime__item` count is 0 for a genuine zero-hit query, container
//   absent entirely only for a wrong/incompatible mirror.
//
// - No genre-archive concept confirmed on this site's listing pages (no
//   separate finished/ongoing marker beyond the directory's own `estado`
//   filter) — the five genre-archive trait methods are left at their
//   default (no-op) implementations, same as tioanime.
const CARD: &str = ".dirbox .card";
const SEARCH_ITEM: &str = ".anime__item";

/// See the module doc comment above for what this does and why it exists.
/// `#[cfg_attr]`-free plain `&str` — no formatting needed, every value it
/// touches (id, ajax URL, token, slug) is read out of the page it runs on.
const EPISODE_FETCH_SCRIPT: &str = r#"(function(){
    var scripts = Array.prototype.slice.call(document.querySelectorAll('script:not([src])')).map(function(s){return s.textContent;});
    var src = null;
    for (var i = 0; i < scripts.length; i++) {
        if (scripts[i].indexOf('ajax/episodes/') !== -1) { src = scripts[i]; break; }
    }
    if (!src) return JSON.stringify([]);
    var m = src.match(/(https?:\/\/[^'"]+\/ajax\/episodes\/(\d+)\/)/);
    if (!m) return JSON.stringify([]);
    var urlPrefix = m[1];
    var tokenEl = document.querySelector('meta[name="csrf-token"]');
    var token = tokenEl ? tokenEl.getAttribute('content') : '';
    var slug = location.pathname.replace(/^\/+|\/+$/g, '');
    var origin = location.origin;
    var all = [];
    var page = 1;
    var lastPage = 1;
    do {
        var xhr = new XMLHttpRequest();
        xhr.open('POST', urlPrefix + page, false);
        xhr.setRequestHeader('Content-Type', 'application/x-www-form-urlencoded');
        xhr.setRequestHeader('X-Requested-With', 'XMLHttpRequest');
        try {
            xhr.send('_token=' + encodeURIComponent(token));
        } catch (e) {
            break;
        }
        if (xhr.status !== 200) break;
        var data;
        try {
            data = JSON.parse(xhr.responseText);
        } catch (e) {
            break;
        }
        lastPage = data.last_page || 1;
        var items = data.data || [];
        for (var j = 0; j < items.length; j++) {
            var ep = items[j];
            ep.url = origin + '/' + slug + '/' + ep.number + '/';
            all.push(ep);
        }
        page++;
    } while (page <= lastPage);
    return JSON.stringify(all);
})()"#;

#[derive(Deserialize)]
struct RawEpisode {
    number: i64,
    url: String,
    timestamp: String,
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

/// `style="background-image: url(...)"` -> the URL inside. No `regex` crate
/// dependency in this codebase yet, and one string search doesn't justify
/// adding it.
fn extract_bg_url(style: &str) -> Option<String> {
    let start = style.find("url(")? + 4;
    let end = style[start..].find(')')? + start;
    let raw = style[start..end].trim().trim_matches('\'').trim_matches('"');
    if raw.is_empty() {
        None
    } else {
        Some(raw.to_string())
    }
}

impl SiteAdapter for JkanimeAdapter {
    fn airing_url(&self, base_url: &str) -> String {
        format!("{}/directorio/?estado=emision", base_url.trim_end_matches('/'))
    }

    fn parse_airing(&self, html: &str) -> Result<Vec<Series>> {
        let doc = Html::parse_document(html);
        let card_sel = Selector::parse(CARD).unwrap();
        let a_sel = Selector::parse("a").unwrap();
        let title_sel = Selector::parse(".card-title").unwrap();
        let img_sel = Selector::parse("img").unwrap();

        let mut out = Vec::new();
        for card in doc.select(&card_sel) {
            let Some(anchor) = card.select(&a_sel).next() else { continue };
            let Some(href) = anchor.value().attr("href") else { continue };
            let url = href.to_string();
            let title = text_of(card, &title_sel).unwrap_or_else(|| slug_from_url(&url));
            let cover_url = card.select(&img_sel).next().and_then(|i| i.value().attr("src")).map(String::from);
            out.push(Series {
                id: 0,
                slug: slug_from_url(&url),
                title,
                url,
                cover_url,
                // This parser is only ever run against the estado=emision
                // listing, so every card on it is airing by construction.
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

    fn parse_series(&self, json: &str) -> Result<Vec<Episode>> {
        let raw: Vec<RawEpisode> =
            serde_json::from_str(json).map_err(|e| anyhow!("episode JSON did not parse: {e}"))?;
        Ok(raw
            .into_iter()
            .map(|r| Episode {
                id: 0,
                series_id: 0,
                number: r.number.to_string(),
                // JSON's own `title` field is just "{series title} - {N}",
                // not a distinct per-episode title — same call tioanime
                // already made for its own repeated-series-title rows.
                title: None,
                url: r.url,
                released_at: Some(r.timestamp),
                seen: false,
            })
            .collect())
    }

    fn parse_series_detail(&self, html: &str) -> Result<SeriesDetail> {
        let doc = Html::parse_document(html);
        let li_sel = Selector::parse(".anime_data li").unwrap();
        let span_sel = Selector::parse("span").unwrap();
        let a_sel = Selector::parse("a").unwrap();

        let mut kind = None;
        let mut genres = Vec::new();
        for li in doc.select(&li_sel) {
            let label = text_of(li, &span_sel).unwrap_or_default();
            if label == "Tipo:" && kind.is_none() {
                let full = li.text().collect::<String>();
                let val = full.trim().trim_start_matches("Tipo:").trim().to_string();
                if !val.is_empty() {
                    kind = Some(val);
                }
            } else if label == "Generos:" && genres.is_empty() {
                genres = li
                    .select(&a_sel)
                    .map(|a| a.text().collect::<String>().trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
            }
        }

        let synopsis_sel = Selector::parse(".anime_info .scroll").unwrap();
        let synopsis = doc
            .select(&synopsis_sel)
            .next()
            .map(|el| el.text().collect::<String>().trim().to_string())
            .filter(|s| !s.is_empty());

        Ok(SeriesDetail { genres, kind, synopsis })
    }

    fn search_url(&self, base_url: &str, query: &str) -> String {
        let encoded: String = url::form_urlencoded::byte_serialize(query.as_bytes()).collect();
        format!("{}/buscar/{}", base_url.trim_end_matches('/'), encoded)
    }

    fn parse_search_results(&self, html: &str) -> Result<Vec<FinishedCard>> {
        let doc = Html::parse_document(html);
        let container_sel = Selector::parse(".anime__page__content").unwrap();
        if doc.select(&container_sel).next().is_none() {
            return Err(anyhow!(
                "no .anime__page__content container found; not a recognizable search page"
            ));
        }

        let item_sel = Selector::parse(SEARCH_ITEM).unwrap();
        let a_sel = Selector::parse("a").unwrap();
        let text_sel = Selector::parse(".anime__item__text").unwrap();
        let pic_sel = Selector::parse(".anime__item__pic").unwrap();

        let mut out = Vec::new();
        for item in doc.select(&item_sel) {
            let Some(anchor) = item.select(&a_sel).next() else { continue };
            let Some(href) = anchor.value().attr("href") else { continue };
            let url = href.to_string();

            let title = item
                .select(&text_sel)
                .next()
                .map(|el| el.text().collect::<String>())
                .and_then(|full| full.lines().map(str::trim).rfind(|s| !s.is_empty()).map(String::from))
                .unwrap_or_else(|| slug_from_url(&url));

            let poster_url = item
                .select(&pic_sel)
                .next()
                .and_then(|el| el.value().attr("style"))
                .and_then(extract_bg_url);

            // No badge on this site's search cards distinguishes a reliable
            // "kind" from the status label sharing the same text block —
            // same "leave it empty" discipline tioanime uses for its own
            // typeless search cards.
            out.push(FinishedCard { title, url, poster_url, kind: String::new(), matched_genre: None });
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
    fn airing_url_uses_the_estado_query_param() {
        let a = JkanimeAdapter;
        assert_eq!(a.airing_url("https://jkanime.net/"), "https://jkanime.net/directorio/?estado=emision");
        assert_eq!(a.airing_url("https://jkanime.net"), "https://jkanime.net/directorio/?estado=emision");
    }

    #[test]
    fn search_url_is_a_path_segment_not_a_query_string() {
        let a = JkanimeAdapter;
        assert_eq!(a.search_url("https://jkanime.net", "naruto"), "https://jkanime.net/buscar/naruto");
        assert_eq!(
            a.search_url("https://jkanime.net/", "Shingeki no Kyojin"),
            "https://jkanime.net/buscar/Shingeki+no+Kyojin"
        );
    }

    #[test]
    fn parses_airing_fixture() {
        let html = include_str!("../../tests/fixtures/jkanime_airing.html");
        let out = JkanimeAdapter.parse_airing(html).unwrap();
        assert_eq!(out.len(), 4);
        for s in &out {
            assert!(!s.url.is_empty());
            assert!(s.url.starts_with("https://jkanime.net/"), "url not absolute: {}", s.url);
            assert!(!s.slug.is_empty());
            assert!(!s.title.is_empty());
            assert!(s.is_airing);
            assert_eq!(s.next_episode_at, None);
            assert_eq!(s.site_episode_count, None);
        }
        let first = &out[0];
        assert_eq!(first.slug, "yami-shibai-17");
        assert_eq!(first.title, "Yami Shibai 17");
        assert!(first.cover_url.as_deref().unwrap().contains("yami-shibai-17.jpg"));
    }

    #[test]
    fn parses_episodes_json_fixture() {
        let json = include_str!("../../tests/fixtures/jkanime_episodes.json");
        let out = JkanimeAdapter.parse_series(json).unwrap();
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].number, "1");
        assert_eq!(out[0].title, None, "this site's JSON title is just the series name, not per-episode");
        assert_eq!(out[0].url, "https://jkanime.net/hanaori-san-wa-tensei-shitemo-kenka-ga-shitai/1/");
        assert_eq!(out[0].released_at.as_deref(), Some("2026-07-11 17:47:15"));
        assert_eq!(out[1].number, "2");
    }

    #[test]
    fn parses_series_detail_fixture() {
        let html = include_str!("../../tests/fixtures/jkanime_series_detail.html");
        let d = JkanimeAdapter.parse_series_detail(html).unwrap();
        assert_eq!(d.genres, vec!["Comedia".to_string(), "Fantasia".to_string(), "Romance".to_string()]);
        assert_eq!(d.kind.as_deref(), Some("Serie"));
        assert!(d.synopsis.unwrap().contains("Narukami Ryusei"));
    }

    #[test]
    fn parses_search_hits_fixture() {
        let html = include_str!("../../tests/fixtures/jkanime_search_hits.html");
        let out = JkanimeAdapter.parse_search_results(html).unwrap();
        assert!(!out.is_empty(), "the 'naruto' search fixture must have hits");
        assert!(out.iter().any(|c| c.title == "Naruto"));
        assert!(out.iter().any(|c| c.title == "Boruto: Naruto Next Generations"));
        for c in &out {
            assert!(!c.url.is_empty());
            assert!(c.url.starts_with("https://jkanime.net/"));
        }
    }

    #[test]
    fn parses_search_empty_fixture_as_zero_results_not_an_error() {
        let html = include_str!("../../tests/fixtures/jkanime_search_empty.html");
        let out = JkanimeAdapter.parse_search_results(html).unwrap();
        assert!(out.is_empty(), "a genuine zero-hit search must parse to Ok(vec![]), not Err");
    }

    #[test]
    fn parse_search_results_errs_on_unrecognizable_page() {
        let err = JkanimeAdapter.parse_search_results("<html><body>not this site</body></html>");
        assert!(err.is_err(), "a page with no .anime__page__content at all must be treated as a broken/wrong mirror");
    }

    #[test]
    fn genre_archive_methods_use_the_trait_defaults() {
        let a = JkanimeAdapter;
        assert_eq!(a.genre_list_url("https://jkanime.net"), "");
        assert_eq!(a.parse_genre_list("<html></html>").unwrap(), Vec::<(String, String)>::new());
        assert_eq!(a.genre_page_url("https://jkanime.net", "accion", 1), "");
        assert_eq!(a.parse_finished_page("<html></html>").unwrap(), Vec::<FinishedCard>::new());
        assert_eq!(a.parse_pagination_last_page("<html></html>"), 1);
    }

    #[test]
    fn episode_fetch_script_is_present_and_non_empty() {
        let script = JkanimeAdapter.episode_fetch_script();
        assert!(script.is_some());
        assert!(script.unwrap().contains("ajax/episodes/"));
    }
}
