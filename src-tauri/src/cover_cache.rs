use anyhow::{anyhow, Context, Result};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::Duration;
use tauri::{AppHandle, Manager};
use url::Url;

/// Cap on downloaded cover image size: 5MB.
pub const MAX_COVER_BYTES: usize = 5 * 1024 * 1024;

/// Timeout for downloading direct cover images: 10s.
pub const COVER_DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(10);

/// Browser User-Agent mimicking a modern desktop Chrome on Windows.
const BROWSER_USER_AGENT: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/133.0.0.0 Safari/537.36";

/// Process-wide HTTP client for cover image downloads, configured with a 10s timeout.
static HTTP_CLIENT: LazyLock<reqwest::Client> = LazyLock::new(|| {
    reqwest::Client::builder()
        .timeout(COVER_DOWNLOAD_TIMEOUT)
        // `is_safe_external_url` only vets the URL we were given; a public
        // host could still 302 to localhost or a LAN address, so re-check
        // every hop and cap the chain.
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            if attempt.previous().len() >= 5 {
                attempt.error("too many redirects")
            } else if !crate::scraper_engine::is_safe_external_url(attempt.url().as_str()) {
                attempt.stop()
            } else {
                attempt.follow()
            }
        }))
        .build()
        .expect("failed to build reqwest client for cover cache")
});

/// Specific download failure modes so callers can differentiate Cloudflare
/// bot-detection blocks (HTTP 403 / 503) from normal HTTP/network/validation errors.
#[derive(Debug)]
pub enum DownloadError {
    /// Server returned 403 or 503, indicating Cloudflare or WAF protection.
    CloudflareBlocked(u16),
    /// Server returned an unexpected non-success HTTP status.
    HttpStatus(u16),
    /// Content-Type header was missing or did not start with `image/`.
    InvalidContentType(String),
    /// Image payload exceeded `MAX_COVER_BYTES` (5MB).
    TooLarge(usize),
    /// Filesystem error when writing to disk.
    Io(std::io::Error),
    /// Transport/network error.
    Network(reqwest::Error),
    /// Cover URL could not be parsed or has an opaque origin.
    InvalidUrl(String),
    /// Other error message.
    Other(String),
}

impl std::fmt::Display for DownloadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::CloudflareBlocked(code) => write!(f, "Cloudflare blocked ({code})"),
            Self::HttpStatus(code) => write!(f, "HTTP error status ({code})"),
            Self::InvalidContentType(ct) => write!(f, "invalid content-type: {ct}"),
            Self::TooLarge(size) => write!(f, "cover exceeded 5MB cap ({size} bytes)"),
            Self::Io(e) => write!(f, "IO error: {e}"),
            Self::Network(e) => write!(f, "network error: {e}"),
            Self::InvalidUrl(u) => write!(f, "invalid url: {u}"),
            Self::Other(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for DownloadError {}

impl DownloadError {
    /// Whether the WebView2 canvas grab is worth trying after this failure.
    /// Anything a real browser session could plausibly get past qualifies —
    /// Cloudflare challenges, 404/429/5xx from hotlink or rate protection, an
    /// HTML interstitial served with a 200, network errors. Not worth it:
    /// a URL we refuse on safety grounds, an oversized payload, or a local
    /// disk error (a browser would hit the same wall).
    pub fn should_fallback_to_webview(&self) -> bool {
        !matches!(self, Self::InvalidUrl(_) | Self::TooLarge(_) | Self::Io(_))
    }
}

/// Compute a 64-character lowercase SHA-256 hex string for `url`.
pub fn hash_url(url: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(url.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Extract canonical file extension from a Content-Type header.
pub fn extension_from_content_type(content_type: &str) -> Option<&'static str> {
    let mime = content_type.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
    match mime.as_str() {
        "image/jpeg" | "image/jpg" => Some("jpg"),
        "image/png" => Some("png"),
        "image/webp" => Some("webp"),
        "image/gif" => Some("gif"),
        "image/avif" => Some("avif"),
        "image/bmp" => Some("bmp"),
        _ => None,
    }
}

/// Validate that `content_type` represents an image MIME type (`image/*`).
pub fn is_valid_image_content_type(content_type: &str) -> bool {
    let mime = content_type.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
    mime.starts_with("image/")
}

/// Inspect the URL's path segment for a recognized image extension.
pub fn extension_from_url_path(url_str: &str) -> Option<&'static str> {
    if let Ok(parsed) = Url::parse(url_str) {
        if let Some(segment) = parsed.path_segments().and_then(|mut s| s.next_back()) {
            if let Some(ext) = Path::new(segment).extension().and_then(|e| e.to_str()) {
                match ext.to_ascii_lowercase().as_str() {
                    "jpg" | "jpeg" => return Some("jpg"),
                    "png" => return Some("png"),
                    "webp" => return Some("webp"),
                    "gif" => return Some("gif"),
                    "avif" => return Some("avif"),
                    "bmp" => return Some("bmp"),
                    _ => {}
                }
            }
        }
    }
    None
}

/// Extract the ASCII origin (scheme + host + optional port) from a URL to use as `Referer`.
pub fn origin_from_url(url_str: &str) -> Result<String> {
    let parsed = Url::parse(url_str).map_err(|e| anyhow!("invalid URL '{url_str}': {e}"))?;
    let origin = parsed.origin().ascii_serialization();
    if origin == "null" {
        return Err(anyhow!("URL '{url_str}' has opaque origin"));
    }
    Ok(origin)
}

/// Validate that response size does not exceed the maximum allowed bytes.
pub fn validate_response_size(size: usize, max: usize) -> Result<()> {
    if size > max {
        Err(anyhow!("response size {size} exceeds cap of {max} bytes"))
    } else {
        Ok(())
    }
}

/// Fetch an image from `image_url` and persist it into `covers_dir/<hash>.<ext>`.
///
/// Sends a browser User-Agent and a Referer header pointing to the image's site origin.
/// If a non-empty cached file already exists for this URL hash, it returns immediately
/// without making an HTTP request.
pub async fn fetch_and_cache_cover_to_dir(
    covers_dir: &Path,
    image_url: &str,
) -> Result<PathBuf, DownloadError> {
    if !crate::scraper_engine::is_safe_external_url(image_url) {
        return Err(DownloadError::InvalidUrl(format!("unsafe or private URL: {image_url}")));
    }

    let origin = origin_from_url(image_url)
        .map_err(|e| DownloadError::InvalidUrl(e.to_string()))?;

    let hash = hash_url(image_url);

    // Fast-path: if an existing cached file exists for this hash, reuse it.
    for ext in &["jpg", "jpeg", "png", "webp", "gif", "avif", "bmp"] {
        let candidate = covers_dir.join(format!("{hash}.{ext}"));
        if candidate.is_file() {
            if let Ok(meta) = candidate.metadata() {
                if meta.len() > 0 {
                    return Ok(candidate);
                }
            }
        }
    }

    // Direct HTTP GET request
    let resp = HTTP_CLIENT
        .get(image_url)
        .header(reqwest::header::USER_AGENT, BROWSER_USER_AGENT)
        .header(reqwest::header::REFERER, &origin)
        .header(
            reqwest::header::ACCEPT,
            "image/avif,image/webp,image/apng,image/svg+xml,image/*,*/*;q=0.8",
        )
        .send()
        .await
        .map_err(|e| {
            if let Some(status) = e.status() {
                let code = status.as_u16();
                if code == 403 || code == 503 {
                    return DownloadError::CloudflareBlocked(code);
                }
            }
            DownloadError::Network(e)
        })?;

    let status = resp.status();
    let status_code = status.as_u16();
    if status_code == 403 || status_code == 503 {
        return Err(DownloadError::CloudflareBlocked(status_code));
    }
    if !status.is_success() {
        return Err(DownloadError::HttpStatus(status_code));
    }

    // Validate Content-Type
    let content_type = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();

    if !is_valid_image_content_type(&content_type) {
        return Err(DownloadError::InvalidContentType(content_type));
    }

    // Determine target file extension
    let ext = extension_from_content_type(&content_type)
        .or_else(|| extension_from_url_path(image_url))
        .unwrap_or("jpg");

    // Early Content-Length check if available
    if let Some(cl) = resp.content_length() {
        validate_response_size(cl as usize, MAX_COVER_BYTES)
            .map_err(|_| DownloadError::TooLarge(cl as usize))?;
    }

    // Download payload bytes
    let bytes = resp.bytes().await.map_err(DownloadError::Network)?;

    validate_response_size(bytes.len(), MAX_COVER_BYTES)
        .map_err(|_| DownloadError::TooLarge(bytes.len()))?;

    if bytes.is_empty() {
        return Err(DownloadError::Other("downloaded cover is 0 bytes".to_string()));
    }

    // Ensure directory exists and write file
    std::fs::create_dir_all(covers_dir).map_err(DownloadError::Io)?;
    let target_path = covers_dir.join(format!("{hash}.{ext}"));
    // Write beside the target and rename into place: two series can share a
    // cover URL and be fetched concurrently, and the fast path above treats
    // any non-empty file as complete, so a half-written one must never be
    // visible under its final name.
    // Unique per call (pid + counter): concurrent downloads of the same URL
    // inside this process must not share a temp file either.
    static TMP_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = TMP_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp_path = covers_dir.join(format!("{hash}.{ext}.{}.{seq}.tmp", std::process::id()));
    std::fs::write(&tmp_path, &bytes).map_err(DownloadError::Io)?;
    if let Err(e) = std::fs::rename(&tmp_path, &target_path) {
        let _ = std::fs::remove_file(&tmp_path);
        // Lost a race with a concurrent identical download: the file is there.
        if !target_path.is_file() {
            return Err(DownloadError::Io(e));
        }
    }

    Ok(target_path)
}

/// Return the `<app_data_dir>/covers` path for the current application.
pub fn covers_dir(app: &AppHandle) -> Result<PathBuf> {
    let app_data = app.path().app_data_dir().context("resolve app data dir")?;
    Ok(app_data.join("covers"))
}

/// Download and cache `image_url` into `<app_data_dir>/covers/<hash>.<ext>`.
pub async fn cache_cover_image(app: &AppHandle, image_url: &str) -> Result<PathBuf, DownloadError> {
    if !crate::scraper_engine::is_safe_external_url(image_url) {
        return Err(DownloadError::InvalidUrl(format!("unsafe or private URL: {image_url}")));
    }
    let dir = covers_dir(app).map_err(|e| DownloadError::Other(e.to_string()))?;
    fetch_and_cache_cover_to_dir(&dir, image_url).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_fallback_to_webview_covers_browser_recoverable_failures_only() {
        assert!(DownloadError::CloudflareBlocked(403).should_fallback_to_webview());
        assert!(DownloadError::HttpStatus(404).should_fallback_to_webview());
        assert!(DownloadError::HttpStatus(429).should_fallback_to_webview());
        assert!(DownloadError::InvalidContentType("text/html".into()).should_fallback_to_webview());
        assert!(DownloadError::Other("downloaded cover is 0 bytes".into()).should_fallback_to_webview());
        assert!(!DownloadError::InvalidUrl("http://127.0.0.1/x".into()).should_fallback_to_webview());
        assert!(!DownloadError::TooLarge(10_000_000).should_fallback_to_webview());
        assert!(!DownloadError::Io(std::io::Error::other("disk")).should_fallback_to_webview());
    }

    #[test]
    fn hash_url_produces_consistent_sha256() {
        let h1 = hash_url("https://example.com/cover.jpg");
        let h2 = hash_url("https://example.com/cover.jpg");
        let h3 = hash_url("https://example.com/other.jpg");
        assert_eq!(h1, h2);
        assert_ne!(h1, h3);
        assert_eq!(h1.len(), 64);
        assert!(h1.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn extension_from_content_type_recognizes_standard_image_types() {
        assert_eq!(extension_from_content_type("image/jpeg"), Some("jpg"));
        assert_eq!(extension_from_content_type("image/jpg"), Some("jpg"));
        assert_eq!(extension_from_content_type("image/png"), Some("png"));
        assert_eq!(extension_from_content_type("image/webp"), Some("webp"));
        assert_eq!(extension_from_content_type("image/gif"), Some("gif"));
        assert_eq!(extension_from_content_type("image/avif"), Some("avif"));
        assert_eq!(extension_from_content_type("image/bmp"), Some("bmp"));
        assert_eq!(extension_from_content_type("image/jpeg; charset=utf-8"), Some("jpg"));
        assert_eq!(extension_from_content_type("IMAGE/PNG"), Some("png"));

        // Non-images return None
        assert_eq!(extension_from_content_type("text/html"), None);
        assert_eq!(extension_from_content_type("application/json"), None);
        assert_eq!(extension_from_content_type(""), None);
    }

    #[test]
    fn is_valid_image_content_type_validates_image_mimes() {
        assert!(is_valid_image_content_type("image/jpeg"));
        assert!(is_valid_image_content_type("image/png; charset=utf-8"));
        assert!(is_valid_image_content_type("IMAGE/WEBP"));
        assert!(is_valid_image_content_type("image/custom-format"));

        assert!(!is_valid_image_content_type("text/html"));
        assert!(!is_valid_image_content_type("application/octet-stream"));
        assert!(!is_valid_image_content_type(""));
    }

    #[test]
    fn origin_from_url_extracts_scheme_and_host() {
        assert_eq!(
            origin_from_url("https://i0.wp.com/animeflv.net/uploads/covers/1.jpg").unwrap(),
            "https://i0.wp.com"
        );
        assert_eq!(
            origin_from_url("http://cdn.jkdesa.com:8080/path/to/img.png").unwrap(),
            "http://cdn.jkdesa.com:8080"
        );
        assert!(origin_from_url("not a url").is_err());
    }

    #[test]
    fn validate_response_size_enforces_cap() {
        assert!(validate_response_size(1024, MAX_COVER_BYTES).is_ok());
        assert!(validate_response_size(MAX_COVER_BYTES, MAX_COVER_BYTES).is_ok());
        assert!(validate_response_size(MAX_COVER_BYTES + 1, MAX_COVER_BYTES).is_err());
    }

    #[test]
    fn extension_from_url_path_extracts_extension() {
        assert_eq!(
            extension_from_url_path("https://example.com/images/show.webp"),
            Some("webp")
        );
        assert_eq!(
            extension_from_url_path("https://example.com/images/show.PNG?resize=200"),
            Some("png")
        );
        assert_eq!(extension_from_url_path("https://example.com/no-ext"), None);
    }

    #[test]
    fn is_safe_external_url_rejects_localhost_and_private_ips() {
        assert!(!crate::scraper_engine::is_safe_external_url("http://localhost/cover.jpg"));
        assert!(!crate::scraper_engine::is_safe_external_url("http://localhost:8080/test.png"));
        assert!(!crate::scraper_engine::is_safe_external_url("https://localhost/test.png"));
        assert!(!crate::scraper_engine::is_safe_external_url("http://127.0.0.1/test.png"));
        assert!(!crate::scraper_engine::is_safe_external_url("http://192.168.1.1/cover.jpg"));
        assert!(!crate::scraper_engine::is_safe_external_url("http://192.168.0.100/test.png"));
        assert!(!crate::scraper_engine::is_safe_external_url("http://10.0.0.1/cover.jpg"));
        assert!(!crate::scraper_engine::is_safe_external_url("http://172.16.0.1/cover.jpg"));
        assert!(!crate::scraper_engine::is_safe_external_url("file:///etc/passwd"));
        assert!(!crate::scraper_engine::is_safe_external_url("ftp://example.com/cover.jpg"));

        // Public URLs are accepted
        assert!(crate::scraper_engine::is_safe_external_url("https://example.com/cover.jpg"));
        assert!(crate::scraper_engine::is_safe_external_url("https://i0.wp.com/animeflv.net/uploads/covers/1.jpg"));
        assert!(crate::scraper_engine::is_safe_external_url("http://cdn.jkdesa.com/cover.png"));
    }

    #[tokio::test]
    async fn fetch_and_cache_cover_to_dir_rejects_unsafe_urls_before_io() {
        let dummy = Path::new("dummy");
        let res = fetch_and_cache_cover_to_dir(dummy, "http://127.0.0.1/cover.jpg").await;
        assert!(matches!(res, Err(DownloadError::InvalidUrl(_))));

        let res2 = fetch_and_cache_cover_to_dir(dummy, "http://192.168.1.1/cover.jpg").await;
        assert!(matches!(res2, Err(DownloadError::InvalidUrl(_))));

        let res3 = fetch_and_cache_cover_to_dir(dummy, "http://localhost/cover.jpg").await;
        assert!(matches!(res3, Err(DownloadError::InvalidUrl(_))));
    }
}
