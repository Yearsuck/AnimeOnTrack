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
            finalize(&part, base, &media.url).await
        }
        Err(e) => {
            let _ = tokio::fs::remove_file(&part).await;
            Err(e)
        }
    }
}

/// Only public web addresses may be fetched. Unit tests talk to a loopback
/// server, so they get that one exception (compiled out of real builds).
fn url_allowed(url: &str) -> bool {
    #[cfg(test)]
    if url.starts_with("http://127.0.0.1:") {
        return true;
    }
    crate::scraper_engine::is_safe_external_url(url)
}

/// Plain GET of a (small) text resource such as a playlist.
async fn get_text(url: &str, referer: &str) -> Result<String, DlError> {
    const MAX_PLAYLIST_BYTES: usize = 4 * 1024 * 1024;
    if !url_allowed(url) {
        return Err(DlError::Net("the playlist is not a public web address".into()));
    }
    let resp = CLIENT
        .get(url)
        .header(reqwest::header::USER_AGENT, USER_AGENT)
        .header(reqwest::header::REFERER, referer)
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .map_err(|e| DlError::Net(e.to_string()))?;
    if !resp.status().is_success() {
        return Err(DlError::Http(resp.status().as_u16()));
    }
    let bytes = resp.bytes().await.map_err(|e| DlError::Net(e.to_string()))?;
    if bytes.len() > MAX_PLAYLIST_BYTES {
        return Err(DlError::TooLarge);
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// Some hosts disguise HLS segments as images (a tiny PNG/JPEG/GIF, served from
/// an ad CDN, with the real MPEG-TS packets appended) to slip past filters.
/// When a segment starts with an image signature, drop everything before the
/// first run of three MPEG-TS sync bytes 188 bytes apart; anything else is
/// returned untouched (and, if it is not video, rejected by the later content check).
fn unwrap_image_segment(data: &[u8]) -> &[u8] {
    let is_image = data.starts_with(&[0x89, b'P', b'N', b'G'])
        || data.starts_with(&[0xFF, 0xD8, 0xFF])
        || data.starts_with(b"GIF8");
    if !is_image {
        return data;
    }
    let last_start = data.len().saturating_sub(188 * 2 + 1);
    for i in 0..last_start {
        if data[i] == 0x47 && data[i + 188] == 0x47 && data[i + 376] == 0x47 {
            return &data[i..];
        }
    }
    data
}

/// Append one HLS segment to `file`; on any failure the file is cut back to
/// `start_len` so a retry never leaves half a segment in the middle.
async fn append_segment(
    url: &str,
    referer: &str,
    file: &mut tokio::fs::File,
    start_len: u64,
    total: &mut u64,
    cancel: &AtomicBool,
) -> Result<(), DlError> {
    use tokio::io::AsyncSeekExt;
    let result: Result<(), DlError> = async {
        if !url_allowed(url) {
            return Err(DlError::Net("a segment is not a public web address".into()));
        }
        let mut resp = CLIENT
            .get(url)
            .header(reqwest::header::USER_AGENT, USER_AGENT)
            .header(reqwest::header::REFERER, referer)
            .send()
            .await
            .map_err(|e| DlError::Net(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(DlError::Http(resp.status().as_u16()));
        }
        let ct = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_ascii_lowercase();
        // Segments are served as video/mp2t, octet-stream or even image/* to
        // dodge filters, so only a text answer (an error page) is refused here;
        // the finished file is sniffed anyway.
        if ct.starts_with("text/") {
            return Err(DlError::BadContentType(ct));
        }
        // Buffer the whole segment (a few MB): an image-wrapped one has to be
        // unwrapped before it is written.
        let mut body: Vec<u8> = Vec::new();
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
            if *total + body.len() as u64 + chunk.len() as u64 > MAX_DOWNLOAD_BYTES {
                return Err(DlError::TooLarge);
            }
            body.extend_from_slice(&chunk);
        }
        let payload = unwrap_image_segment(&body);
        *total += payload.len() as u64;
        file.write_all(payload).await.map_err(|e| DlError::Io(e.to_string()))?;
        Ok(())
    }
    .await;
    if result.is_err() {
        let _ = file.flush().await;
        let _ = file.set_len(start_len).await;
        let _ = file.seek(std::io::SeekFrom::Start(start_len)).await;
        *total = start_len;
    }
    result
}

/// Download an HLS stream (`.m3u8`) by fetching its segments one after the
/// other into one file (no remux: MPEG-TS segments simply concatenate into a
/// playable `.ts`). A master playlist picks its best variant first. Encrypted,
/// live, fragmented-mp4 or oversized streams are refused with a clear reason.
/// Progress is `(segments_done, Some(total_segments))`.
pub async fn fetch_hls_to_file(
    playlist_url: &str,
    referer: &str,
    base: &Path,
    cancel: &AtomicBool,
    mut on_progress: impl FnMut(u64, Option<u64>),
) -> Result<PathBuf, DlError> {
    use crate::hls::{best_variant, check_downloadable, parse_playlist, Playlist};
    const SEGMENT_ATTEMPTS: u32 = 3;

    if let Some(parent) = base.parent() {
        tokio::fs::create_dir_all(parent).await.map_err(|e| DlError::Io(e.to_string()))?;
    }

    let mut media_url = playlist_url.to_string();
    let mut parsed = None;
    // A master playlist points at media playlists; one hop is all that is valid.
    for _ in 0..2 {
        let text = get_text(&media_url, referer).await?;
        match parse_playlist(&text, &media_url) {
            Some(Playlist::Master { variants }) => {
                let Some(best) = best_variant(&variants) else {
                    return Err(DlError::Net("the playlist has no streams".into()));
                };
                media_url = best.url.clone();
            }
            Some(media @ Playlist::Media { .. }) => {
                parsed = Some(media);
                break;
            }
            None => return Err(DlError::Net("the stream playlist is not valid".into())),
        }
    }
    let Some(Playlist::Media { segments, ended, encrypted, has_init_map }) = parsed else {
        return Err(DlError::Net("the stream playlist is nested too deeply".into()));
    };
    check_downloadable(&segments, ended, encrypted, has_init_map)
        .map_err(|crate::hls::HlsError::NotPlayable(why)| DlError::Net(format!("this stream cannot be downloaded: {why}")))?;

    let part = part_path(base);
    let mut file = tokio::fs::File::create(&part).await.map_err(|e| DlError::Io(e.to_string()))?;
    let total_segments = segments.len() as u64;
    let mut written = 0u64;
    on_progress(0, Some(total_segments));

    let outcome: Result<(), DlError> = async {
        for (idx, seg) in segments.iter().enumerate() {
            let start_len = written;
            let mut last_err = None;
            for attempt in 1..=SEGMENT_ATTEMPTS {
                match append_segment(&seg.url, &media_url, &mut file, start_len, &mut written, cancel).await {
                    Ok(()) => {
                        last_err = None;
                        break;
                    }
                    // A cancel or a size cap is final; a flaky segment gets retries.
                    Err(e @ (DlError::Cancelled | DlError::TooLarge | DlError::Io(_))) => return Err(e),
                    Err(e) => {
                        last_err = Some(e);
                        if attempt < SEGMENT_ATTEMPTS {
                            tokio::time::sleep(Duration::from_millis(500 * attempt as u64)).await;
                        }
                    }
                }
            }
            if let Some(e) = last_err {
                return Err(e);
            }
            on_progress(idx as u64 + 1, Some(total_segments));
        }
        file.flush().await.map_err(|e| DlError::Io(e.to_string()))?;
        Ok(())
    }
    .await;
    drop(file);

    match outcome {
        Ok(()) if written == 0 => {
            let _ = tokio::fs::remove_file(&part).await;
            Err(DlError::Incomplete { got: 0, expected: 1 })
        }
        Ok(()) => finalize(&part, base, playlist_url).await,
        Err(e) => {
            let _ = tokio::fs::remove_file(&part).await;
            Err(e)
        }
    }
}

/// Inspect the finished `.part` (real container + Defender), then move it to its
/// final name and mark it as downloaded from the Internet. A rejected file is deleted.
async fn finalize(part: &Path, base: &Path, source_url: &str) -> Result<PathBuf, DlError> {
    // Blocking work (reads the head, runs Defender): off the async workers.
    let to_inspect = part.to_path_buf();
    let verdict = tokio::task::spawn_blocking(move || crate::download_safety::inspect(&to_inspect))
        .await
        .map_err(|e| DlError::Io(e.to_string()))?;
    let container = match verdict {
        crate::download_safety::Inspection::Accept(c) => c,
        crate::download_safety::Inspection::Reject(why) => {
            let _ = tokio::fs::remove_file(part).await;
            return Err(DlError::Rejected(why));
        }
    };
    let dest = final_path(base, container.extension());
    tokio::fs::rename(part, &dest).await.map_err(|e| DlError::Io(e.to_string()))?;
    crate::download_safety::mark_of_the_web(&dest, source_url);
    Ok(dest)
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

    /// Loopback server answering any number of requests from a path -> (content-type, body) table.
    async fn serve_routes(routes: Vec<(&'static str, &'static str, Vec<u8>)>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else { return };
                let routes = routes.clone();
                tokio::spawn(async move {
                    let mut buf = [0u8; 4096];
                    let n = sock.read(&mut buf).await.unwrap_or(0);
                    let req = String::from_utf8_lossy(&buf[..n]).to_string();
                    let path = req.split_whitespace().nth(1).unwrap_or("/").to_string();
                    let resp = match routes.iter().find(|(p, _, _)| *p == path) {
                        Some((_, ct, body)) => response("200 OK", ct, body.len(), body),
                        None => response("404 Not Found", "text/plain", 0, b""),
                    };
                    let _ = sock.write_all(&resp).await;
                    let _ = sock.shutdown().await;
                });
            }
        });
        format!("http://{addr}")
    }

    fn ts_segment(fill: u8) -> Vec<u8> {
        let mut v = vec![fill; 188 * 3];
        for i in 0..3 {
            v[i * 188] = 0x47;
        }
        v
    }

    fn media_playlist(n: usize) -> Vec<u8> {
        let mut s = String::from("#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:6\n#EXT-X-PLAYLIST-TYPE:VOD\n");
        for i in 0..n {
            s.push_str(&format!("#EXTINF:6.0,\nseg{i}.ts\n"));
        }
        s.push_str("#EXT-X-ENDLIST\n");
        s.into_bytes()
    }

    #[tokio::test]
    async fn hls_segments_are_joined_into_one_ts_through_a_master_playlist() {
        let master = b"#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=900000,RESOLUTION=640x360\nlow.m3u8\n#EXT-X-STREAM-INF:BANDWIDTH=2000000,RESOLUTION=1280x720\nhi.m3u8\n".to_vec();
        let base_url = serve_routes(vec![
            ("/master.m3u8", "application/vnd.apple.mpegurl", master),
            ("/low.m3u8", "application/vnd.apple.mpegurl", media_playlist(1)),
            ("/hi.m3u8", "application/vnd.apple.mpegurl", media_playlist(3)),
            ("/seg0.ts", "video/mp2t", ts_segment(1)),
            ("/seg1.ts", "video/mp2t", ts_segment(2)),
            ("/seg2.ts", "application/octet-stream", ts_segment(3)),
        ])
        .await;
        let base = temp_dest("hls");
        let mut progress = Vec::new();
        let path = fetch_hls_to_file(&format!("{base_url}/master.m3u8"), "https://megaplay.su/", &base, &AtomicBool::new(false), |d, t| {
            progress.push((d, t))
        })
        .await
        .unwrap();
        assert_eq!(path, final_path(&base, "ts"));
        let data = std::fs::read(&path).unwrap();
        assert_eq!(data.len(), 188 * 3 * 3, "the 720p variant has three segments");
        assert_eq!(data[1], 1);
        assert_eq!(data[188 * 3 + 1], 2);
        assert_eq!(data[188 * 6 + 1], 3);
        assert_eq!(progress.first(), Some(&(0, Some(3))));
        assert_eq!(progress.last(), Some(&(3, Some(3))));
        cleanup(&base);
    }

    #[tokio::test]
    async fn an_encrypted_or_live_stream_is_refused_before_downloading() {
        let enc = b"#EXTM3U\n#EXT-X-KEY:METHOD=AES-128,URI=\"k.key\"\n#EXTINF:6.0,\nseg0.ts\n#EXT-X-ENDLIST\n".to_vec();
        let live = b"#EXTM3U\n#EXTINF:6.0,\nseg0.ts\n".to_vec();
        let base_url = serve_routes(vec![
            ("/enc.m3u8", "application/vnd.apple.mpegurl", enc),
            ("/live.m3u8", "application/vnd.apple.mpegurl", live),
            ("/seg0.ts", "video/mp2t", ts_segment(1)),
        ])
        .await;
        for (name, expect) in [("enc", "encrypted stream"), ("live", "live stream")] {
            let base = temp_dest(name);
            let err = fetch_hls_to_file(&format!("{base_url}/{name}.m3u8"), "https://x/", &base, &AtomicBool::new(false), |_, _| {})
                .await
                .unwrap_err();
            assert!(matches!(&err, DlError::Net(m) if m.contains(expect)), "got {err:?}");
            assert!(no_leftovers(&base));
            cleanup(&base);
        }
    }

    #[tokio::test]
    async fn a_missing_segment_fails_the_download_and_leaves_nothing() {
        let base_url = serve_routes(vec![
            ("/p.m3u8", "application/vnd.apple.mpegurl", media_playlist(2)),
            ("/seg0.ts", "video/mp2t", ts_segment(1)),
            // seg1.ts is a 404
        ])
        .await;
        let base = temp_dest("hlsmiss");
        let err = fetch_hls_to_file(&format!("{base_url}/p.m3u8"), "https://x/", &base, &AtomicBool::new(false), |_, _| {})
            .await
            .unwrap_err();
        assert_eq!(err, DlError::Http(404));
        assert!(no_leftovers(&base));
        cleanup(&base);
    }

    #[tokio::test]
    async fn hls_segments_that_are_not_video_are_discarded() {
        let mut exe = b"MZ\x90\x00".to_vec();
        exe.resize(188 * 3, 0);
        let base_url = serve_routes(vec![
            ("/p.m3u8", "application/vnd.apple.mpegurl", media_playlist(1)),
            ("/seg0.ts", "video/mp2t", exe),
        ])
        .await;
        let base = temp_dest("hlsexe");
        let err = fetch_hls_to_file(&format!("{base_url}/p.m3u8"), "https://x/", &base, &AtomicBool::new(false), |_, _| {})
            .await
            .unwrap_err();
        assert!(matches!(&err, DlError::Rejected(_)), "got {err:?}");
        assert!(no_leftovers(&base));
        cleanup(&base);
    }

    #[tokio::test]
    async fn an_hls_cancel_stops_between_chunks_and_cleans_up() {
        let base_url = serve_routes(vec![
            ("/p.m3u8", "application/vnd.apple.mpegurl", media_playlist(2)),
            ("/seg0.ts", "video/mp2t", ts_segment(1)),
            ("/seg1.ts", "video/mp2t", ts_segment(2)),
        ])
        .await;
        let base = temp_dest("hlscancel");
        let err = fetch_hls_to_file(&format!("{base_url}/p.m3u8"), "https://x/", &base, &AtomicBool::new(true), |_, _| {})
            .await
            .unwrap_err();
        assert_eq!(err, DlError::Cancelled);
        assert!(no_leftovers(&base));
        cleanup(&base);
    }

    #[test]
    fn image_wrapped_segments_are_unwrapped_to_the_transport_stream() {
        let mut wrapped = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        wrapped.extend_from_slice(&[0u8; 50]);
        let ts = ts_segment(5);
        wrapped.extend_from_slice(&ts);
        assert_eq!(unwrap_image_segment(&wrapped), &ts[..]);
        // JPEG and GIF headers too.
        let mut jpeg = vec![0xFF, 0xD8, 0xFF, 0xE0];
        jpeg.extend_from_slice(&ts);
        assert_eq!(unwrap_image_segment(&jpeg), &ts[..]);
        let mut gif = b"GIF89a".to_vec();
        gif.extend_from_slice(&ts);
        assert_eq!(unwrap_image_segment(&gif), &ts[..]);
    }

    #[test]
    fn plain_and_unrecognised_segments_are_left_alone() {
        let ts = ts_segment(1);
        assert_eq!(unwrap_image_segment(&ts), &ts[..]);
        // An image with no transport stream inside stays as it is (and will be rejected later).
        let png_only = vec![0x89, b'P', b'N', b'G', 1, 2, 3, 4];
        assert_eq!(unwrap_image_segment(&png_only), &png_only[..]);
        assert_eq!(unwrap_image_segment(&[]), &[] as &[u8]);
    }

    #[tokio::test]
    async fn an_hls_stream_of_image_wrapped_segments_still_downloads() {
        let wrap = |fill: u8| {
            let mut v = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
            v.extend_from_slice(&[0u8; 30]);
            v.extend_from_slice(&ts_segment(fill));
            v
        };
        let base_url = serve_routes(vec![
            ("/p.m3u8", "application/vnd.apple.mpegurl", media_playlist(2)),
            ("/seg0.ts", "image/png", wrap(1)),
            ("/seg1.ts", "image/png", wrap(2)),
        ])
        .await;
        let base = temp_dest("hlsimg");
        let path = fetch_hls_to_file(&format!("{base_url}/p.m3u8"), "https://x/", &base, &AtomicBool::new(false), |_, _| {})
            .await
            .unwrap();
        assert_eq!(path, final_path(&base, "ts"));
        let data = std::fs::read(&path).unwrap();
        assert_eq!(data.len(), 188 * 3 * 2);
        assert_eq!(data[0], 0x47);
        assert_eq!(data[1], 1);
        assert_eq!(data[188 * 3 + 1], 2);
        cleanup(&base);
    }
}
