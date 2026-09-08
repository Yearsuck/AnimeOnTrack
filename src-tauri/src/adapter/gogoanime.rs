use super::{digits_in, slug_from_url, text_of, SiteAdapter};
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


/// The next-release timestamp for one `.epx.cndwn` countdown span, or `None`
/// when the site genuinely told us nothing.
///
/// `data-rlsdt` is the unix timestamp of the NEXT episode's release, and — as
/// the module comment above records — it is the **empty string** once that
/// episode has ALREADY released and the weekly `/schedule/` card hasn't
/// rolled over yet (the first card of `gogoanime_airing.html` is exactly
/// this shape). `"".parse::<i64>()` fails, so that used to collapse to
/// `None`, which is the same value the adapter reports for a card with no
/// countdown at all — throwing away the single strongest "check this series
/// now" signal the listing carries. `should_fetch_series` (commands/scan.rs)
/// branches on it directly: a `next_episode_at` in the past is an
/// unconditional fetch ("the episode aired and the card hasn't rolled over"),
/// while `None` falls through to the episode-count-badge rule, which happily
/// skips a series whose `.sb` badge hasn't caught up yet.
///
/// So an empty `data-rlsdt` resolves to a real timestamp in the *past*
/// instead. The sibling `data-cndwn` is the seconds-remaining countdown and
/// goes negative once the episode released (`-3406` on that same fixture card
/// = it aired ~57 minutes before the page was generated), which recovers
/// roughly when that happened — worth using, because `next_episode_at` also
/// drives the airing grid's "hace 57 min" chip and its weekday bucket, and
/// pinning every already-released card to the scrape instant would move them
/// all onto today. Without a usable (negative) `data-cndwn`, "just now" is
/// the honest fallback: still in the past, which is all the fetch decision
/// needs.
///
/// No `.epx.cndwn` span, or no `data-rlsdt` attribute at all, still yields
/// `None` — genuinely absent, exactly like `animeytx` treats it.
fn countdown_timestamp(el: scraper::ElementRef, now_unix: i64) -> Option<i64> {
    let rlsdt = el.value().attr("data-rlsdt")?.trim();
    if !rlsdt.is_empty() {
        return rlsdt.parse::<i64>().ok();
    }
    let elapsed = el
        .value()
        .attr("data-cndwn")
        .and_then(|s| s.trim().parse::<i64>().ok())
        .filter(|&secs| secs < 0)
        .unwrap_or(0);
    Some(now_unix + elapsed)
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

        // Read once for the whole page so every already-released card on this
        // scrape resolves against the same instant — see `countdown_timestamp`.
        let now_unix = chrono::Utc::now().timestamp();

        let mut out = Vec::new();
        for card in doc.select(&card_sel) {
            let Some((title, url, cover_url)) = card_basics(card, &a_sel, &tt_sel, &img_sel) else { continue };
            let next_episode_at = card
                .select(&cndwn_sel)
                .next()
                .and_then(|el| countdown_timestamp(el, now_unix));
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
        // Scoped to `.infox` (the series' own info block, confirmed against
        // the live fixture: `.typez`/`.genxed` are siblings inside it) rather
        // than searched page-wide — an unscoped `.typez` would grab the
        // FIRST match anywhere in the DOM, including a DooPlay "related/
        // recommended" sidebar widget's own `.typez` badge if one sits
        // earlier in the page than the series' own.
        let infox_sel = Selector::parse(".infox").unwrap();
        let typez_sel = Selector::parse(".typez").unwrap();
        let synopsis_sel = Selector::parse(".ninfo > p").unwrap();

        let genres = doc
            .select(&genxed_sel)
            .map(|a| a.text().collect::<String>().trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        let kind = doc.select(&infox_sel).next().and_then(|infox| text_of(infox, &typez_sel));
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
        // First card has an empty data-rlsdt (already released) alongside
        // data-cndwn="-3406". That must resolve to a timestamp in the PAST —
        // "due now" for should_fetch_series — not to None, which reads as
        // "this site carries no countdown signal at all".
        let now = chrono::Utc::now().timestamp();
        assert_eq!(out[0].slug, "haibara-kun-no-tsuyokute-seishun-new-game");
        let released = out[0].next_episode_at.expect("an already-released card still has a countdown signal");
        assert!(released < now, "an already-released episode must be in the past: {released} vs {now}");
        assert!(
            (now - released - 3406).abs() <= 5,
            "recovered from data-cndwn=-3406 (~57 min ago), got {} secs ago",
            now - released
        );
        assert_eq!(out[0].site_episode_count, Some(13));
        // Second card has a real populated data-rlsdt.
        assert_eq!(out[1].slug, "xiao-lu-he-xiao-lan-5th-season");
        assert_eq!(out[1].next_episode_at, Some(1784779500));
        assert_eq!(out[1].site_episode_count, Some(10));
    }

    /// `countdown_timestamp` in isolation, against a fixed "now" so the
    /// already-released branch is deterministic.
    #[test]
    fn countdown_timestamp_resolves_every_documented_shape() {
        const NOW: i64 = 1_800_000_000;
        let parse_one = |html: &str| -> Option<i64> {
            let doc = Html::parse_fragment(html);
            let sel = Selector::parse(".epx.cndwn").unwrap();
            doc.select(&sel).next().and_then(|el| countdown_timestamp(el, NOW))
        };

        // A populated data-rlsdt is taken verbatim.
        assert_eq!(
            parse_one(r#"<span class="epx cndwn" data-cndwn="22694" data-rlsdt="1784779500">0d 6h</span>"#),
            Some(1_784_779_500)
        );
        // Empty data-rlsdt + negative data-cndwn: released that many seconds ago.
        assert_eq!(
            parse_one(r#"<span class="epx cndwn" data-cndwn="-3406" data-rlsdt="">01:50</span>"#),
            Some(NOW - 3406)
        );
        // Empty data-rlsdt with no usable data-cndwn: "just now", still past-or-now.
        assert_eq!(
            parse_one(r#"<span class="epx cndwn" data-rlsdt="">01:50</span>"#),
            Some(NOW)
        );
        assert_eq!(
            parse_one(r#"<span class="epx cndwn" data-cndwn="nope" data-rlsdt="  ">01:50</span>"#),
            Some(NOW)
        );
        // A positive data-cndwn alongside an empty data-rlsdt shouldn't happen
        // and must never be read as a FUTURE release (that would re-introduce
        // the skip this fix exists to remove).
        assert_eq!(
            parse_one(r#"<span class="epx cndwn" data-cndwn="500" data-rlsdt="">01:50</span>"#),
            Some(NOW)
        );
        // No data-rlsdt attribute at all: genuinely absent, same as animeytx.
        assert_eq!(parse_one(r#"<span class="epx cndwn" data-cndwn="-10">01:50</span>"#), None);
        // Present but unparseable: no timestamp can be recovered.
        assert_eq!(parse_one(r#"<span class="epx cndwn" data-rlsdt="soon">01:50</span>"#), None);
    }

    /// The whole point of the fix: an already-released card is "due now" for
    /// the refresh skip logic, exactly like a real past timestamp is.
    #[test]
    fn an_already_released_card_reads_as_due_now() {
        let html = include_str!("../../tests/fixtures/gogoanime_airing.html");
        let out = GogoanimeAdapter.parse_airing(html).unwrap();
        let now = chrono::Utc::now().timestamp();
        assert!(
            out[0].next_episode_at.is_some_and(|t| t <= now),
            "should_fetch_series' `next_episode_at <= now` fetch rule must fire for it"
        );
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

    /// Regression: an unscoped `.typez` lookup would grab a sidebar widget's
    /// badge instead of the series' own, if the widget sits earlier in the
    /// DOM — scoping to `.infox` must ignore it.
    #[test]
    fn parse_series_detail_ignores_a_typez_badge_outside_infox() {
        let html = r#"<html><body>
            <div class="sidebar-widget"><div class="bsx"><span class="typez">Movie</span></div></div>
            <div class="infox">
                <div class="ninfo"><p>Synopsis text.</p></div>
                <span class="typez">TV Show</span>
                <div class="genxed"><a href="/genre/comedy/">Comedy</a></div>
            </div>
        </body></html>"#;
        let d = GogoanimeAdapter.parse_series_detail(html).unwrap();
        assert_eq!(d.kind.as_deref(), Some("TV Show"));
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
