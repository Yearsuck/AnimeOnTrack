//! Parsing layer that resolves episode pages and embed pages into downloadable media URLs.
//!
//! This module is completely pure and performs no network requests, filesystem access,
//! or other I/O. All inputs are provided as in-memory HTML/URL strings and parsed using
//! `base64`, `scraper`, `serde_json`, and `url`.

use base64::Engine;
use scraper::{Html, Selector};
use std::collections::HashSet;
use std::sync::OnceLock;
use url::Url;

/// Streaming server option found on an episode page.
#[allow(dead_code)] // Consumed by download supervisor in later stages
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Server {
    pub name: String,
    pub url: String,
}

/// Direct media resource resolved from an embed page ready for HTTP download.
#[allow(dead_code)] // Consumed by download supervisor in later stages
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Media {
    pub url: String,
    pub referer: String,
    pub host: &'static str,
}

/// Player tab parsed from a Mytsumi container runtime page.
#[allow(dead_code)] // Consumed by download supervisor in later stages
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MytsumiTab {
    pub label: String,
    pub url: String,
    pub direct_video: bool,
    pub quality: Option<u32>,
}

/// Helper that extracts the full JSON array substring starting at `[` in `s`.
/// Handles quotes (`"` and `'`), backslash escapes, and bracket nesting.
fn extract_json_array(s: &str) -> Option<&str> {
    if !s.starts_with('[') {
        return None;
    }
    let mut depth = 0;
    let mut in_quote: Option<char> = None;
    let mut escaped = false;

    for (i, ch) in s.char_indices() {
        if let Some(q) = in_quote {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == q {
                in_quote = None;
            }
        } else {
            match ch {
                '"' | '\'' => in_quote = Some(ch),
                '[' => depth += 1,
                ']' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(&s[..i + ch.len_utf8()]);
                    }
                }
                _ => {}
            }
        }
    }
    None
}

/// Locates the `videos = [...]` assignment in TioAnime HTML and parses its entries.
fn extract_videos_json(html: &str) -> Option<Vec<Server>> {
    let mut search_from = 0;
    while let Some(pos) = html[search_from..].find("videos") {
        let idx = search_from + pos;
        search_from = idx + 6; // length of "videos"

        // Ensure "videos" is not preceded by an identifier character (e.g. `prev_videos`)
        if idx > 0 {
            if let Some(prev_ch) = html[..idx].chars().next_back() {
                if prev_ch.is_alphanumeric() || prev_ch == '_' || prev_ch == '$' {
                    continue;
                }
            }
        }

        let after = &html[idx + 6..];
        let trimmed_after = after.trim_start();
        if !trimmed_after.starts_with('=') {
            continue;
        }

        let after_eq = trimmed_after[1..].trim_start();
        if !after_eq.starts_with('[') {
            continue;
        }

        let Some(json_slice) = extract_json_array(after_eq) else {
            continue;
        };

        let Ok(val) = serde_json::from_str::<serde_json::Value>(json_slice) else {
            continue;
        };

        let Some(items) = val.as_array() else {
            continue;
        };

        let mut servers = Vec::new();
        for item in items {
            let Some(entry) = item.as_array() else {
                continue;
            };
            if entry.len() < 2 {
                continue;
            }
            let (Some(name), Some(url_str)) = (entry[0].as_str(), entry[1].as_str()) else {
                continue;
            };
            let Ok(parsed_url) = Url::parse(url_str) else {
                continue;
            };
            if parsed_url.scheme() != "http" && parsed_url.scheme() != "https" {
                continue;
            }
            servers.push(Server {
                name: name.to_string(),
                url: url_str.to_string(),
            });
        }
        return Some(servers);
    }
    None
}

/// Parses the server options from a TioAnime episode HTML page.
///
/// Returns the entries of the `videos` array in page order as `Server`s,
/// skipping entries without a valid string name/url or whose url is not `http`/`https`.
/// Returns an empty vector when the array is missing or malformed. Never panics.
#[allow(dead_code)] // Consumed by download supervisor in later stages
pub fn parse_tioanime_servers(html: &str) -> Vec<Server> {
    extract_videos_json(html).unwrap_or_default()
}

/// Decodes a base64 remote URL, tolerating whitespace and missing padding.
/// Returns Some(url) only if decoded successfully, is valid UTF-8, and is http(s).
fn decode_base64_url(raw: &str) -> Option<String> {
    let mut clean: String = raw.chars().filter(|c| !c.is_whitespace()).collect();
    let rem = clean.len() % 4;
    if rem == 2 {
        clean.push_str("==");
    } else if rem == 3 {
        clean.push('=');
    } else if rem == 1 {
        return None;
    }

    let decoded_bytes = base64::engine::general_purpose::STANDARD.decode(clean.as_bytes()).ok()?;
    let s = std::str::from_utf8(&decoded_bytes).ok()?;
    let trimmed = s.trim();
    let parsed = Url::parse(trimmed).ok()?;
    if parsed.scheme() != "http" && parsed.scheme() != "https" {
        return None;
    }
    Some(trimmed.to_string())
}

/// Locates the `servers = [...]` assignment in JKAnime HTML and parses its entries.
fn extract_jkanime_servers_json(html: &str) -> Option<Vec<Server>> {
    let mut search_from = 0;
    while let Some(pos) = html[search_from..].find("servers") {
        let idx = search_from + pos;
        search_from = idx + 7; // length of "servers"

        // Ensure "servers" is not preceded by an identifier character (e.g. `bg_servers`)
        if idx > 0 {
            if let Some(prev_ch) = html[..idx].chars().next_back() {
                if prev_ch.is_alphanumeric() || prev_ch == '_' || prev_ch == '$' {
                    continue;
                }
            }
        }

        let after = &html[idx + 7..];
        let trimmed_after = after.trim_start();
        if !trimmed_after.starts_with('=') {
            continue;
        }

        let after_eq = trimmed_after[1..].trim_start();
        if !after_eq.starts_with('[') {
            continue;
        }

        let Some(json_slice) = extract_json_array(after_eq) else {
            continue;
        };

        let Ok(val) = serde_json::from_str::<serde_json::Value>(json_slice) else {
            continue;
        };

        let Some(items) = val.as_array() else {
            continue;
        };

        let mut servers = Vec::new();
        for item in items {
            let Some(obj) = item.as_object() else {
                continue;
            };
            let (Some(name), Some(remote)) = (
                obj.get("server").and_then(|v| v.as_str()),
                obj.get("remote").and_then(|v| v.as_str()),
            ) else {
                continue;
            };
            let Some(url) = decode_base64_url(remote) else {
                continue;
            };
            servers.push(Server {
                name: name.to_string(),
                url,
            });
        }
        return Some(servers);
    }
    None
}

/// Parses the server options from a JKAnime episode HTML page.
///
/// Finds the `servers = [...]` JSON array, decodes each server's base64 `remote` URL,
/// and returns valid http(s) servers in page order. Skips entries that fail to decode,
/// are not valid UTF-8, or are not `http`/`https`. Returns an empty vector on missing
/// or malformed inputs. Never panics.
#[allow(dead_code)] // Consumed by download supervisor in later stages
pub fn parse_jkanime_servers(html: &str) -> Vec<Server> {
    extract_jkanime_servers_json(html).unwrap_or_default()
}

/// Parses the Mytsumi player container URL from an AnimeYT episode HTML page.
///
/// Looks for an iframe `src`, `data-src`, or `data-lazy-src` attribute whose host is
/// `mytsumi.com` (or a subdomain) and path is `/container.php`. Returns the URL with the
/// query parameter `open=1` set (replacing an existing `open` or appending it), preserving
/// other parameters in order. Returns `None` if no matching iframe is found.
#[allow(dead_code)] // Consumed by download supervisor in later stages
pub fn parse_animeyt_container_url(html: &str) -> Option<String> {
    static IFRAME_SELECTOR: OnceLock<Selector> = OnceLock::new();
    let selector = IFRAME_SELECTOR.get_or_init(|| {
        Selector::parse("iframe").expect("valid iframe selector")
    });

    let doc = Html::parse_document(html);
    for el in doc.select(selector) {
        for attr_name in &["src", "data-src", "data-lazy-src"] {
            let Some(val) = el.value().attr(attr_name) else {
                continue;
            };
            let Ok(parsed) = Url::parse(val.trim()) else {
                continue;
            };
            if parsed.scheme() != "http" && parsed.scheme() != "https" {
                continue;
            }
            let Some(host) = parsed.host_str() else {
                continue;
            };
            let host_lower = host.to_ascii_lowercase();
            if (host_lower == "mytsumi.com" || host_lower.ends_with(".mytsumi.com"))
                && parsed.path() == "/container.php"
            {
                let mut pairs: Vec<(String, String)> = Vec::new();
                let mut found_open = false;
                for (k, v) in parsed.query_pairs() {
                    if k == "open" {
                        if !found_open {
                            pairs.push(("open".to_string(), "1".to_string()));
                            found_open = true;
                        }
                    } else {
                        pairs.push((k.into_owned(), v.into_owned()));
                    }
                }
                if !found_open {
                    pairs.push(("open".to_string(), "1".to_string()));
                }

                let mut new_url = parsed.clone();
                new_url.set_query(None);
                {
                    let mut qp = new_url.query_pairs_mut();
                    for (k, v) in &pairs {
                        qp.append_pair(k, v);
                    }
                }
                return Some(new_url.to_string());
            }
        }
    }
    None
}

/// Helper that extracts the quality number before `p` after the last `·` in a tab title.
fn parse_tab_quality(title: &str) -> Option<u32> {
    let pos = title.rfind('·')?;
    let segment = &title[pos + '·'.len_utf8()..];
    let trimmed = segment.trim();
    let p_pos = trimmed.find(['p', 'P'])?;
    trimmed[..p_pos].trim().parse::<u32>().ok()
}

/// Parses the player tabs from a Mytsumi runtime container HTML page.
///
/// Finds every `button.mytsumi-tab[data-player-url]` in page order. Tabs with
/// `data-player-kind="video"` are marked as direct video. Quality is parsed from the
/// title attribute (`... · 720p` -> `Some(720)`). Skips tabs whose URL is not http(s).
#[allow(dead_code)] // Consumed by download supervisor in later stages
pub fn parse_mytsumi_tabs(html: &str) -> Vec<MytsumiTab> {
    static TAB_SELECTOR: OnceLock<Selector> = OnceLock::new();
    let selector = TAB_SELECTOR.get_or_init(|| {
        Selector::parse("button.mytsumi-tab[data-player-url]").expect("valid tab selector")
    });

    let doc = Html::parse_document(html);
    let mut tabs = Vec::new();

    for el in doc.select(selector) {
        let Some(raw_url) = el.value().attr("data-player-url") else {
            continue;
        };
        let trimmed_url = raw_url.trim();
        let Ok(parsed_url) = Url::parse(trimmed_url) else {
            continue;
        };
        if parsed_url.scheme() != "http" && parsed_url.scheme() != "https" {
            continue;
        }

        let label = el
            .value()
            .attr("data-player-label")
            .map(|s| s.trim())
            .unwrap_or("")
            .to_string();

        let direct_video = el
            .value()
            .attr("data-player-kind")
            .map(|k| k.trim() == "video")
            .unwrap_or(false);

        let quality = el.value().attr("title").and_then(parse_tab_quality);

        tabs.push(MytsumiTab {
            label,
            url: trimmed_url.to_string(),
            direct_video,
            quality,
        });
    }

    tabs
}

/// Selects the best direct video media from a list of Mytsumi tabs.
///
/// Prefers direct video tabs whose URL path ends in `.mp4` (case-insensitive, ignoring
/// query/fragment), choosing the highest quality (`None` counts as 0). Ties preserve page
/// order. Returns `None` if no qualifying direct tab exists.
#[allow(dead_code)] // Consumed by download supervisor in later stages
pub fn mytsumi_direct_media(tabs: &[MytsumiTab]) -> Option<Media> {
    let mut best: Option<&MytsumiTab> = None;
    let mut best_quality: u32 = 0;

    for tab in tabs {
        if tab.direct_video && ends_with_mp4(&tab.url) {
            let q = tab.quality.unwrap_or(0);
            match best {
                None => {
                    best = Some(tab);
                    best_quality = q;
                }
                Some(_) => {
                    if q > best_quality {
                        best = Some(tab);
                        best_quality = q;
                    }
                }
            }
        }
    }

    best.map(|tab| Media {
        url: tab.url.clone(),
        referer: "https://mytsumi.com/".to_string(),
        host: "direct",
    })
}

/// Helper that checks whether a URL path ends with `.mp4` (case-insensitive, ignoring query/hash).
fn ends_with_mp4(url: &str) -> bool {
    let without_hash = url.split('#').next().unwrap_or(url);
    let path = without_hash.split('?').next().unwrap_or(without_hash);
    path.to_ascii_lowercase().ends_with(".mp4")
}

/// Helper that resolves a raw URL string into an absolute http(s) URL.
fn resolve_http_url(candidate: &str, base_url: &str) -> Option<String> {
    let trimmed = candidate.trim();
    if trimmed.is_empty() {
        return None;
    }
    // Check if candidate is already a valid absolute URL
    if let Ok(parsed) = Url::parse(trimmed) {
        if parsed.scheme() == "http" || parsed.scheme() == "https" {
            return Some(parsed.to_string());
        }
        // Non-http schemes (e.g. javascript:, data:) are rejected
        return None;
    }
    // Attempt to join relative path against the base URL
    if let Ok(base) = Url::parse(base_url) {
        if let Ok(joined) = base.join(trimmed) {
            if joined.scheme() == "http" || joined.scheme() == "https" {
                return Some(joined.to_string());
            }
        }
    }
    None
}

/// Looks for the `<meta property="og:video" content="...">` tag in the embed HTML.
fn find_og_video(html: &str, base_url: &str) -> Option<String> {
    static OG_SELECTOR: OnceLock<Selector> = OnceLock::new();
    let selector = OG_SELECTOR.get_or_init(|| {
        Selector::parse(r#"meta[property="og:video"], meta[name="og:video"]"#)
            .expect("valid og:video selector")
    });

    let doc = Html::parse_document(html);
    for el in doc.select(selector) {
        if let Some(content) = el.value().attr("content") {
            if let Some(resolved) = resolve_http_url(content, base_url) {
                return Some(resolved);
            }
        }
    }
    None
}

/// Scans HTML/JS for the first `<prop_name>: '...'` or `<prop_name>: "..."` property
/// whose URL ends in `.mp4` (case-insensitive, ignoring query/hash) and resolves to http(s).
fn find_first_mp4_property(prop_name: &str, html: &str, base_url: &str) -> Option<String> {
    let prop_len = prop_name.len();
    let mut search_from = 0;
    while let Some(pos) = html[search_from..].find(prop_name) {
        let prop_idx = search_from + pos;
        search_from = prop_idx + prop_len;

        // Ensure property name is preceded by a non-identifier character (if not at start)
        if prop_idx > 0 {
            if let Some(prev_ch) = html[..prop_idx].chars().next_back() {
                if prev_ch.is_alphanumeric() || prev_ch == '_' || prev_ch == '$' {
                    continue;
                }
            }
        }

        let after = &html[prop_idx + prop_len..];
        let trimmed_after = after.trim_start();
        if !trimmed_after.starts_with(':') {
            continue;
        }

        let after_colon = trimmed_after[1..].trim_start();
        let Some(quote_char) = after_colon.chars().next() else {
            continue;
        };
        if quote_char != '\'' && quote_char != '"' {
            continue;
        }

        let content = &after_colon[quote_char.len_utf8()..];
        let mut escaped = false;
        let mut matched_val = None;
        for (i, ch) in content.char_indices() {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == quote_char {
                matched_val = Some(&content[..i]);
                break;
            }
        }

        let Some(candidate) = matched_val else {
            continue;
        };

        if !ends_with_mp4(candidate) {
            continue;
        }

        if let Some(resolved) = resolve_http_url(candidate, base_url) {
            return Some(resolved);
        }
    }
    None
}

/// Scans the HTML/JS text for the first `file: '...'` or `file: "..."` property
/// whose URL ends in `.mp4` (case-insensitive, ignoring query/hash) and resolves to http(s).
fn find_first_mp4_file(html: &str, base_url: &str) -> Option<String> {
    find_first_mp4_property("file", html, base_url)
}

/// Scans the HTML/JS text for the first `src: '...'` or `src: "..."` property
/// whose URL ends in `.mp4` (case-insensitive, ignoring query/hash) and resolves to http(s).
fn find_first_mp4_src(html: &str, base_url: &str) -> Option<String> {
    find_first_mp4_property("src", html, base_url)
}

/// Resolves a YourUpload embed page into downloadable media metadata.
///
/// Prefers the `og:video` meta content; otherwise falls back to the first `file: '...'`
/// value whose URL ends in `.mp4` (case-insensitive, ignoring a query string).
/// Resolves relative URLs against `embed_url` with `Url::join`.
/// Returns `None` when nothing qualifies.
#[allow(dead_code)] // Consumed by download supervisor in later stages
pub fn resolve_yourupload(embed_html: &str, embed_url: &str) -> Option<Media> {
    let media_url = find_og_video(embed_html, embed_url)
        .or_else(|| find_first_mp4_file(embed_html, embed_url))?;

    Some(Media {
        url: media_url,
        referer: "https://www.yourupload.com/".to_string(),
        host: "yourupload",
    })
}

/// Resolves an Mp4upload embed page into downloadable media metadata.
///
/// Searches for the first `src: "..."`/`src: '...'` property whose URL path ends in `.mp4`,
/// resolved to an absolute http(s) URL. Returns `None` if no mp4 src is found.
#[allow(dead_code)] // Consumed by download supervisor in later stages
pub fn resolve_mp4upload(embed_html: &str, embed_url: &str) -> Option<Media> {
    let media_url = find_first_mp4_src(embed_html, embed_url)?;
    Some(Media {
        url: media_url,
        referer: "https://www.mp4upload.com/".to_string(),
        host: "mp4upload",
    })
}

/// Resolves a Mediafire file page into direct downloadable media metadata.
///
/// Finds the element with `id="downloadButton"` using scraper, validates that its `href`
/// is an absolute http(s) URL hosted on `mediafire.com` or a subdomain.
#[allow(dead_code)] // Consumed by download supervisor in later stages
pub fn resolve_mediafire(page_html: &str, _page_url: &str) -> Option<Media> {
    static MEDIAFIRE_SELECTOR: OnceLock<Selector> = OnceLock::new();
    let selector = MEDIAFIRE_SELECTOR.get_or_init(|| {
        Selector::parse("#downloadButton").expect("valid #downloadButton selector")
    });

    let doc = Html::parse_document(page_html);
    let el = doc.select(selector).next()?;
    let raw_href = el.value().attr("href")?;
    let trimmed = raw_href.trim();

    let parsed = Url::parse(trimmed).ok()?;
    if parsed.scheme() != "http" && parsed.scheme() != "https" {
        return None;
    }
    let host = parsed.host_str()?;
    let host_lower = host.to_ascii_lowercase();
    if host_lower != "mediafire.com" && !host_lower.ends_with(".mediafire.com") {
        return None;
    }

    Some(Media {
        url: trimmed.to_string(),
        referer: "https://www.mediafire.com/".to_string(),
        host: "mediafire",
    })
}

/// Finds the last `getElementById('robotlink').innerHTML = ...` assignment in HTML.
fn find_last_streamtape_assignment(html: &str) -> Option<&str> {
    let mut last_after_eq = None;
    let mut search_from = 0;

    while let Some(pos) = html[search_from..].find("robotlink") {
        let idx = search_from + pos;
        search_from = idx + "robotlink".len();

        let before = &html[..idx];
        let trimmed_before = before.trim_end();
        let Some(quote_char) = trimmed_before.chars().next_back() else {
            continue;
        };
        if quote_char != '\'' && quote_char != '"' {
            continue;
        }

        let before_quote = trimmed_before[..trimmed_before.len() - quote_char.len_utf8()].trim_end();
        if !before_quote.ends_with("getElementById(") {
            continue;
        }

        let after = &html[idx + "robotlink".len()..];
        let trimmed_after = after.trim_start();
        let Some(after_quote) = trimmed_after.chars().next() else {
            continue;
        };
        if after_quote != quote_char {
            continue;
        }

        let after_quote_str = trimmed_after[quote_char.len_utf8()..].trim_start();
        if !after_quote_str.starts_with(')') {
            continue;
        }

        let after_paren = after_quote_str[1..].trim_start();
        if !after_paren.starts_with(".innerHTML") {
            continue;
        }

        let after_inner = after_paren[".innerHTML".len()..].trim_start();
        if !after_inner.starts_with('=') {
            continue;
        }

        last_after_eq = Some(&after_inner[1..]);
    }

    last_after_eq
}

/// Resolves a Streamtape embed page into downloadable media metadata.
///
/// Implements the JS assembly for the `robotlink` assignment: reads the string literal prefix,
/// the parenthesised quoted string, applies chained `.substring(n)` calls in order (clamping safely
/// on Unicode scalar boundaries), prepends `https:` if starting with `//`, and verifies the host
/// is `streamtape.com` or a subdomain.
#[allow(dead_code)] // Consumed by download supervisor in later stages
pub fn resolve_streamtape(embed_html: &str) -> Option<Media> {
    let after_eq = find_last_streamtape_assignment(embed_html)?;

    let trimmed = after_eq.trim_start();
    let quote_char = trimmed.chars().next()?;
    if quote_char != '\'' && quote_char != '"' {
        return None;
    }
    let after_quote = &trimmed[quote_char.len_utf8()..];
    let mut escaped = false;
    let mut prefix_end = None;
    for (i, ch) in after_quote.char_indices() {
        if escaped {
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else if ch == quote_char {
            prefix_end = Some(i);
            break;
        }
    }
    let prefix_idx = prefix_end?;
    let prefix = &after_quote[..prefix_idx];
    let after_prefix = &after_quote[prefix_idx + quote_char.len_utf8()..];

    let open_paren_pos = after_prefix.find('(')?;
    let after_open_paren = after_prefix[open_paren_pos + 1..].trim_start();
    let inner_quote_char = after_open_paren.chars().next()?;
    if inner_quote_char != '\'' && inner_quote_char != '"' {
        return None;
    }
    let after_inner_quote = &after_open_paren[inner_quote_char.len_utf8()..];
    let mut escaped2 = false;
    let mut inner_end = None;
    for (i, ch) in after_inner_quote.char_indices() {
        if escaped2 {
            escaped2 = false;
        } else if ch == '\\' {
            escaped2 = true;
        } else if ch == inner_quote_char {
            inner_end = Some(i);
            break;
        }
    }
    let inner_idx = inner_end?;
    let inner_str = &after_inner_quote[..inner_idx];
    let after_inner_str = &after_inner_quote[inner_idx + inner_quote_char.len_utf8()..];

    let close_paren_pos = after_inner_str.find(')')?;
    let mut after_close_paren = &after_inner_str[close_paren_pos + 1..];

    let mut transformed = inner_str.to_string();
    loop {
        let trimmed_rest = after_close_paren.trim_start();
        if !trimmed_rest.starts_with(".substring(") {
            break;
        }
        let after_sub = &trimmed_rest[".substring(".len()..];
        let Some(close_pos) = after_sub.find(')') else {
            break;
        };
        let num_str = after_sub[..close_pos].trim();
        let Ok(n) = num_str.parse::<usize>() else {
            break;
        };
        transformed = transformed.chars().skip(n).collect();
        after_close_paren = &after_sub[close_pos + 1..];
    }

    let mut full = format!("{}{}", prefix, transformed);
    if full.starts_with("//") {
        full = format!("https:{}", full);
    }

    let parsed = Url::parse(&full).ok()?;
    if parsed.scheme() != "http" && parsed.scheme() != "https" {
        return None;
    }
    let host = parsed.host_str()?;
    let host_lower = host.to_ascii_lowercase();
    if host_lower != "streamtape.com" && !host_lower.ends_with(".streamtape.com") {
        return None;
    }

    Some(Media {
        url: full,
        referer: "https://streamtape.com/".to_string(),
        host: "streamtape",
    })
}

/// Encodes an integer `n` into the Dean Edwards Packer base-N representation with radix `radix`.
/// Supports radix 2..=62 using characters 0..=9, a..=z, A..=Z.
fn packer_base(mut n: usize, radix: usize) -> String {
    if !(2..=62).contains(&radix) {
        return String::new();
    }
    let digit = |d: usize| -> char {
        match d {
            0..=9 => (b'0' + d as u8) as char,
            10..=35 => (b'a' + (d - 10) as u8) as char,
            36..=61 => (b'A' + (d - 36) as u8) as char,
            _ => '0',
        }
    };
    if n == 0 {
        return "0".to_string();
    }
    let mut chars = Vec::new();
    while n > 0 {
        chars.push(digit(n % radix));
        n /= radix;
    }
    chars.reverse();
    chars.into_iter().collect()
}

/// Helper that checks if a character is an ASCII word character [A-Za-z0-9_].
fn is_word_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// Replaces all occurrences of `token` in `text` that match regex `\b<token>\b` with `replacement`.
fn replace_word_boundary(text: &str, token: &str, replacement: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let token_len = token.len();
    let mut i = 0;

    while i < text.len() {
        if let Some(sub) = text.get(i..) {
            if sub.starts_with(token) {
                let prev_is_word = text
                    .get(..i)
                    .and_then(|s| s.chars().next_back())
                    .is_some_and(is_word_char);
                let next_idx = i + token_len;
                let next_is_word = text
                    .get(next_idx..)
                    .and_then(|s| s.chars().next())
                    .is_some_and(is_word_char);

                if !prev_is_word && !next_is_word {
                    result.push_str(replacement);
                    i = next_idx;
                    continue;
                }
            }
        }
        let Some(ch) = text.get(i..).and_then(|s| s.chars().next()) else {
            break;
        };
        result.push(ch);
        i += ch.len_utf8();
    }
    result
}

/// Finds the index of the `(` opening the packer function argument list.
/// The `(` must be immediately preceded by `}` (ignoring whitespace).
fn find_packer_args(slice: &str) -> Option<usize> {
    let mut search_from = 0;
    while let Some(sub) = slice.get(search_from..) {
        let pos = sub.find('(')?;
        let abs_pos = search_from + pos;
        search_from = abs_pos + 1;
        let before = slice.get(..abs_pos)?;
        if before.trim_end().ends_with('}') {
            return Some(abs_pos);
        }
    }
    None
}

/// Attempts to parse and unpack a single Dean Edwards P.A.C.K.E.R. invocation starting at `eval(...)`.
fn try_unpack(slice: &str) -> Option<String> {
    let open_paren = find_packer_args(slice)?;
    let after_paren = slice.get(open_paren + 1..)?.trim_start();
    let quote = after_paren.chars().next()?;
    if quote != '\'' && quote != '"' {
        return None;
    }
    let after_quote = after_paren.get(quote.len_utf8()..)?;

    let mut escaped = false;
    let mut p_end = None;
    for (i, ch) in after_quote.char_indices() {
        if escaped {
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else if ch == quote {
            p_end = Some(i);
            break;
        }
    }
    let p_end = p_end?;
    let raw_p = after_quote.get(..p_end)?;
    let rest = after_quote.get(p_end + quote.len_utf8()..)?;

    let trimmed_rest = rest.trim_start();
    if !trimmed_rest.starts_with(',') {
        return None;
    }
    let after_comma1 = trimmed_rest.get(1..)?.trim_start();
    let comma2_pos = after_comma1.find(',')?;
    let radix: usize = after_comma1.get(..comma2_pos)?.trim().parse().ok()?;
    if !(2..=62).contains(&radix) {
        return None;
    }

    let after_comma2 = after_comma1.get(comma2_pos + 1..)?.trim_start();
    let comma3_pos = after_comma2.find(',')?;
    let count: usize = after_comma2.get(..comma3_pos)?.trim().parse().ok()?;
    if count > 100_000 {
        return None;
    }

    let after_comma3 = after_comma2.get(comma3_pos + 1..)?.trim_start();
    let words_quote = after_comma3.chars().next()?;
    if words_quote != '\'' && words_quote != '"' {
        return None;
    }
    let words_content = after_comma3.get(words_quote.len_utf8()..)?;

    let split_pos = words_content.find(".split(")?;
    let before_split = words_content.get(..split_pos)?.trim_end();
    if !before_split.ends_with(words_quote) {
        return None;
    }
    let words_len = before_split.len().checked_sub(words_quote.len_utf8())?;
    let words_str = before_split.get(..words_len)?;

    let after_split = words_content.get(split_pos + ".split(".len()..)?;
    let after_split_trimmed = after_split.trim_start();
    if !after_split_trimmed.starts_with("'|'")
        && !after_split_trimmed.starts_with(r#""|""#)
        && !after_split_trimmed.starts_with(r"\'|\'")
    {
        return None;
    }

    let mut p = if quote == '\'' {
        raw_p.replace(r"\'", "'").replace(r"\\", "\\")
    } else {
        raw_p.replace(r#"\""#, "\"").replace(r"\\", "\\")
    };

    let k: Vec<&str> = words_str.split('|').collect();

    for c in (0..count).rev() {
        let word = match k.get(c) {
            Some(&w) if !w.is_empty() => w,
            _ => continue,
        };
        let token = packer_base(c, radix);
        if !p.contains(&token) {
            continue;
        }
        p = replace_word_boundary(&p, &token, word);
        if p.len() > 10 * 1024 * 1024 {
            return None;
        }
    }

    Some(p)
}

/// Unpacks a script packed with Dean Edwards P.A.C.K.E.R.
///
/// Finds the first `eval(function(p,a,c,k,e,d)` occurrence that parses correctly and executes
/// the unpacking algorithm sequentially from highest index down to 0, matching `\b<token>\b`.
/// Returns `None` if absent, malformed, or invalid (e.g. radix > 62 or count > 100,000). Never panics.
#[allow(dead_code)] // Consumed by download supervisor in later stages
pub fn unpack_packer(js: &str) -> Option<String> {
    if js.len() > 2 * 1024 * 1024 {
        return None;
    }
    let mut search_from = 0;
    while let Some(sub) = js.get(search_from..) {
        let Some(pos) = sub.find("eval(function(p,a,c,k,e,d)") else {
            break;
        };
        let eval_idx = search_from + pos;
        search_from = eval_idx + "eval(function(p,a,c,k,e,d)".len();
        if let Some(eval_slice) = js.get(eval_idx..) {
            if let Some(unpacked) = try_unpack(eval_slice) {
                return Some(unpacked);
            }
        }
    }
    None
}

/// Extracts the JSON string value of a specified key (e.g. `"hls4":"/stream/..."`) from a JS/JSON text.
fn extract_json_property_string(text: &str, key: &str) -> Option<String> {
    let key_pat1 = format!("\"{}\"", key);
    let key_pat2 = format!("'{}'", key);

    let mut search_from = 0;
    while search_from < text.len() {
        let sub = text.get(search_from..)?;
        let found_pos = match (sub.find(&key_pat1), sub.find(&key_pat2)) {
            (Some(p1), Some(p2)) => Some(search_from + p1.min(p2)),
            (Some(p1), None) => Some(search_from + p1),
            (None, Some(p2)) => Some(search_from + p2),
            (None, None) => None,
        };

        let key_idx = found_pos?;
        let key_len = key.len() + 2; // includes enclosing quotes
        let after_key = text.get(key_idx + key_len..)?;
        search_from = key_idx + key_len;

        let trimmed_after = after_key.trim_start();
        if !trimmed_after.starts_with(':') {
            continue;
        }
        let after_colon = trimmed_after.get(1..)?.trim_start();
        let quote_ch = after_colon.chars().next()?;
        if quote_ch != '"' && quote_ch != '\'' {
            continue;
        }

        let content = after_colon.get(quote_ch.len_utf8()..)?;
        let mut escaped = false;
        let mut val_end = None;
        for (i, ch) in content.char_indices() {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == quote_ch {
                val_end = Some(i);
                break;
            }
        }
        let val_idx = val_end?;
        let raw_val = content.get(..val_idx)?;

        if quote_ch == '"' {
            let quoted_slice = after_colon.get(..val_idx + 2)?;
            if let Ok(parsed) = serde_json::from_str::<String>(quoted_slice) {
                return Some(parsed);
            }
        } else {
            let json_str = format!("\"{}\"", raw_val.replace('\\', "\\\\").replace('"', "\\\""));
            if let Ok(parsed) = serde_json::from_str::<String>(&json_str) {
                return Some(parsed);
            }
        }
    }
    None
}

/// Resolves a Vidhide embed page into downloadable media metadata.
///
/// Unpacks the Dean Edwards P.A.C.K.E.R. script, extracts `hls4`, `hls2`, `hls3` string values,
/// resolves them against `page_url`, filters for http(s) URLs whose path ends in `.m3u8`
/// (case-insensitive, ignoring query parameters), and selects the first match in preference
/// order `hls4` > `hls2` > `hls3`. Referer is `<scheme>://<host of page_url>/`.
#[allow(dead_code)] // Consumed by download supervisor in later stages
pub fn resolve_vidhide(embed_html: &str, page_url: &str) -> Option<Media> {
    let unpacked = unpack_packer(embed_html)?;
    let parsed_page = Url::parse(page_url).ok()?;
    let scheme = parsed_page.scheme();
    if scheme != "http" && scheme != "https" {
        return None;
    }
    let host = parsed_page.host_str()?;
    let referer = if let Some(port) = parsed_page.port() {
        format!("{}://{}:{}/", scheme, host, port)
    } else {
        format!("{}://{}/", scheme, host)
    };

    for key in &["hls4", "hls2", "hls3"] {
        let Some(val) = extract_json_property_string(&unpacked, key) else {
            continue;
        };
        let Ok(resolved) = parsed_page.join(&val) else {
            continue;
        };
        if resolved.scheme() != "http" && resolved.scheme() != "https" {
            continue;
        }
        let path = resolved.path().to_ascii_lowercase();
        if !path.ends_with(".m3u8") {
            continue;
        }
        return Some(Media {
            url: resolved.to_string(),
            referer,
            host: "vidhide",
        });
    }

    None
}

/// Extracts the `MDCore.wurl="..."` URL assignment from unpacked Mixdrop script text.
fn extract_mdcore_wurl(text: &str) -> Option<String> {
    let mut search_from = 0;
    while let Some(sub) = text.get(search_from..) {
        let pos = sub.find("MDCore.wurl")?;
        let idx = search_from + pos;
        let after = text.get(idx + "MDCore.wurl".len()..)?;
        search_from = idx + "MDCore.wurl".len();

        let trimmed_after = after.trim_start();
        if !trimmed_after.starts_with('=') {
            continue;
        }
        let after_eq = trimmed_after.get(1..)?.trim_start();
        let quote_ch = after_eq.chars().next()?;
        if quote_ch != '"' && quote_ch != '\'' {
            continue;
        }
        let content = after_eq.get(quote_ch.len_utf8()..)?;
        let mut escaped = false;
        let mut val_end = None;
        for (i, ch) in content.char_indices() {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == quote_ch {
                val_end = Some(i);
                break;
            }
        }
        let val_idx = val_end?;
        let raw_val = content.get(..val_idx)?;
        return Some(raw_val.to_string());
    }
    None
}

/// Resolves a Mixdrop embed page into downloadable media metadata.
///
/// Unpacks the Dean Edwards P.A.C.K.E.R. script, extracts `MDCore.wurl`, prepends `https:`
/// if scheme-relative (`//`), resolves against `page_url`, and ensures the resolved URL
/// is http(s) with path ending in `.mp4`. Referer is `<scheme>://<host of page_url>/`.
#[allow(dead_code)] // Consumed by download supervisor in later stages
pub fn resolve_mixdrop(embed_html: &str, page_url: &str) -> Option<Media> {
    let unpacked = unpack_packer(embed_html)?;
    let raw_wurl = extract_mdcore_wurl(&unpacked)?;
    let parsed_page = Url::parse(page_url).ok()?;
    let scheme = parsed_page.scheme();
    if scheme != "http" && scheme != "https" {
        return None;
    }
    let host = parsed_page.host_str()?;
    let referer = if let Some(port) = parsed_page.port() {
        format!("{}://{}:{}/", scheme, host, port)
    } else {
        format!("{}://{}/", scheme, host)
    };

    let full_wurl = if raw_wurl.starts_with("//") {
        format!("https:{}", raw_wurl)
    } else {
        raw_wurl
    };

    let resolved = parsed_page.join(&full_wurl).ok()?;
    if resolved.scheme() != "http" && resolved.scheme() != "https" {
        return None;
    }
    let path = resolved.path().to_ascii_lowercase();
    if !path.ends_with(".mp4") {
        return None;
    }

    Some(Media {
        url: resolved.to_string(),
        referer,
        host: "mixdrop",
    })
}

/// Ranks an Ok.ru video quality name: full (6) > hd (5) > sd (4) > low (3) > lowest (2) > mobile (1) > other (0).
fn okru_video_rank(name: &str) -> u8 {
    match name.trim().to_ascii_lowercase().as_str() {
        "full" => 6,
        "hd" => 5,
        "sd" => 4,
        "low" => 3,
        "lowest" => 2,
        "mobile" => 1,
        _ => 0,
    }
}

/// Resolves an Ok.ru embed page into downloadable media metadata.
///
/// Parses `data-options` on `[data-module="OKVideo"]`, reads `flashvars.metadata` (handling either
/// a nested JSON string or object), and selects the highest ranked direct video from `videos`.
/// Falls back to `hlsManifestUrl` when `videos` is empty or missing. Referer is `https://ok.ru/`.
#[allow(dead_code)] // Consumed by download supervisor in later stages
pub fn resolve_okru(embed_html: &str) -> Option<Media> {
    static OKVIDEO_SELECTOR: OnceLock<Option<Selector>> = OnceLock::new();
    let selector = OKVIDEO_SELECTOR
        .get_or_init(|| Selector::parse(r#"[data-module="OKVideo"]"#).ok())
        .as_ref()?;

    let doc = Html::parse_document(embed_html);
    let el = doc.select(selector).next()?;
    let raw_options = el.value().attr("data-options")?;

    let val: serde_json::Value = serde_json::from_str(raw_options).ok()?;
    let flashvars = val.get("flashvars")?;
    let metadata_val = flashvars.get("metadata")?;
    let metadata: serde_json::Value = if let Some(s) = metadata_val.as_str() {
        serde_json::from_str(s).ok()?
    } else if metadata_val.is_object() {
        metadata_val.clone()
    } else {
        return None;
    };

    let mut best_url: Option<String> = None;
    let mut best_rank: u8 = 0;

    if let Some(videos) = metadata.get("videos").and_then(|v| v.as_array()) {
        for video in videos {
            let Some(url_str) = video.get("url").and_then(|u| u.as_str()) else {
                continue;
            };
            let Ok(parsed_url) = Url::parse(url_str) else {
                continue;
            };
            if parsed_url.scheme() != "http" && parsed_url.scheme() != "https" {
                continue;
            }
            let name = video.get("name").and_then(|n| n.as_str()).unwrap_or("");
            let rank = okru_video_rank(name);

            match &best_url {
                None => {
                    best_url = Some(url_str.to_string());
                    best_rank = rank;
                }
                Some(_) => {
                    if rank > best_rank {
                        best_url = Some(url_str.to_string());
                        best_rank = rank;
                    }
                }
            }
        }
    }

    let final_url = match best_url {
        Some(u) => u,
        None => {
            let manifest = metadata.get("hlsManifestUrl").and_then(|u| u.as_str())?;
            let parsed = Url::parse(manifest).ok()?;
            if parsed.scheme() != "http" && parsed.scheme() != "https" {
                return None;
            }
            manifest.to_string()
        }
    };

    Some(Media {
        url: final_url,
        referer: "https://ok.ru/".to_string(),
        host: "okru",
    })
}

/// Returns the host identifier if the given server is supported for downloads.
///
/// Supports `"mp4upload"`, `"yourupload"`, `"mediafire"`, `"streamtape"`, and `"okru"`
/// when the URL's host matches the domain or is a subdomain thereof.
/// Also supports `"vidhide"` and `"mixdrop"` when the server name matches (case-insensitive, trimmed)
/// and the URL is http(s). Domain-based matching takes priority over name-based matching.
#[allow(dead_code)] // Consumed by download supervisor in later stages
pub fn supported_host(server: &Server) -> Option<&'static str> {
    let parsed = Url::parse(&server.url).ok()?;
    if parsed.scheme() != "http" && parsed.scheme() != "https" {
        return None;
    }
    let host = parsed.host_str()?;
    let host_lower = host.to_ascii_lowercase();
    if host_lower == "mp4upload.com" || host_lower.ends_with(".mp4upload.com") {
        Some("mp4upload")
    } else if host_lower == "yourupload.com" || host_lower.ends_with(".yourupload.com") {
        Some("yourupload")
    } else if host_lower == "mediafire.com" || host_lower.ends_with(".mediafire.com") {
        Some("mediafire")
    } else if host_lower == "streamtape.com" || host_lower.ends_with(".streamtape.com") {
        Some("streamtape")
    } else if host_lower == "ok.ru" || host_lower.ends_with(".ok.ru") {
        Some("okru")
    } else {
        let name_lower = server.name.trim().to_ascii_lowercase();
        if name_lower == "vidhide" {
            Some("vidhide")
        } else if name_lower == "mixdrop" {
            Some("mixdrop")
        } else {
            None
        }
    }
}

/// Priority rank for supported hosts: mp4upload (0), yourupload (1), mediafire (2),
/// streamtape (3), okru (4), mixdrop (5), vidhide (6).
fn host_preference(host: &str) -> u8 {
    match host {
        "mp4upload" => 0,
        "yourupload" => 1,
        "mediafire" => 2,
        "streamtape" => 3,
        "okru" => 4,
        "mixdrop" => 5,
        "vidhide" => 6,
        _ => 255,
    }
}

/// Filters a list of servers down to those on supported hosts, ordering by host preference
/// first (`mp4upload` > `yourupload` > `mediafire` > `streamtape` > `okru` > `mixdrop` > `vidhide`)
/// and page order second, deduplicating by URL (keeping the first occurrence).
#[allow(dead_code)] // Consumed by download supervisor in later stages
pub fn candidates(servers: &[Server]) -> Vec<Server> {
    let mut seen_urls = HashSet::new();
    let mut collected: Vec<(u8, usize, Server)> = Vec::new();

    for (idx, server) in servers.iter().enumerate() {
        if let Some(host) = supported_host(server) {
            if seen_urls.insert(server.url.as_str()) {
                collected.push((host_preference(host), idx, server.clone()));
            }
        }
    }

    collected.sort_by_key(|&(pref, idx, _)| (pref, idx));
    collected.into_iter().map(|(_, _, server)| server).collect()
}

/// Dispatches to the host-specific resolver based on the host identifier.
#[allow(dead_code)] // Consumed by download supervisor in later stages
pub fn resolve_from_page(host: &str, page_html: &str, page_url: &str) -> Option<Media> {
    match host {
        "yourupload" => resolve_yourupload(page_html, page_url),
        "mp4upload" => resolve_mp4upload(page_html, page_url),
        "mediafire" => resolve_mediafire(page_html, page_url),
        "streamtape" => resolve_streamtape(page_html),
        "okru" => resolve_okru(page_html),
        "mixdrop" => resolve_mixdrop(page_html, page_url),
        "vidhide" => resolve_vidhide(page_html, page_url),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_tioanime_servers_from_fixture() {
        let fixture = include_str!("../tests/fixtures/tioanime_episode.html");
        let servers = parse_tioanime_servers(fixture);

        assert_eq!(servers.len(), 14, "expected exactly 14 servers from fixture");
        assert_eq!(
            servers[0],
            Server {
                name: "YourUpload".to_string(),
                url: "https://www.yourupload.com/embed/C25TFyQ5pnNs".to_string(),
            }
        );

        let names: Vec<&str> = servers.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "YourUpload",
                "StreamSB",
                "Mega",
                "Mega",
                "Amus",
                "Okru",
                "Okru",
                "Mepu",
                "Mepu",
                "YourUpload",
                "Netu",
                "Maru",
                "Maru",
                "Netu",
            ]
        );
    }

    #[test]
    fn candidates_from_fixture_extracts_distinct_yourupload_servers() {
        let fixture = include_str!("../tests/fixtures/tioanime_episode.html");
        let servers = parse_tioanime_servers(fixture);
        let cand = candidates(&servers);

        // The TioAnime page also lists two distinct Okru mirrors, a supported host
        // since the Ok.ru resolver exists, ranked after YourUpload.
        assert_eq!(cand.len(), 4, "2 YourUpload + 2 Okru mirrors");
        assert_eq!(
            cand[0],
            Server {
                name: "YourUpload".to_string(),
                url: "https://www.yourupload.com/embed/C25TFyQ5pnNs".to_string(),
            }
        );
        assert_eq!(
            cand[1],
            Server {
                name: "YourUpload".to_string(),
                url: "https://www.yourupload.com/embed/sJQepOnp2Srg".to_string(),
            }
        );
    }

    #[test]
    fn candidates_deduplicates_identical_urls() {
        let servers = vec![
            Server {
                name: "YourUpload".to_string(),
                url: "https://www.yourupload.com/embed/same".to_string(),
            },
            Server {
                name: "YourUpload Alt".to_string(),
                url: "https://www.yourupload.com/embed/same".to_string(),
            },
        ];
        let cand = candidates(&servers);
        assert_eq!(cand.len(), 1);
        assert_eq!(cand[0].name, "YourUpload");
        assert_eq!(cand[0].url, "https://www.yourupload.com/embed/same");
    }

    #[test]
    fn resolves_yourupload_fixture() {
        let fixture = include_str!("../tests/fixtures/yourupload_embed.html");
        let media = resolve_yourupload(fixture, "https://www.yourupload.com/embed/C25TFyQ5pnNs")
            .expect("should resolve yourupload fixture");

        assert_eq!(
            media,
            Media {
                url: "https://vidcache.net:8161/a202610141ioUfk54Sd2/video.mp4".to_string(),
                referer: "https://www.yourupload.com/".to_string(),
                host: "yourupload",
            }
        );
    }

    #[test]
    fn resolves_synthetic_single_file_relative_mp4() {
        let html = r#"<script>var opts = { file: '/media/x.mp4' };</script>"#;
        let media = resolve_yourupload(html, "https://www.yourupload.com/embed/abc")
            .expect("should resolve relative mp4");

        assert_eq!(
            media,
            Media {
                url: "https://www.yourupload.com/media/x.mp4".to_string(),
                referer: "https://www.yourupload.com/".to_string(),
                host: "yourupload",
            }
        );
    }

    #[test]
    fn ignores_synthetic_non_mp4_file() {
        let html = r#"<script>var opts = { file: '/images/watermark.png' };</script>"#;
        assert_eq!(
            resolve_yourupload(html, "https://www.yourupload.com/embed/abc"),
            None
        );
    }

    #[test]
    fn prefers_og_video_over_file_attribute() {
        let html = r#"
            <meta property="og:video" content="https://vidcache.net/og_video.mp4">
            <script>file: 'https://vidcache.net/other_file.mp4'</script>
        "#;
        let media = resolve_yourupload(html, "https://www.yourupload.com/embed/abc")
            .expect("should resolve og:video");

        assert_eq!(media.url, "https://vidcache.net/og_video.mp4");
        assert_eq!(media.referer, "https://www.yourupload.com/");
        assert_eq!(media.host, "yourupload");
    }

    #[test]
    fn resolves_empty_html_as_none() {
        assert_eq!(
            resolve_yourupload("", "https://www.yourupload.com/embed/abc"),
            None
        );
    }

    #[test]
    fn resolves_mp4_with_query_string_and_uppercase_extension() {
        let html = r#"<script>file: 'https://cdn.example.com/stream/video.MP4?token=secret123&exp=999'</script>"#;
        let media = resolve_yourupload(html, "https://www.yourupload.com/embed/abc")
            .expect("should resolve uppercase .MP4 with query string");

        assert_eq!(
            media.url,
            "https://cdn.example.com/stream/video.MP4?token=secret123&exp=999"
        );
    }

    #[test]
    fn parse_tioanime_servers_edge_cases() {
        // Empty string
        assert_eq!(parse_tioanime_servers(""), Vec::new());

        // Incomplete / unclosed array
        assert_eq!(parse_tioanime_servers("videos = [["), Vec::new());

        // Skips non-http(s) schemes like javascript:
        let js_html = r#"<script>var videos = [["A","javascript:alert(1)"],["B","https://ok.example/x"]];</script>"#;
        let servers = parse_tioanime_servers(js_html);
        assert_eq!(
            servers,
            vec![Server {
                name: "B".to_string(),
                url: "https://ok.example/x".to_string(),
            }]
        );

        // Skips malformed items (numbers instead of strings, missing entries)
        let malformed_html = r#"videos = [[123, "https://example.com/1"], ["Valid", "https://example.com/2"], null, ["NoUrl"]];"#;
        let servers2 = parse_tioanime_servers(malformed_html);
        assert_eq!(
            servers2,
            vec![Server {
                name: "Valid".to_string(),
                url: "https://example.com/2".to_string(),
            }]
        );
    }

    #[test]
    fn supported_host_checks() {
        let valid_www = Server {
            name: "YourUpload".to_string(),
            url: "https://www.yourupload.com/embed/x".to_string(),
        };
        assert_eq!(supported_host(&valid_www), Some("yourupload"));

        let valid_bare = Server {
            name: "YourUpload".to_string(),
            url: "https://yourupload.com/embed/x".to_string(),
        };
        assert_eq!(supported_host(&valid_bare), Some("yourupload"));

        let evil_suffix = Server {
            name: "Evil".to_string(),
            url: "https://yourupload.com.evil.example/x".to_string(),
        };
        assert_eq!(supported_host(&evil_suffix), None);

        let not_yourupload = Server {
            name: "NotYourUpload".to_string(),
            url: "https://notyourupload.com/x".to_string(),
        };
        assert_eq!(supported_host(&not_yourupload), None);

        let invalid_url = Server {
            name: "Invalid".to_string(),
            url: "not_a_valid_url".to_string(),
        };
        assert_eq!(supported_host(&invalid_url), None);
    }

    #[test]
    fn rejects_non_http_urls_in_resolve_yourupload() {
        let js_og = r#"<meta property="og:video" content="javascript:alert(1)">"#;
        assert_eq!(
            resolve_yourupload(js_og, "https://www.yourupload.com/embed/abc"),
            None
        );

        let js_file = r#"<script>file: 'javascript:alert(1).mp4'</script>"#;
        assert_eq!(
            resolve_yourupload(js_file, "https://www.yourupload.com/embed/abc"),
            None
        );
    }

    #[test]
    fn parses_jkanime_servers_from_fixture_and_orders_candidates() {
        let fixture = include_str!("../tests/fixtures/jkanime_episode.html");
        let servers = parse_jkanime_servers(fixture);

        assert_eq!(servers.len(), 11, "expected exactly 11 servers from fixture");
        assert_eq!(
            servers[0],
            Server {
                name: "Mediafire".to_string(),
                url: "https://mediafire.com/file/az2max9srvdrm0i/".to_string(),
            }
        );

        let mp4upload = servers.iter().find(|s| s.name == "Mp4upload");
        assert_eq!(
            mp4upload,
            Some(&Server {
                name: "Mp4upload".to_string(),
                url: "https://www.mp4upload.com/embed-ydx6kydvoh7g.html".to_string(),
            })
        );

        let streamtape = servers.iter().find(|s| s.name == "Streamtape");
        assert_eq!(
            streamtape,
            Some(&Server {
                name: "Streamtape".to_string(),
                url: "https://streamtape.com/e/2OaxZZVQpBUZ410/kaaanoojoonootomodaachissss-02.mp4".to_string(),
            })
        );

        let cand = candidates(&servers);
        assert_eq!(
            cand,
            vec![
                Server {
                    name: "Mp4upload".to_string(),
                    url: "https://www.mp4upload.com/embed-ydx6kydvoh7g.html".to_string(),
                },
                Server {
                    name: "Mediafire".to_string(),
                    url: "https://mediafire.com/file/az2max9srvdrm0i/".to_string(),
                },
                Server {
                    name: "Streamtape".to_string(),
                    url: "https://streamtape.com/e/2OaxZZVQpBUZ410/kaaanoojoonootomodaachissss-02.mp4".to_string(),
                },
                // The hosts that rotate domains (identified by name) come last.
                Server {
                    name: "Mixdrop".to_string(),
                    url: "https://mixdrop.top/e/366ovrg9tv9ekm".to_string(),
                },
                Server {
                    name: "Vidhide".to_string(),
                    url: "https://vidhidevip.com/embed/etxwwazwbqtr".to_string(),
                },
            ]
        );
    }

    #[test]
    fn parse_jkanime_servers_edge_cases() {
        assert_eq!(parse_jkanime_servers(""), Vec::new());

        let invalid_b64 = r#"var servers = [{"remote":"!!!notbase64","server":"X"}];"#;
        assert_eq!(parse_jkanime_servers(invalid_b64), Vec::new());

        // Base64 of "javascript:alert(1)" is "amF2YXNjcmlwdDphbGVydCgxKQ=="
        let js_scheme = r#"var servers = [{"remote":"amF2YXNjcmlwdDphbGVydCgxKQ==","server":"X"}];"#;
        assert_eq!(parse_jkanime_servers(js_scheme), Vec::new());

        // Padding-less base64 ("aHR0cHM6Ly9leGFtcGxlLmNvbS9hYg" decodes to "https://example.com/ab")
        let paddingless = r#"var servers = [{"remote":"aHR0cHM6Ly9leGFtcGxlLmNvbS9hYg","server":"Paddingless"}];"#;
        let res = parse_jkanime_servers(paddingless);
        assert_eq!(
            res,
            vec![Server {
                name: "Paddingless".to_string(),
                url: "https://example.com/ab".to_string(),
            }]
        );

        // Newline-terminated base64
        let newline_terminated = r#"var servers = [{"remote":"aHR0cHM6Ly9leGFtcGxlLmNvbS9hYg==\n","server":"Newline"}];"#;
        let res2 = parse_jkanime_servers(newline_terminated);
        assert_eq!(
            res2,
            vec![Server {
                name: "Newline".to_string(),
                url: "https://example.com/ab".to_string(),
            }]
        );
    }

    #[test]
    fn parses_animeyt_container_url_fixture_and_edge_cases() {
        let fixture = include_str!("../tests/fixtures/animeyt_episode.html");
        let container_url = parse_animeyt_container_url(fixture);
        assert_eq!(
            container_url,
            Some("https://mytsumi.com/container.php?id=dsq4g-wMO44NrSxp&only=SUB&lang=SUB&open=1".to_string())
        );

        // Replaces existing open=0
        let with_open_zero = r#"<iframe src="https://mytsumi.com/container.php?id=123&open=0&lang=SUB"></iframe>"#;
        assert_eq!(
            parse_animeyt_container_url(with_open_zero),
            Some("https://mytsumi.com/container.php?id=123&open=1&lang=SUB".to_string())
        );

        // No iframe -> None
        assert_eq!(parse_animeyt_container_url("<div>No iframe</div>"), None);

        // evil-mytsumi.com rejected
        let evil = r#"<iframe src="https://evil-mytsumi.com/container.php?id=123"></iframe>"#;
        assert_eq!(parse_animeyt_container_url(evil), None);
    }

    #[test]
    fn parses_mytsumi_tabs_and_direct_media() {
        let fixture = include_str!("../tests/fixtures/mytsumi_container.html");
        let tabs = parse_mytsumi_tabs(fixture);

        assert_eq!(tabs.len(), 5, "expected 5 tabs from fixture");
        assert_eq!(
            tabs[0],
            MytsumiTab {
                label: "Byse".to_string(),
                url: "https://bysesukior.com/e/zoiyseczz38m".to_string(),
                direct_video: false,
                quality: Some(720),
            }
        );
        assert_eq!(
            tabs[1],
            MytsumiTab {
                label: "Omega".to_string(),
                url: "https://archive.org/download/pajemqxw3f-rd732gj8/rxtqakcvxms4.mp4".to_string(),
                direct_video: true,
                quality: Some(720),
            }
        );
        assert_eq!(
            tabs[2],
            MytsumiTab {
                label: "MEGA".to_string(),
                url: "https://mega.nz/embed/HWRHzBIK#OGtBH5_r43ic9_wyey2ja6fah4d_TJIHJg5zCNWWFH4".to_string(),
                direct_video: false,
                quality: Some(720),
            }
        );
        assert_eq!(
            tabs[3],
            MytsumiTab {
                label: "Epsilon".to_string(),
                url: "https://ytplay.rpmvid.com/#z6tech".to_string(),
                direct_video: false,
                quality: Some(1080),
            }
        );
        assert_eq!(
            tabs[4],
            MytsumiTab {
                label: "Abyss".to_string(),
                url: "https://player.abyssplayer.com/mNw-Gu1TL".to_string(),
                direct_video: false,
                quality: Some(1080),
            }
        );

        let media = mytsumi_direct_media(&tabs).expect("direct media should resolve");
        assert_eq!(
            media,
            Media {
                url: "https://archive.org/download/pajemqxw3f-rd732gj8/rxtqakcvxms4.mp4".to_string(),
                referer: "https://mytsumi.com/".to_string(),
                host: "direct",
            }
        );

        // Only iframe tabs -> None
        let iframe_only = vec![
            MytsumiTab {
                label: "Byse".to_string(),
                url: "https://bysesukior.com/e/1".to_string(),
                direct_video: false,
                quality: Some(720),
            },
        ];
        assert_eq!(mytsumi_direct_media(&iframe_only), None);

        // Two direct tabs 720 and 1080 -> 1080 wins
        let two_direct = vec![
            MytsumiTab {
                label: "Low".to_string(),
                url: "https://example.com/720.mp4".to_string(),
                direct_video: true,
                quality: Some(720),
            },
            MytsumiTab {
                label: "High".to_string(),
                url: "https://example.com/1080.mp4".to_string(),
                direct_video: true,
                quality: Some(1080),
            },
        ];
        let best = mytsumi_direct_media(&two_direct).expect("best direct media");
        assert_eq!(best.url, "https://example.com/1080.mp4");
    }

    #[test]
    fn resolves_mp4upload_fixture_and_edge_cases() {
        let fixture = include_str!("../tests/fixtures/mp4upload_embed.html");
        let media = resolve_mp4upload(fixture, "https://www.mp4upload.com/embed-ydx6kydvoh7g.html")
            .expect("should resolve mp4upload fixture");

        assert_eq!(
            media,
            Media {
                url: "https://a3.mp4upload.com:183/d/xsx52qflz3b4quuog2sagnsyc4d6q3lnob2rjecxfgzbumqjcws5bymr47mosfohcxnwyrp5/video.mp4".to_string(),
                referer: "https://www.mp4upload.com/".to_string(),
                host: "mp4upload",
            }
        );

        assert_eq!(
            resolve_mp4upload("<html><body>No mp4</body></html>", "https://www.mp4upload.com/embed"),
            None
        );
    }

    #[test]
    fn resolves_mediafire_fixture_and_edge_cases() {
        let fixture = include_str!("../tests/fixtures/mediafire_file.html");
        let media = resolve_mediafire(fixture, "https://mediafire.com/file/az2max9srvdrm0i/")
            .expect("should resolve mediafire fixture");

        assert_eq!(
            media,
            Media {
                url: "https://download2436.mediafire.com/cdy3pniyizkgzYpa8GvziJnmT3PDLcI6qYcyKImj371-1id6uhdznBMGvZACXxoKeYFksTmrAYd5sGV1JEgZtHVxL7B720tZ_-80lrH06UwVoBUfgNklu0pscTqjou2s0OTanlJ7KV2NSP8x2Mn97iRIDWuAOVy8Rt13GS-H2FsH/az2max9srvdrm0i/kaaanoojoonootomodaachissss-02.mp4".to_string(),
                referer: "https://www.mediafire.com/".to_string(),
                host: "mediafire",
            }
        );

        // downloadButton pointing at evil domain -> None
        let evil_html = r#"<a id="downloadButton" href="https://evil.example/x.mp4">Download</a>"#;
        assert_eq!(
            resolve_mediafire(evil_html, "https://mediafire.com/file/x/"),
            None
        );

        // Page without #downloadButton -> None
        assert_eq!(
            resolve_mediafire("<html><body>No button</body></html>", "https://mediafire.com/file/x/"),
            None
        );
    }

    #[test]
    fn resolves_streamtape_fixture_and_edge_cases() {
        let fixture = include_str!("../tests/fixtures/streamtape_embed.html");
        let media = resolve_streamtape(fixture).expect("should resolve streamtape fixture");

        assert_eq!(
            media,
            Media {
                url: "https://streamtape.com/get_video?id=2OaxZZVQpBUZ410&expires=1792049666&ip=DxEsEHcnDS9X&token=44xJNKK8SoyE".to_string(),
                referer: "https://streamtape.com/".to_string(),
                host: "streamtape",
            }
        );

        // Synthetic: .substring(1) only
        let sub1 = r#"document.getElementById('robotlink').innerHTML = '//streamtape.com/get_'+ ('avideo?id=123').substring(1);"#;
        let m1 = resolve_streamtape(sub1).expect("sub1");
        assert_eq!(m1.url, "https://streamtape.com/get_video?id=123");

        // Synthetic: double-quoted prefix
        let dbl_quote = r#"document.getElementById("robotlink").innerHTML = "//streamtape.com/get_" + ('bvideo?id=456').substring(1);"#;
        let m2 = resolve_streamtape(dbl_quote).expect("dbl_quote");
        assert_eq!(m2.url, "https://streamtape.com/get_video?id=456");

        // Synthetic: two robotlink assignments (the last wins)
        let two_assignments = r#"
            document.getElementById('robotlink').innerHTML = '//streamtape.com/first'+ ('x_video').substring(1);
            document.getElementById('robotlink').innerHTML = '//streamtape.com/second'+ ('y_video').substring(1);
        "#;
        let m3 = resolve_streamtape(two_assignments).expect("two assignments");
        assert_eq!(m3.url, "https://streamtape.com/second_video");

        // Assembled host that is not streamtape -> None
        let bad_host = r#"document.getElementById('robotlink').innerHTML = '//evil.com/video'+ ('x').substring(1);"#;
        assert_eq!(resolve_streamtape(bad_host), None);

        // No robotlink -> None
        assert_eq!(resolve_streamtape("<div>No script</div>"), None);

        // .substring(500) on a short string must not panic
        let large_sub = r#"document.getElementById('robotlink').innerHTML = '//streamtape.com/short'+ ('xyz').substring(500);"#;
        let m4 = resolve_streamtape(large_sub).expect("large_sub");
        assert_eq!(m4.url, "https://streamtape.com/short");
    }

    #[test]
    fn supported_host_and_candidates_ordering() {
        assert_eq!(
            supported_host(&Server {
                name: "Mp4upload".to_string(),
                url: "https://www.mp4upload.com/embed-x.html".to_string(),
            }),
            Some("mp4upload")
        );
        assert_eq!(
            supported_host(&Server {
                name: "Mediafire".to_string(),
                url: "https://mediafire.com/file/x/".to_string(),
            }),
            Some("mediafire")
        );
        assert_eq!(
            supported_host(&Server {
                name: "Streamtape".to_string(),
                url: "https://streamtape.com/e/x".to_string(),
            }),
            Some("streamtape")
        );
        assert_eq!(
            supported_host(&Server {
                name: "Evil".to_string(),
                url: "https://mp4upload.com.evil.example/x".to_string(),
            }),
            None
        );
        assert_eq!(
            supported_host(&Server {
                name: "NotMediafire".to_string(),
                url: "https://notmediafire.com/x".to_string(),
            }),
            None
        );

        // Candidates preference: mp4upload (0), yourupload (1), mediafire (2), streamtape (3)
        let mixed = vec![
            Server {
                name: "ST".to_string(),
                url: "https://streamtape.com/e/1".to_string(),
            },
            Server {
                name: "MF".to_string(),
                url: "https://mediafire.com/file/1/".to_string(),
            },
            Server {
                name: "YU".to_string(),
                url: "https://www.yourupload.com/embed/1".to_string(),
            },
            Server {
                name: "MP4".to_string(),
                url: "https://www.mp4upload.com/embed-1.html".to_string(),
            },
        ];
        let cand = candidates(&mixed);
        let names: Vec<&str> = cand.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["MP4", "YU", "MF", "ST"]);
    }

    #[test]
    fn resolve_from_page_dispatcher() {
        let fixture = include_str!("../tests/fixtures/mp4upload_embed.html");
        let resolved = resolve_from_page(
            "mp4upload",
            fixture,
            "https://www.mp4upload.com/embed-ydx6kydvoh7g.html",
        );
        let direct = resolve_mp4upload(
            fixture,
            "https://www.mp4upload.com/embed-ydx6kydvoh7g.html",
        );
        assert_eq!(resolved, direct);
        assert!(resolved.is_some());

        assert_eq!(
            resolve_from_page("nope", fixture, "https://example.com"),
            None
        );
    }

    #[test]
    fn unpacks_packer_fixtures_and_edge_cases() {
        // Mixdrop fixture
        let mixdrop_fixture = include_str!("../tests/fixtures/mixdrop_embed.html");
        let mixdrop_unpacked = unpack_packer(mixdrop_fixture).expect("should unpack mixdrop");
        assert!(
            mixdrop_unpacked.contains(
                r#"MDCore.wurl="//30xplewoo.mxcontent.net/v2/366ovrg9tv9ekm.mp4?s=ThpcRIVQ7mtWr0omYtp1MQ&e=1792006222&_t=1791988082""#
            ),
            "expected MDCore.wurl with exact URL in unpacked mixdrop"
        );

        // Vidhide fixture
        let vidhide_fixture = include_str!("../tests/fixtures/vidhide_embed.html");
        let vidhide_unpacked = unpack_packer(vidhide_fixture).expect("should unpack vidhide");
        assert!(
            vidhide_unpacked.contains(
                r#""hls4":"/stream/pYXzvRSs8jBowEh4Sed8vQ/hjkrhuihghfvu/1792027821/43559031/master.m3u8""#
            ),
            "expected hls4 key in unpacked vidhide"
        );

        // Tiny hand-made packed sample
        let tiny = r#"eval(function(p,a,c,k,e,d){while(c--)if(k[c])p=p.replace(new RegExp('\\b'+c.toString(a)+'\\b','g'),k[c]);return p}('0 1',2,2,'hello|world'.split('|'),0,{}))"#;
        assert_eq!(unpack_packer(tiny), Some("hello world".to_string()));

        // Text without eval(function(p,a,c,k,e,d) -> None
        assert_eq!(unpack_packer("var x = 1;"), None);

        // Malformed radix 99 -> None
        let bad_radix = r#"eval(function(p,a,c,k,e,d){}('abc',99,3,'x'.split('|'))"#;
        assert_eq!(unpack_packer(bad_radix), None);

        // 3 MB string -> None
        let large = "a".repeat(3 * 1024 * 1024);
        assert_eq!(unpack_packer(&large), None);

        // Empty string -> None
        assert_eq!(unpack_packer(""), None);
    }

    #[test]
    fn resolves_vidhide_fixture_and_edge_cases() {
        let fixture = include_str!("../tests/fixtures/vidhide_embed.html");
        let media = resolve_vidhide(fixture, "https://callistanise.com/embed/etxwwazwbqtr")
            .expect("should resolve vidhide fixture");

        assert_eq!(
            media,
            Media {
                url: "https://callistanise.com/stream/pYXzvRSs8jBowEh4Sed8vQ/hjkrhuihghfvu/1792027821/43559031/master.m3u8".to_string(),
                referer: "https://callistanise.com/".to_string(),
                host: "vidhide",
            }
        );

        // Page where only hls3 (.txt) exists -> None
        let txt_only = r#"eval(function(p,a,c,k,e,d){while(c--)if(k[c])p=p.replace(new RegExp('\\b'+c.toString(a)+'\\b','g'),k[c]);return p}('var p = {"hls3":"https://example.com/master.txt"};',2,2,'||'.split('|'),0,{}))"#;
        assert_eq!(resolve_vidhide(txt_only, "https://callistanise.com/embed/etxwwazwbqtr"), None);

        // Where hls4 is missing, hls2 (absolute) is chosen
        let hls2_only = r#"eval(function(p,a,c,k,e,d){while(c--)if(k[c])p=p.replace(new RegExp('\\b'+c.toString(a)+'\\b','g'),k[c]);return p}('var p = {"hls2":"https://1hyahuweWHyVWmq.acek-cdn.com/hls2/master.m3u8?t=123"};',2,2,'||'.split('|'),0,{}))"#;
        let media_hls2 = resolve_vidhide(hls2_only, "https://callistanise.com/embed/etxwwazwbqtr")
            .expect("hls2 should resolve");
        assert_eq!(
            media_hls2,
            Media {
                // `url::Url` lower-cases the host, which is fine: hosts are case-insensitive.
                url: "https://1hyahuwewhyvwmq.acek-cdn.com/hls2/master.m3u8?t=123".to_string(),
                referer: "https://callistanise.com/".to_string(),
                host: "vidhide",
            }
        );
    }

    #[test]
    fn resolves_mixdrop_fixture_and_edge_cases() {
        let fixture = include_str!("../tests/fixtures/mixdrop_embed.html");
        let media = resolve_mixdrop(fixture, "https://mxdrop.click/e/366ovrg9tv9ekm")
            .expect("should resolve mixdrop fixture");

        assert_eq!(
            media,
            Media {
                url: "https://30xplewoo.mxcontent.net/v2/366ovrg9tv9ekm.mp4?s=ThpcRIVQ7mtWr0omYtp1MQ&e=1792006222&_t=1791988082".to_string(),
                referer: "https://mxdrop.click/".to_string(),
                host: "mixdrop",
            }
        );

        // Page without MDCore.wurl -> None
        let no_wurl = r#"eval(function(p,a,c,k,e,d){while(c--)if(k[c])p=p.replace(new RegExp('\\b'+c.toString(a)+'\\b','g'),k[c]);return p}('var x = 1;',2,2,'||'.split('|'),0,{}))"#;
        assert_eq!(resolve_mixdrop(no_wurl, "https://mxdrop.click/e/366ovrg9tv9ekm"), None);

        // A wurl ending in .js -> None
        let js_wurl = r#"eval(function(p,a,c,k,e,d){while(c--)if(k[c])p=p.replace(new RegExp('\\b'+c.toString(a)+'\\b','g'),k[c]);return p}('MDCore.wurl="//30xplewoo.mxcontent.net/v2/video.js";',2,2,'||'.split('|'),0,{}))"#;
        assert_eq!(resolve_mixdrop(js_wurl, "https://mxdrop.click/e/366ovrg9tv9ekm"), None);
    }

    #[test]
    fn resolves_okru_fixture_and_edge_cases() {
        let fixture = include_str!("../tests/fixtures/okru_embed.html");
        let media = resolve_okru(fixture).expect("should resolve okru fixture");

        assert_eq!(
            media,
            Media {
                url: "https://vd423.okcdn.ru/?expires=1792071069485&srcIp=84.79.202.66&pr=10&srcAg=CHROME&ms=178.237.23.39&type=2&sig=GBwHZBbjRbk&ct=0&urls=95.163.35.41&clientType=0&zs=65&id=555566762577".to_string(),
                referer: "https://ok.ru/".to_string(),
                host: "okru",
            }
        );

        // Synthetic JSON with hd and sd -> hd wins
        let synthetic = r#"
            <div data-module="OKVideo" data-options="{&quot;flashvars&quot;:{&quot;metadata&quot;:{&quot;videos&quot;:[
                {&quot;name&quot;:&quot;sd&quot;,&quot;url&quot;:&quot;https://cdn.ok.ru/sd.mp4&quot;},
                {&quot;name&quot;:&quot;hd&quot;,&quot;url&quot;:&quot;https://cdn.ok.ru/hd.mp4&quot;}
            ]}}}"></div>
        "#;
        let media_hd = resolve_okru(synthetic).expect("hd should win");
        assert_eq!(media_hd.url, "https://cdn.ok.ru/hd.mp4");
        assert_eq!(media_hd.referer, "https://ok.ru/");
        assert_eq!(media_hd.host, "okru");

        // Synthetic JSON with metadata as string -> hd wins
        let synthetic_str = r#"
            <div data-module="OKVideo" data-options="{&quot;flashvars&quot;:{&quot;metadata&quot;:&quot;{\&quot;videos\&quot;:[{\&quot;name\&quot;:\&quot;hd\&quot;,\&quot;url\&quot;:\&quot;https://cdn.ok.ru/hd_str.mp4\&quot;}]}&quot;}}"></div>
        "#;
        let media_str = resolve_okru(synthetic_str).expect("metadata as string");
        assert_eq!(media_str.url, "https://cdn.ok.ru/hd_str.mp4");

        // videos empty plus hlsManifestUrl -> that manifest
        let manifest_only = r#"
            <div data-module="OKVideo" data-options="{&quot;flashvars&quot;:{&quot;metadata&quot;:{
                &quot;videos&quot;:[],
                &quot;hlsManifestUrl&quot;:&quot;https://cdn.ok.ru/live/master.m3u8&quot;
            }}}"></div>
        "#;
        let media_man = resolve_okru(manifest_only).expect("hlsManifestUrl fallback");
        assert_eq!(media_man.url, "https://cdn.ok.ru/live/master.m3u8");
        assert_eq!(media_man.referer, "https://ok.ru/");
        assert_eq!(media_man.host, "okru");

        // HTML without the OKVideo element -> None
        assert_eq!(resolve_okru("<div>No OKVideo here</div>"), None);
    }

    #[test]
    fn supported_host_and_candidates_ordering_extended() {
        assert_eq!(
            supported_host(&Server {
                name: "Vidhide".to_string(),
                url: "https://callistanise.com/embed/x".to_string(),
            }),
            Some("vidhide")
        );
        assert_eq!(
            supported_host(&Server {
                name: "Whatever".to_string(),
                url: "https://callistanise.com/embed/x".to_string(),
            }),
            None
        );
        assert_eq!(
            supported_host(&Server {
                name: "Mixdrop".to_string(),
                url: "https://mxdrop.click/e/x".to_string(),
            }),
            Some("mixdrop")
        );
        assert_eq!(
            supported_host(&Server {
                name: "Whatever".to_string(),
                url: "https://ok.ru/videoembed/1".to_string(),
            }),
            Some("okru")
        );
        assert_eq!(
            supported_host(&Server {
                name: "Whatever".to_string(),
                url: "https://notok.ru/x".to_string(),
            }),
            None
        );
        assert_eq!(
            supported_host(&Server {
                name: "Vidhide".to_string(),
                url: "javascript:alert(1)".to_string(),
            }),
            None
        );

        // candidates over [Vidhide, Mixdrop, Okru, Streamtape, Mediafire, Mp4upload]
        let servers = vec![
            Server {
                name: "Vidhide".to_string(),
                url: "https://callistanise.com/embed/1".to_string(),
            },
            Server {
                name: "Mixdrop".to_string(),
                url: "https://mxdrop.click/e/1".to_string(),
            },
            Server {
                name: "Okru".to_string(),
                url: "https://ok.ru/videoembed/1".to_string(),
            },
            Server {
                name: "Streamtape".to_string(),
                url: "https://streamtape.com/e/1".to_string(),
            },
            Server {
                name: "Mediafire".to_string(),
                url: "https://mediafire.com/file/1/".to_string(),
            },
            Server {
                name: "Mp4upload".to_string(),
                url: "https://www.mp4upload.com/embed-1.html".to_string(),
            },
        ];
        let cand = candidates(&servers);
        let names: Vec<&str> = cand.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["Mp4upload", "Mediafire", "Streamtape", "Okru", "Mixdrop", "Vidhide"]
        );
    }

    #[test]
    fn resolve_from_page_extended_dispatcher() {
        let vidhide_fix = include_str!("../tests/fixtures/vidhide_embed.html");
        assert_eq!(
            resolve_from_page("vidhide", vidhide_fix, "https://callistanise.com/embed/etxwwazwbqtr"),
            resolve_vidhide(vidhide_fix, "https://callistanise.com/embed/etxwwazwbqtr")
        );

        let mixdrop_fix = include_str!("../tests/fixtures/mixdrop_embed.html");
        assert_eq!(
            resolve_from_page("mixdrop", mixdrop_fix, "https://mxdrop.click/e/366ovrg9tv9ekm"),
            resolve_mixdrop(mixdrop_fix, "https://mxdrop.click/e/366ovrg9tv9ekm")
        );

        let okru_fix = include_str!("../tests/fixtures/okru_embed.html");
        assert_eq!(
            resolve_from_page("okru", okru_fix, "https://ok.ru/videoembed/1815852419665"),
            resolve_okru(okru_fix)
        );
    }
}
