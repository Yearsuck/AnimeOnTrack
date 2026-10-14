use super::{slug_from_url, text_of, SiteAdapter};
use crate::models::{Episode, FinishedCard, Series, SeriesDetail};
use anyhow::Result;
use scraper::{Html, Selector};
use std::collections::{HashMap, HashSet};

pub struct AnimeflvAdapter;

// Selectors confirmed against real captured HTML (src-tauri/tests/fixtures,
// captured live 2026-10-14 against https://animeflv.or.at/ — WordPress-based
// site; the previous www4.animeflv.net custom layout is completely replaced).
//
// - Airing listing: `{base_url}/` (home page). Every page of the site features
//   a left sidebar titled "ANIMES EN EMISIÓN" (`.left-sidebar ul#left-menu`),
//   containing currently airing series (`li.sidebar-cat-item a[href]`, with
//   hrefs like `https://animeflv.or.at/anime/<slug>/`). The series title is in
//   `.sidebar-cat-name` (with fallback to the anchor's `title` attribute, decoded).
//   Additional airing series are stored in a JSON script element on the page,
//   `<script type="application/json" id="left-menu-deferred-cats">[{"name":"...","url":"..."},...]</script>`,
//   which the theme's JS dynamically appends when clicking "Ver más". Both DOM items
//   and deferred items are included in page order, deduplicated by slug.
//   Episode thumbnail covers are extracted from the home page's "Últimos episodios" cards
//   (`.List-Episodes .Episode` and `.List-Episodes .ht_grid_1_4`), which link to
//   `/YYYY/MM/DD/<series-slug>-episodio-<n>/` with an `img` poster. When a card matches
//   a sidebar series slug (by stripping `-episodio-<n>`), its image URL is assigned as
//   `Series.cover_url`. A page lacking the sidebar container is rejected as `Err`.
//
// - Series page: `/anime/<slug>/`. Episodes are not pre-rendered in the static DOM;
//   instead, they reside in a JSON array inside `<script type="application/json" class="animeflv-episodes-data">`:
//   `[{"post_id": 1555, "permalink": "https://animeflv.or.at/YYYY/MM/DD/<ep-slug>/", "number": 3, "range": 1}, ...]`.
//   `Episode.number` is formatted as text without any trailing `.0` (e.g. 3 -> "3", 12.5 -> "12.5").
//   `released_at` is extracted from the `/YYYY/MM/DD/` segment of the permalink URL when present.
//   Duplicates by episode number are discarded (preserving first occurrence). Empty array -> `Ok(vec![])`.
//   Missing script tag -> `Err`.
//
// - Series detail: `/anime/<slug>/`. Genres are extracted from `.anime-genres .genre-tag`.
//   Synopsis is taken from the trimmed text of `.anime-synopsis p`. The WordPress layout
//   does not display a type badge on the series detail page, so `kind` defaults to `None`.
//
// - Search: `{base_url}/?s={urlencoded}`. Results render in `#recent-content.search-series-grid`.
//   Each series result card is a `.search-series-card` wrapping `a.thumbnail-link` with `h2.entry-title`
//   and `img`. Cards are scoped strictly to `.search-series-grid` to ensure sidebar airing items
//   never leak into search results. An empty search result page displays `.no-results` inside
//   `.search-series-grid`, returning `Ok(vec![])`. A page lacking `.search-series-grid` returns `Err`.
//
// - Genre-archive methods: AnimeFLV does not have a separate finished-vs-ongoing genre archive;
//   all five genre-archive methods rely on the trait's default implementations.
const BASE: &str = "https://animeflv.or.at";

fn abs(path: &str) -> String {
    if path.starts_with("http://") || path.starts_with("https://") {
        path.to_string()
    } else if let Some(stripped) = path.strip_prefix('/') {
        format!("{BASE}/{stripped}")
    } else {
        format!("{BASE}/{path}")
    }
}

fn series_slug_from_episode_slug(ep_slug: &str) -> Option<&str> {
    ep_slug.rsplit_once("-episodio-").map(|(series, _num)| series)
}

fn parse_date_from_url(url: &str) -> Option<String> {
    let segments: Vec<&str> = url.split('/').collect();
    for window in segments.windows(3) {
        if let (Some(&year), Some(&month), Some(&day)) = (window.first(), window.get(1), window.get(2)) {
            if year.len() == 4
                && year.chars().all(|c| c.is_ascii_digit())
                && month.len() == 2
                && month.chars().all(|c| c.is_ascii_digit())
                && day.len() == 2
                && day.chars().all(|c| c.is_ascii_digit())
            {
                return Some(format!("{year}-{month}-{day}"));
            }
        }
    }
    None
}

fn format_ep_number(val: &serde_json::Value) -> Option<String> {
    match val {
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Some(i.to_string())
            } else if let Some(u) = n.as_u64() {
                Some(u.to_string())
            } else if let Some(f) = n.as_f64() {
                if f.fract() == 0.0 && f >= i64::MIN as f64 && f <= i64::MAX as f64 {
                    Some((f as i64).to_string())
                } else {
                    let mut s = f.to_string();
                    if s.ends_with(".0") {
                        s.truncate(s.len() - 2);
                    }
                    Some(s)
                }
            } else {
                Some(n.to_string())
            }
        }
        serde_json::Value::String(s) => {
            let trimmed = s.trim();
            if trimmed.is_empty() {
                None
            } else if let Some(stripped) = trimmed.strip_suffix(".0") {
                Some(stripped.to_string())
            } else {
                Some(trimmed.to_string())
            }
        }
        _ => None,
    }
}

impl SiteAdapter for AnimeflvAdapter {
    fn airing_url(&self, base_url: &str) -> String {
        format!("{}/", base_url.trim_end_matches('/'))
    }

    fn parse_airing(&self, html: &str) -> Result<Vec<Series>> {
        let doc = Html::parse_document(html);

        let menu_sel = Selector::parse(".left-sidebar ul#left-menu, ul#left-menu")
            .map_err(|e| anyhow::anyhow!("invalid selector: {e}"))?;
        if doc.select(&menu_sel).next().is_none() {
            return Err(anyhow::anyhow!(
                "no left sidebar menu found; not a recognizable animeflv page"
            ));
        }

        // Extract cover thumbnails from "Últimos episodios" cards when present.
        let ep_card_sel = Selector::parse(".List-Episodes .Episode, .List-Episodes .ht_grid_1_4")
            .map_err(|e| anyhow::anyhow!("invalid selector: {e}"))?;
        let a_sel = Selector::parse("a").map_err(|e| anyhow::anyhow!("invalid selector: {e}"))?;
        let img_sel = Selector::parse("img").map_err(|e| anyhow::anyhow!("invalid selector: {e}"))?;

        let mut covers: HashMap<String, String> = HashMap::new();
        for card in doc.select(&ep_card_sel) {
            let Some(anchor) = card.select(&a_sel).next() else { continue };
            let Some(href) = anchor.value().attr("href") else { continue };
            let ep_slug = slug_from_url(href);
            let Some(series_slug) = series_slug_from_episode_slug(&ep_slug) else { continue };
            if let Some(img) = card.select(&img_sel).next() {
                if let Some(src) = img.value().attr("src").or_else(|| img.value().attr("data-src")) {
                    covers.entry(series_slug.to_string()).or_insert_with(|| abs(src.trim()));
                }
            }
        }

        let mut out: Vec<Series> = Vec::new();
        let mut seen_slugs = HashSet::new();

        // 1. Sidebar DOM items
        let item_a_sel = Selector::parse("ul#left-menu li.sidebar-cat-item a")
            .map_err(|e| anyhow::anyhow!("invalid selector: {e}"))?;
        let name_sel = Selector::parse(".sidebar-cat-name")
            .map_err(|e| anyhow::anyhow!("invalid selector: {e}"))?;

        for anchor in doc.select(&item_a_sel) {
            let Some(href) = anchor.value().attr("href") else { continue };
            let url = abs(href.trim());
            let slug = slug_from_url(&url);
            if slug.is_empty() || !seen_slugs.insert(slug.clone()) {
                continue;
            }
            let title = text_of(anchor, &name_sel)
                .or_else(|| anchor.value().attr("title").map(|s| s.trim().to_string()).filter(|s| !s.is_empty()))
                .unwrap_or_else(|| slug.clone());
            let cover_url = covers.get(&slug).cloned();

            out.push(Series {
                id: 0,
                slug,
                title,
                url,
                cover_url,
                is_airing: true,
                followed: false,
                next_episode_at: None,
                site_episode_count: None,
            });
        }

        // 2. Deferred items from <script id="left-menu-deferred-cats"> if present
        #[derive(serde::Deserialize)]
        struct DeferredCat {
            name: String,
            url: String,
        }

        let deferred_sel = Selector::parse("script#left-menu-deferred-cats")
            .map_err(|e| anyhow::anyhow!("invalid selector: {e}"))?;
        if let Some(script_el) = doc.select(&deferred_sel).next() {
            let json_text = script_el.text().collect::<String>();
            if let Ok(deferred_items) = serde_json::from_str::<Vec<DeferredCat>>(&json_text) {
                for item in deferred_items {
                    let url = abs(item.url.trim());
                    let slug = slug_from_url(&url);
                    if slug.is_empty() || !seen_slugs.insert(slug.clone()) {
                        continue;
                    }
                    let title = item.name.trim().to_string();
                    let cover_url = covers.get(&slug).cloned();

                    out.push(Series {
                        id: 0,
                        slug,
                        title,
                        url,
                        cover_url,
                        is_airing: true,
                        followed: false,
                        next_episode_at: None,
                        site_episode_count: None,
                    });
                }
            }
        }

        Ok(out)
    }

    fn parse_series(&self, html: &str) -> Result<Vec<Episode>> {
        let doc = Html::parse_document(html);
        let script_sel = Selector::parse("script.animeflv-episodes-data")
            .map_err(|e| anyhow::anyhow!("invalid selector: {e}"))?;
        let script_el = doc.select(&script_sel).next().ok_or_else(|| {
            anyhow::anyhow!("missing script.animeflv-episodes-data; not a valid series page")
        })?;

        let json_text = script_el.text().collect::<String>();

        #[derive(serde::Deserialize)]
        struct RawEpisode {
            permalink: Option<String>,
            number: Option<serde_json::Value>,
        }

        let raw_list: Vec<RawEpisode> = serde_json::from_str(&json_text)
            .map_err(|e| anyhow::anyhow!("failed to parse animeflv-episodes-data JSON: {e}"))?;

        let mut out = Vec::new();
        let mut seen_numbers = HashSet::new();

        for raw in raw_list {
            let Some(permalink) = raw.permalink.filter(|s| !s.trim().is_empty()) else {
                continue;
            };
            let Some(num_val) = raw.number else {
                continue;
            };
            let Some(number) = format_ep_number(&num_val) else {
                continue;
            };
            if !seen_numbers.insert(number.clone()) {
                continue;
            }

            let url = abs(permalink.trim());
            let released_at = parse_date_from_url(&url);

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
        let genres_sel = Selector::parse(".anime-genres .genre-tag")
            .map_err(|e| anyhow::anyhow!("invalid selector: {e}"))?;
        let synopsis_sel = Selector::parse(".anime-synopsis p")
            .map_err(|e| anyhow::anyhow!("invalid selector: {e}"))?;
        let kind_sel = Selector::parse(".anime-header .Type, .anime-sidebar .Type, .anime-main-content .Type")
            .map_err(|e| anyhow::anyhow!("invalid selector: {e}"))?;

        let genres = doc
            .select(&genres_sel)
            .map(|el| el.text().collect::<String>().trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();

        // The markup is `<p><p>text</p> </p>`: the HTML parser closes the outer `<p>`
        // when the inner one opens, so the first `<p>` is empty and the text sits in the
        // second. Take the first non-empty paragraph.
        let synopsis = doc
            .select(&synopsis_sel)
            .map(|el| el.text().collect::<String>().trim().to_string())
            .find(|s| !s.is_empty());

        let kind = doc
            .select(&kind_sel)
            .next()
            .map(|el| el.text().collect::<String>().trim().to_string())
            .filter(|s| !s.is_empty());

        Ok(SeriesDetail { genres, kind, synopsis })
    }

    fn search_url(&self, base_url: &str, query: &str) -> String {
        let encoded = urlencoding::encode(query);
        format!("{}/?s={}", base_url.trim_end_matches('/'), encoded)
    }

    fn parse_search_results(&self, html: &str) -> Result<Vec<FinishedCard>> {
        let doc = Html::parse_document(html);
        let container_sel = Selector::parse(".search-series-grid")
            .map_err(|e| anyhow::anyhow!("invalid selector: {e}"))?;

        if doc.select(&container_sel).next().is_none() {
            return Err(anyhow::anyhow!(
                "no .search-series-grid container found; not a recognizable search-results page"
            ));
        }

        let card_sel = Selector::parse(".search-series-grid .search-series-card")
            .map_err(|e| anyhow::anyhow!("invalid selector: {e}"))?;
        let a_sel = Selector::parse("a").map_err(|e| anyhow::anyhow!("invalid selector: {e}"))?;
        let title_sel = Selector::parse(".entry-title, h2")
            .map_err(|e| anyhow::anyhow!("invalid selector: {e}"))?;
        let img_sel = Selector::parse("img").map_err(|e| anyhow::anyhow!("invalid selector: {e}"))?;
        let type_sel = Selector::parse(".Type").map_err(|e| anyhow::anyhow!("invalid selector: {e}"))?;

        let mut out = Vec::new();
        let mut seen_slugs = HashSet::new();

        for card in doc.select(&card_sel) {
            let Some(anchor) = card.select(&a_sel).next() else { continue };
            let Some(href) = anchor.value().attr("href") else { continue };
            let url = abs(href.trim());
            let slug = slug_from_url(&url);
            if slug.is_empty() || !seen_slugs.insert(slug.clone()) {
                continue;
            }

            let title = text_of(card, &title_sel).unwrap_or_else(|| slug.clone());
            let poster_url = card
                .select(&img_sel)
                .next()
                .and_then(|i| i.value().attr("src").or_else(|| i.value().attr("data-src")))
                .map(|s| abs(s.trim()));
            let kind = text_of(card, &type_sel).unwrap_or_default();

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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn airing_url_targets_root_and_trims_trailing_slash() {
        let a = AnimeflvAdapter;
        assert_eq!(a.airing_url("https://animeflv.or.at"), "https://animeflv.or.at/");
        assert_eq!(a.airing_url("https://animeflv.or.at/"), "https://animeflv.or.at/");
    }

    #[test]
    fn search_url_is_urlencoded_query_string() {
        let a = AnimeflvAdapter;
        assert_eq!(
            a.search_url("https://animeflv.or.at", "one piece"),
            "https://animeflv.or.at/?s=one%20piece"
        );
        assert_eq!(
            a.search_url("https://animeflv.or.at/", "one piece"),
            "https://animeflv.or.at/?s=one%20piece"
        );
        assert_eq!(
            a.search_url("https://animeflv.or.at", "naruto"),
            "https://animeflv.or.at/?s=naruto"
        );
    }

    #[test]
    fn parses_airing_fixture() {
        let html = include_str!("../../tests/fixtures/animeflv_airing.html");
        let out = AnimeflvAdapter.parse_airing(html).unwrap();
        assert!(!out.is_empty(), "airing listing must not be empty");

        // Verify kimi-wo-aisuru... with its HTML-escaped quotes decoded in title
        let kimi = out
            .iter()
            .find(|s| s.slug == "kimi-wo-aisuru-ki-wa-nai-to-itta-jiki-koushaku-sama-ga-nazeka-dekiai-shitekimasu")
            .expect("expected kimi-wo-aisuru in airing list");
        assert_eq!(
            kimi.title,
            "\"Kimi wo Aisuru Ki wa Nai\" to Itta Jiki Koushaku-sama ga Nazeka Dekiai shitekimasu"
        );
        assert_eq!(
            kimi.url,
            "https://animeflv.or.at/anime/kimi-wo-aisuru-ki-wa-nai-to-itta-jiki-koushaku-sama-ga-nazeka-dekiai-shitekimasu/"
        );
        assert_eq!(kimi.cover_url, None, "no matching episode card on home page");

        // Verify #Zombie Sagashitemasu
        let zombie = out
            .iter()
            .find(|s| s.slug == "zombie-sagashitemasu")
            .expect("expected zombie-sagashitemasu in airing list");
        assert_eq!(zombie.title, "#Zombie Sagashitemasu");

        // Check deduplication
        let mut slugs: Vec<_> = out.iter().map(|s| s.slug.as_str()).collect();
        slugs.sort();
        let dedup_len = {
            let mut d = slugs.clone();
            d.dedup();
            d.len()
        };
        assert_eq!(slugs.len(), dedup_len, "no duplicate slugs allowed in airing output");

        // Check URL prefix and base fields
        for s in &out {
            assert!(
                s.url.starts_with("https://animeflv.or.at/anime/"),
                "unexpected url format: {}",
                s.url
            );
            assert!(s.is_airing);
            assert_eq!(s.next_episode_at, None);
            assert_eq!(s.site_episode_count, None);
        }

        // Series with a card in "Últimos episodios" gets a cover_url
        let tantei = out
            .iter()
            .find(|s| s.slug == "tantei-wa-mou-shindeiru-season-2")
            .expect("expected tantei-wa-mou-shindeiru-season-2 in airing list");
        assert_eq!(
            tantei.cover_url.as_deref(),
            Some("https://animeflv.or.at/wp-content/uploads/2026/10/4480-260x200.jpg")
        );
    }

    #[test]
    fn parse_airing_errs_on_unrecognizable_page() {
        let err = AnimeflvAdapter.parse_airing("<html><body>nope</body></html>");
        assert!(err.is_err(), "missing left sidebar menu must return Err");
    }

    #[test]
    fn parses_series_fixture() {
        let html = include_str!("../../tests/fixtures/animeflv_series.html");
        let out = AnimeflvAdapter.parse_series(html).unwrap();
        assert_eq!(out.len(), 3, "expected exactly 3 episodes in the fixture");

        assert_eq!(out[0].number, "3");
        assert_eq!(
            out[0].url,
            "https://animeflv.or.at/2026/10/11/tensei-kizoku-kantei-skill-de-nariagaru-3rd-season-episodio-3/"
        );
        assert_eq!(out[0].released_at.as_deref(), Some("2026-10-11"));
        assert_eq!(out[0].title, None);

        assert_eq!(out[1].number, "2");
        assert_eq!(
            out[1].url,
            "https://animeflv.or.at/2026/10/04/tensei-kizoku-kantei-skill-de-nariagaru-3rd-season-episodio-2/"
        );
        assert_eq!(out[1].released_at.as_deref(), Some("2026-10-04"));

        assert_eq!(out[2].number, "1");
        assert_eq!(
            out[2].url,
            "https://animeflv.or.at/2026/09/27/tensei-kizoku-kantei-skill-de-nariagaru-3rd-season-episodio-1/"
        );
        assert_eq!(out[2].released_at.as_deref(), Some("2026-09-27"));
    }

    #[test]
    fn parses_series_synthetic_cases() {
        // Float episode number: 12.5 -> "12.5"
        let html_float = r#"<html><head></head><body>
            <script type="application/json" class="animeflv-episodes-data">[
                {"permalink":"https://animeflv.or.at/2026/01/02/x-episodio-12-5/","number":12.5}
            ]</script>
        </body></html>"#;
        let out_float = AnimeflvAdapter.parse_series(html_float).unwrap();
        assert_eq!(out_float.len(), 1);
        assert_eq!(out_float[0].number, "12.5");
        assert_eq!(out_float[0].released_at.as_deref(), Some("2026-01-02"));

        // String episode number: "7" -> "7"
        let html_str = r#"<html><head></head><body>
            <script type="application/json" class="animeflv-episodes-data">[
                {"permalink":"https://animeflv.or.at/2026/01/03/x-episodio-7/","number":"7"}
            ]</script>
        </body></html>"#;
        let out_str = AnimeflvAdapter.parse_series(html_str).unwrap();
        assert_eq!(out_str.len(), 1);
        assert_eq!(out_str[0].number, "7");

        // Duplicate number keeps the first occurrence
        let html_dup = r#"<html><head></head><body>
            <script type="application/json" class="animeflv-episodes-data">[
                {"permalink":"https://animeflv.or.at/2026/01/04/x-episodio-1/","number":1},
                {"permalink":"https://animeflv.or.at/2026/01/05/x-episodio-1-dup/","number":1}
            ]</script>
        </body></html>"#;
        let out_dup = AnimeflvAdapter.parse_series(html_dup).unwrap();
        assert_eq!(out_dup.len(), 1);
        assert_eq!(out_dup[0].url, "https://animeflv.or.at/2026/01/04/x-episodio-1/");

        // Entry without permalink is skipped
        let html_skip = r#"<html><head></head><body>
            <script type="application/json" class="animeflv-episodes-data">[
                {"number":2},
                {"permalink":"https://animeflv.or.at/2026/01/06/x-episodio-3/","number":3}
            ]</script>
        </body></html>"#;
        let out_skip = AnimeflvAdapter.parse_series(html_skip).unwrap();
        assert_eq!(out_skip.len(), 1);
        assert_eq!(out_skip[0].number, "3");

        // Empty array returns Ok(vec![])
        let html_empty = r#"<html><head></head><body>
            <script type="application/json" class="animeflv-episodes-data">[]</script>
        </body></html>"#;
        let out_empty = AnimeflvAdapter.parse_series(html_empty).unwrap();
        assert!(out_empty.is_empty());

        // Missing script returns Err
        let html_missing = "<html><body>no script here</body></html>";
        assert!(AnimeflvAdapter.parse_series(html_missing).is_err());
    }

    #[test]
    fn parses_series_detail_fixture() {
        let html = include_str!("../../tests/fixtures/animeflv_series_detail.html");
        let d = AnimeflvAdapter.parse_series_detail(html).unwrap();
        assert_eq!(
            d.genres,
            vec!["Aventura".to_string(), "Fantasía".to_string(), "Isekai".to_string()]
        );
        assert!(
            d.synopsis.as_deref().unwrap().contains("Tercera temporada de Tensei Kizoku"),
            "synopsis must contain expected text"
        );
        assert_eq!(d.kind, None, "WordPress theme detail page carries no type badge");
    }

    #[test]
    fn parses_search_hits_fixture() {
        let html = include_str!("../../tests/fixtures/animeflv_search_hits.html");
        let out = AnimeflvAdapter.parse_search_results(html).unwrap();
        assert!(!out.is_empty(), "search hits fixture must produce cards");

        // Real cards matching 'tensei' query
        assert!(
            out.iter().any(|c| c.title == "Tensei Kizoku, Kantei Skill de Nariagaru 3rd Season"),
            "expected Tensei Kizoku 3rd Season in results"
        );
        assert!(
            out.iter().any(|c| c.title == "Mushoku Tensei III: Isekai Ittara Honki Dasu"),
            "expected Mushoku Tensei III in results"
        );
        assert!(
            out.iter().any(|c| c.url == "https://animeflv.or.at/anime/hanaori-san-wa-tensei-shitemo-kenka-ga-shitai/"),
            "expected Hanaori-san in results"
        );

        // Sidebar entries must NOT leak into search results
        assert!(
            out.iter().all(|c| !c.url.contains("kimi-wo-aisuru") && !c.url.contains("zombie-sagashitemasu")),
            "sidebar entries must never be returned as search results"
        );

        for c in &out {
            assert!(!c.title.is_empty());
            assert!(c.url.starts_with("https://animeflv.or.at/anime/"));
            assert!(c.poster_url.is_some());
        }
    }

    #[test]
    fn parses_search_empty_fixture_as_zero_results() {
        let html = include_str!("../../tests/fixtures/animeflv_search_empty.html");
        let out = AnimeflvAdapter.parse_search_results(html).unwrap();
        assert!(out.is_empty(), "empty search page must parse to Ok(vec![])");
    }

    #[test]
    fn parse_search_results_errs_on_unrecognizable_page() {
        let err = AnimeflvAdapter.parse_search_results("<html><body>not this site</body></html>");
        assert!(err.is_err(), "page lacking .search-series-grid must return Err");
    }

    #[test]
    fn genre_archive_methods_use_the_trait_defaults() {
        let a = AnimeflvAdapter;
        assert_eq!(a.genre_list_url("https://animeflv.or.at"), "");
        assert_eq!(a.parse_genre_list("<html></html>").unwrap(), Vec::<(String, String)>::new());
        assert_eq!(a.genre_page_url("https://animeflv.or.at", "accion", 1), "");
        assert_eq!(a.parse_finished_page("<html></html>").unwrap(), Vec::<FinishedCard>::new());
        assert_eq!(a.parse_pagination_last_page("<html></html>"), 1);
    }
}
