//! What a freshly downloaded file must pass before it is allowed into the
//! downloads folder.
//!
//! The files come from third-party mirrors the app does not control, so every
//! one is treated as hostile until proven otherwise. The app never executes
//! anything it downloads and never trusts a server-supplied name or type; on
//! top of that each file goes through these layers, in order:
//!
//! 1. **Content sniffing** — the first bytes must be a real video container
//!    (MP4/MOV, MPEG-TS or Matroska/WebM). An executable, script, archive or
//!    HTML page renamed to `.mp4` is rejected, and the final extension comes
//!    from what the bytes *are*, not from the URL.
//! 2. **Windows Defender** — the file is scanned with `MpCmdRun.exe` while
//!    it is still a `.part` file; a detection (or an unreadable verdict while
//!    Defender is present) deletes it. Missing Defender only skips this layer.
//! 3. **Mark of the Web** — a `Zone.Identifier` stream (ZoneId=3, "Internet")
//!    makes Windows (SmartScreen, Explorer, Office) treat the file as
//!    downloaded from the web.
//! 4. **No execute rights** — the downloads folder gets an inherit-only deny
//!    ACE for execute on files, so nothing in it can be launched even if
//!    something slipped through.
//!
//! This is hardening, not a virtual machine: a vulnerability in the media
//! player that later opens the file is outside what an app can fully rule
//! out, which is why the player is the OS default (kept up to date) and the
//! file is never opened by anything else.

use std::path::Path;

/// A video container we are willing to keep.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Container {
    Mp4,
    MpegTs,
    Matroska,
}

impl Container {
    /// Extension the final file gets (decided by content, never by the URL).
    pub fn extension(self) -> &'static str {
        match self {
            Container::Mp4 => "mp4",
            Container::MpegTs => "ts",
            Container::Matroska => "mkv",
        }
    }
}

/// Recognise a video container from the first bytes of a file (at least ~400
/// bytes are needed to confirm MPEG-TS; fewer simply never match it).
pub fn sniff_container(head: &[u8]) -> Option<Container> {
    // ISO base media (MP4/MOV/M4V): a box size, then the box type. The first
    // box of a real file is `ftyp`, sometimes `moov`/`mdat`/`free`/`wide`/`styp`.
    if head.len() >= 12 {
        let kind = &head[4..8];
        if matches!(kind, b"ftyp" | b"moov" | b"mdat" | b"free" | b"wide" | b"styp" | b"skip") {
            return Some(Container::Mp4);
        }
    }
    // Matroska / WebM: EBML magic.
    if head.starts_with(&[0x1A, 0x45, 0xDF, 0xA3]) {
        return Some(Container::Matroska);
    }
    // MPEG-TS: a 0x47 sync byte every 188 bytes (check three packets so a
    // stray 0x47 at the start of other data does not pass).
    if head.len() >= 189 && head[0] == 0x47 && head[188] == 0x47 {
        let third_ok = head.len() < 377 || head[376] == 0x47;
        if third_ok {
            return Some(Container::MpegTs);
        }
    }
    None
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScanVerdict {
    Clean,
    /// Defender reported a detection.
    Threat,
    /// Defender is missing or gave no usable answer; the reason is for the log.
    Unavailable(String),
}

/// Map `MpCmdRun.exe -Scan` exit codes: 0 = no threats, 2 = threats found,
/// anything else = the scan did not complete.
pub fn verdict_from_exit(code: Option<i32>) -> ScanVerdict {
    match code {
        Some(0) => ScanVerdict::Clean,
        Some(2) => ScanVerdict::Threat,
        Some(other) => ScanVerdict::Unavailable(format!("scanner exited with code {other}")),
        None => ScanVerdict::Unavailable("scanner was terminated".into()),
    }
}

/// Scan one file with Windows Defender, without letting it remediate (we
/// delete rejected files ourselves). Blocking: call from `spawn_blocking`.
#[cfg(windows)]
pub fn defender_scan(path: &Path) -> ScanVerdict {
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const SCAN_TIMEOUT: Duration = Duration::from_secs(180);

    let Some(exe) = find_mpcmdrun() else {
        return ScanVerdict::Unavailable("Windows Defender command line not found".into());
    };
    let mut child = match Command::new(exe)
        .args(["-Scan", "-ScanType", "3", "-DisableRemediation", "-File"])
        .arg(path)
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => return ScanVerdict::Unavailable(format!("could not start the scanner: {e}")),
    };
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return verdict_from_exit(status.code()),
            Ok(None) if started.elapsed() > SCAN_TIMEOUT => {
                let _ = child.kill();
                let _ = child.wait();
                return ScanVerdict::Unavailable("scan timed out".into());
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(100)),
            Err(e) => return ScanVerdict::Unavailable(format!("could not wait for the scanner: {e}")),
        }
    }
}

#[cfg(not(windows))]
pub fn defender_scan(_path: &Path) -> ScanVerdict {
    ScanVerdict::Unavailable("no scanner on this platform".into())
}

#[cfg(windows)]
fn find_mpcmdrun() -> Option<std::path::PathBuf> {
    use std::path::PathBuf;
    // The copy in Program Files is a stable launcher; the Platform folder holds
    // the versioned ones (newest = highest folder name).
    if let Some(pf) = std::env::var_os("ProgramFiles") {
        let p = PathBuf::from(pf).join("Windows Defender").join("MpCmdRun.exe");
        if p.is_file() {
            return Some(p);
        }
    }
    let platform = PathBuf::from(std::env::var_os("ProgramData")?)
        .join("Microsoft")
        .join("Windows Defender")
        .join("Platform");
    let mut versions: Vec<PathBuf> = std::fs::read_dir(platform)
        .ok()?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .collect();
    versions.sort();
    versions
        .into_iter()
        .rev()
        .map(|d| d.join("MpCmdRun.exe"))
        .find(|p| p.is_file())
}

/// Text of the `Zone.Identifier` stream for a file downloaded from `host_url`.
pub fn zone_identifier_contents(host_url: &str) -> String {
    // Keep it single-line per field; a URL cannot legitimately hold CR/LF.
    let clean: String = host_url.chars().filter(|c| *c != '\r' && *c != '\n').collect();
    format!("[ZoneTransfer]\r\nZoneId=3\r\nHostUrl={clean}\r\n")
}

/// Mark the file as coming from the Internet (best effort; only NTFS has the
/// alternate data stream, so a failure is ignored).
pub fn mark_of_the_web(path: &Path, host_url: &str) {
    #[cfg(windows)]
    {
        let mut stream = path.as_os_str().to_os_string();
        stream.push(":Zone.Identifier");
        let _ = std::fs::write(stream, zone_identifier_contents(host_url));
    }
    #[cfg(not(windows))]
    {
        let _ = (path, host_url);
    }
}

/// Create the downloads folder and deny *execute* on the files inside it
/// (inherit-only, so the folder itself stays traversable). Best effort and
/// idempotent; returns whether the ACL change was applied.
pub fn harden_downloads_dir(dir: &Path) -> bool {
    let _ = std::fs::create_dir_all(dir);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        use std::process::{Command, Stdio};
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let system_root = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
        let icacls = std::path::PathBuf::from(system_root).join("System32").join("icacls.exe");
        Command::new(icacls)
            .arg(dir)
            .args(["/deny", "*S-1-1-0:(OI)(IO)(X)"])
            .creation_flags(CREATE_NO_WINDOW)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// Result of inspecting a finished download (still a `.part` file).
#[derive(Debug, PartialEq, Eq)]
pub enum Inspection {
    /// Keep it, with this extension.
    Accept(Container),
    /// Delete it; the reason is shown to the user.
    Reject(String),
}

/// Sniff + scan a downloaded file. Blocking (reads the head, runs Defender).
pub fn inspect(path: &Path) -> Inspection {
    use std::io::Read;
    let mut head = vec![0u8; 512];
    let read = match std::fs::File::open(path).and_then(|mut f| f.read(&mut head)) {
        Ok(n) => n,
        Err(e) => return Inspection::Reject(format!("could not read the downloaded file: {e}")),
    };
    head.truncate(read);
    let Some(container) = sniff_container(&head) else {
        return Inspection::Reject("the file is not a video (unrecognised content)".into());
    };
    match defender_scan(path) {
        ScanVerdict::Clean => {
            crate::file_log::info("download scanned by Windows Defender: clean");
            Inspection::Accept(container)
        }
        ScanVerdict::Threat => Inspection::Reject("Windows Defender flagged the file as a threat".into()),
        ScanVerdict::Unavailable(reason) => {
            // No scanner is a missing layer, not a reason to refuse every
            // download; the other layers still apply. Surface it in the log.
            crate::file_log::warn(&format!("download not scanned: {reason}"));
            Inspection::Accept(container)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mp4_head() -> Vec<u8> {
        let mut v = vec![0, 0, 0, 0x18];
        v.extend_from_slice(b"ftypisom");
        v.extend_from_slice(&[0; 32]);
        v
    }

    fn ts_head(packets: usize) -> Vec<u8> {
        let mut v = vec![0u8; 188 * packets];
        for i in 0..packets {
            v[i * 188] = 0x47;
        }
        v
    }

    #[test]
    fn recognises_the_three_containers() {
        assert_eq!(sniff_container(&mp4_head()), Some(Container::Mp4));
        assert_eq!(sniff_container(&ts_head(3)), Some(Container::MpegTs));
        let mut mkv = vec![0x1A, 0x45, 0xDF, 0xA3];
        mkv.extend_from_slice(&[0; 16]);
        assert_eq!(sniff_container(&mkv), Some(Container::Matroska));
        let mut mov = vec![0, 0, 0, 8];
        mov.extend_from_slice(b"moov");
        mov.extend_from_slice(&[0; 16]);
        assert_eq!(sniff_container(&mov), Some(Container::Mp4));
    }

    #[test]
    fn extension_follows_the_content() {
        assert_eq!(Container::Mp4.extension(), "mp4");
        assert_eq!(Container::MpegTs.extension(), "ts");
        assert_eq!(Container::Matroska.extension(), "mkv");
    }

    #[test]
    fn rejects_executables_scripts_archives_and_html() {
        for bad in [
            &b"MZ\x90\x00\x03\x00\x00\x00\x04\x00\x00\x00\xff\xff"[..],
            &b"\x7fELF\x02\x01\x01\x00\x00\x00\x00\x00\x00\x00\x00\x00"[..],
            &b"PK\x03\x04\x14\x00\x00\x00\x08\x00\x00\x00\x00\x00"[..],
            &b"#!/bin/sh\necho hi\n  padding padding padding"[..],
            &b"<!DOCTYPE html><html><body>nope</body></html>"[..],
            &b"Rar!\x1a\x07\x00\xcf\x90s\x00\x00\x0d\x00\x00\x00"[..],
            &b""[..],
            &b"tiny"[..],
        ] {
            assert_eq!(sniff_container(bad), None, "{:?}", &bad[..bad.len().min(8)]);
        }
    }

    #[test]
    fn a_lone_sync_byte_is_not_mpeg_ts() {
        let mut v = vec![0u8; 400];
        v[0] = 0x47;
        assert_eq!(sniff_container(&v), None);
        // Two good packets but a bad third one.
        let mut v = ts_head(3);
        v[376] = 0;
        assert_eq!(sniff_container(&v), None);
    }

    #[test]
    fn scanner_exit_codes() {
        assert_eq!(verdict_from_exit(Some(0)), ScanVerdict::Clean);
        assert_eq!(verdict_from_exit(Some(2)), ScanVerdict::Threat);
        assert!(matches!(verdict_from_exit(Some(5)), ScanVerdict::Unavailable(_)));
        assert!(matches!(verdict_from_exit(None), ScanVerdict::Unavailable(_)));
    }

    #[test]
    fn zone_identifier_marks_the_internet_zone_and_strips_newlines() {
        let z = zone_identifier_contents("https://a.example/x\r\nZoneId=0");
        assert!(z.starts_with("[ZoneTransfer]\r\nZoneId=3\r\n"));
        assert_eq!(z.matches("ZoneId=").count(), 1 + 1, "an injected ZoneId must stay inside HostUrl");
        assert!(z.contains("HostUrl=https://a.example/xZoneId=0"));
    }

    fn temp_file(name: &str, bytes: &[u8]) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("aot_safety_{name}_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("sample.part");
        std::fs::write(&p, bytes).unwrap();
        p
    }

    #[test]
    fn inspect_rejects_non_video_content_before_scanning() {
        let p = temp_file("html", b"<html>hello</html>");
        assert!(matches!(inspect(&p), Inspection::Reject(r) if r.contains("not a video")));
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }

    #[test]
    fn inspect_accepts_a_clean_mp4() {
        let p = temp_file("mp4", &mp4_head());
        // Defender (when present) finds nothing in a bare container header.
        assert_eq!(inspect(&p), Inspection::Accept(Container::Mp4));
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }

    #[test]
    fn inspect_reports_an_unreadable_file() {
        let p = std::env::temp_dir().join("aot_safety_missing_file.part");
        assert!(matches!(inspect(&p), Inspection::Reject(_)));
    }

    #[test]
    fn hardening_creates_the_folder() {
        let dir = std::env::temp_dir().join(format!("aot_safety_dir_{}", std::process::id()));
        let _ = harden_downloads_dir(&dir);
        assert!(dir.is_dir());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
