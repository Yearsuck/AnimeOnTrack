//! Streams one resolved media file to disk.
//!
//! Kept free of Tauri and the database so it can be tested against a local
//! socket: the command layer (`commands/download.rs`) decides *what* to fetch
//! and records the outcome, this module only moves bytes safely. A download is
//! written to `<dest>.part` and renamed on success, so an interrupted or
//! cancelled transfer never leaves something that looks like a finished file.

use crate::download_sources::Media;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::LazyLock;
use std::time::Duration;
use tokio::io::AsyncWriteExt;

/// Refuse anything larger than this: an episode is a few hundred MB, so a
/// multi-gigabyte answer is not a video the user asked for.
pub const MAX_DOWNLOAD_BYTES: u64 = 6 * 1024 * 1024 * 1024;
/// A transfer that delivers nothing for this long is abandoned.
const STALL_TIMEOUT: Duration = Duration::from_secs(30);
const USER_AGENT: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/133.0.0.0 Safari/537.36";

#[derive(Debug, PartialEq, Eq)]
pub enum DlError {
    Cancelled,
    Http(u16),
    BadContentType(String),
    TooLarge,
    Stalled,
    Incomplete { got: u64, expected: u64 },
    Net(String),
    Io(String),
    /// The finished file failed the safety checks (not a video, or flagged).
    Rejected(String),
}

impl std::fmt::Display for DlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DlError::Cancelled => write!(f, "cancelled"),
            DlError::Http(code) => write!(f, "the server answered HTTP {code}"),
            DlError::BadContentType(t) => write!(f, "the server did not send a video (content-type {t})"),
            DlError::TooLarge => write!(f, "the file is larger than the allowed size"),
            DlError::Stalled => write!(f, "the transfer stalled"),
            DlError::Incomplete { got, expected } => {
                write!(f, "the transfer ended early ({got} of {expected} bytes)")
            }
            DlError::Net(e) => write!(f, "network error: {e}"),
            DlError::Io(e) => write!(f, "could not write the file: {e}"),
            DlError::Rejected(why) => write!(f, "the download was discarded: {why}"),
        }
    }
}

/// Shared client: no overall timeout (episodes are big), a connect timeout,
/// and every redirect hop re-checked so a public host cannot bounce us to
/// localhost or the LAN.
static CLIENT: LazyLock<reqwest::Client> = LazyLock::new(|| {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(20))
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
        .expect("failed to build reqwest client for downloads")
});

pub fn client() -> &'static reqwest::Client {
    &CLIENT
}

pub fn browser_user_agent() -> &'static str {
    USER_AGENT
}

/// `<downloads>/<series_id>/<episode_id>`, without an extension: the extension
/// is decided by what the downloaded bytes turn out to be (see `download_safety`).
pub fn episode_file_base(downloads_dir: &Path, series_id: i64, episode_id: i64) -> PathBuf {
    downloads_dir.join(series_id.to_string()).join(episode_id.to_string())
}

fn part_path(base: &Path) -> PathBuf {
    let mut name = base.file_name().map(|n| n.to_os_string()).unwrap_or_default();
    name.push(".part");
    base.with_file_name(name)
}

fn final_path(base: &Path, ext: &str) -> PathBuf {
    let mut name = base.file_name().map(|n| n.to_os_string()).unwrap_or_default();
    name.push(".");
    name.push(ext);
    base.with_file_name(name)
}

fn is_acceptable_content_type(ct: &str) -> bool {
    let ct = ct.trim().to_ascii_lowercase();
    ct.starts_with("video/") || ct.starts_with("application/octet-stream") || ct.starts_with("binary/")
}

/// Download `media` to `<base>.<ext>`, reporting `(bytes_so_far, total_if_known)`
/// as it goes, and return the final path. The body is written to `<base>.part`,
/// and only a complete file that passes `download_safety::inspect` (real video
/// container, no Defender detection) is renamed into place; the extension comes
/// from the content, never from the URL. Anything else is deleted.
pub async fn fetch_to_file(
    media: &Media,
    base: &Path,
    cancel: &AtomicBool,
    mut on_progress: impl FnMut(u64, Option<u64>),
) -> Result<PathBuf, DlError> {
    if let Some(parent) = base.parent() {
        tokio::fs::create_dir_all(parent).await.map_err(|e| DlError::Io(e.to_string()))?;
    }
    let part = part_path(base);

    let mut resp = CLIENT
        .get(&media.url)
        .header(reqwest::header::USER_AGENT, USER_AGENT)
        .header(reqwest::header::REFERER, &media.referer)
        .header(reqwest::header::ACCEPT, "video/*,*/*;q=0.8")
        .send()
        .await
        .map_err(|e| DlError::Net(e.to_string()))?;

    if !resp.status().is_success() {
        return Err(DlError::Http(resp.status().as_u16()));
    }
    let content_type = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    if !is_acceptable_content_type(&content_type) {
        return Err(DlError::BadContentType(content_type));
    }
    let expected = resp.content_length();
    if expected.is_some_and(|n| n > MAX_DOWNLOAD_BYTES) {
        return Err(DlError::TooLarge);
    }

    let result = write_body(&mut resp, &part, expected, cancel, &mut on_progress).await;
    match result {
        Ok(written) => {
            if let Some(exp) = expected {
                if written != exp {
                    let _ = tokio::fs::remove_file(&part).await;
                    return Err(DlError::Incomplete { got: written, expected: exp });
                }
            }
            if written == 0 {
                let _ = tokio::fs::remove_file(&part).await;
                return Err(DlError::Incomplete { got: 0, expected: expected.unwrap_or(0) });
            }
            // Blocking work (reads the head, runs Defender): off the async workers.
            let to_inspect = part.clone();
            let verdict = tokio::task::spawn_blocking(move || crate::download_safety::inspect(&to_inspect))
                .await
                .map_err(|e| DlError::Io(e.to_string()))?;
            let container = match verdict {
                crate::download_safety::Inspection::Accept(c) => c,
                crate::download_safety::Inspection::Reject(why) => {
                    let _ = tokio::fs::remove_file(&part).await;
                    return Err(DlError::Rejected(why));
                }
            };
            let dest = final_path(base, container.extension());
            tokio::fs::rename(&part, &dest).await.map_err(|e| DlError::Io(e.to_string()))?;
            crate::download_safety::mark_of_the_web(&dest, &media.url);
            Ok(dest)
        }
        Err(e) => {
            let _ = tokio::fs::remove_file(&part).await;
            Err(e)
        }
    }
}

async fn write_body(
    resp: &mut reqwest::Response,
    part: &Path,
    expected: Option<u64>,
    cancel: &AtomicBool,
    on_progress: &mut impl FnMut(u64, Option<u64>),
) -> Result<u64, DlError> {
    let mut file = tokio::fs::File::create(part).await.map_err(|e| DlError::Io(e.to_string()))?;
    let mut written = 0u64;
    loop {
        if cancel.load(Ordering::SeqCst) {
            return Err(DlError::Cancelled);
        }
        let chunk = match tokio::time::timeout(STALL_TIMEOUT, resp.chunk()).await {
            Err(_) => return Err(DlError::Stalled),
            Ok(Err(e)) => return Err(DlError::Net(e.to_string())),
            Ok(Ok(c)) => c,
        };
        let Some(chunk) = chunk else { break };
        written += chunk.len() as u64;
        if written > MAX_DOWNLOAD_BYTES {
            return Err(DlError::TooLarge);
        }
        file.write_all(&chunk).await.map_err(|e| DlError::Io(e.to_string()))?;
        on_progress(written, expected);
    }
    file.flush().await.map_err(|e| DlError::Io(e.to_string()))?;
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    /// One-shot HTTP server on a random loopback port that answers a single
    /// request with the given raw response bytes, then closes.
    async fn serve_once(response: Vec<u8>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            if let Ok((mut sock, _)) = listener.accept().await {
                let mut buf = [0u8; 2048];
                let _ = sock.read(&mut buf).await;
                let _ = sock.write_all(&response).await;
                let _ = sock.shutdown().await;
            }
        });
        format!("http://{addr}/video.mp4")
    }

    fn response(status: &str, content_type: &str, declared_len: usize, body: &[u8]) -> Vec<u8> {
        let mut out = format!(
            "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {declared_len}\r\nConnection: close\r\n\r\n"
        )
        .into_bytes();
        out.extend_from_slice(body);
        out
    }

    fn media(url: String) -> Media {
        Media { url, referer: "https://www.yourupload.com/".into(), host: "yourupload" }
    }

    /// A body that passes the content check: an MP4 box header plus filler.
    fn mp4_body(len: usize) -> Vec<u8> {
        let mut v = vec![0, 0, 0, 0x18];
        v.extend_from_slice(b"ftypisom");
        v.resize(len.max(12), 7);
        v
    }

    fn temp_dest(name: &str) -> PathBuf {
        static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "aot_dl_{name}_{}_{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        dir.join("1").join("2")
    }

    fn cleanup(base: &Path) {
        if let Some(root) = base.parent().and_then(|p| p.parent()) {
            let _ = std::fs::remove_dir_all(root);
        }
    }

    fn no_leftovers(base: &Path) -> bool {
        !part_path(base).exists() && !final_path(base, "mp4").exists() && !final_path(base, "ts").exists()
    }

    #[test]
    fn episode_path_is_series_dir_then_episode_file() {
        let p = episode_file_base(Path::new("/d"), 7, 42);
        assert!(p.ends_with(Path::new("7").join("42")));
        assert_eq!(part_path(&p).file_name().unwrap().to_str().unwrap(), "42.part");
        assert_eq!(final_path(&p, "ts").file_name().unwrap().to_str().unwrap(), "42.ts");
    }

    #[test]
    fn content_type_gate() {
        assert!(is_acceptable_content_type("video/mp4"));
        assert!(is_acceptable_content_type("Video/MP4; charset=x"));
        assert!(is_acceptable_content_type("application/octet-stream"));
        assert!(!is_acceptable_content_type("text/html; charset=utf-8"));
        assert!(!is_acceptable_content_type(""));
    }

    #[tokio::test]
    async fn downloads_a_whole_body_and_renames_it() {
        let body = mp4_body(100_000);
        let url = serve_once(response("200 OK", "video/mp4", body.len(), &body)).await;
        let base = temp_dest("ok");
        let cancel = AtomicBool::new(false);
        let mut last = (0u64, None);
        let path = fetch_to_file(&media(url), &base, &cancel, |d, t| last = (d, t)).await.unwrap();
        assert_eq!(path, final_path(&base, "mp4"));
        assert_eq!(last, (100_000, Some(100_000)));
        assert_eq!(std::fs::read(&path).unwrap(), body);
        assert!(!part_path(&base).exists());
        cleanup(&base);
    }

    #[tokio::test]
    async fn rejects_an_html_answer_without_creating_the_file() {
        let url = serve_once(response("200 OK", "text/html", 5, b"<html")).await;
        let dest = temp_dest("html");
        let err = fetch_to_file(&media(url), &dest, &AtomicBool::new(false), |_, _| {}).await.unwrap_err();
        assert!(matches!(err, DlError::BadContentType(_)));
        assert!(no_leftovers(&dest));
        cleanup(&dest);
    }

    #[tokio::test]
    async fn rejects_an_http_error_status() {
        let url = serve_once(response("404 Not Found", "text/plain", 0, b"")).await;
        let dest = temp_dest("404");
        let err = fetch_to_file(&media(url), &dest, &AtomicBool::new(false), |_, _| {}).await.unwrap_err();
        assert_eq!(err, DlError::Http(404));
        cleanup(&dest);
    }

    #[tokio::test]
    async fn a_truncated_body_leaves_no_file() {
        // Declares 1000 bytes, sends 10 and closes.
        let url = serve_once(response("200 OK", "video/mp4", 1000, &[1u8; 10])).await;
        let dest = temp_dest("short");
        let err = fetch_to_file(&media(url), &dest, &AtomicBool::new(false), |_, _| {}).await.unwrap_err();
        assert!(matches!(err, DlError::Incomplete { .. } | DlError::Net(_)), "got {err:?}");
        assert!(no_leftovers(&dest));
        cleanup(&dest);
    }

    #[tokio::test]
    async fn a_cancel_flag_stops_the_transfer_and_cleans_up() {
        let body = mp4_body(50_000);
        let url = serve_once(response("200 OK", "video/mp4", body.len(), &body)).await;
        let dest = temp_dest("cancel");
        let cancel = AtomicBool::new(true);
        let err = fetch_to_file(&media(url), &dest, &cancel, |_, _| {}).await.unwrap_err();
        assert_eq!(err, DlError::Cancelled);
        assert!(no_leftovers(&dest));
        cleanup(&dest);
    }

    #[tokio::test]
    async fn an_executable_served_as_video_is_discarded() {
        let mut body = b"MZ\x90\x00\x03\x00\x00\x00".to_vec();
        body.resize(20_000, 0);
        let url = serve_once(response("200 OK", "video/mp4", body.len(), &body)).await;
        let base = temp_dest("exe");
        let err = fetch_to_file(&media(url), &base, &AtomicBool::new(false), |_, _| {}).await.unwrap_err();
        assert!(matches!(&err, DlError::Rejected(why) if why.contains("not a video")), "got {err:?}");
        assert!(no_leftovers(&base));
        cleanup(&base);
    }

    #[tokio::test]
    async fn the_extension_comes_from_the_content_not_the_url() {
        // MPEG-TS served from a URL that says .mp4 ends up as .ts.
        let mut body = vec![0u8; 188 * 4];
        for i in 0..4 {
            body[i * 188] = 0x47;
        }
        let url = serve_once(response("200 OK", "application/octet-stream", body.len(), &body)).await;
        let base = temp_dest("ts");
        let path = fetch_to_file(&media(url), &base, &AtomicBool::new(false), |_, _| {}).await.unwrap();
        assert_eq!(path, final_path(&base, "ts"));
        assert!(path.is_file());
        cleanup(&base);
    }
}
