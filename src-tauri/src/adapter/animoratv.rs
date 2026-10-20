use super::{digits_in, SiteAdapter};
use crate::models::{Episode, FinishedCard, Series, SeriesDetail};
use anyhow::{anyhow, Result};
use scraper::{Html, Selector};

pub struct AnimoratvAdapter;

// Selectors and JSON keys confirmed against real captured fixtures in tests/fixtures/:
// - `animoratv_airing.json` (currently airing JSON listing)
// - `animoratv_search_hits.json` / `animoratv_search_empty.json` (search API responses)
// - `animoratv_series.html` / `animoratv_series_movie.html` (prerendered series pages with JSON-LD & episode links)
// - `animoratv_episodes.json` / `animoratv_anime.json` / `animoratv_episode.html` (informational)
//
// Site architecture:
// - Domain: `https://www.animoratv.com` (Spanish subbed anime, React SPA, no Cloudflare).
// - WebView2 Scraping behavior:
//   The WebView2 scraper loads any URL — including JSON endpoints — and returns the DOM
//   as HTML. For a JSON endpoint, the browser typically wraps the text inside
//   `<html><body><pre>{...}</pre></body></html>` or `<html><body>{...}</body></html>`.
//   All JSON parsing routines (`parse_airing`, `parse_search_results`) accept both raw JSON
//   and HTML-wrapped JSON via `extract_json_payload`.
//
// - Airing listing:
//   Endpoint: `{base}/api/animes?estado=en%20emisi%C3%B3n&sort=populares&limite=24`.
//   Pagination: page 1 is the base airing URL; page N > 1 appends `&pagina=N`.
//   JSON payload structure: `{"success": true, "data": {"animes": [...]}}`.
//   Each anime entry holds:
//     - `slug`: series slug (e.g. "one-piece"). Series URL is `https://www.animoratv.com/anime/<slug>`.
//     - `titulo`: main title (fallback `tituloIngles`).
//     - `portada`: poster image URL (absolute URL on anilist CDN or relative).
//     - `totalEpisodios`: total episodes integer if > 0 (0 for ongoing/unknown -> None).
//     - Listing entries do not provide next episode countdown timestamp (`next_episode_at: None`).
//
// - Search:
//   Endpoint: `{base}/api/busqueda?termino=<form-urlencoded>`.
//   JSON payload structure: `{"success": true, "data": {"animes": [...]}}`.
//   Recognizability contract:
//     - Returns 0 cards for empty results (`"animes": []`).
//     - Any text that is not valid JSON or lacks `data.animes` returns `Err` (indicating an incompatible mirror).
//   Card fields:
//     - `title`: `titulo` (fallback `tituloIngles`, fallback `slug`).
//     - `url`: `https://www.animoratv.com/anime/<slug>`.
//     - `poster_url`: `portada` if present and non-empty.
//     - `kind`: mapped from `tipo`: "serie" -> "TV", "ova" -> "OVA", "especial" -> "Special", "película" -> "Movie".
//
// - Series detail & episode list:
//   URL: `https://www.animoratv.com/anime/<slug>`.
//   The series page is prerendered for SEO (`#ssr-seo`), embedding:
//     - Episode links: `<a href="/anime/<slug>/episodio/<N>">`.
//     - Episode number: extracted from the path segment following `/episodio/` (retaining decimals for e.g. 13.5).
//     - Episode title: `None` per spec (the link text in the fixture repeats a movie/arc name or "Episodio N").
//     - Episodes are sorted ascending by numeric episode number, deduplicated by number.
//     - JSON-LD metadata: script tag `script[type="application/ld+json"]` contains an array of schema objects.
//       The object with `"@type": "TVSeries"` supplies:
//         - `genre`: list of strings (e.g. `["Comedy", "Drama", "Sports"]`).
//         - `description`: synopsis string.
//       If JSON-LD description is absent, falls back to `<div id="ssr-seo"><p>...</p></div>`.
//       `kind`: extracted from static HTML if a recognizable badge is present, else `None`.
//
// KNOWN LIMITATION:
// Root-relative hrefs are resolved against `BASE`. Mirror divergence is handled
// host-side by `commands::scan::rebase_to_mirror`.

const BASE: &str = "https://www.animoratv.com";

/// Resolves a root-relative path or keeps an already absolute URL.
fn abs(url: &str) -> String {
    if url.starts_with("http://") || url.starts_with("https://") {
        url.to_string()
    } else if let Some(stripped) = url.strip_prefix("//") {
        // protocol-relative (a CDN image): keep its own host
        format!("https://{stripped}")
    } else if let Some(stripped) = url.strip_prefix('/') {
        format!("{BASE}/{stripped}")
    } else {
        format!("{BASE}/{url}")
    }
}

/// Builds an absolute series URL from a slug.
/// In-page script (run once the series page is ready) that reads the episode numbers from the
/// site's JSON API instead of the page. The pre-rendered page does carry the episode links, but
/// the React app removes that block when it mounts, so the live DOM the scraper sees has none
/// (verified in the sandbox: 0 episodes for a 12-episode series). The script is synchronous
/// (ExecuteScript cannot await a promise) and returns `{"slug": "...", "numbers": [..]}`.
const EPISODE_FETCH_SCRIPT: &str = r#"(function(){
    try {
        var m = location.pathname.match(/^\/anime\/([^\/]+)/);
        if (!m) return JSON.stringify({slug: "", numbers: []});
        var slug = m[1];
        var xhr = new XMLHttpRequest();
        xhr.open('GET', '/api/animes/' + encodeURIComponent(slug) + '/episodios', false);
        xhr.setRequestHeader('X-Requested-With', 'XMLHttpRequest');
        xhr.send(null);
        if (xhr.status !== 200) return JSON.stringify({slug: slug, numbers: []});
        var data = JSON.parse(xhr.responseText);
        var eps = (data && data.data && data.data.episodios) || [];
        var numbers = [];
        for (var i = 0; i < eps.length; i++) {
            if (typeof eps[i].numero === 'number') numbers.push(eps[i].numero);
        }
        return JSON.stringify({slug: slug, numbers: numbers});
    } catch (e) {
        return JSON.stringify({slug: "", numbers: []});
    }
})()"#;

/// Episodes from the script's `{"slug": .., "numbers": [..]}` answer.
fn episodes_from_script_json(input: &str) -> Vec<Episode> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(input.trim()) else { return Vec::new() };
    let Some(slug) = v.get("slug").and_then(|s| s.as_str()).map(str::trim).filter(|s| !s.is_empty()) else {
        return Vec::new();
    };
    let Some(numbers) = v.get("numbers").and_then(|n| n.as_array()) else { return Vec::new() };
    let mut nums: Vec<f64> = numbers.iter().filter_map(|n| n.as_f64()).filter(|n| n.is_finite() && *n >= 0.0).collect();
    nums.sort_by(|a, b| a.total_cmp(b));
    nums.dedup();
    nums.into_iter()
        .map(|n| {
            let number = if n.fract() == 0.0 { format!("{}", n as i64) } else { format!("{n}") };
            Episode {
                id: 0,
                series_id: 0,
                url: format!("{BASE}/anime/{slug}/episodio/{number}"),
                number,
                title: None,
                released_at: None,
                seen: false,
                download_path: None,
                download_status: None,
            }
        })
        .collect()
}

fn series_url(slug: &str) -> String {
    if slug.starts_with("http://") || slug.starts_with("https://") {
        slug.to_string()
    } else if let Some(stripped) = slug.strip_prefix('/') {
        if stripped.starts_with("anime/") {
            abs(slug)
        } else {
            format!("{BASE}/anime/{stripped}")
        }
    } else if slug.starts_with("anime/") {
        format!("{BASE}/{slug}")
    } else {
        format!("{BASE}/anime/{slug}")
    }
}

/// Extracts a JSON string from either raw JSON or HTML wrapping generated by WebView2
/// (typically `<html><body><pre>{...}</pre></body></html>`).
fn extract_json_payload(input: &str) -> String {
    let trimmed = input.trim();
    // Fast path: already raw JSON object or array
    if (trimmed.starts_with('{') && trimmed.ends_with('}'))
        || (trimmed.starts_with('[') && trimmed.ends_with(']'))
    {
        return trimmed.to_string();
    }

    // Parse as HTML and extract text from <pre> or <body>
    let doc = Html::parse_document(trimmed);
    let pre_sel = Selector::parse("pre").unwrap();
    if let Some(pre) = doc.select(&pre_sel).next() {
        let t = pre.text().collect::<String>();
        let t_trim = t.trim();
        if !t_trim.is_empty() {
            return t_trim.to_string();
        }
    }

    let body_sel = Selector::parse("body").unwrap();
    if let Some(body) = doc.select(&body_sel).next() {
        let t = body.text().collect::<String>();
        let t_trim = t.trim();
        if !t_trim.is_empty() {
            return t_trim.to_string();
        }
    }

    trimmed.to_string()
}

/// Maps site `tipo` ("serie", "ova", "especial", "película") to standard kind names.
fn map_tipo(tipo: &str) -> String {
    match tipo.trim().to_lowercase().as_str() {
        "serie" => "TV".to_string(),
        "ova" => "OVA".to_string(),
        "especial" | "special" => "Special".to_string(),
        "película" | "pelicula" | "movie" => "Movie".to_string(),
        "ona" => "ONA".to_string(),
        _ => "TV".to_string(),
    }
}

impl SiteAdapter for AnimoratvAdapter {
    fn airing_url(&self, base_url: &str) -> String {
        let base = base_url.trim_end_matches('/');
        format!("{base}/api/animes?estado=en%20emisi%C3%B3n&sort=populares&limite=24")
    }

    fn airing_page_url(&self, base_url: &str, page: u32) -> Option<String> {
        let base_airing = self.airing_url(base_url);
        Some(if page <= 1 {
            base_airing
        } else {
            format!("{base_airing}&pagina={page}")
        })
    }

    fn parse_airing(&self, html: &str) -> Result<Vec<Series>> {
        let payload = extract_json_payload(html);
        let parsed: serde_json::Value = serde_json::from_str(&payload)
            .map_err(|e| anyhow!("animoratv: invalid JSON in airing response: {e}"))?;

        let animes = parsed
            .get("data")
            .and_then(|d| d.get("animes"))
            .and_then(|a| a.as_array())
            .ok_or_else(|| anyhow!("animoratv: missing data.animes array in airing response"))?;

        let mut out = Vec::new();
        for anime in animes {
            let Some(slug) = anime
                .get("slug")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
            else {
                continue;
            };

            let title = if let Some(t) = anime
                .get("titulo")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                t.to_string()
            } else if let Some(ti) = anime
                .get("tituloIngles")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                ti.to_string()
            } else {
                slug.to_string()
            };

            let cover_url = anime
                .get("portada")
                .and_then(|p| p.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(abs);

            let site_episode_count = anime
                .get("totalEpisodios")
                .and_then(|v| v.as_i64())
                .filter(|&cnt| cnt > 0);

            out.push(Series {
                id: 0,
                slug: slug.to_string(),
                title,
                url: series_url(slug),
                cover_url,
                is_airing: true,
                followed: false,
                next_episode_at: None,
                site_episode_count,
            });
        }

        Ok(out)
    }

    fn episode_fetch_script(&self) -> Option<&'static str> {
        Some(EPISODE_FETCH_SCRIPT)
    }

    fn parse_series(&self, html: &str) -> Result<Vec<Episode>> {
        // The live page hands over the script's JSON (see `EPISODE_FETCH_SCRIPT`); the
        // pre-rendered HTML with its episode links is the fallback.
        if html.trim_start().starts_with('{') {
            return Ok(episodes_from_script_json(html));
        }
        let doc = Html::parse_document(html);
        let a_sel = Selector::parse("a[href*='/episodio/']").unwrap();

        let mut out = Vec::new();
        for anchor in doc.select(&a_sel) {
            let Some(href) = anchor.value().attr("href") else {
                continue;
            };
            let trimmed_href = href.trim();
            if trimmed_href.is_empty() {
                continue;
            }

            let clean_href = trimmed_href
                .split('?')
                .next()
                .unwrap_or(trimmed_href)
                .split('#')
                .next()
                .unwrap_or(trimmed_href);

            let Some((_prefix, ep_part)) = clean_href.split_once("/episodio/") else {
                continue;
            };

            let ep_raw = ep_part.trim_matches('/');
            let ep_segment = ep_raw.split('/').next().unwrap_or(ep_raw);
            let number = digits_in(ep_segment);
            if number.is_empty() {
                continue;
            }

            let url = abs(trimmed_href);

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
        let ld_sel = Selector::parse("script[type='application/ld+json']").unwrap();

        let mut genres = Vec::new();
        let mut synopsis = None;

        for script in doc.select(&ld_sel) {
            let text = script.text().collect::<String>();
            let trimmed = text.trim();
            if trimmed.is_empty() {
                continue;
            }

            let Ok(val) = serde_json::from_str::<serde_json::Value>(trimmed) else {
                continue;
            };

            let items: Vec<&serde_json::Value> = match &val {
                serde_json::Value::Array(arr) => arr.iter().collect(),
                serde_json::Value::Object(_) => vec![&val],
                _ => continue,
            };

            for item in items {
                let is_tv_series = item
                    .get("@type")
                    .and_then(|t| t.as_str())
                    .map(|t| t.eq_ignore_ascii_case("TVSeries"))
                    .unwrap_or(false);

                if is_tv_series {
                    // Extract genres
                    if let Some(genre_val) = item.get("genre") {
                        if let Some(arr) = genre_val.as_array() {
                            for g in arr {
                                if let Some(s) = g.as_str().map(str::trim).filter(|s| !s.is_empty()) {
                                    if !genres.contains(&s.to_string()) {
                                        genres.push(s.to_string());
                                    }
                                }
                            }
                        } else if let Some(s) = genre_val.as_str().map(str::trim).filter(|s| !s.is_empty()) {
                            if !genres.contains(&s.to_string()) {
                                genres.push(s.to_string());
                            }
                        }
                    }

                    // Extract description
                    if let Some(desc) = item
                        .get("description")
                        .and_then(|d| d.as_str())
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                    {
                        synopsis = Some(desc.to_string());
                    }
                    break;
                }
            }

            if !genres.is_empty() || synopsis.is_some() {
                break;
            }
        }

        // Fallback synopsis from static HTML if JSON-LD lacked it
        if synopsis.is_none() {
            let p_sel = Selector::parse("#ssr-seo p").unwrap();
            synopsis = doc
                .select(&p_sel)
                .next()
                .map(|el| el.text().collect::<String>().trim().to_string())
                .filter(|s| !s.is_empty());
        }

        // Kind from static HTML if available else None
        let kind_sel = Selector::parse(".tipo, .type, .anime-type, .badge-tipo").unwrap();
        let kind = doc
            .select(&kind_sel)
            .next()
            .map(|el| el.text().collect::<String>().trim().to_string())
            .filter(|s| !s.is_empty());

        Ok(SeriesDetail {
            genres,
            kind,
            synopsis,
        })
    }

    fn search_url(&self, base_url: &str, query: &str) -> String {
        let encoded: String = url::form_urlencoded::byte_serialize(query.as_bytes()).collect();
        format!("{}/api/busqueda?termino={encoded}", base_url.trim_end_matches('/'))
    }

    fn parse_search_results(&self, html: &str) -> Result<Vec<FinishedCard>> {
        let payload = extract_json_payload(html);
        let parsed: serde_json::Value = serde_json::from_str(&payload)
            .map_err(|e| anyhow!("invalid JSON for animoratv search results: {e}"))?;

        let animes = parsed
            .get("data")
            .and_then(|d| d.get("animes"))
            .and_then(|a| a.as_array())
            .ok_or_else(|| anyhow!("missing data.animes in search response"))?;

        let mut out = Vec::new();
        for anime in animes {
            let Some(slug) = anime
                .get("slug")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
            else {
                continue;
            };

            let title = if let Some(t) = anime
                .get("titulo")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                t.to_string()
            } else if let Some(ti) = anime
                .get("tituloIngles")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                ti.to_string()
            } else {
                slug.to_string()
            };

            let poster_url = anime
                .get("portada")
                .and_then(|p| p.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(abs);

            let tipo_str = anime.get("tipo").and_then(|t| t.as_str()).unwrap_or("");
            let kind = map_tipo(tipo_str);

            out.push(FinishedCard {
                title,
                url: series_url(slug),
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
    fn parse_airing_rejects_unrecognizable_and_accepts_empty_data() {
        let a = AnimoratvAdapter;
        assert!(a.parse_airing("").is_err());
        assert!(a.parse_airing("<html><body>error</body></html>").is_err());
        assert!(a.parse_airing("{}").is_err());
        assert!(a.parse_airing(r#"{"data":{}}"#).is_err());

        let empty = a.parse_airing(r#"{"data":{"animes":[]}}"#).unwrap();
        assert!(empty.is_empty(), "valid empty animes array must parse to Ok(vec![])");
    }


    #[test]
    fn search_url_encodes_query() {
        let a = AnimoratvAdapter;
        assert_eq!(
            a.search_url("https://www.animoratv.com/", "Haikyuu!!"),
            "https://www.animoratv.com/api/busqueda?termino=Haikyuu%21%21"
        );
        assert_eq!(
            a.search_url("https://www.animoratv.com", "one piece"),
            "https://www.animoratv.com/api/busqueda?termino=one+piece"
        );
    }

    #[test]
    fn airing_url_and_pagination() {
        let a = AnimoratvAdapter;
        assert_eq!(
            a.airing_url("https://www.animoratv.com/"),
            "https://www.animoratv.com/api/animes?estado=en%20emisi%C3%B3n&sort=populares&limite=24"
        );
        assert_eq!(
            a.airing_page_url("https://www.animoratv.com/", 1),
            Some("https://www.animoratv.com/api/animes?estado=en%20emisi%C3%B3n&sort=populares&limite=24".to_string())
        );
        assert_eq!(
            a.airing_page_url("https://www.animoratv.com/", 2),
            Some("https://www.animoratv.com/api/animes?estado=en%20emisi%C3%B3n&sort=populares&limite=24&pagina=2".to_string())
        );
    }

    #[test]
    fn parses_search_hits_fixture() {
        let raw = include_str!("../../tests/fixtures/animoratv_search_hits.json");
        let a = AnimoratvAdapter;

        // Raw JSON
        let out = a.parse_search_results(raw).unwrap();
        assert_eq!(out.len(), 14);

        let main = out.iter().find(|c| c.title == "Haikyuu!!").unwrap();
        assert_eq!(main.url, "https://www.animoratv.com/anime/haikyuu");
        assert_eq!(main.kind, "TV");
        assert_eq!(
            main.poster_url.as_deref(),
            Some("https://s4.anilist.co/file/anilistcdn/media/anime/cover/large/bx20464-ooZUyBe4ptp9.png")
        );

        let movie = out.iter().find(|c| c.title == "Haikyuu!!: Sainou to Sense").unwrap();
        assert_eq!(movie.kind, "Movie");
        assert_eq!(movie.url, "https://www.animoratv.com/anime/haikyuu-sainou-to-sense");

        let ova = out.iter().find(|c| c.title == "Haikyuu!! Riku VS Kuu").unwrap();
        assert_eq!(ova.kind, "OVA");
        assert_eq!(ova.url, "https://www.animoratv.com/anime/haikyuu-riku-vs-kuu");

        let special = out.iter().find(|c| c.title == "Haikyuu!!: VS Akaten").unwrap();
        assert_eq!(special.kind, "Special");

        // Same result when wrapped in <html><body><pre>...</pre></body></html>
        let wrapped = format!("<html><body><pre>{raw}</pre></body></html>");
        let out_wrapped = a.parse_search_results(&wrapped).unwrap();
        assert_eq!(out, out_wrapped);
    }

    #[test]
    fn parses_search_empty_fixture() {
        let raw = include_str!("../../tests/fixtures/animoratv_search_empty.json");
        let a = AnimoratvAdapter;
        let out = a.parse_search_results(raw).unwrap();
        assert!(out.is_empty());
    }

    #[test]
    fn parse_search_results_junk_fails() {
        let a = AnimoratvAdapter;
        assert!(a.parse_search_results("<html></html>").is_err());
        assert!(a.parse_search_results("").is_err());
        assert!(a.parse_search_results(r#"{"success":true,"data":{"other":[]}}"#).is_err());
    }

    #[test]
    fn parses_airing_fixture() {
        let raw = include_str!("../../tests/fixtures/animoratv_airing.json");
        let a = AnimoratvAdapter;
        let out = a.parse_airing(raw).unwrap();
        assert_eq!(out.len(), 24);

        let first = &out[0];
        assert_eq!(first.slug, "one-piece");
        assert_eq!(first.title, "ONE PIECE");
        assert_eq!(first.url, "https://www.animoratv.com/anime/one-piece");
        assert!(first.is_airing);
        assert!(!first.followed);
        assert_eq!(first.next_episode_at, None);
        assert_eq!(first.site_episode_count, None);
        assert_eq!(
            first.cover_url.as_deref(),
            Some("https://s4.anilist.co/file/anilistcdn/media/anime/cover/large/bx21-ELSYx3yMPcKM.jpg")
        );

        // A series with positive totalEpisodios in the fixture (e.g. Bleach with 10 eps)
        let bleach = out.iter().find(|s| s.slug == "bleach-sennen-kessen-hen-kashin-tan").unwrap();
        assert_eq!(bleach.site_episode_count, Some(10));

        // Test HTML-wrapped JSON
        let wrapped = format!("<html><body><pre>{raw}</pre></body></html>");
        let out_wrapped = a.parse_airing(&wrapped).unwrap();
        assert_eq!(out, out_wrapped);
    }

    #[test]
    fn parses_series_fixture() {
        let html = include_str!("../../tests/fixtures/animoratv_series.html");
        let a = AnimoratvAdapter;
        let out = a.parse_series(html).unwrap();
        assert_eq!(out.len(), 25);

        let first = &out[0];
        assert_eq!(first.number, "1");
        assert_eq!(first.url, "https://www.animoratv.com/anime/haikyuu/episodio/1");
        assert_eq!(first.title, None);
        assert!(!first.seen);

        let last = &out[24];
        assert_eq!(last.number, "25");
        assert_eq!(last.url, "https://www.animoratv.com/anime/haikyuu/episodio/25");
        assert_eq!(last.title, None);
    }

    #[test]
    fn parses_series_movie_fixture() {
        let html = include_str!("../../tests/fixtures/animoratv_series_movie.html");
        let a = AnimoratvAdapter;
        let out = a.parse_series(html).unwrap();
        assert_eq!(out.len(), 13);
        assert_eq!(out[0].number, "1");
        assert_eq!(out[0].url, "https://www.animoratv.com/anime/haikyuu-to-the-top/episodio/1");
        assert_eq!(out[12].number, "13");
        assert_eq!(out[12].url, "https://www.animoratv.com/anime/haikyuu-to-the-top/episodio/13");
    }

    #[test]
    fn parse_series_empty_and_junk() {
        let a = AnimoratvAdapter;
        assert_eq!(a.parse_series("").unwrap(), Vec::<Episode>::new());
        assert_eq!(a.parse_series("<html></html>").unwrap(), Vec::<Episode>::new());
    }

    #[test]
    fn parses_series_detail_fixture() {
        let html = include_str!("../../tests/fixtures/animoratv_series.html");
        let a = AnimoratvAdapter;
        let detail = a.parse_series_detail(html).unwrap();
        assert_eq!(detail.genres, vec!["Comedy", "Drama", "Sports"]);
        assert!(detail.synopsis.is_some());
        let syn = detail.synopsis.unwrap();
        assert!(!syn.is_empty());
        assert!(syn.starts_with("Hinata Shouyou"));
    }

    #[test]
    fn parse_series_detail_empty() {
        let a = AnimoratvAdapter;
        let detail = a.parse_series_detail("").unwrap();
        assert!(detail.genres.is_empty());
        assert_eq!(detail.synopsis, None);
        assert_eq!(detail.kind, None);
    }

    #[test]
    fn parses_the_episode_numbers_the_script_reads_from_the_api() {
        let eps = AnimoratvAdapter
            .parse_series(r#"{"slug":"haikyuu-to-the-top-2","numbers":[3,1,2,2,13.5]}"#)
            .unwrap();
        let numbers: Vec<&str> = eps.iter().map(|e| e.number.as_str()).collect();
        assert_eq!(numbers, ["1", "2", "3", "13.5"]);
        assert_eq!(eps[0].url, "https://www.animoratv.com/anime/haikyuu-to-the-top-2/episodio/1");
        assert_eq!(eps[3].url, "https://www.animoratv.com/anime/haikyuu-to-the-top-2/episodio/13.5");
    }

    #[test]
    fn script_answers_with_no_slug_or_numbers_give_no_episodes_and_never_panic() {
        for junk in [r#"{"slug":"","numbers":[1,2]}"#, r#"{"slug":"x","numbers":[]}"#, r#"{"slug":"x"}"#, r#"{"slug":"x","numbers":["a",-1]}"#, "{}", "{not json"] {
            assert!(AnimoratvAdapter.parse_series(junk).unwrap().is_empty(), "{junk}");
        }
    }

    #[test]
    fn the_episode_script_is_synchronous_and_reads_the_api() {
        let script = AnimoratvAdapter.episode_fetch_script().expect("script");
        assert!(script.contains("/api/animes/") && script.contains("/episodios"));
        assert!(script.contains(", false)"), "sync XHR flag");
        assert!(!script.contains("await") && !script.contains("fetch("));
    }

    #[test]
    fn abs_keeps_the_host_of_a_protocol_relative_url() {
        assert_eq!(abs("//cdn.example/x.jpg"), "https://cdn.example/x.jpg");
        assert_eq!(abs("/anime/x"), "https://www.animoratv.com/anime/x");
        assert_eq!(abs("anime/x"), "https://www.animoratv.com/anime/x");
        assert_eq!(abs("https://a.b/c"), "https://a.b/c");
    }
}
