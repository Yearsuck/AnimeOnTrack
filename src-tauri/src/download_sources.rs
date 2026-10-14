//! Parsing layer that resolves episode pages and embed pages into downloadable media URLs.
//!
//! This module is completely pure and performs no network requests, filesystem access,
//! or other I/O. All inputs are provided as in-memory HTML/URL strings and parsed using
//! `scraper`, `serde_json`, and `url`.

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
            let prev_ch = html[..idx].chars().next_back().unwrap();
            if prev_ch.is_alphanumeric() || prev_ch == '_' || prev_ch == '$' {
                continue;
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

/// Scans the HTML/JS text for the first `file: '...'` or `file: "..."` property
/// whose URL ends in `.mp4` (case-insensitive, ignoring query/hash) and resolves to http(s).
fn find_first_mp4_file(html: &str, base_url: &str) -> Option<String> {
    let mut search_from = 0;
    while let Some(pos) = html[search_from..].find("file") {
        let file_idx = search_from + pos;
        search_from = file_idx + 4; // length of "file"

        // Ensure "file" is preceded by a non-identifier character (if not at start)
        if file_idx > 0 {
            let prev_ch = html[..file_idx].chars().next_back().unwrap();
            if prev_ch.is_alphanumeric() || prev_ch == '_' || prev_ch == '$' {
                continue;
            }
        }

        let after = &html[file_idx + 4..];
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

/// Returns the host identifier if the given server is supported for downloads.
///
/// Currently supports `"yourupload"` when the URL's host is `yourupload.com`
/// or ends with `.yourupload.com`. Returns `None` for unsupported hosts.
#[allow(dead_code)] // Consumed by download supervisor in later stages
pub fn supported_host(server: &Server) -> Option<&'static str> {
    let parsed = Url::parse(&server.url).ok()?;
    let host = parsed.host_str()?;
    let host_lower = host.to_ascii_lowercase();
    if host_lower == "yourupload.com" || host_lower.ends_with(".yourupload.com") {
        Some("yourupload")
    } else {
        None
    }
}

/// Filters a list of servers down to those on supported hosts, preserving
/// original order and deduplicating by URL (keeping the first occurrence).
#[allow(dead_code)] // Consumed by download supervisor in later stages
pub fn candidates(servers: &[Server]) -> Vec<Server> {
    let mut seen_urls = HashSet::new();
    let mut result = Vec::new();

    for server in servers {
        if supported_host(server).is_some() && seen_urls.insert(server.url.as_str()) {
            result.push(server.clone());
        }
    }

    result
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
}
