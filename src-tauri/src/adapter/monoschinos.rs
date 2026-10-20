use super::{slug_from_url, text_of, SiteAdapter};
use crate::models::{Episode, FinishedCard, Series, SeriesDetail};
use anyhow::{anyhow, Result};
use scraper::{Html, Selector};

pub struct MonoschinosAdapter;

// Selectors confirmed against real captured HTML and JSON in tests/fixtures:
// - `monoschinos_emision.html` (airing listing)
// - `monoschinos_search_hits.html` / `monoschinos_search_empty.html` (search)
// - `monoschinos_series.html` (series detail & caplist container)
// - `monoschinos_ajax_pagination.json` (caplist pagination metadata)
// - `monoschinos_caplist_p1.json` / `monoschinos_caplist_p2.json` (episode pages)
//
// Site architecture:
// - Base: `https://monoschinos.st`. Modern Tailwind-based markup with Spanish
//   subbed anime. No Cloudflare challenge on captured endpoints.
//
// - Airing listing: `{base}/emision`.
//   Cards are inside `<main>` as `<article>` elements containing:
//   `<a href="https://monoschinos.st/anime/<slug>-sub-espanol" class="card-wrap block group">`.
//   Inside each card:
//   - Poster: `<img class="lazy card-img ..." src=".../anime.png" data-src="...">`
//     The `src` attribute is a static placeholder; the real image URL is in
//     `data-src`.
//   - Title: `img[alt]` carries the full title ("Dreamland", etc.), also
//     mirrored in `.card-title`.
//   - Slug: last path segment of the series URL.
//   - Listing cards carry no countdown or episode-count badge, so
//     `next_episode_at` and `site_episode_count` are always `None`.
//   - Airing directory paginates via `?p=N` (confirmed by pagination links
//     like `<a href="https://monoschinos.st/emision?p=2" rel="next">`).
//
// - Search: `{base}/buscar?q=<form-urlencoded>`.
//   Cards share the exact same `main article a.card-wrap` structure as the
//   airing listing.
//   - Type badge: `.top-2.left-2` contains the badge text ("Anime", "Ova",
//     "Pelicula"). Falls back to "TV" if absent.
//   - Recognizability contract: a valid search results page contains an `h1`
//     heading with "Resultados para" (e.g. `<h1>Resultados para: ...</h1>`).
//     An empty query result renders `No se encontraron resultados para...`
//     inside `<main>`, yielding 0 cards -> `Ok(vec![])`. Any page missing the
//     search heading is rejected with `Err(...)`.
//
// - Series detail: `{base}/anime/<slug>-sub-espanol`.
//   - Genres: `<a href="https://monoschinos.st/genero/<genre>">` text.
//   - Kind: `<span class="font-mono text-[11px] tracking-[0.18em] uppercase text-brand">Anime</span>`
//     in the header section, or `<dt>Tipo</dt><dd>Anime</dd>` in the details
//     table.
//   - Synopsis: `<p>` inside the header section (`section .min-w-0 p`) or
//     within `#tab-info p`.
//
// - Episode list: Client-side AJAX pagination.
//   The series HTML does not embed the episode list directly; it holds:
//   `<section class="caplist" data-ajax="https://monoschinos.st/ajax/ajax_pagination/<id>">`.
//   `EPISODE_FETCH_SCRIPT` below is injected via `episode_fetch_script()`:
//   1. Reads `data-ajax` attribute from `.caplist[data-ajax]`.
//   2. Performs synchronous GET -> JSON with `paginate_url`, `perpage`, `eps`.
//   3. Loops page = 1..ceil(eps.length / perpage) (hard-capped at 200 pages),
//      GETting `{paginate_url}?p={page}` and collecting every `caps` entry.
//   4. Returns `JSON.stringify({caps: [...]})` or `{caps: []}` on failure.
//   `parse_series` deserializes this JSON (tolerating single-page p1/p2
//   fixtures as well as malformed/HTML inputs without panicking), formats
//   `episodio` into a clean string (preserving decimals for fractional
//   episodes like 13.5), sorts ascending by numeric episode number, and
//   deduplicates by episode number.

const BASE: &str = "https://monoschinos.st";
const CARD: &str = "main article";

/// In-page script running in WebView2 to synchronously fetch all paginated
/// episodes from monoschinos's AJAX endpoint.
const EPISODE_FETCH_SCRIPT: &str = r#"(function(){
    try {
        var el = document.querySelector('.caplist[data-ajax]');
        if (!el) return JSON.stringify({caps: []});
        var ajaxUrl = el.getAttribute('data-ajax');
        if (!ajaxUrl) return JSON.stringify({caps: []});

        var xhr = new XMLHttpRequest();
        xhr.open('GET', ajaxUrl, false);
        xhr.send(null);
        if (xhr.status !== 200) return JSON.stringify({caps: []});

        var pagination = JSON.parse(xhr.responseText);
        var paginateUrl = pagination.paginate_url;
        var perpage = pagination.perpage;
        var eps = pagination.eps;
        if (!paginateUrl || !perpage || !eps || !Array.isArray(eps) || perpage <= 0) {
            return JSON.stringify({caps: []});
        }

        var totalPages = Math.ceil(eps.length / perpage);
        if (totalPages > 200) totalPages = 200;
        var allCaps = [];

        for (var p = 1; p <= totalPages; p++) {
            var pxhr = new XMLHttpRequest();
            pxhr.open('GET', paginateUrl + '?p=' + p, false);
            pxhr.send(null);
            if (pxhr.status !== 200) break;
            var pdata = JSON.parse(pxhr.responseText);
            if (pdata && Array.isArray(pdata.caps)) {
                for (var j = 0; j < pdata.caps.length; j++) {
                    allCaps.push(pdata.caps[j]);
                }
            }
        }
        return JSON.stringify({caps: allCaps});
    } catch (e) {
        return JSON.stringify({caps: []});
    }
})()"#;

/// Resolve relative URLs against the default base URL if needed.
fn abs(url: &str) -> String {
    if url.starts_with("http://") || url.starts_with("https://") {
        url.to_string()
    } else if let Some(stripped) = url.strip_prefix('/') {
        format!("{BASE}/{stripped}")
    } else {
        format!("{BASE}/{url}")
    }
}

/// Format an `episodio` JSON value into an episode number string.
/// Renders integers without decimal points (e.g. `1`), and keeps fractional
/// parts if present (e.g. `13.5`).
fn format_ep_number(val: &serde_json::Value) -> Option<String> {
    match val {
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Some(i.to_string())
            } else if let Some(u) = n.as_u64() {
                Some(u.to_string())
            } else if let Some(f) = n.as_f64() {
                if f.fract() == 0.0 {
                    Some((f as i64).to_string())
                } else {
                    Some(f.to_string())
                }
            } else {
                Some(n.to_string())
            }
        }
        serde_json::Value::String(s) => {
            let trimmed = s.trim();
            if trimmed.is_empty() {
                None
            } else if let Ok(f) = trimmed.parse::<f64>() {
                if f.fract() == 0.0 {
                    Some((f as i64).to_string())
                } else {
                    Some(trimmed.to_string())
                }
            } else {
                Some(trimmed.to_string())
            }
        }
        _ => None,
    }
}

impl SiteAdapter for MonoschinosAdapter {
    fn airing_url(&self, base_url: &str) -> String {
        format!("{}/emision", base_url.trim_end_matches('/'))
    }

    fn airing_page_url(&self, base_url: &str, page: u32) -> Option<String> {
        let base = base_url.trim_end_matches('/');
        Some(if page <= 1 {
            format!("{base}/emision")
        } else {
            format!("{base}/emision?p={page}")
        })
    }

    fn parse_airing(&self, html: &str) -> Result<Vec<Series>> {
        let doc = Html::parse_document(html);
        let container_sel = Selector::parse("main .grid, main div.grid").unwrap();
        if doc.select(&container_sel).next().is_none() {
            return Err(anyhow!("no listing grid container found; not a recognizable monoschinos airing page"));
        }
        let card_sel = Selector::parse(CARD).unwrap();
        let a_sel = Selector::parse("a.card-wrap").unwrap();
        let img_sel = Selector::parse("img.card-img, img").unwrap();
        let title_sel = Selector::parse(".card-title").unwrap();

        let mut out = Vec::new();
        for card in doc.select(&card_sel) {
            let Some(anchor) = card.select(&a_sel).next().or_else(|| {
                if card.value().name() == "a" {
                    Some(card)
                } else {
                    None
                }
            }) else {
                continue;
            };
            let Some(href) = anchor.value().attr("href") else { continue };
            let url = abs(href.trim());
            let slug = slug_from_url(&url);
            if slug.is_empty() {
                continue;
            }

            let img = card.select(&img_sel).next();
            let title = img
                .and_then(|i| i.value().attr("alt"))
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(String::from)
                .or_else(|| text_of(card, &title_sel))
                .unwrap_or_else(|| slug.clone());

            let cover_url = img
                .and_then(|i| i.value().attr("data-src").or_else(|| i.value().attr("src")))
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(abs);

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
        Ok(out)
    }

    fn parse_series(&self, json: &str) -> Result<Vec<Episode>> {
        let parsed: serde_json::Value = match serde_json::from_str(json) {
            Ok(v) => v,
            Err(_) => return Ok(Vec::new()),
        };

        let Some(caps) = parsed.get("caps").and_then(|c| c.as_array()) else {
            return Ok(Vec::new());
        };

        let mut out = Vec::new();
        for item in caps {
            let Some(ep_val) = item.get("episodio") else { continue };
            let Some(number) = format_ep_number(ep_val) else { continue };
            let Some(raw_url) = item.get("url").and_then(|u| u.as_str()) else { continue };
            let trimmed_url = raw_url.trim();
            if trimmed_url.is_empty() {
                continue;
            }
            let url = abs(trimmed_url);

            out.push(Episode {
                id: 0,
                series_id: 0,
                number,
                title: None,
                url,
                released_at: None,
                seen: false,
                download_path: None,
                download_status: None,
            });
        }

        // Sort ascending by episode number, comparing numerically where possible
        out.sort_by(|a, b| {
            let num_a = a.number.parse::<f64>().unwrap_or(f64::MAX);
            let num_b = b.number.parse::<f64>().unwrap_or(f64::MAX);
            num_a.total_cmp(&num_b).then_with(|| a.number.cmp(&b.number))
        });
        out.dedup_by(|a, b| a.number == b.number);

        Ok(out)
    }

    fn parse_series_detail(&self, html: &str) -> Result<SeriesDetail> {
        let doc = Html::parse_document(html);

        // Genres: links under `/genero/`
        let genre_sel = Selector::parse("a[href*=\"/genero/\"]").unwrap();
        let mut genres = Vec::new();
        for g in doc.select(&genre_sel) {
            let name = g.text().collect::<String>().trim().to_string();
            if !name.is_empty() && !genres.contains(&name) {
                genres.push(name);
            }
        }

        // Kind: header badge `.text-brand` or details table `dl dd` for "Tipo"
        let kind_sel = Selector::parse("section .min-w-0 span.text-brand").unwrap();
        let kind = text_of(doc.root_element(), &kind_sel).or_else(|| {
            let dt_sel = Selector::parse("dl dt").unwrap();
            let dd_sel = Selector::parse("dl dd").unwrap();
            let dts: Vec<_> = doc.select(&dt_sel).collect();
            let dds: Vec<_> = doc.select(&dd_sel).collect();
            for (dt, dd) in dts.iter().zip(dds.iter()) {
                if dt.text().collect::<String>().trim().eq_ignore_ascii_case("tipo") {
                    let txt = dd.text().collect::<String>().trim().to_string();
                    if !txt.is_empty() {
                        return Some(txt);
                    }
                }
            }
            None
        });

        // Synopsis: header section paragraph, fallback to info-tab paragraph
        let synopsis_sel = Selector::parse("section .min-w-0 p").unwrap();
        let tab_synopsis_sel = Selector::parse("#tab-info p").unwrap();
        let synopsis = text_of(doc.root_element(), &synopsis_sel)
            .or_else(|| text_of(doc.root_element(), &tab_synopsis_sel));

        Ok(SeriesDetail { genres, kind, synopsis })
    }

    fn search_url(&self, base_url: &str, query: &str) -> String {
        let encoded: String = url::form_urlencoded::byte_serialize(query.as_bytes()).collect();
        format!("{}/buscar?q={}", base_url.trim_end_matches('/'), encoded)
    }

    fn parse_search_results(&self, html: &str) -> Result<Vec<FinishedCard>> {
        let doc = Html::parse_document(html);

        // A recognizable search page must contain the search heading
        // (e.g. `<h1>Resultados para: ...</h1>`). If absent, this is not a
        // monoschinos search page (e.g. wrong mirror or error page).
        let h1_sel = Selector::parse("h1").unwrap();
        let is_search_page = doc
            .select(&h1_sel)
            .any(|h| h.text().collect::<String>().to_lowercase().contains("resultados para"));
        if !is_search_page {
            return Err(anyhow!("no search results heading found; not a recognizable monoschinos search page"));
        }

        let card_sel = Selector::parse(CARD).unwrap();
        let a_sel = Selector::parse("a.card-wrap").unwrap();
        let img_sel = Selector::parse("img.card-img, img").unwrap();
        let title_sel = Selector::parse(".card-title").unwrap();
        let badge_sel = Selector::parse(".top-2.left-2").unwrap();

        let mut out = Vec::new();
        for card in doc.select(&card_sel) {
            let Some(anchor) = card.select(&a_sel).next().or_else(|| {
                if card.value().name() == "a" {
                    Some(card)
                } else {
                    None
                }
            }) else {
                continue;
            };
            let Some(href) = anchor.value().attr("href") else { continue };
            let url = abs(href.trim());
            let slug = slug_from_url(&url);

            let img = card.select(&img_sel).next();
            let title = img
                .and_then(|i| i.value().attr("alt"))
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(String::from)
                .or_else(|| text_of(card, &title_sel))
                .unwrap_or(slug);

            let poster_url = img
                .and_then(|i| i.value().attr("data-src").or_else(|| i.value().attr("src")))
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(abs);

            let kind = text_of(card, &badge_sel).unwrap_or_else(|| "TV".to_string());

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
    fn parse_airing_rejects_unrecognizable_and_accepts_empty_listing() {
        assert!(MonoschinosAdapter.parse_airing("<html><body>error</body></html>").is_err());
        assert!(MonoschinosAdapter.parse_airing("").is_err());
        let empty = MonoschinosAdapter.parse_airing("<html><body><main><div class=\"grid\"></div></main></body></html>").unwrap();
        assert!(empty.is_empty(), "a recognizable main .grid with 0 cards must parse to Ok(vec![])");
    }


    #[test]
    fn airing_url_returns_emision_path() {
        let a = MonoschinosAdapter;
        assert_eq!(a.airing_url("https://monoschinos.st/"), "https://monoschinos.st/emision");
        assert_eq!(a.airing_url("https://monoschinos.st"), "https://monoschinos.st/emision");
    }

    #[test]
    fn airing_page_url_paginates_with_p_query() {
        let a = MonoschinosAdapter;
        assert_eq!(a.airing_page_url("https://monoschinos.st/", 1).as_deref(), Some("https://monoschinos.st/emision"));
        assert_eq!(a.airing_page_url("https://monoschinos.st", 2).as_deref(), Some("https://monoschinos.st/emision?p=2"));
    }

    #[test]
    fn search_url_is_form_urlencoded() {
        let a = MonoschinosAdapter;
        assert_eq!(
            a.search_url("https://monoschinos.st", "Shingeki no Kyojin"),
            "https://monoschinos.st/buscar?q=Shingeki+no+Kyojin"
        );
        assert_eq!(
            a.search_url("https://monoschinos.st/", "Shingeki no Kyojin"),
            "https://monoschinos.st/buscar?q=Shingeki+no+Kyojin"
        );
    }

    #[test]
    fn parses_search_hits_fixture() {
        let html = include_str!("../../tests/fixtures/monoschinos_search_hits.html");
        let out = MonoschinosAdapter.parse_search_results(html).unwrap();
        assert_eq!(out.len(), 6);
        for card in &out {
            assert!(!card.title.is_empty());
            assert!(
                card.url.starts_with("https://monoschinos.st/anime/"),
                "url not absolute: {}",
                card.url
            );
            assert!(card.poster_url.is_some());
            assert!(!card.kind.is_empty());
        }
        let haikyuu = out.iter().find(|c| c.title == "Haikyuu!!").expect("Haikyuu!! card not found");
        assert_eq!(haikyuu.url, "https://monoschinos.st/anime/haikyuu-sub-espanol");
        assert!(haikyuu.poster_url.as_deref().unwrap().contains("haikyuu-T1"));
    }

    #[test]
    fn parses_search_empty_fixture() {
        let html = include_str!("../../tests/fixtures/monoschinos_search_empty.html");
        let out = MonoschinosAdapter.parse_search_results(html).unwrap();
        assert!(out.is_empty());
    }

    #[test]
    fn parse_search_results_rejects_unrecognizable_page() {
        assert!(MonoschinosAdapter.parse_search_results("<html><body>Error</body></html>").is_err());
    }

    #[test]
    fn parses_airing_fixture() {
        let html = include_str!("../../tests/fixtures/monoschinos_emision.html");
        let out = MonoschinosAdapter.parse_airing(html).unwrap();
        assert!(!out.is_empty());
        for s in &out {
            assert!(!s.url.is_empty());
            assert!(
                s.url.starts_with("https://monoschinos.st/anime/"),
                "url not absolute under https://monoschinos.st/anime/: {}",
                s.url
            );
            assert!(!s.slug.is_empty());
            assert!(!s.title.is_empty());
            assert!(s.is_airing);
            assert!(!s.followed);
            assert_eq!(s.next_episode_at, None);
            assert_eq!(s.site_episode_count, None);
        }
        let first = &out[0];
        assert_eq!(first.slug, "dreamland-sub-espanol");
        assert_eq!(first.title, "Dreamland");
        assert_eq!(first.url, "https://monoschinos.st/anime/dreamland-sub-espanol");
        assert!(first.cover_url.is_some());
        assert!(first.cover_url.as_deref().unwrap().contains("dreamland"));
    }

    #[test]
    fn parses_series_p1_fixture() {
        let json = include_str!("../../tests/fixtures/monoschinos_caplist_p1.json");
        let out = MonoschinosAdapter.parse_series(json).unwrap();
        assert_eq!(out.len(), 50);
        assert_eq!(out[0].number, "1");
        assert_eq!(out[0].url, "https://monoschinos.st/ver/haikyuu-episodio-1");
        assert_eq!(out[49].number, "50");
        assert_eq!(out[49].url, "https://monoschinos.st/ver/haikyuu-episodio-50");
    }

    #[test]
    fn parses_series_p2_fixture() {
        let json = include_str!("../../tests/fixtures/monoschinos_caplist_p2.json");
        let out = MonoschinosAdapter.parse_series(json).unwrap();
        assert_eq!(out.len(), 10);
        assert_eq!(out[0].number, "51");
        assert_eq!(out[0].url, "https://monoschinos.st/ver/haikyuu-episodio-51");
        assert_eq!(out[9].number, "60");
        assert_eq!(out[9].url, "https://monoschinos.st/ver/haikyuu-episodio-60");
    }

    #[test]
    fn parses_series_concatenated_json_to_60_sorted_episodes() {
        let p1 = include_str!("../../tests/fixtures/monoschinos_caplist_p1.json");
        let p2 = include_str!("../../tests/fixtures/monoschinos_caplist_p2.json");
        let v1: serde_json::Value = serde_json::from_str(p1).unwrap();
        let v2: serde_json::Value = serde_json::from_str(p2).unwrap();
        // Concatenate p2 before p1 to verify sort orders them ascending
        let mut caps = v2.get("caps").unwrap().as_array().unwrap().clone();
        caps.extend(v1.get("caps").unwrap().as_array().unwrap().clone());
        let combined = serde_json::to_string(&serde_json::json!({ "caps": caps })).unwrap();

        let out = MonoschinosAdapter.parse_series(&combined).unwrap();
        assert_eq!(out.len(), 60);
        assert_eq!(out[0].number, "1");
        assert_eq!(out[0].url, "https://monoschinos.st/ver/haikyuu-episodio-1");
        assert_eq!(out[59].number, "60");
        assert_eq!(out[59].url, "https://monoschinos.st/ver/haikyuu-episodio-60");
        for (i, ep) in out.iter().enumerate() {
            assert_eq!(ep.number, (i + 1).to_string());
            assert_eq!(ep.title, None);
            assert_eq!(ep.released_at, None);
            assert!(!ep.seen);
            assert_eq!(ep.id, 0);
            assert_eq!(ep.series_id, 0);
        }
    }

    #[test]
    fn parses_series_tolerates_malformed_and_empty_inputs() {
        let a = MonoschinosAdapter;
        assert_eq!(a.parse_series("").unwrap(), Vec::<Episode>::new());
        assert_eq!(a.parse_series("not json").unwrap(), Vec::<Episode>::new());
        assert_eq!(a.parse_series("<html></html>").unwrap(), Vec::<Episode>::new());
        assert_eq!(a.parse_series("{\"caps\":[]}").unwrap(), Vec::<Episode>::new());
    }

    #[test]
    fn parses_series_fractional_number_and_deduplication() {
        let json = r#"{"caps":[
            {"episodio": 1, "url": "https://monoschinos.st/ver/ep-1"},
            {"episodio": 1, "url": "https://monoschinos.st/ver/ep-1-dup"},
            {"episodio": 1.5, "url": "https://monoschinos.st/ver/ep-1-5"},
            {"episodio": 2, "url": "https://monoschinos.st/ver/ep-2"}
        ]}"#;
        let out = MonoschinosAdapter.parse_series(json).unwrap();
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].number, "1");
        assert_eq!(out[1].number, "1.5");
        assert_eq!(out[2].number, "2");
    }

    #[test]
    fn parses_series_detail_fixture() {
        let html = include_str!("../../tests/fixtures/monoschinos_series.html");
        let detail = MonoschinosAdapter.parse_series_detail(html).unwrap();
        assert!(!detail.genres.is_empty());
        assert!(detail.genres.contains(&"Comedia".to_string()));
        assert!(detail.genres.contains(&"Drama".to_string()));
        assert!(detail.genres.contains(&"Escolares".to_string()));
        assert_eq!(detail.kind.as_deref(), Some("Anime"));
        assert!(detail.synopsis.is_some());
        let synopsis = detail.synopsis.as_deref().unwrap();
        assert!(!synopsis.is_empty());
        assert!(synopsis.contains("Little Giant"));
    }

    #[test]
    fn episode_fetch_script_contract() {
        let script = MonoschinosAdapter.episode_fetch_script();
        assert!(script.is_some());
        let s = script.unwrap();
        assert!(s.contains("data-ajax"), "missing data-ajax selector in script");
        assert!(s.contains("?p="), "missing ?p= query param in script");
        assert!(s.contains("false"), "missing false (sync XHR) flag in script");
    }
}
