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

/// Returns the host identifier if the given server is supported for downloads.
///
/// Supports `"mp4upload"`, `"yourupload"`, `"mediafire"`, and `"streamtape"`
/// when the URL's host matches the domain or is a subdomain thereof.
#[allow(dead_code)] // Consumed by download supervisor in later stages
pub fn supported_host(server: &Server) -> Option<&'static str> {
    let parsed = Url::parse(&server.url).ok()?;
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
    } else {
        None
    }
}

/// Priority rank for supported hosts: mp4upload (0), yourupload (1), mediafire (2), streamtape (3).
fn host_preference(host: &str) -> u8 {
    match host {
        "mp4upload" => 0,
        "yourupload" => 1,
        "mediafire" => 2,
        "streamtape" => 3,
        _ => 255,
    }
}

/// Filters a list of servers down to those on supported hosts, ordering by host preference
/// first (`mp4upload` > `yourupload` > `mediafire` > `streamtape`) and page order second,
/// deduplicating by URL (keeping the first occurrence).
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

        assert_eq!(cand.len(), 2, "expected exactly 2 distinct YourUpload servers");
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
}
