use super::{digits_in, slug_from_url, text_of, SiteAdapter};
use crate::models::{Episode, FinishedCard, Series, SeriesDetail};
use anyhow::{anyhow, Result};
use scraper::{Html, Selector};

pub struct AnimefenixAdapter;

// Selectors confirmed against real captured HTML in src-tauri/tests/fixtures/
// (animefenix_list_anime__status_ongoing_order_update.html,
//  animefenix_list_anime__status_ongoing.html,
//  animefenix_search_hits.html,
//  animefenix_search_empty.html,
//  animefenix_series.html,
//  animefenix_episode.html,
//  animefenix_home.html).
//
// Site characteristics:
// - Domain: https://animefenix.com.co (Spanish subtitles, no Cloudflare challenge).
// - Engine / Theme: WordPress with AnimeStream theme (same family as gogoanime.by,
//   but with standard .eplister markup for episodes rather than .episodes-container).
//
// - Airing listing:
//   Endpoint: `{base}/anime/?status=ongoing&order=update`
//   Cards sit in `.listupd .bsx` inside the main content area (30 cards per page).
//   Each card provides:
//   - Link: `a[href]` to series page `{base}/<slug>/`.
//   - Title: `.tt h2` (or fallback `a[title]` / `.tt` text) with HTML entities decoded.
//   - Cover: `img` tag with `src` or `data-src`.
//   - Quality badge: `.typez` (e.g. "HD").
//   - Episode / Status badge: `.epx` holds "Ongoing" or episode numbers. Numeric values
//     are captured as `site_episode_count`; non-numeric ("Ongoing") yields `None`.
//   - No release countdown timestamp exists on listing cards (`next_episode_at: None`).
//   - Pagination: `<a href="?page=2&status=ongoing&order=update" class="r">Next</a>`.
//     Page 1 is `{base}/anime/?status=ongoing&order=update`; page N>1 appends `&page=N`.
//
// - Search:
//   Endpoint: `{base}/?s=<query>` (form-urlencoded, space encoded as `+`).
//   Cards share the `.listupd .bsx` structure.
//   Contract:
//   - `.listupd` container is present on all valid site search pages.
//   - On empty search, `.listupd` contains `<center><h3>Not Found</h3></center>` and 0 cards,
//     returning `Ok(vec![])`.
//   - A page missing the `.listupd` container entirely is rejected as not this site (`Err`).
//   - Card shape produces `FinishedCard` with title, absolute url, poster_url, kind (.typez),
//     and `matched_genre: None`.
//
// - Series detail page (`{base}/<slug>/`):
//   - Metadata in `.infox`:
//     - Genres: `.genxed a` links (`https://animefenix.com.co/genres/<genre>/`).
//     - Kind: `.spe span` containing "Type:" or "Tipo:" (e.g. `<span><b>Type:</b> HD</span>`).
//     - Synopsis: `.synp .entry-content p` or `.entry-content[itemprop="description"] p`.
//   - Episode list in `.eplister`:
//     - Items: `.eplister ul li` with anchor `a[href]`.
//     - Number: `.epl-num` text parsed via `digits_in` (preserves decimals like 13.5).
//     - Title: `.epl-title` if present and non-empty.
//     - Released date: `.epl-date` text.
//     - Order: site renders newest episode first; `parse_series` sorts ascending by
//       numeric episode number and deduplicates.
//
// - Absolute URLs:
//   Resolved against `BASE` constant below; mirror changes are handled at runtime by
//   `commands::scan::rebase_to_mirror`.

const BASE: &str = "https://animefenix.com.co";
const LISTUPD_CONTAINER: &str = ".listupd";
const CARD: &str = ".listupd .bsx";

/// The site fills its "Type" field and the card's `.typez` badge with the VIDEO QUALITY
/// ("HD" in 115 of 122 captured cards), not with a media type. Only a value that really names a
/// type is kept; anything else is "unknown" so downstream code never sees "HD" as a kind.
fn media_kind(raw: &str) -> Option<String> {
    let t = raw.trim();
    match t.to_lowercase().as_str() {
        "tv" | "tv show" | "serie" => Some("TV".to_string()),
        "movie" | "pelicula" | "película" => Some("Movie".to_string()),
        "ova" | "oad" => Some("OVA".to_string()),
        "ona" => Some("ONA".to_string()),
        "special" | "especial" => Some("Special".to_string()),
        "music" => Some("Music".to_string()),
        _ => None,
    }
}

fn abs(url: &str) -> String {
    if url.starts_with("http://") || url.starts_with("https://") {
        url.to_string()
    } else if let Some(stripped) = url.strip_prefix("//") {
        format!("https://{stripped}")
    } else if let Some(stripped) = url.strip_prefix('/') {
        format!("{BASE}/{stripped}")
    } else {
        format!("{BASE}/{url}")
    }
}

impl SiteAdapter for AnimefenixAdapter {
    fn airing_url(&self, base_url: &str) -> String {
        format!("{}/anime/?status=ongoing&order=update", base_url.trim_end_matches('/'))
    }

    fn airing_page_url(&self, base_url: &str, page: u32) -> Option<String> {
        let base = base_url.trim_end_matches('/');
        Some(if page <= 1 {
            format!("{base}/anime/?status=ongoing&order=update")
        } else {
            format!("{base}/anime/?status=ongoing&order=update&page={page}")
        })
    }

    fn parse_airing(&self, html: &str) -> Result<Vec<Series>> {
        let doc = Html::parse_document(html);
        let card_sel = Selector::parse(CARD).unwrap();
        let a_sel = Selector::parse("a").unwrap();
        let tt_h2_sel = Selector::parse(".tt h2").unwrap();
        let tt_sel = Selector::parse(".tt").unwrap();
        let img_sel = Selector::parse("img").unwrap();
        let epx_sel = Selector::parse(".epx").unwrap();

        let mut out = Vec::new();
        for card in doc.select(&card_sel) {
            let Some(anchor) = card.select(&a_sel).next() else { continue };
            let Some(href) = anchor.value().attr("href") else { continue };
            let trimmed_href = href.trim();
            if trimmed_href.is_empty() {
                continue;
            }
            let url = abs(trimmed_href);
            let slug = slug_from_url(&url);
            if slug.is_empty() {
                continue;
            }

            let title = text_of(card, &tt_h2_sel)
                .or_else(|| {
                    anchor
                        .value()
                        .attr("title")
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(String::from)
                })
                .or_else(|| text_of(card, &tt_sel))
                .unwrap_or_else(|| slug.clone());

            let cover_url = card
                .select(&img_sel)
                .next()
                .and_then(|img| {
                    img.value()
                        .attr("data-src")
                        .or_else(|| img.value().attr("src"))
                })
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(abs);

            let site_episode_count = text_of(card, &epx_sel).and_then(|s| {
                let t = s.trim();
                t.parse::<i64>().ok().or_else(|| {
                    let d = digits_in(t);
                    d.parse::<i64>().ok()
                })
            });

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

    fn parse_series(&self, html: &str) -> Result<Vec<Episode>> {
        let doc = Html::parse_document(html);
        let item_sel = Selector::parse(".eplister ul li, .eplister li").unwrap();
        let a_sel = Selector::parse("a").unwrap();
        let num_sel = Selector::parse(".epl-num").unwrap();
        let title_sel = Selector::parse(".epl-title").unwrap();
        let date_sel = Selector::parse(".epl-date").unwrap();

        let mut out = Vec::new();
        for item in doc.select(&item_sel) {
            let Some(anchor) = item.select(&a_sel).next() else { continue };
            let Some(href) = anchor.value().attr("href") else { continue };
            let trimmed_href = href.trim();
            if trimmed_href.is_empty() {
                continue;
            }
            let url = abs(trimmed_href);
            let number = text_of(item, &num_sel)
                .map(|s| digits_in(&s))
                .unwrap_or_else(|| slug_from_url(&url));
            if number.is_empty() {
                continue;
            }
            let title = text_of(item, &title_sel);
            let released_at = text_of(item, &date_sel);

            out.push(Episode {
                id: 0,
                series_id: 0,
                number,
                title,
                url,
                released_at,
                seen: false,
                download_path: None,
                download_status: None,
            });
        }

        // The page lists newest first: sort ascending by numeric episode number,
        // preserving decimals (e.g. 13.5), and deduplicate by episode number.
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

        // Genres: tags in .genxed inside series detail
        let genre_sel = Selector::parse(".infox .genxed a, .genxed a").unwrap();
        let mut genres = Vec::new();
        for a in doc.select(&genre_sel) {
            let name = a.text().collect::<String>().trim().to_string();
            if !name.is_empty() && !genres.contains(&name) {
                genres.push(name);
            }
        }

        // Kind: metadata span in .spe (e.g. <span><b>Type:</b> HD</span>)
        let spe_span_sel = Selector::parse(".infox .spe span, .spe span").unwrap();
        let mut kind = None;
        for span in doc.select(&spe_span_sel) {
            let text = span.text().collect::<String>();
            let trimmed = text.trim();
            if let Some(rest) = trimmed.strip_prefix("Type:").or_else(|| trimmed.strip_prefix("Tipo:")) {
                if let Some(k) = media_kind(rest) {
                    kind = Some(k);
                    break;
                }
            }
        }

        // Synopsis: paragraph in .synp .entry-content or .entry-content[itemprop="description"]
        let p_sel = Selector::parse(".synp .entry-content p, .entry-content[itemprop=\"description\"] p").unwrap();
        let paragraphs: Vec<String> = doc
            .select(&p_sel)
            .map(|p| p.text().collect::<String>().trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        let synopsis = if !paragraphs.is_empty() {
            Some(paragraphs.join("\n\n"))
        } else {
            let fallback_sel = Selector::parse(".synp .entry-content, .entry-content[itemprop=\"description\"], .mindesc").unwrap();
            text_of(doc.root_element(), &fallback_sel)
        };

        Ok(SeriesDetail { genres, kind, synopsis })
    }

    fn search_url(&self, base_url: &str, query: &str) -> String {
        let encoded: String = url::form_urlencoded::byte_serialize(query.as_bytes()).collect();
        format!("{}/?s={encoded}", base_url.trim_end_matches('/'))
    }

    fn parse_search_results(&self, html: &str) -> Result<Vec<FinishedCard>> {
        let doc = Html::parse_document(html);
        let listupd_sel = Selector::parse(LISTUPD_CONTAINER).unwrap();
        if doc.select(&listupd_sel).next().is_none() {
            return Err(anyhow!("page does not look like animefenix search results"));
        }

        let card_sel = Selector::parse(CARD).unwrap();
        let a_sel = Selector::parse("a").unwrap();
        let tt_h2_sel = Selector::parse(".tt h2").unwrap();
        let tt_sel = Selector::parse(".tt").unwrap();
        let img_sel = Selector::parse("img").unwrap();
        let typez_sel = Selector::parse(".typez").unwrap();

        let mut out = Vec::new();
        for card in doc.select(&card_sel) {
            let Some(anchor) = card.select(&a_sel).next() else { continue };
            let Some(href) = anchor.value().attr("href") else { continue };
            let trimmed_href = href.trim();
            if trimmed_href.is_empty() {
                continue;
            }
            let url = abs(trimmed_href);
            let slug = slug_from_url(&url);
            if slug.is_empty() {
                continue;
            }

            let title = text_of(card, &tt_h2_sel)
                .or_else(|| {
                    anchor
                        .value()
                        .attr("title")
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(String::from)
                })
                .or_else(|| text_of(card, &tt_sel))
                .unwrap_or(slug);

            let poster_url = card
                .select(&img_sel)
                .next()
                .and_then(|img| {
                    img.value()
                        .attr("data-src")
                        .or_else(|| img.value().attr("src"))
                })
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(abs);

            let kind = text_of(card, &typez_sel).and_then(|t| media_kind(&t)).unwrap_or_else(|| "TV".to_string());

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
    fn airing_url_matches_expected() {
        assert_eq!(
            AnimefenixAdapter.airing_url("https://animefenix.com.co/"),
            "https://animefenix.com.co/anime/?status=ongoing&order=update"
        );
        assert_eq!(
            AnimefenixAdapter.airing_url("https://animefenix.com.co"),
            "https://animefenix.com.co/anime/?status=ongoing&order=update"
        );
    }

    #[test]
    fn airing_page_url_paginates() {
        assert_eq!(
            AnimefenixAdapter.airing_page_url("https://animefenix.com.co", 1),
            Some("https://animefenix.com.co/anime/?status=ongoing&order=update".to_string())
        );
        assert_eq!(
            AnimefenixAdapter.airing_page_url("https://animefenix.com.co/", 2),
            Some("https://animefenix.com.co/anime/?status=ongoing&order=update&page=2".to_string())
        );
        assert_eq!(
            AnimefenixAdapter.airing_page_url("https://animefenix.com.co", 3),
            Some("https://animefenix.com.co/anime/?status=ongoing&order=update&page=3".to_string())
        );
    }

    #[test]
    fn search_url_formats_query() {
        assert_eq!(
            AnimefenixAdapter.search_url("https://animefenix.com.co", "Shingeki no Kyojin"),
            "https://animefenix.com.co/?s=Shingeki+no+Kyojin"
        );
        assert_eq!(
            AnimefenixAdapter.search_url("https://animefenix.com.co/", "jujutsu"),
            "https://animefenix.com.co/?s=jujutsu"
        );
    }

    #[test]
    fn parse_airing_ongoing_order_update_fixture() {
        let html = include_str!("../../tests/fixtures/animefenix_list_anime__status_ongoing_order_update.html");
        let list = AnimefenixAdapter.parse_airing(html).unwrap();

        // Exactly the 30 .bsx cards in the main listing
        assert_eq!(list.len(), 30);

        for item in &list {
            assert!(
                item.url.starts_with("https://animefenix.com.co/"),
                "url not absolute: {}",
                item.url
            );
            assert!(item.is_airing, "airing item must have is_airing=true");
            assert!(!item.followed, "airing item must have followed=false");
            assert_eq!(item.next_episode_at, None);
        }

        // Verify first card
        let first = &list[0];
        assert_eq!(first.title, "Mission: Yozakura Family Season 2 Part 2");
        assert_eq!(first.slug, "mission-yozakura-family-season-2-part-2");
        assert_eq!(
            first.url,
            "https://animefenix.com.co/mission-yozakura-family-season-2-part-2/"
        );
        assert_eq!(
            first.cover_url,
            Some("https://i1.wp.com/animefenix.com.co/wp-content/uploads/2026/10/Mission-Yozakura-Family-Season-2.jpg".to_string())
        );
        assert_eq!(first.site_episode_count, None);

        // Verify last card
        let last = &list[29];
        assert_eq!(last.title, "Battle Spirits [Re]: Zekkai no Kuu");
        assert_eq!(last.slug, "battle-spirits-re-zekkai-no-kuu");
        assert_eq!(
            last.url,
            "https://animefenix.com.co/battle-spirits-re-zekkai-no-kuu/"
        );
    }

    #[test]
    fn parse_airing_ongoing_fixture() {
        let html = include_str!("../../tests/fixtures/animefenix_list_anime__status_ongoing.html");
        let list = AnimefenixAdapter.parse_airing(html).unwrap();
        assert_eq!(list.len(), 30);

        // Card 1 in this fixture (rel=6838) has empty title and missing image;
        // must fall back gracefully to slug without panicking.
        let empty_card = &list[0];
        assert_eq!(empty_card.slug, "6838");
        assert_eq!(empty_card.title, "6838");
        assert_eq!(empty_card.url, "https://animefenix.com.co/6838/");
        assert_eq!(empty_card.cover_url, None);
    }

    #[test]
    fn parse_search_results_hits_fixture() {
        let html = include_str!("../../tests/fixtures/animefenix_search_hits.html");
        let hits = AnimefenixAdapter.parse_search_results(html).unwrap();

        assert_eq!(hits.len(), 1);
        let card = &hits[0];
        // The HTML entity &#8211; must come out as a real dash (en-dash \u{2013})
        assert_eq!(card.title, "Jujutsu Kaisen: Shimetsu Kaiyuu – Zenpen");
        assert_eq!(
            card.url,
            "https://animefenix.com.co/jujutsu-kaisen-shimetsu-kaiyuu-zenpen/"
        );
        assert_eq!(card.kind, "TV", "the .typez badge is the quality (HD), not a type: default to TV");
        assert_eq!(
            card.poster_url,
            Some("https://i3.wp.com/animefenix.com.co/wp-content/uploads/2026/03/Jujutsu-Kaisen-Shimetsu-Kaiyuu-Zenpen.jpg".to_string())
        );
        assert_eq!(card.matched_genre, None);
    }

    #[test]
    fn parse_search_results_empty_fixture() {
        let html = include_str!("../../tests/fixtures/animefenix_search_empty.html");
        let hits = AnimefenixAdapter.parse_search_results(html).unwrap();
        assert!(hits.is_empty(), "empty search fixture must yield empty vector");
    }

    #[test]
    fn parse_search_results_rejects_non_site_html() {
        let res = AnimefenixAdapter.parse_search_results("<html></html>");
        assert!(res.is_err(), "html missing .listupd must yield Err");
    }

    #[test]
    fn parse_series_fixture() {
        let html = include_str!("../../tests/fixtures/animefenix_series.html");
        let eps = AnimefenixAdapter.parse_series(html).unwrap();

        assert_eq!(eps.len(), 1);
        let last = eps.last().unwrap();
        assert_eq!(last.number, "12");
        assert_eq!(
            last.url,
            "https://animefenix.com.co/jujutsu-kaisen-shimetsu-kaiyuu-zenpen-episode-12-sub-espanol/"
        );
        assert_eq!(
            last.title,
            Some("Jujutsu Kaisen: Shimetsu Kaiyuu - Zenpen".to_string())
        );
        assert_eq!(last.released_at, Some("marzo 26, 2026".to_string()));
        assert!(!last.seen);
    }

    #[test]
    fn parse_series_empty_input() {
        let eps = AnimefenixAdapter.parse_series("").unwrap();
        assert!(eps.is_empty());
    }

    #[test]
    fn parse_series_sorting_and_fractional_dedup() {
        let html = r#"
            <div class="eplister">
                <ul>
                    <li>
                        <a href="https://animefenix.com.co/series-episode-13-5-sub-espanol/">
                            <div class="epl-num">13.5</div>
                            <div class="epl-title">Recap</div>
                            <div class="epl-date">apr 1, 2026</div>
                        </a>
                    </li>
                    <li>
                        <a href="https://animefenix.com.co/series-episode-12-sub-espanol/">
                            <div class="epl-num">12</div>
                            <div class="epl-title">Episode 12</div>
                            <div class="epl-date">mar 26, 2026</div>
                        </a>
                    </li>
                    <li>
                        <a href="https://animefenix.com.co/series-episode-12-sub-espanol/">
                            <div class="epl-num">12</div>
                            <div class="epl-title">Episode 12 Duplicate</div>
                            <div class="epl-date">mar 26, 2026</div>
                        </a>
                    </li>
                    <li>
                        <a href="https://animefenix.com.co/series-episode-1-sub-espanol/">
                            <div class="epl-num">1</div>
                            <div class="epl-title">Episode 1</div>
                            <div class="epl-date">jan 1, 2026</div>
                        </a>
                    </li>
                </ul>
            </div>
        "#;
        let eps = AnimefenixAdapter.parse_series(html).unwrap();
        let numbers: Vec<&str> = eps.iter().map(|e| e.number.as_str()).collect();
        assert_eq!(numbers, vec!["1", "12", "13.5"]);
    }

    #[test]
    fn parse_series_detail_fixture() {
        let html = include_str!("../../tests/fixtures/animefenix_series.html");
        let detail = AnimefenixAdapter.parse_series_detail(html).unwrap();

        assert!(!detail.genres.is_empty(), "genres must not be empty");
        assert_eq!(detail.genres, vec!["Action", "Supernatural"]);
        assert_eq!(detail.kind, None, "the site's Type field is the quality (HD), not a media type");

        assert!(detail.synopsis.is_some(), "synopsis must be present");
        let syn = detail.synopsis.unwrap();
        assert!(!syn.is_empty(), "synopsis must not be empty");
        assert!(syn.contains("Jujutsu Kaisen: Shimetsu Kaiyuu"));
    }

    #[test]
    fn media_kind_keeps_real_types_and_drops_quality_labels() {
        assert_eq!(media_kind("TV").as_deref(), Some("TV"));
        assert_eq!(media_kind(" Movie ").as_deref(), Some("Movie"));
        assert_eq!(media_kind("OVA").as_deref(), Some("OVA"));
        assert_eq!(media_kind("Special").as_deref(), Some("Special"));
        for quality in ["HD", "FHD", "720p", "Donghua", "Yuri", ""] {
            assert_eq!(media_kind(quality), None, "{quality}");
        }
    }
}
