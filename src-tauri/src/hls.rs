//! Pure in-memory HLS playlist parsing and streaming source resolution.
//!
//! This module performs no I/O, network requests, or filesystem access.
//! All inputs are provided as in-memory strings and parsed using `scraper`
//! and `url`.

use scraper::{Html, Selector};
use std::sync::OnceLock;
use url::Url;

/// Single media segment within an HLS media playlist.
#[allow(dead_code)] // Consumed by HLS download supervisor in later stages
#[derive(Debug, Clone, PartialEq)]
pub struct Segment {
    pub url: String,
    pub duration: f64,
}

/// Variant stream within an HLS multivariant (master) playlist.
#[allow(dead_code)] // Consumed by HLS download supervisor in later stages
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Variant {
    pub url: String,
    pub bandwidth: u64,
    pub height: Option<u32>,
}

/// Parsed HLS playlist, either Master (multivariant) or Media.
#[allow(dead_code)] // Consumed by HLS download supervisor in later stages
#[derive(Debug, Clone, PartialEq)]
pub enum Playlist {
    Master {
        variants: Vec<Variant>,
    },
    Media {
        segments: Vec<Segment>,
        ended: bool,
        encrypted: bool,
        has_init_map: bool,
    },
}

/// Maximum number of segments allowed in a single downloadable stream.
#[allow(dead_code)] // Consumed by HLS download supervisor in later stages
pub const MAX_SEGMENTS: usize = 20_000;

/// Error returned when an HLS stream cannot be downloaded.
#[allow(dead_code)] // Consumed by HLS download supervisor in later stages
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HlsError {
    NotPlayable(&'static str),
}

impl std::fmt::Display for HlsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HlsError::NotPlayable(reason) => write!(f, "{reason}"),
        }
    }
}

impl std::error::Error for HlsError {}

/// Splits comma-separated HLS tag attributes quote-aware.
///
/// Attribute values enclosed in double quotes may contain commas (e.g. `CODECS="avc1,mp4a"`),
/// which must not be treated as attribute delimiters.
fn split_hls_attributes(attrs: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut in_quote = false;
    let mut start = 0;

    for (i, ch) in attrs.char_indices() {
        if ch == '"' {
            in_quote = !in_quote;
        } else if ch == ',' && !in_quote {
            if let Some(slice) = attrs.get(start..i) {
                let trimmed = slice.trim();
                if !trimmed.is_empty() {
                    parts.push(trimmed);
                }
            }
            start = i.saturating_add(ch.len_utf8());
        }
    }
    if let Some(slice) = attrs.get(start..) {
        let trimmed = slice.trim();
        if !trimmed.is_empty() {
            parts.push(trimmed);
        }
    }
    parts
}

/// Parses `BANDWIDTH` and `RESOLUTION` from an `#EXT-X-STREAM-INF` attribute line.
fn parse_stream_inf_attrs(attrs: &str) -> (u64, Option<u32>) {
    let mut bandwidth = 0u64;
    let mut height = None;

    for attr in split_hls_attributes(attrs) {
        if let Some((key, val)) = attr.split_once('=') {
            let key = key.trim();
            let val = val.trim();
            let unquoted = val
                .strip_prefix('"')
                .and_then(|s| s.strip_suffix('"'))
                .unwrap_or(val)
                .trim();

            if key.eq_ignore_ascii_case("BANDWIDTH") {
                if let Ok(b) = unquoted.parse::<u64>() {
                    bandwidth = b;
                }
            } else if key.eq_ignore_ascii_case("RESOLUTION") {
                if let Some((_w, h_str)) = unquoted.split_once(['x', 'X']) {
                    if let Ok(h) = h_str.trim().parse::<u32>() {
                        height = Some(h);
                    }
                }
            }
        }
    }

    (bandwidth, height)
}

/// Resolves a candidate URI against a base URL using `Url::join`.
///
/// Absolute URLs stay as they are. Returns `None` if the resulting URL does not
/// use the `http` or `https` scheme.
fn resolve_hls_url(candidate: &str, base_url: &str) -> Option<String> {
    let trimmed = candidate.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Ok(base) = Url::parse(base_url) {
        if let Ok(joined) = base.join(trimmed) {
            let scheme = joined.scheme();
            if scheme == "http" || scheme == "https" {
                return Some(joined.to_string());
            }
            return None;
        }
    }
    if let Ok(parsed) = Url::parse(trimmed) {
        let scheme = parsed.scheme();
        if scheme == "http" || scheme == "https" {
            return Some(parsed.to_string());
        }
    }
    None
}

/// Parses an HLS playlist (Master or Media) from in-memory text and resolves relative URIs.
///
/// Returns `None` if the text does not start with `#EXTM3U` (after optional BOM and whitespace).
/// A playlist containing `#EXT-X-STREAM-INF` is parsed as Master; otherwise it is parsed as Media.
#[allow(dead_code)] // Consumed by HLS download supervisor in later stages
pub fn parse_playlist(text: &str, base_url: &str) -> Option<Playlist> {
    let trimmed = text.trim_start_matches(|c: char| c.is_whitespace() || c == '\u{feff}');
    let after_header = trimmed.strip_prefix("#EXTM3U")?;
    if let Some(next_ch) = after_header.chars().next() {
        if !next_ch.is_whitespace() {
            return None;
        }
    }

    if text.contains("#EXT-X-STREAM-INF") {
        let mut variants = Vec::new();
        let mut pending_variant: Option<(u64, Option<u32>)> = None;

        for raw_line in trimmed.lines() {
            let line = raw_line.trim();
            if line.is_empty() {
                continue;
            }
            let attrs_opt = if let Some(rest) = line.strip_prefix("#EXT-X-STREAM-INF:") {
                Some(rest)
            } else if let Some(rest) = line.strip_prefix("#EXT-X-STREAM-INF") {
                Some(rest.strip_prefix(':').unwrap_or(rest))
            } else {
                None
            };

            if let Some(attrs_str) = attrs_opt {
                let (bandwidth, height) = parse_stream_inf_attrs(attrs_str);
                pending_variant = Some((bandwidth, height));
            } else if line.starts_with('#') {
                continue;
            } else if let Some((bandwidth, height)) = pending_variant.take() {
                if let Some(resolved_url) = resolve_hls_url(line, base_url) {
                    variants.push(Variant {
                        url: resolved_url,
                        bandwidth,
                        height,
                    });
                }
            }
        }

        Some(Playlist::Master { variants })
    } else {
        let mut segments = Vec::new();
        let mut ended = false;
        let mut encrypted = false;
        let mut has_init_map = false;
        let mut pending_duration: Option<f64> = None;

        for raw_line in trimmed.lines() {
            let line = raw_line.trim();
            if line.is_empty() {
                continue;
            }
            if line.starts_with("#EXT-X-ENDLIST") {
                ended = true;
            } else if line.starts_with("#EXT-X-MAP") {
                has_init_map = true;
            } else if let Some(key_attrs) = line.strip_prefix("#EXT-X-KEY:") {
                for attr in split_hls_attributes(key_attrs) {
                    if let Some((k, v)) = attr.split_once('=') {
                        if k.trim().eq_ignore_ascii_case("METHOD") {
                            let v_trim = v.trim().trim_matches('"');
                            if !v_trim.eq_ignore_ascii_case("NONE") {
                                encrypted = true;
                            }
                        }
                    }
                }
            } else if let Some(inf_rest) = line.strip_prefix("#EXTINF:") {
                let dur_part = inf_rest.split(',').next().unwrap_or("").trim();
                let dur = dur_part.parse::<f64>().unwrap_or(0.0);
                let valid_dur = if dur.is_finite() && dur >= 0.0 { dur } else { 0.0 };
                pending_duration = Some(valid_dur);
            } else if line.starts_with('#') {
                continue;
            } else if let Some(duration) = pending_duration.take() {
                if let Some(resolved_url) = resolve_hls_url(line, base_url) {
                    segments.push(Segment {
                        url: resolved_url,
                        duration,
                    });
                }
            }
        }

        Some(Playlist::Media {
            segments,
            ended,
            encrypted,
            has_init_map,
        })
    }
}

/// Selects the best variant stream based on height, breaking ties by bandwidth.
///
/// A variant with no resolution (`height: None`) counts as 0 height. Ties in both
/// height and bandwidth preserve the first occurrence in the slice. Returns `None`
/// for an empty slice.
#[allow(dead_code)] // Consumed by HLS download supervisor in later stages
pub fn best_variant(variants: &[Variant]) -> Option<&Variant> {
    let mut best: Option<&Variant> = None;
    for v in variants {
        match best {
            None => best = Some(v),
            Some(cur) => {
                let cur_key = (cur.height.unwrap_or(0), cur.bandwidth);
                let v_key = (v.height.unwrap_or(0), v.bandwidth);
                if v_key > cur_key {
                    best = Some(v);
                }
            }
        }
    }
    best
}

/// Selects a variant stream from an HLS master playlist based on user preference.
///
/// When `preferred_height` is `None`, selects the best variant (highest height,
/// breaking ties by bandwidth). When `Some(preferred)`, returns the highest
/// variant with height <= preferred. If none exist with height <= preferred, returns
/// the lowest variant above preferred (breaking ties by highest bandwidth).
/// Returns `None` for an empty slice.
#[allow(dead_code)] // Consumed by the downloader in the next commit.
pub fn select_variant(variants: &[Variant], preferred_height: Option<u32>) -> Option<&Variant> {
    let pref = match preferred_height {
        Some(h) => h,
        None => return best_variant(variants),
    };

    // Variants without RESOLUTION are skipped here: their height is unknown
    // (they can even be audio-only), so a preference can't rank them. They are
    // still reachable through the `best_variant` fallback below.
    // Highest variant with height <= preferred
    let mut under_or_eq: Option<&Variant> = None;
    for v in variants {
        let Some(h) = v.height else { continue };
        if h <= pref {
            match under_or_eq {
                None => under_or_eq = Some(v),
                Some(cur) => {
                    let cur_key = (cur.height.unwrap_or(0), cur.bandwidth);
                    let v_key = (h, v.bandwidth);
                    if v_key > cur_key {
                        under_or_eq = Some(v);
                    }
                }
            }
        }
    }
    if let Some(v) = under_or_eq {
        return Some(v);
    }

    // Else: lowest variant above preferred (min height; ties broken by max bandwidth)
    let mut above: Option<&Variant> = None;
    for v in variants {
        let Some(h) = v.height else { continue };
        if h > pref {
            match above {
                None => above = Some(v),
                Some(cur) => {
                    let cur_h = cur.height.unwrap_or(0);
                    if h < cur_h || (h == cur_h && v.bandwidth > cur.bandwidth) {
                        above = Some(v);
                    }
                }
            }
        }
    }
    above.or_else(|| best_variant(variants))
}

/// Validates that an HLS stream is playable and suitable for downloading.
///
/// Checks are performed in strict order:
/// 1. `encrypted` -> "encrypted stream"
/// 2. `!ended` -> "live stream"
/// 3. `has_init_map` -> "fragmented mp4 stream"
/// 4. empty `segments` -> "empty playlist"
/// 5. segments exceeding `MAX_SEGMENTS` -> "playlist too long"
#[allow(dead_code)] // Consumed by HLS download supervisor in later stages
pub fn check_downloadable(
    segments: &[Segment],
    ended: bool,
    encrypted: bool,
    has_init_map: bool,
) -> Result<(), HlsError> {
    if encrypted {
        return Err(HlsError::NotPlayable("encrypted stream"));
    }
    if !ended {
        return Err(HlsError::NotPlayable("live stream"));
    }
    if has_init_map {
        return Err(HlsError::NotPlayable("fragmented mp4 stream"));
    }
    if segments.is_empty() {
        return Err(HlsError::NotPlayable("empty playlist"));
    }
    if segments.len() > MAX_SEGMENTS {
        return Err(HlsError::NotPlayable("playlist too long"));
    }
    Ok(())
}

/// Validates if a raw URL points to a GogoAnime player embed.
fn is_valid_gogoanime_player_url(raw: &str) -> bool {
    let Ok(parsed) = Url::parse(raw) else {
        return false;
    };
    let scheme = parsed.scheme();
    if scheme != "http" && scheme != "https" {
        return false;
    }
    let Some(host) = parsed.host_str() else {
        return false;
    };
    let host_lower = host.to_ascii_lowercase();
    if host_lower != "gogoanime.by" && !host_lower.ends_with(".gogoanime.by") {
        return false;
    }
    if !parsed.path().starts_with("/player/") {
        return false;
    }
    parsed.query_pairs().any(|(k, v)| k == "source" && v == "embed")
}

/// Parses the player wrapper URL from a GogoAnime episode HTML page.
///
/// Returns the first `data-src` (or `href`) attribute value whose host is
/// `gogoanime.by` (or subdomain), path starts with `/player/`, and contains
/// `source=embed` in the query string. HTML entities are decoded by `scraper`.
#[allow(dead_code)] // Consumed by HLS download supervisor in later stages
pub fn parse_gogoanime_player_url(episode_html: &str) -> Option<String> {
    static LINK_SELECTOR: OnceLock<Option<Selector>> = OnceLock::new();
    let selector = LINK_SELECTOR.get_or_init(|| Selector::parse("[data-src], [href]").ok());

    let sel = selector.as_ref()?;

    let doc = Html::parse_document(episode_html);
    for el in doc.select(sel) {
        if let Some(val) = el.value().attr("data-src") {
            let trimmed = val.trim();
            if is_valid_gogoanime_player_url(trimmed) {
                return Some(trimmed.to_string());
            }
        }
        if let Some(val) = el.value().attr("href") {
            let trimmed = val.trim();
            if is_valid_gogoanime_player_url(trimmed) {
                return Some(trimmed.to_string());
            }
        }
    }

    None
}

/// Parses the MegaPlay embed URL from a player wrapper HTML page.
///
/// Looks for an iframe with class `player-iframe` and extracts its `src` attribute.
/// Falls back to the first iframe with a valid `http` or `https` `src`.
#[allow(dead_code)] // Consumed by HLS download supervisor in later stages
pub fn parse_wrapper_iframe(player_html: &str) -> Option<String> {
    static PLAYER_IFRAME_SELECTOR: OnceLock<Option<Selector>> = OnceLock::new();
    static ALL_IFRAME_SELECTOR: OnceLock<Option<Selector>> = OnceLock::new();

    let player_sel = PLAYER_IFRAME_SELECTOR.get_or_init(|| Selector::parse("iframe.player-iframe").ok());
    let all_sel = ALL_IFRAME_SELECTOR.get_or_init(|| Selector::parse("iframe").ok());

    let doc = Html::parse_document(player_html);

    if let Some(sel) = player_sel.as_ref() {
        for el in doc.select(sel) {
            if let Some(raw_src) = el.value().attr("src") {
                let trimmed = raw_src.trim();
                if let Ok(parsed) = Url::parse(trimmed) {
                    let scheme = parsed.scheme();
                    if scheme == "http" || scheme == "https" {
                        return Some(trimmed.to_string());
                    }
                }
            }
        }
    }

    if let Some(sel) = all_sel.as_ref() {
        for el in doc.select(sel) {
            if let Some(raw_src) = el.value().attr("src") {
                let trimmed = raw_src.trim();
                if let Ok(parsed) = Url::parse(trimmed) {
                    let scheme = parsed.scheme();
                    if scheme == "http" || scheme == "https" {
                        return Some(trimmed.to_string());
                    }
                }
            }
        }
    }

    None
}

/// Parses the `.m3u8` playlist URL from a MegaPlay embed HTML page.
///
/// Unescapes `\/` if present, then scans for `http://` or `https://` URLs whose host
/// is `megaplay.su` (or a subdomain) and path ends with `.m3u8`.
#[allow(dead_code)] // Consumed by HLS download supervisor in later stages
pub fn parse_megaplay_playlist_url(embed_html: &str) -> Option<String> {
    let unescaped;
    let text: &str = if embed_html.contains(r"\/") {
        unescaped = embed_html.replace(r"\/", "/");
        &unescaped
    } else {
        embed_html
    };

    let mut search_from = 0;
    while search_from < text.len() {
        let remaining = match text.get(search_from..) {
            Some(r) => r,
            None => break,
        };

        let http_pos = remaining.find("http://");
        let https_pos = remaining.find("https://");

        let rel_pos = match (http_pos, https_pos) {
            (Some(p1), Some(p2)) => p1.min(p2),
            (Some(p), None) | (None, Some(p)) => p,
            (None, None) => break,
        };

        let start_idx = search_from + rel_pos;
        search_from = start_idx.saturating_add(7);

        let candidate_slice = match text.get(start_idx..) {
            Some(s) => s,
            None => continue,
        };

        let cut_pos = candidate_slice
            .find(|c: char| {
                c == '"'
                    || c == '\''
                    || c == '`'
                    || c.is_whitespace()
                    || c == '<'
                    || c == '>'
                    || c == '\\'
            })
            .unwrap_or(candidate_slice.len());

        let raw_url = match candidate_slice.get(..cut_pos) {
            Some(u) => u,
            None => continue,
        };

        if let Ok(parsed) = Url::parse(raw_url) {
            let scheme = parsed.scheme();
            if scheme == "http" || scheme == "https" {
                if let Some(host) = parsed.host_str() {
                    let host_lower = host.to_ascii_lowercase();
                    if (host_lower == "megaplay.su" || host_lower.ends_with(".megaplay.su"))
                        && parsed.path().to_ascii_lowercase().ends_with(".m3u8")
                    {
                        return Some(raw_url.to_string());
                    }
                }
            }
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_megaplay_media_playlist_fixture() {
        let fixture = include_str!("../tests/fixtures/megaplay_index.m3u8");
        let base_url = "https://megaplay.su/uploads/hls/Wm9xcEFlRjdEVWZEM2FxbGdhOHFPUT09/index.m3u8?v=1791919534";

        let playlist = parse_playlist(fixture, base_url).expect("should parse fixture playlist");
        let Playlist::Media {
            segments,
            ended,
            encrypted,
            has_init_map,
        } = playlist
        else {
            panic!("expected media playlist, got {playlist:?}");
        };

        assert_eq!(segments.len(), 229);
        assert_eq!(
            segments[0].url,
            "https://megaplay.su/uploads/hls/Wm9xcEFlRjdEVWZEM2FxbGdhOHFPUT09/segment_00000.ts"
        );
        assert_eq!(segments[0].duration, 6.006);
        assert_eq!(
            segments[228].url,
            "https://megaplay.su/uploads/hls/Wm9xcEFlRjdEVWZEM2FxbGdhOHFPUT09/segment_00228.ts"
        );
        assert!((segments[228].duration - 2.002133).abs() < 1e-5);
        assert!(ended);
        assert!(!encrypted);
        assert!(!has_init_map);

        assert_eq!(
            check_downloadable(&segments, ended, encrypted, has_init_map),
            Ok(())
        );
    }

    #[test]
    fn parses_master_playlist_synthetic_and_resolves_best_variant() {
        let master = "\
#EXTM3U\n\
#EXT-X-STREAM-INF:BANDWIDTH=800000,RESOLUTION=640x360,CODECS=\"avc1.4d401f,mp4a.40.2\"\n\
low/index.m3u8\n\
#EXT-X-STREAM-INF:BANDWIDTH=2800000,RESOLUTION=1280x720\n\
https://cdn.example/hi/index.m3u8\n";

        let base_url = "https://x.example/a/master.m3u8";
        let parsed = parse_playlist(master, base_url).expect("valid master");

        let expected_variants = vec![
            Variant {
                url: "https://x.example/a/low/index.m3u8".to_string(),
                bandwidth: 800_000,
                height: Some(360),
            },
            Variant {
                url: "https://cdn.example/hi/index.m3u8".to_string(),
                bandwidth: 2_800_000,
                height: Some(720),
            },
        ];

        assert_eq!(
            parsed,
            Playlist::Master {
                variants: expected_variants.clone()
            }
        );

        let best = best_variant(&expected_variants);
        assert_eq!(best, Some(&expected_variants[1]));

        // Two variants without resolution pick the higher bandwidth
        let no_res_variants = vec![
            Variant {
                url: "https://example.com/v1.m3u8".to_string(),
                bandwidth: 500_000,
                height: None,
            },
            Variant {
                url: "https://example.com/v2.m3u8".to_string(),
                bandwidth: 1_200_000,
                height: None,
            },
        ];
        assert_eq!(best_variant(&no_res_variants), Some(&no_res_variants[1]));

        // Ties in height and bandwidth preserve the first item
        let tie_variants = vec![
            Variant {
                url: "https://example.com/first.m3u8".to_string(),
                bandwidth: 1_000_000,
                height: Some(1080),
            },
            Variant {
                url: "https://example.com/second.m3u8".to_string(),
                bandwidth: 1_000_000,
                height: Some(1080),
            },
        ];
        assert_eq!(best_variant(&tie_variants), Some(&tie_variants[0]));

        // Empty slice returns None
        assert_eq!(best_variant(&[]), None);
    }

    #[test]
    fn select_variant_quality_preference() {
        let v1080 = Variant {
            url: "https://example.com/1080.m3u8".to_string(),
            bandwidth: 3_000_000,
            height: Some(1080),
        };
        let v720 = Variant {
            url: "https://example.com/720.m3u8".to_string(),
            bandwidth: 1_800_000,
            height: Some(720),
        };
        let v480 = Variant {
            url: "https://example.com/480.m3u8".to_string(),
            bandwidth: 900_000,
            height: Some(480),
        };

        let all_three = vec![v1080.clone(), v720.clone(), v480.clone()];

        // 1. best (= None preferred_height) picks highest (1080)
        assert_eq!(select_variant(&all_three, None), Some(&v1080));

        // 2. 720 with 1080/720/480 present picks 720 (highest <= 720)
        assert_eq!(select_variant(&all_three, Some(720)), Some(&v720));

        // 3. 720 with only 1080 present picks 1080 (lowest above 720)
        let only_1080 = vec![v1080.clone()];
        assert_eq!(select_variant(&only_1080, Some(720)), Some(&v1080));

        // 4. 480 with only 720+ present picks 720 (lowest above 480)
        let only_720_plus = vec![v1080.clone(), v720.clone()];
        assert_eq!(select_variant(&only_720_plus, Some(480)), Some(&v720));

        // A variant without RESOLUTION (possibly audio-only) never wins a
        // preference over one whose height is known
        let no_res = Variant {
            url: "https://example.com/audio.m3u8".to_string(),
            bandwidth: 128_000,
            height: None,
        };
        let with_unknown = vec![no_res.clone(), v1080.clone()];
        assert_eq!(select_variant(&with_unknown, Some(720)), Some(&v1080));

        // Only unknown heights: falls back to best_variant
        assert_eq!(select_variant(std::slice::from_ref(&no_res), Some(720)), Some(&no_res));

        // Empty slice returns None
        assert_eq!(select_variant(&[], Some(720)), None);
    }

    #[test]
    fn validates_check_downloadable_conditions_and_order() {
        let single_seg = vec![Segment {
            url: "https://example.com/001.ts".to_string(),
            duration: 6.0,
        }];

        // 1. Encrypted stream error
        assert_eq!(
            check_downloadable(&single_seg, true, true, false),
            Err(HlsError::NotPlayable("encrypted stream"))
        );

        // 2. Live stream (not ended)
        assert_eq!(
            check_downloadable(&single_seg, false, false, false),
            Err(HlsError::NotPlayable("live stream"))
        );

        // 3. Fragmented MP4 stream (has init map)
        assert_eq!(
            check_downloadable(&single_seg, true, false, true),
            Err(HlsError::NotPlayable("fragmented mp4 stream"))
        );

        // 4. Empty playlist
        assert_eq!(
            check_downloadable(&[], true, false, false),
            Err(HlsError::NotPlayable("empty playlist"))
        );

        // 5. Playlist too long (more than MAX_SEGMENTS)
        let too_many_segments: Vec<Segment> = (0..=MAX_SEGMENTS)
            .map(|i| Segment {
                url: format!("https://example.com/seg_{i}.ts"),
                duration: 2.0,
            })
            .collect();
        assert_eq!(
            check_downloadable(&too_many_segments, true, false, false),
            Err(HlsError::NotPlayable("playlist too long"))
        );

        // Strict priority order: encrypted takes precedence over live stream
        assert_eq!(
            check_downloadable(&single_seg, false, true, true),
            Err(HlsError::NotPlayable("encrypted stream"))
        );

        // Live stream takes precedence over init map
        assert_eq!(
            check_downloadable(&single_seg, false, false, true),
            Err(HlsError::NotPlayable("live stream"))
        );
    }

    #[test]
    fn parses_synthetic_media_tags_and_edge_cases() {
        // Encrypted with AES-128
        let enc_m3u8 = "#EXTM3U\n#EXT-X-KEY:METHOD=AES-128,URI=\"k.key\"\n#EXTINF:4.0,\nseg1.ts\n#EXT-X-ENDLIST\n";
        let parsed_enc = parse_playlist(enc_m3u8, "https://example.com/index.m3u8");
        match parsed_enc {
            Some(Playlist::Media { encrypted, .. }) => assert!(encrypted),
            other => panic!("expected media playlist, got {other:?}"),
        }

        // METHOD=NONE is not encrypted
        let none_m3u8 = "#EXTM3U\n#EXT-X-KEY:METHOD=NONE\n#EXTINF:4.0,\nseg1.ts\n#EXT-X-ENDLIST\n";
        let parsed_none = parse_playlist(none_m3u8, "https://example.com/index.m3u8");
        match parsed_none {
            Some(Playlist::Media { encrypted, .. }) => assert!(!encrypted),
            other => panic!("expected media playlist, got {other:?}"),
        }

        // No #EXT-X-ENDLIST means not ended
        let live_m3u8 = "#EXTM3U\n#EXTINF:4.0,\nseg1.ts\n";
        let parsed_live = parse_playlist(live_m3u8, "https://example.com/index.m3u8");
        match parsed_live {
            Some(Playlist::Media { ended, .. }) => assert!(!ended),
            other => panic!("expected media playlist, got {other:?}"),
        }

        // #EXT-X-MAP tag detection
        let map_m3u8 = "#EXTM3U\n#EXT-X-MAP:URI=\"init.mp4\"\n#EXTINF:4.0,\nseg1.ts\n#EXT-X-ENDLIST\n";
        let parsed_map = parse_playlist(map_m3u8, "https://example.com/index.m3u8");
        match parsed_map {
            Some(Playlist::Media { has_init_map, .. }) => assert!(has_init_map),
            other => panic!("expected media playlist, got {other:?}"),
        }

        // CRLF line endings parse identically to LF
        let crlf_m3u8 = "#EXTM3U\r\n#EXT-X-VERSION:3\r\n#EXTINF:5.0,\r\nsegment1.ts\r\n#EXT-X-ENDLIST\r\n";
        let lf_m3u8 = "#EXTM3U\n#EXT-X-VERSION:3\n#EXTINF:5.0,\nsegment1.ts\n#EXT-X-ENDLIST\n";
        assert_eq!(
            parse_playlist(crlf_m3u8, "https://example.com/index.m3u8"),
            parse_playlist(lf_m3u8, "https://example.com/index.m3u8")
        );

        // UTF-8 BOM prefix
        let bom_m3u8 = "\u{feff}#EXTM3U\n#EXTINF:3.5,\nseg.ts\n#EXT-X-ENDLIST\n";
        assert!(parse_playlist(bom_m3u8, "https://example.com/index.m3u8").is_some());

        // Text not starting with #EXTM3U returns None
        assert_eq!(parse_playlist("<html><body>not hls</body></html>", "https://example.com/"), None);
        assert_eq!(parse_playlist("", "https://example.com/"), None);
        assert_eq!(parse_playlist("   ", "https://example.com/"), None);

        // Non-http segment URLs are dropped
        let bad_seg_m3u8 = "#EXTM3U\n#EXTINF:4.0,\njavascript:alert(1)\n#EXTINF:5.0,\nseg2.ts\n#EXT-X-ENDLIST\n";
        let parsed_bad_seg = parse_playlist(bad_seg_m3u8, "https://example.com/index.m3u8");
        match parsed_bad_seg {
            Some(Playlist::Media { segments, .. }) => {
                assert_eq!(segments.len(), 1);
                assert_eq!(segments[0].url, "https://example.com/seg2.ts");
                assert_eq!(segments[0].duration, 5.0);
            }
            other => panic!("expected media playlist, got {other:?}"),
        }

        // Unparsable duration counts as 0.0
        let unparsable_dur_m3u8 = "#EXTM3U\n#EXTINF:abc,\nseg.ts\n#EXT-X-ENDLIST\n";
        let parsed_unparsable = parse_playlist(unparsable_dur_m3u8, "https://example.com/index.m3u8");
        match parsed_unparsable {
            Some(Playlist::Media { segments, .. }) => {
                assert_eq!(segments.len(), 1);
                assert_eq!(segments[0].duration, 0.0);
            }
            other => panic!("expected media playlist, got {other:?}"),
        }
    }

    #[test]
    fn parses_gogoanime_player_url_fixture_and_edge_cases() {
        let fixture = include_str!("../tests/fixtures/gogoanime_episode.html");
        let expected = "https://gogoanime.by/player/?source=embed&url=V3oraS9OdVNOdFNoTUZuWm9qa1FrQys3VS9Udzg0Y2REM2JtUThONEZzNmx0b29XZzdKQkQxejd6VzJrS3dkNkhWbEswUzdXK2hIRlh5Nm9TWHpTSEh2SWtXVmt5YmIzOHlCWEpZNVYxclE9";
        assert_eq!(parse_gogoanime_player_url(fixture), Some(expected.to_string()));

        // Non-gogoanime host is rejected
        let evil_html = r#"<li data-src="https://evil.example/player/?source=embed&url=x">HD</li>"#;
        assert_eq!(parse_gogoanime_player_url(evil_html), None);

        // Host ending with similar text but not dot-delimited is rejected
        let spoof_html = r#"<li data-src="https://fakegogoanime.by/player/?source=embed&url=x">HD</li>"#;
        assert_eq!(parse_gogoanime_player_url(spoof_html), None);

        // HTML without player element returns None
        let empty_html = "<div>No player here</div>";
        assert_eq!(parse_gogoanime_player_url(empty_html), None);

        // Subdomain of gogoanime.by is accepted
        let sub_html = r#"<li data-src="https://stream.gogoanime.by/player/?source=embed&url=abc">HD</li>"#;
        assert_eq!(
            parse_gogoanime_player_url(sub_html),
            Some("https://stream.gogoanime.by/player/?source=embed&url=abc".to_string())
        );

        // href fallback works when data-src is absent
        let href_html = r#"<a href="https://gogoanime.by/player/?source=embed&url=test">Play</a>"#;
        assert_eq!(
            parse_gogoanime_player_url(href_html),
            Some("https://gogoanime.by/player/?source=embed&url=test".to_string())
        );

        // Missing source=embed parameter is rejected
        let no_source_html = r#"<li data-src="https://gogoanime.by/player/?url=abc">HD</li>"#;
        assert_eq!(parse_gogoanime_player_url(no_source_html), None);

        // Path not starting with /player/ is rejected
        let bad_path_html = r#"<li data-src="https://gogoanime.by/video/?source=embed&url=abc">HD</li>"#;
        assert_eq!(parse_gogoanime_player_url(bad_path_html), None);
    }

    #[test]
    fn parses_wrapper_iframe_fixture_and_edge_cases() {
        let fixture = include_str!("../tests/fixtures/gogoanime_player_wrapper.html");
        let expected = "https://megaplay.su/embed.php?sid=Wm9xcEFlRjdEVWZEM2FxbGdhOHFPUT09";
        assert_eq!(parse_wrapper_iframe(fixture), Some(expected.to_string()));

        // iframe with non-http src is rejected
        let js_iframe = r#"<iframe class="player-iframe" src="javascript:false"></iframe>"#;
        assert_eq!(parse_wrapper_iframe(js_iframe), None);

        // Fallback to first http iframe when player-iframe class is absent
        let fallback_iframe = r#"<div><iframe src="https://example.com/embed.html"></iframe></div>"#;
        assert_eq!(
            parse_wrapper_iframe(fallback_iframe),
            Some("https://example.com/embed.html".to_string())
        );

        // Multiple iframes: player-iframe takes precedence
        let multi_iframe = r#"
            <iframe src="https://example.com/ad.html"></iframe>
            <iframe class="player-iframe" src="https://megaplay.su/embed.php?sid=123"></iframe>
        "#;
        assert_eq!(
            parse_wrapper_iframe(multi_iframe),
            Some("https://megaplay.su/embed.php?sid=123".to_string())
        );

        // HTML without iframes returns None
        assert_eq!(parse_wrapper_iframe("<div>No iframe</div>"), None);
    }

    #[test]
    fn parses_megaplay_playlist_url_fixture_and_edge_cases() {
        let fixture = include_str!("../tests/fixtures/megaplay_embed.html");
        let expected = "https://megaplay.su/uploads/hls/Wm9xcEFlRjdEVWZEM2FxbGdhOHFPUT09/index.m3u8?v=1791919534";
        assert_eq!(parse_megaplay_playlist_url(fixture), Some(expected.to_string()));

        // Escaped slashes are unescaped
        let escaped_html = r#"var url = "https:\/\/megaplay.su\/uploads\/hls\/abc\/index.m3u8";"#;
        assert_eq!(
            parse_megaplay_playlist_url(escaped_html),
            Some("https://megaplay.su/uploads/hls/abc/index.m3u8".to_string())
        );

        // Host that is not megaplay.su is rejected
        let evil_m3u8 = r#"file: "https://evil.example/x.m3u8""#;
        assert_eq!(parse_megaplay_playlist_url(evil_m3u8), None);

        // Subdomain spoofing rejected (not ending in dot-megaplay.su)
        let spoof_m3u8 = r#"file: "https://notmegaplay.su/x.m3u8""#;
        assert_eq!(parse_megaplay_playlist_url(spoof_m3u8), None);

        // Megaplay URL not ending in .m3u8 is rejected
        let non_m3u8 = r#"file: "https://megaplay.su/video.mp4""#;
        assert_eq!(parse_megaplay_playlist_url(non_m3u8), None);

        // Subdomain of megaplay.su is accepted
        let sub_m3u8 = r#"file: "https://cdn.megaplay.su/stream/index.m3u8""#;
        assert_eq!(
            parse_megaplay_playlist_url(sub_m3u8),
            Some("https://cdn.megaplay.su/stream/index.m3u8".to_string())
        );

        // URL terminated by various delimiter characters
        let space_delim = "url is https://megaplay.su/stream/index.m3u8 and more text";
        assert_eq!(
            parse_megaplay_playlist_url(space_delim),
            Some("https://megaplay.su/stream/index.m3u8".to_string())
        );

        let angle_delim = "<https://megaplay.su/stream/index.m3u8>";
        assert_eq!(
            parse_megaplay_playlist_url(angle_delim),
            Some("https://megaplay.su/stream/index.m3u8".to_string())
        );

        // Empty text returns None
        assert_eq!(parse_megaplay_playlist_url(""), None);
    }
}
