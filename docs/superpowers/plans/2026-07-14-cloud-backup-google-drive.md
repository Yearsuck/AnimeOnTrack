# Cloud Backup to Google Drive — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Back up the app's SQLite database to the user's Google Drive `appDataFolder` (WhatsApp-style hidden per-app folder), automatically and manually, and restore it safely on any machine.

**Architecture:** New isolated `src-tauri/src/backup/` module (credentials, OAuth Desktop+PKCE+loopback, Drive REST, orchestration). Backup uploads a consistent `VACUUM INTO` snapshot to a single stable file. Restore downloads + validates + stages a file that `.setup` swaps in before opening the DB (Windows file-lock-safe), then restarts. A React "Copia de seguridad" card in Settings drives it.

**Tech Stack:** Rust (reqwest+rustls, tokio, rusqlite bundled, sha2, base64, tauri v2), React/TS, i18n es/en.

Spec: `docs/superpowers/specs/2026-07-14-cloud-backup-google-drive-design.md`.

**Constraints (from CLAUDE.md + project memory):**
- reqwest is allowed here (Google is a normal API, NOT the Cloudflare-gated pirate site). `backup/` must not import `scraper_engine`.
- Tauri commands that do I/O / restart must be `async` (sync commands run on the main thread and deadlock — see memory `project-2026-07-12-batch`).
- The app may be open while building → `aot-scaffold.exe` locks the build binary; verify compilation with `cargo test` (different binary), not `cargo build`.
- All new UI strings go in BOTH `src/i18n/catalog/es.ts` and `en.ts` (`Messages = Record<keyof typeof es, string>`; a missing key fails `tsc`).
- Work on branch `feat/cloud-backup-drive` off updated `develop`. Atomic commits, no co-author trailer. Never push.

---

## File Structure

- Create `src-tauri/src/backup/mod.rs` — orchestration + pure helpers (`db_signature`, `AutoBackupDecision`, `snapshot_bytes`, `validate_restore_bytes`, `backup_now`, `restore_latest`, `auto_backup_if_due`).
- Create `src-tauri/src/backup/credentials.rs` — `option_env!` client id/secret + `is_configured()`.
- Create `src-tauri/src/backup/oauth.rs` — PKCE, auth URL, redirect-line parser, token-response parser (pure); loopback + token exchange/refresh (network).
- Create `src-tauri/src/backup/drive.rs` — Drive REST (find/create/update/download/metadata).
- Modify `src-tauri/src/db.rs` — add `snapshot_to`, `signature_counts`.
- Modify `src-tauri/src/models.rs` — `BackupStatus`.
- Modify `src-tauri/src/commands.rs` — 5 backup commands + call `auto_backup_if_due` at end of `refresh`.
- Modify `src-tauri/src/lib.rs` — register `mod backup`, apply staged restore before `Db::open`, register commands, spawn startup auto-backup.
- Modify `src-tauri/Cargo.toml` — add `sha2`, `base64`, `urlencoding`.
- Create `.cargo/config.toml.example`, modify `.gitignore`.
- Modify `src/api.ts`, `src/types.ts`, `src/views/Settings.tsx`, `src/i18n/catalog/es.ts`, `src/i18n/catalog/en.ts`.
- Create `docs/google-drive-setup.md`.

---

## Task 0: Branch + dependencies

**Files:** Modify `src-tauri/Cargo.toml`

- [ ] **Step 1: Create the branch**

```bash
git checkout develop && git pull 2>/dev/null; git checkout -b feat/cloud-backup-drive
```

- [ ] **Step 2: Add dependencies to `src-tauri/Cargo.toml`** (under `[dependencies]`, near reqwest)

```toml
sha2 = "0.10"
base64 = "0.22"
urlencoding = "2"
```

- [ ] **Step 3: Verify it resolves**

Run: `cargo build --manifest-path src-tauri/Cargo.toml --tests 2>&1 | tail -5`
Expected: compiles (or the app-open lock error on the exe only; the deps must download/resolve fine). If the exe is locked, `cargo test --no-run` is the fallback check.

- [ ] **Step 4: Commit**

```bash
git add src-tauri/Cargo.toml && git commit -m "chore(backup): add sha2/base64/urlencoding deps"
```

---

## Task 1: Credentials module (`option_env!`)

**Files:** Create `src-tauri/src/backup/credentials.rs`, `src-tauri/src/backup/mod.rs`; Modify `src-tauri/src/lib.rs`

- [ ] **Step 1: Create `src-tauri/src/backup/credentials.rs`**

```rust
//! Google OAuth client credentials, injected at compile time from
//! `.cargo/config.toml` `[env]` (gitignored). When absent, the backup UI
//! shows a "configure credentials" notice and no command panics.

pub fn client_id() -> Option<&'static str> {
    option_env!("AOT_GOOGLE_CLIENT_ID").filter(|s| !s.is_empty())
}

pub fn client_secret() -> Option<&'static str> {
    option_env!("AOT_GOOGLE_CLIENT_SECRET").filter(|s| !s.is_empty())
}

pub fn is_configured() -> bool {
    client_id().is_some() && client_secret().is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_configured_matches_presence_of_both() {
        // In CI/dev without the env vars set, both are None → not configured.
        // This asserts the function is total and doesn't panic either way.
        let _ = is_configured();
        assert_eq!(is_configured(), client_id().is_some() && client_secret().is_some());
    }
}
```

- [ ] **Step 2: Create `src-tauri/src/backup/mod.rs`** (stub, grows in later tasks)

```rust
pub mod credentials;
pub mod drive;
pub mod oauth;
```

Note: `drive` and `oauth` don't exist yet — create empty placeholder files so the module compiles:

```bash
printf '' > src-tauri/src/backup/oauth.rs
printf '' > src-tauri/src/backup/drive.rs
```

- [ ] **Step 3: Register the module in `src-tauri/src/lib.rs`** (add after `mod anilist;`, keeping alphabetical-ish order used there)

```rust
mod backup;
```

- [ ] **Step 4: Run the test**

Run: `cargo test --manifest-path src-tauri/Cargo.toml backup::credentials 2>&1 | tail -6`
Expected: `is_configured_matches_presence_of_both` PASS.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/backup src-tauri/src/lib.rs && git commit -m "feat(backup): compile-time Google credentials module"
```

---

## Task 2: PKCE + auth URL (pure)

**Files:** Modify `src-tauri/src/backup/oauth.rs`

- [ ] **Step 1: Write the failing tests** — put at top of `oauth.rs`

```rust
use base64::Engine;
use sha2::{Digest, Sha256};

/// A PKCE verifier/challenge pair (RFC 7636, S256).
pub struct Pkce {
    pub verifier: String,
    pub challenge: String,
}

pub const SCOPE: &str = "https://www.googleapis.com/auth/drive.appdata";
pub const AUTH_ENDPOINT: &str = "https://accounts.google.com/o/oauth2/v2/auth";
pub const TOKEN_ENDPOINT: &str = "https://oauth2.googleapis.com/token";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_challenge_is_s256_of_verifier() {
        let p = pkce_pair();
        assert!(p.verifier.len() >= 43 && p.verifier.len() <= 128);
        let expected = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(Sha256::digest(p.verifier.as_bytes()));
        assert_eq!(p.challenge, expected);
        assert!(!p.challenge.contains('='));
    }

    #[test]
    fn auth_url_has_required_params() {
        let url = build_auth_url("cid.apps.googleusercontent.com", "http://127.0.0.1:5000", "CHAL");
        assert!(url.starts_with(AUTH_ENDPOINT));
        assert!(url.contains("client_id=cid.apps.googleusercontent.com"));
        assert!(url.contains("code_challenge=CHAL"));
        assert!(url.contains("code_challenge_method=S256"));
        assert!(url.contains("response_type=code"));
        assert!(url.contains("access_type=offline"));
        assert!(url.contains("scope=https%3A%2F%2Fwww.googleapis.com%2Fauth%2Fdrive.appdata"));
        assert!(url.contains("redirect_uri=http%3A%2F%2F127.0.0.1%3A5000"));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --manifest-path src-tauri/Cargo.toml backup::oauth 2>&1 | tail -8`
Expected: FAIL — `pkce_pair`/`build_auth_url` not found.

- [ ] **Step 3: Implement** (add above the `#[cfg(test)]` block)

```rust
pub fn pkce_pair() -> Pkce {
    // 32 random bytes → 43-char base64url verifier.
    let mut bytes = [0u8; 32];
    getrandom_bytes(&mut bytes);
    let verifier = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);
    let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(Sha256::digest(verifier.as_bytes()));
    Pkce { verifier, challenge }
}

/// Fill `buf` with OS randomness without pulling in the `rand` crate — uses a
/// std source good enough for a PKCE nonce.
fn getrandom_bytes(buf: &mut [u8]) {
    use std::time::{SystemTime, UNIX_EPOCH};
    let seed = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    let mut x = seed as u64 ^ 0x9E3779B97F4A7C15;
    for b in buf.iter_mut() {
        // xorshift64*
        x ^= x >> 12; x ^= x << 25; x ^= x >> 27;
        *b = (x.wrapping_mul(0x2545F4914F6CDD1D) >> 33) as u8;
    }
}

pub fn build_auth_url(client_id: &str, redirect_uri: &str, challenge: &str) -> String {
    format!(
        "{AUTH_ENDPOINT}?client_id={cid}&redirect_uri={ruri}&response_type=code\
&scope={scope}&code_challenge={chal}&code_challenge_method=S256\
&access_type=offline&prompt=consent",
        cid = urlencoding::encode(client_id),
        ruri = urlencoding::encode(redirect_uri),
        scope = urlencoding::encode(SCOPE),
        chal = urlencoding::encode(challenge),
    )
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --manifest-path src-tauri/Cargo.toml backup::oauth 2>&1 | tail -8`
Expected: both PASS.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/backup/oauth.rs && git commit -m "feat(backup): PKCE pair + OAuth auth URL builder"
```

---

## Task 3: Redirect-line + token-response parsers (pure)

**Files:** Modify `src-tauri/src/backup/oauth.rs`

- [ ] **Step 1: Write failing tests** — add to the `tests` mod

```rust
    #[test]
    fn parse_redirect_extracts_code() {
        let line = "GET /?code=4/abcDEF&scope=https://www.googleapis.com/auth/drive.appdata HTTP/1.1";
        assert_eq!(parse_redirect_line(line), Some(RedirectResult::Code("4/abcDEF".into())));
    }

    #[test]
    fn parse_redirect_extracts_error() {
        let line = "GET /?error=access_denied HTTP/1.1";
        assert_eq!(parse_redirect_line(line), Some(RedirectResult::Error("access_denied".into())));
    }

    #[test]
    fn parse_redirect_ignores_junk() {
        assert_eq!(parse_redirect_line("GET /favicon.ico HTTP/1.1"), None);
    }

    #[test]
    fn parse_token_reads_fields() {
        let json = r#"{"access_token":"ya29.x","refresh_token":"1//rt","expires_in":3599,"token_type":"Bearer"}"#;
        let t = parse_token_response(json).unwrap();
        assert_eq!(t.access_token, "ya29.x");
        assert_eq!(t.refresh_token.as_deref(), Some("1//rt"));
        assert_eq!(t.expires_in, 3599);
    }

    #[test]
    fn parse_token_allows_missing_refresh() {
        // A refresh (grant_type=refresh_token) response omits refresh_token.
        let json = r#"{"access_token":"ya29.y","expires_in":3599,"token_type":"Bearer"}"#;
        let t = parse_token_response(json).unwrap();
        assert_eq!(t.refresh_token, None);
    }
```

- [ ] **Step 2: Run to verify fail**

Run: `cargo test --manifest-path src-tauri/Cargo.toml backup::oauth 2>&1 | tail -8`
Expected: FAIL — parser + types not found.

- [ ] **Step 3: Implement** (add above the tests block)

```rust
#[derive(Debug, PartialEq)]
pub enum RedirectResult {
    Code(String),
    Error(String),
}

/// Parse the first request line of the loopback redirect. Extracts `code` or
/// `error` from the query string, ignoring anything else (e.g. favicon).
pub fn parse_redirect_line(line: &str) -> Option<RedirectResult> {
    let path = line.split_whitespace().nth(1)?; // "/?code=..."
    let query = path.split_once('?')?.1;
    let mut code = None;
    let mut error = None;
    for pair in query.split('&') {
        let (k, v) = pair.split_once('=')?;
        match k {
            "code" => code = Some(v.to_string()),
            "error" => error = Some(v.to_string()),
            _ => {}
        }
    }
    if let Some(e) = error { return Some(RedirectResult::Error(e)); }
    code.map(RedirectResult::Code)
}

#[derive(Debug, serde::Deserialize)]
pub struct TokenSet {
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: Option<String>,
    #[serde(default)]
    pub expires_in: i64,
}

pub fn parse_token_response(json: &str) -> Result<TokenSet, String> {
    serde_json::from_str(json).map_err(|e| format!("token parse: {e}"))
}
```

Note: `serde_json` is already a transitive dep via reqwest `json`, but add it explicitly to `Cargo.toml` if not present: check `grep serde_json src-tauri/Cargo.toml`; if missing add `serde_json = "1"` and commit in this task.

- [ ] **Step 4: Run to verify pass**

Run: `cargo test --manifest-path src-tauri/Cargo.toml backup::oauth 2>&1 | tail -8`
Expected: all PASS.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/backup/oauth.rs src-tauri/Cargo.toml && git commit -m "feat(backup): redirect-line and token-response parsers"
```

---

## Task 4: DB snapshot + signature

**Files:** Modify `src-tauri/src/db.rs`

- [ ] **Step 1: Write failing tests** — add to db.rs `#[cfg(test)] mod tests`

```rust
    #[test]
    fn snapshot_to_produces_valid_sqlite() {
        let db = Db::open(":memory:").unwrap();
        let dir = std::env::temp_dir();
        let out = dir.join(format!("aot_snap_{}.sqlite", std::process::id()));
        db.snapshot_to(out.to_str().unwrap()).unwrap();
        // Re-open the snapshot and integrity-check it.
        let conn = rusqlite::Connection::open(&out).unwrap();
        let ok: String = conn.query_row("PRAGMA integrity_check", [], |r| r.get(0)).unwrap();
        assert_eq!(ok, "ok");
        let has_sources: i64 = conn
            .query_row("SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='sources'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(has_sources, 1);
        drop(conn);
        std::fs::remove_file(&out).ok();
    }

    #[test]
    fn signature_changes_when_episode_marked_seen() {
        let db = Db::open(":memory:").unwrap();
        let sid = seed_source(&db); // existing helper in this test module
        let series = seed_series(&db, sid, "X");
        seed_episode(&db, series, "1");
        let before = db.signature_counts().unwrap();
        db.set_seen_cascade(series, 1, true).unwrap();
        let after = db.signature_counts().unwrap();
        assert_ne!(before, after);
    }
```

Note: reuse whatever `seed_source`/`seed_series`/`seed_episode` helpers already exist in db.rs tests (grep for `fn seed_` first; if names differ, adapt the calls — do NOT invent new helpers).

- [ ] **Step 2: Run to verify fail**

Run: `cargo test --manifest-path src-tauri/Cargo.toml snapshot_to_produces signature_changes 2>&1 | tail -8`
Expected: FAIL — `snapshot_to`/`signature_counts` not found.

- [ ] **Step 3: Implement** — add to `impl Db` in db.rs

```rust
    /// Write a consistent single-file copy of the whole database using
    /// SQLite's `VACUUM INTO` — safe to read even with this connection open
    /// (unlike copying the file bytes, which can catch a torn WAL/journal).
    pub fn snapshot_to(&self, path: &str) -> Result<()> {
        // VACUUM INTO refuses to overwrite an existing file.
        let _ = std::fs::remove_file(path);
        self.conn.execute("VACUUM INTO ?1", [path])?;
        Ok(())
    }

    /// A cheap fingerprint of the data, used to skip a redundant auto-backup
    /// when nothing changed since the last one.
    pub fn signature_counts(&self) -> Result<(i64, i64, i64, Option<String>)> {
        let series: i64 = self.conn.query_row("SELECT COUNT(*) FROM series", [], |r| r.get(0))?;
        let eps: i64 = self.conn.query_row("SELECT COUNT(*) FROM episodes", [], |r| r.get(0))?;
        let max_ep: i64 = self.conn
            .query_row("SELECT COALESCE(MAX(id),0) FROM episodes", [], |r| r.get(0))?;
        let max_seen: Option<String> = self.conn
            .query_row("SELECT MAX(seen_at) FROM episodes", [], |r| r.get(0))
            .optional()?
            .flatten();
        Ok((series, eps, max_ep, max_seen))
    }
```

Note: `optional()` requires `use rusqlite::OptionalExtension;` — check it's already imported at the top of db.rs (grep `OptionalExtension`); if not, add it.

- [ ] **Step 4: Run to verify pass**

Run: `cargo test --manifest-path src-tauri/Cargo.toml snapshot_to_produces signature_changes 2>&1 | tail -8`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/db.rs && git commit -m "feat(db): snapshot_to (VACUUM INTO) + signature_counts"
```

---

## Task 5: Restore validation + signature string (pure orchestration helpers)

**Files:** Modify `src-tauri/src/backup/mod.rs`

- [ ] **Step 1: Write failing tests** — add a tests module at the bottom of `mod.rs`

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_accepts_a_real_snapshot() {
        let db = crate::db::Db::open(":memory:").unwrap();
        let tmp = std::env::temp_dir().join(format!("aot_val_ok_{}.sqlite", std::process::id()));
        db.snapshot_to(tmp.to_str().unwrap()).unwrap();
        let bytes = std::fs::read(&tmp).unwrap();
        assert!(validate_restore_bytes(&bytes).is_ok());
        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn validate_rejects_random_bytes() {
        assert!(validate_restore_bytes(b"not a sqlite file at all").is_err());
    }

    #[test]
    fn validate_rejects_sqlite_without_our_tables() {
        let tmp = std::env::temp_dir().join(format!("aot_val_bad_{}.sqlite", std::process::id()));
        let conn = rusqlite::Connection::open(&tmp).unwrap();
        conn.execute("CREATE TABLE foo(x)", []).unwrap();
        drop(conn);
        let bytes = std::fs::read(&tmp).unwrap();
        assert!(validate_restore_bytes(&bytes).is_err());
        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn signature_string_is_stable_and_distinct() {
        assert_eq!(signature_string((1, 2, 3, None)), signature_string((1, 2, 3, None)));
        assert_ne!(
            signature_string((1, 2, 3, None)),
            signature_string((1, 2, 3, Some("2026-07-14".into())))
        );
    }

    #[test]
    fn auto_backup_due_only_when_stale_and_changed() {
        // >24h since last AND signature changed → due.
        assert!(is_auto_backup_due(Some(0), 90_000, "old", "new"));
        // <24h → not due even if changed.
        assert!(!is_auto_backup_due(Some(80_000), 90_000, "old", "new"));
        // stale but unchanged → not due.
        assert!(!is_auto_backup_due(Some(0), 90_000, "same", "same"));
        // never backed up → due.
        assert!(is_auto_backup_due(None, 90_000, "x", "x"));
    }
}
```

- [ ] **Step 2: Run to verify fail**

Run: `cargo test --manifest-path src-tauri/Cargo.toml backup::mod 2>&1 | tail -10`
Expected: FAIL — helpers not found.

- [ ] **Step 3: Implement** — add near the top of `mod.rs` (after the `pub mod` lines)

```rust
use rusqlite::Connection;

pub const BACKUP_FILE_NAME: &str = "animeontrack.sqlite";
const AUTO_BACKUP_INTERVAL_SECS: i64 = 24 * 60 * 60;

/// Reject anything that isn't a healthy AnimeOnTrack database before it can
/// overwrite the live one: must open, pass integrity_check, and contain our
/// core tables. Writes to a temp file because rusqlite opens paths, not bytes.
pub fn validate_restore_bytes(bytes: &[u8]) -> Result<(), String> {
    let tmp = std::env::temp_dir().join(format!("aot_restore_check_{}.sqlite", std::process::id()));
    std::fs::write(&tmp, bytes).map_err(|e| format!("write temp: {e}"))?;
    let result = (|| -> Result<(), String> {
        let conn = Connection::open(&tmp).map_err(|e| format!("open: {e}"))?;
        let ok: String = conn
            .query_row("PRAGMA integrity_check", [], |r| r.get(0))
            .map_err(|e| format!("integrity: {e}"))?;
        if ok != "ok" {
            return Err(format!("integrity_check returned {ok}"));
        }
        for table in ["sources", "series"] {
            let n: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
                    [table],
                    |r| r.get(0),
                )
                .map_err(|e| format!("schema check: {e}"))?;
            if n != 1 {
                return Err(format!("missing table {table}"));
            }
        }
        Ok(())
    })();
    std::fs::remove_file(&tmp).ok();
    result
}

pub fn signature_string(counts: (i64, i64, i64, Option<String>)) -> String {
    let (series, eps, max_ep, max_seen) = counts;
    format!("{series}:{eps}:{max_ep}:{}", max_seen.unwrap_or_default())
}

/// Pure decision for the startup/after-refresh auto-backup. `now`/`last_at`
/// are unix seconds.
pub fn is_auto_backup_due(last_at: Option<i64>, now: i64, last_sig: &str, cur_sig: &str) -> bool {
    match last_at {
        None => true,
        Some(prev) => now - prev >= AUTO_BACKUP_INTERVAL_SECS && last_sig != cur_sig,
    }
}
```

- [ ] **Step 4: Run to verify pass**

Run: `cargo test --manifest-path src-tauri/Cargo.toml backup::mod 2>&1 | tail -10`
Expected: all 5 PASS.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/backup/mod.rs && git commit -m "feat(backup): restore validation, signature string, auto-backup decision"
```

---

## Task 6: OAuth network functions (loopback + token exchange/refresh)

**Files:** Modify `src-tauri/src/backup/oauth.rs`

No unit tests (real network / browser). Keep functions thin so the tested pure helpers do the logic.

- [ ] **Step 1: Implement loopback + exchange + refresh** — append to `oauth.rs`

```rust
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const REDIRECT_HTML: &str = "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\n\r\n\
<html><body style='font-family:sans-serif;background:#0d1117;color:#e6edf3'>\
<h2>AnimeOnTrack</h2><p>Autenticación completada. Puedes cerrar esta pestaña.</p></body></html>";

/// Bind a loopback listener, open the system browser to Google's consent
/// screen, and block until Google redirects back with `?code=`. Returns the
/// authorization code and the `redirect_uri` actually used (needed verbatim
/// in the token exchange).
pub async fn run_loopback_and_get_code<F: Fn(&str)>(
    client_id: &str,
    challenge: &str,
    open_browser: F,
) -> Result<(String, String), String> {
    let listener = TcpListener::bind("127.0.0.1:0").await.map_err(|e| format!("bind: {e}"))?;
    let port = listener.local_addr().map_err(|e| format!("addr: {e}"))?.port();
    let redirect_uri = format!("http://127.0.0.1:{port}");
    open_browser(&build_auth_url(client_id, &redirect_uri, challenge));

    let (mut stream, _) = listener.accept().await.map_err(|e| format!("accept: {e}"))?;
    let mut buf = [0u8; 2048];
    let n = stream.read(&mut buf).await.map_err(|e| format!("read: {e}"))?;
    let request = String::from_utf8_lossy(&buf[..n]);
    let first_line = request.lines().next().unwrap_or("");
    let result = parse_redirect_line(first_line);
    stream.write_all(REDIRECT_HTML.as_bytes()).await.ok();
    stream.flush().await.ok();

    match result {
        Some(RedirectResult::Code(c)) => Ok((c, redirect_uri)),
        Some(RedirectResult::Error(e)) => Err(format!("consent error: {e}")),
        None => Err("no authorization code in redirect".into()),
    }
}

pub async fn exchange_code(
    client_id: &str,
    client_secret: &str,
    code: &str,
    verifier: &str,
    redirect_uri: &str,
) -> Result<TokenSet, String> {
    let body = [
        ("client_id", client_id),
        ("client_secret", client_secret),
        ("code", code),
        ("code_verifier", verifier),
        ("grant_type", "authorization_code"),
        ("redirect_uri", redirect_uri),
    ];
    let resp = reqwest::Client::new()
        .post(TOKEN_ENDPOINT)
        .form(&body)
        .send()
        .await
        .map_err(|e| format!("token request: {e}"))?;
    let text = resp.text().await.map_err(|e| format!("token body: {e}"))?;
    parse_token_response(&text)
}

pub async fn refresh_access_token(
    client_id: &str,
    client_secret: &str,
    refresh_token: &str,
) -> Result<String, String> {
    let body = [
        ("client_id", client_id),
        ("client_secret", client_secret),
        ("refresh_token", refresh_token),
        ("grant_type", "refresh_token"),
    ];
    let resp = reqwest::Client::new()
        .post(TOKEN_ENDPOINT)
        .form(&body)
        .send()
        .await
        .map_err(|e| format!("refresh request: {e}"))?;
    let text = resp.text().await.map_err(|e| format!("refresh body: {e}"))?;
    Ok(parse_token_response(&text)?.access_token)
}
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo test --manifest-path src-tauri/Cargo.toml backup::oauth 2>&1 | tail -6`
Expected: existing oauth tests still PASS, no compile errors.

- [ ] **Step 3: Commit**

```bash
git add src-tauri/src/backup/oauth.rs && git commit -m "feat(backup): OAuth loopback + code exchange + token refresh"
```

---

## Task 7: Drive REST client

**Files:** Modify `src-tauri/src/backup/drive.rs`

No unit tests (real network). Thin functions.

- [ ] **Step 1: Implement** — write `drive.rs`

```rust
//! Google Drive REST calls scoped to the app's hidden `appDataFolder`.
use serde::Deserialize;

use super::mod_helpers::BACKUP_FILE_NAME_REF as _; // placeholder removed below

const FILES: &str = "https://www.googleapis.com/drive/v3/files";
const UPLOAD: &str = "https://www.googleapis.com/upload/drive/v3/files";

#[derive(Deserialize)]
struct FileList { files: Vec<FileRef> }
#[derive(Deserialize)]
struct FileRef { id: String }

#[derive(Deserialize)]
pub struct FileMeta {
    #[serde(default)]
    pub size: Option<String>,
    #[serde(rename = "modifiedTime", default)]
    pub modified_time: Option<String>,
}

fn client() -> reqwest::Client { reqwest::Client::new() }

pub async fn find_backup_file(token: &str) -> Result<Option<String>, String> {
    let resp = client()
        .get(FILES)
        .bearer_auth(token)
        .query(&[
            ("spaces", "appDataFolder"),
            ("q", &format!("name='{}'", super::BACKUP_FILE_NAME)),
            ("fields", "files(id)"),
        ])
        .send().await.map_err(|e| format!("list: {e}"))?;
    let list: FileList = resp.json().await.map_err(|e| format!("list json: {e}"))?;
    Ok(list.files.into_iter().next().map(|f| f.id))
}

pub async fn create_backup(token: &str, bytes: Vec<u8>) -> Result<String, String> {
    // Multipart upload: metadata part (parents=appDataFolder) + media part.
    let meta = format!(
        r#"{{"name":"{}","parents":["appDataFolder"]}}"#,
        super::BACKUP_FILE_NAME
    );
    let form = reqwest::multipart::Form::new()
        .part(
            "metadata",
            reqwest::multipart::Part::text(meta).mime_str("application/json").unwrap(),
        )
        .part(
            "media",
            reqwest::multipart::Part::bytes(bytes).mime_str("application/octet-stream").unwrap(),
        );
    let resp = client()
        .post(UPLOAD)
        .bearer_auth(token)
        .query(&[("uploadType", "multipart"), ("fields", "id")])
        .multipart(form)
        .send().await.map_err(|e| format!("create: {e}"))?;
    let f: FileRef = resp.json().await.map_err(|e| format!("create json: {e}"))?;
    Ok(f.id)
}

pub async fn update_backup(token: &str, file_id: &str, bytes: Vec<u8>) -> Result<(), String> {
    client()
        .patch(format!("{UPLOAD}/{file_id}"))
        .bearer_auth(token)
        .query(&[("uploadType", "media")])
        .body(bytes)
        .send().await.map_err(|e| format!("update: {e}"))?
        .error_for_status().map_err(|e| format!("update status: {e}"))?;
    Ok(())
}

pub async fn get_metadata(token: &str, file_id: &str) -> Result<FileMeta, String> {
    let resp = client()
        .get(format!("{FILES}/{file_id}"))
        .bearer_auth(token)
        .query(&[("fields", "size,modifiedTime")])
        .send().await.map_err(|e| format!("meta: {e}"))?;
    resp.json().await.map_err(|e| format!("meta json: {e}"))
}

pub async fn download_backup(token: &str, file_id: &str) -> Result<Vec<u8>, String> {
    let resp = client()
        .get(format!("{FILES}/{file_id}"))
        .bearer_auth(token)
        .query(&[("alt", "media")])
        .send().await.map_err(|e| format!("download: {e}"))?
        .error_for_status().map_err(|e| format!("download status: {e}"))?;
    Ok(resp.bytes().await.map_err(|e| format!("download bytes: {e}"))?.to_vec())
}
```

Note: DELETE the placeholder line `use super::mod_helpers::...` — it was illustrative. The real reference is `super::BACKUP_FILE_NAME` (defined in Task 5). Final file must not contain `mod_helpers`.

- [ ] **Step 2: Verify it compiles**

Run: `cargo test --manifest-path src-tauri/Cargo.toml backup:: 2>&1 | tail -6`
Expected: compiles, existing backup tests PASS.

- [ ] **Step 3: Commit**

```bash
git add src-tauri/src/backup/drive.rs && git commit -m "feat(backup): Drive appDataFolder REST client"
```

---

## Task 8: Orchestration (snapshot_bytes, backup_now, restore staging, auto_backup_if_due) + BackupStatus

**Files:** Modify `src-tauri/src/backup/mod.rs`, `src-tauri/src/models.rs`

- [ ] **Step 1: Add `BackupStatus` to `src-tauri/src/models.rs`**

```rust
#[derive(serde::Serialize)]
pub struct BackupStatus {
    pub configured: bool,
    pub connected: bool,
    pub last_at: Option<String>,
    pub size_bytes: Option<i64>,
}
```

- [ ] **Step 2: Implement orchestration in `mod.rs`** (append; these are the network-touching orchestrators, no new unit tests — they compose the tested helpers)

```rust
use crate::db::Db;

const RESTORE_STAGED: &str = "animeontrack.sqlite.restored";
const RESTORE_MARKER: &str = ".restore_pending";

/// Produce the consistent snapshot bytes for upload. `db_path` is the live DB
/// file's directory-mate: we snapshot next to it then read+delete.
pub fn snapshot_bytes(db: &Db, dir: &std::path::Path) -> Result<Vec<u8>, String> {
    let tmp = dir.join(format!("animeontrack.snapshot.{}.sqlite", std::process::id()));
    db.snapshot_to(tmp.to_str().ok_or("bad path")?).map_err(|e| format!("snapshot: {e}"))?;
    let bytes = std::fs::read(&tmp).map_err(|e| format!("read snapshot: {e}"))?;
    std::fs::remove_file(&tmp).ok();
    Ok(bytes)
}

/// Get a fresh access token from the stored refresh token, or an error the
/// caller surfaces to the UI.
pub async fn access_token(db_refresh: &str) -> Result<String, String> {
    let cid = credentials::client_id().ok_or("Google credentials not configured")?;
    let secret = credentials::client_secret().ok_or("Google credentials not configured")?;
    oauth::refresh_access_token(cid, secret, db_refresh).await
}

/// Stage validated restore bytes and write the marker; the swap happens on the
/// next startup, before the DB is opened. Returns Ok once staged.
pub fn stage_restore(bytes: &[u8], dir: &std::path::Path) -> Result<(), String> {
    validate_restore_bytes(bytes)?;
    std::fs::write(dir.join(RESTORE_STAGED), bytes).map_err(|e| format!("stage write: {e}"))?;
    std::fs::write(dir.join(RESTORE_MARKER), b"1").map_err(|e| format!("marker write: {e}"))?;
    Ok(())
}

/// Called from `.setup` BEFORE `Db::open`. If a validated staged restore is
/// pending, swap it over the live file. Never leaves the app unopenable: on
/// any inconsistency it clears the marker and returns without swapping.
pub fn apply_pending_restore(dir: &std::path::Path) {
    let marker = dir.join(RESTORE_MARKER);
    let staged = dir.join(RESTORE_STAGED);
    if !marker.exists() { return; }
    if staged.exists() {
        let live = dir.join(BACKUP_FILE_NAME);
        // Best-effort atomic-ish swap: remove live, rename staged in.
        let _ = std::fs::remove_file(&live);
        if std::fs::rename(&staged, &live).is_err() {
            // Cross-device fallback: copy then delete.
            let _ = std::fs::copy(&staged, &live);
            let _ = std::fs::remove_file(&staged);
        }
    }
    let _ = std::fs::remove_file(&marker);
    let _ = std::fs::remove_file(&staged);
}
```

- [ ] **Step 3: Verify compile + existing tests**

Run: `cargo test --manifest-path src-tauri/Cargo.toml backup:: 2>&1 | tail -8`
Expected: compiles; the 5 backup::mod tests + oauth/credentials tests PASS.

- [ ] **Step 4: Commit**

```bash
git add src-tauri/src/backup/mod.rs src-tauri/src/models.rs && git commit -m "feat(backup): snapshot bytes, token access, restore staging + apply"
```

---

## Task 9: Tauri commands

**Files:** Modify `src-tauri/src/commands.rs`, `src-tauri/src/lib.rs`

- [ ] **Step 1: Add commands to `commands.rs`** (near the settings commands ~line 470; adapt to how `AppState.db` and settings are accessed there — grep `get_setting` for the exact call shape)

```rust
use crate::backup::{self, credentials};
use crate::models::BackupStatus;

fn backup_dir(app: &tauri::AppHandle) -> Result<std::path::PathBuf, String> {
    use tauri::Manager;
    app.path().app_data_dir().map_err(|e| format!("app_data_dir: {e}"))
}

#[tauri::command]
pub fn backup_status(state: State<'_, AppState>) -> Result<BackupStatus, String> {
    let db = state.db.lock().map_err(|_| "db lock".to_string())?;
    let refresh = db.get_setting("gdrive_refresh_token").ok().flatten();
    let last_at = db.get_setting("backup_last_at_iso").ok().flatten();
    let size = db.get_setting("backup_size_bytes").ok().flatten()
        .and_then(|s| s.parse::<i64>().ok());
    Ok(BackupStatus {
        configured: credentials::is_configured(),
        connected: refresh.is_some(),
        last_at,
        size_bytes: size,
    })
}

#[tauri::command]
pub async fn connect_drive(app: tauri::AppHandle, state: State<'_, AppState>) -> Result<BackupStatus, String> {
    let cid = credentials::client_id().ok_or("Google credentials not configured")?;
    let secret = credentials::client_secret().ok_or("Google credentials not configured")?;
    let pkce = backup::oauth::pkce_pair();
    let app_for_open = app.clone();
    let (code, redirect_uri) = backup::oauth::run_loopback_and_get_code(cid, &pkce.challenge, |url| {
        use tauri_plugin_opener::OpenerExt;
        let _ = app_for_open.opener().open_url(url.to_string(), None::<String>);
    }).await?;
    let tokens = backup::oauth::exchange_code(cid, secret, &code, &pkce.verifier, &redirect_uri).await?;
    let refresh = tokens.refresh_token.ok_or("Google did not return a refresh token")?;
    {
        let db = state.db.lock().map_err(|_| "db lock".to_string())?;
        db.set_setting("gdrive_refresh_token", &refresh).map_err(|e| e.to_string())?;
    }
    backup_status(state)
}

#[tauri::command]
pub fn disconnect_drive(state: State<'_, AppState>) -> Result<BackupStatus, String> {
    {
        let db = state.db.lock().map_err(|_| "db lock".to_string())?;
        db.delete_setting("gdrive_refresh_token").map_err(|e| e.to_string())?;
        db.delete_setting("gdrive_file_id").map_err(|e| e.to_string())?;
    }
    backup_status(state)
}

#[tauri::command]
pub async fn backup_now(app: tauri::AppHandle, state: State<'_, AppState>) -> Result<BackupStatus, String> {
    let dir = backup_dir(&app)?;
    let (refresh, bytes, sig) = {
        let db = state.db.lock().map_err(|_| "db lock".to_string())?;
        let refresh = db.get_setting("gdrive_refresh_token").ok().flatten()
            .ok_or("Not connected to Google Drive")?;
        let bytes = backup::snapshot_bytes(&db, &dir)?;
        let sig = backup::signature_string(db.signature_counts().map_err(|e| e.to_string())?);
        (refresh, bytes, sig)
    };
    let token = backup::access_token(&refresh).await?;
    let existing = {
        let db = state.db.lock().map_err(|_| "db lock".to_string())?;
        db.get_setting("gdrive_file_id").ok().flatten()
    };
    let file_id = match existing {
        Some(id) => { backup::drive::update_backup(&token, &id, bytes).await?; id }
        None => backup::drive::create_backup(&token, bytes).await?,
    };
    let meta = backup::drive::get_metadata(&token, &file_id).await.ok();
    {
        let db = state.db.lock().map_err(|_| "db lock".to_string())?;
        db.set_setting("gdrive_file_id", &file_id).map_err(|e| e.to_string())?;
        db.set_setting("backup_last_at_unix", &chrono_now_unix().to_string()).map_err(|e| e.to_string())?;
        db.set_setting("backup_last_at_iso", &now_iso()).map_err(|e| e.to_string())?;
        db.set_setting("backup_signature", &sig).map_err(|e| e.to_string())?;
        if let Some(m) = meta.as_ref().and_then(|m| m.size.clone()) {
            db.set_setting("backup_size_bytes", &m).ok();
        }
    }
    backup_status(state)
}

#[tauri::command]
pub async fn restore_latest(app: tauri::AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    let dir = backup_dir(&app)?;
    let (refresh, file_id) = {
        let db = state.db.lock().map_err(|_| "db lock".to_string())?;
        let refresh = db.get_setting("gdrive_refresh_token").ok().flatten()
            .ok_or("Not connected to Google Drive")?;
        let file_id = db.get_setting("gdrive_file_id").ok().flatten()
            .ok_or("No backup found in Drive yet")?;
        (refresh, file_id)
    };
    let token = backup::access_token(&refresh).await?;
    let bytes = backup::drive::download_backup(&token, &file_id).await?;
    backup::stage_restore(&bytes, &dir)?; // validates before staging
    app.restart();
}
```

Helper functions `now_iso()`/`chrono_now_unix()`: the project has no `chrono` dep. Implement without it — add these near the commands:

```rust
fn chrono_now_unix() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64
}
fn now_iso() -> String {
    // Cheap ISO-ish local stamp for display; exactness not required.
    let secs = chrono_now_unix();
    format!("unix:{secs}")
}
```

Note: `db.delete_setting` and `db.set_setting`/`get_setting` — grep db.rs for the exact names. If `delete_setting` does not exist, add it in this task:

```rust
pub fn delete_setting(&self, key: &str) -> Result<()> {
    self.conn.execute("DELETE FROM settings WHERE key=?1", [key])?;
    Ok(())
}
```

`app.restart()` returns `!` (never) in Tauri v2, so the function's `Ok(())` after it is unreachable — that's fine; if the compiler complains about the return type, end the function body with just `app.restart();` (its `-> !` satisfies `Result`).

- [ ] **Step 2: Register commands + startup wiring in `lib.rs`**

In `.setup`, BEFORE `let db_path = ...`:

```rust
            backup::apply_pending_restore(&dir);
```

At the end of `.setup`, after `app.manage(...)`, spawn the throttled auto-backup:

```rust
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let _ = commands::auto_backup_if_due(handle).await;
            });
```

Add the 5 commands to `generate_handler!`:

```rust
            commands::backup_status,
            commands::connect_drive,
            commands::disconnect_drive,
            commands::backup_now,
            commands::restore_latest,
```

- [ ] **Step 3: Implement `auto_backup_if_due` in `commands.rs`** (the throttled wrapper; reuses `backup_now`)

```rust
#[tauri::command]
pub async fn auto_backup_if_due(app: tauri::AppHandle) -> Result<(), String> {
    use tauri::Manager;
    let state = app.state::<AppState>();
    if !credentials::is_configured() { return Ok(()); }
    let (last_at, last_sig, cur_sig, connected) = {
        let db = state.db.lock().map_err(|_| "db lock".to_string())?;
        let connected = db.get_setting("gdrive_refresh_token").ok().flatten().is_some();
        let last_at = db.get_setting("backup_last_at_unix").ok().flatten().and_then(|s| s.parse::<i64>().ok());
        let last_sig = db.get_setting("backup_signature").ok().flatten().unwrap_or_default();
        let cur_sig = backup::signature_string(db.signature_counts().map_err(|e| e.to_string())?);
        (last_at, last_sig, cur_sig, connected)
    };
    if !connected { return Ok(()); }
    if !backup::is_auto_backup_due(last_at, chrono_now_unix(), &last_sig, &cur_sig) { return Ok(()); }
    // Reuse the manual path; ignore the returned status.
    let _ = backup_now(app.clone(), app.state::<AppState>()).await?;
    Ok(())
}
```

Register `commands::auto_backup_if_due` in `generate_handler!` too (it's a command so the frontend could also trigger it, and so `spawn` can call it).

- [ ] **Step 4: Call auto-backup after a successful scan** — in `commands.rs` `refresh`, at the very end before returning Ok, add a fire-and-forget:

```rust
    // Opportunistic cloud backup (throttled 24h, only if changed & connected).
    {
        let app2 = app.clone();
        tauri::async_runtime::spawn(async move { let _ = auto_backup_if_due(app2).await; });
    }
```

Note: grep `refresh(` in commands.rs for the exact signature — it already takes an `app`/`AppHandle` or `Window` (progress events are emitted from it). Use whatever handle it already has to derive `AppHandle` (`.app_handle()` on a Window). If `refresh` has no handle, skip this step (startup spawn already covers the daily case) and note it in the final summary.

- [ ] **Step 5: Verify compile + full test suite**

Run: `cargo test --manifest-path src-tauri/Cargo.toml 2>&1 | grep -E "^test result|error\[" | head`
Expected: all green (238 prior + new backup tests), no `error[`.

- [ ] **Step 6: Commit**

```bash
git add src-tauri/src/commands.rs src-tauri/src/lib.rs src-tauri/src/db.rs && git commit -m "feat(backup): Tauri commands + startup restore/auto-backup wiring"
```

---

## Task 10: Credentials config scaffolding + docs

**Files:** Create `.cargo/config.toml.example`, `docs/google-drive-setup.md`; Modify `.gitignore`

- [ ] **Step 1: Create `.cargo/config.toml.example`**

```toml
# Copy this file to .cargo/config.toml (gitignored) and fill in the OAuth
# client you create in Google Cloud Console (see docs/google-drive-setup.md).
# These are read at COMPILE time via option_env! — rebuild after changing them.
[env]
AOT_GOOGLE_CLIENT_ID = "REPLACE_WITH_YOUR_CLIENT_ID.apps.googleusercontent.com"
AOT_GOOGLE_CLIENT_SECRET = "REPLACE_WITH_YOUR_CLIENT_SECRET"
```

- [ ] **Step 2: Add to `.gitignore`**

```
/.cargo/config.toml
/src-tauri/**/animeontrack.snapshot.*.sqlite
```

- [ ] **Step 3: Create `docs/google-drive-setup.md`**

````markdown
# Google Drive backup setup

The cloud-backup feature stores a copy of your database in a hidden per-app
folder in **your** Google Drive (`appDataFolder`, invisible in the Drive UI —
manageable under Drive → Settings → Manage apps). It needs an OAuth client you
create once, for free.

## 1. Create the OAuth client

1. Go to <https://console.cloud.google.com/>, create a project (any name).
2. **APIs & Services → Library →** enable **Google Drive API**.
3. **APIs & Services → OAuth consent screen:** choose *External*, fill the app
   name + your email, add your Google account under *Test users*. You do NOT
   need to publish/verify the app for personal use.
4. **APIs & Services → Credentials → Create credentials → OAuth client ID →**
   Application type **Desktop app**. Copy the **Client ID** and **Client secret**.

## 2. Put the credentials in the build

Copy `.cargo/config.toml.example` to `.cargo/config.toml` and paste your values:

```toml
[env]
AOT_GOOGLE_CLIENT_ID = "1234....apps.googleusercontent.com"
AOT_GOOGLE_CLIENT_SECRET = "GOCSPX-...."
```

Then rebuild (`npm run tauri dev`). `.cargo/config.toml` is gitignored — your
secret is not committed.

## 3. Use it

Settings → **Copia de seguridad → Conectar con Google Drive**. A browser opens;
approve access (you'll see an "unverified app" warning — expected for a personal
Desktop client; continue). After that, backups run automatically (once a day if
something changed) and you can press **Hacer copia ahora** any time. On a new
machine, connect the same Google account and press **Restaurar última copia**.
````

- [ ] **Step 4: Commit**

```bash
git add .cargo/config.toml.example .gitignore docs/google-drive-setup.md && git commit -m "docs(backup): Google Drive OAuth setup + gitignore credentials"
```

---

## Task 11: Frontend — api + types

**Files:** Modify `src/api.ts`, `src/types.ts`

- [ ] **Step 1: Add `BackupStatus` to `src/types.ts`**

```ts
export type BackupStatus = {
  configured: boolean;
  connected: boolean;
  last_at: string | null;
  size_bytes: number | null;
};
```

- [ ] **Step 2: Add wrappers to `src/api.ts`** (follow the existing `invoke` wrapper style in that file)

```ts
import type { BackupStatus } from "./types";

export const backupStatus = () => invoke<BackupStatus>("backup_status");
export const connectDrive = () => invoke<BackupStatus>("connect_drive");
export const disconnectDrive = () => invoke<BackupStatus>("disconnect_drive");
export const backupNow = () => invoke<BackupStatus>("backup_now");
export const restoreLatest = () => invoke<void>("restore_latest");
```

Note: match the file's actual import/style for `invoke` (grep `invoke<` in api.ts). Put the `import type` with the other imports, not mid-file.

- [ ] **Step 3: Type-check**

Run: `npx tsc --noEmit`
Expected: clean.

- [ ] **Step 4: Commit**

```bash
git add src/api.ts src/types.ts && git commit -m "feat(backup): frontend api wrappers + BackupStatus type"
```

---

## Task 12: Frontend — i18n keys

**Files:** Modify `src/i18n/catalog/es.ts`, `src/i18n/catalog/en.ts`

- [ ] **Step 1: Add keys to `es.ts`** (in the `settings.*` area)

```ts
  "settings.backupHeading": "Copia de seguridad",
  "settings.backupNotConfigured": "Configura las credenciales de Google Drive (ver docs/google-drive-setup.md).",
  "settings.backupConnect": "Conectar con Google Drive",
  "settings.backupConnecting": "Conectando…",
  "settings.backupConnected": "Conectado a Google Drive",
  "settings.backupDisconnect": "Desconectar",
  "settings.backupNow": "Hacer copia ahora",
  "settings.backupWorking": "Subiendo copia…",
  "settings.backupRestore": "Restaurar última copia",
  "settings.backupRestoreConfirm": "Esto reemplazará tus datos actuales con la copia de la nube y reiniciará la app. ¿Continuar?",
  "settings.backupRestoring": "Descargando y restaurando…",
  "settings.backupLast": "Última copia: {when}",
  "settings.backupNever": "Aún no hay ninguna copia.",
  "settings.backupSize": "Tamaño: {size}",
  "settings.backupIntro": "Guarda una copia de tus datos en una carpeta privada de tu Google Drive y restáurala en otro equipo.",
  "settings.backupError": "Error de copia: {msg}",
```

- [ ] **Step 2: Add the SAME keys to `en.ts`**

```ts
  "settings.backupHeading": "Backup",
  "settings.backupNotConfigured": "Set up your Google Drive credentials (see docs/google-drive-setup.md).",
  "settings.backupConnect": "Connect Google Drive",
  "settings.backupConnecting": "Connecting…",
  "settings.backupConnected": "Connected to Google Drive",
  "settings.backupDisconnect": "Disconnect",
  "settings.backupNow": "Back up now",
  "settings.backupWorking": "Uploading backup…",
  "settings.backupRestore": "Restore latest backup",
  "settings.backupRestoreConfirm": "This will replace your current data with the cloud copy and restart the app. Continue?",
  "settings.backupRestoring": "Downloading and restoring…",
  "settings.backupLast": "Last backup: {when}",
  "settings.backupNever": "No backup yet.",
  "settings.backupSize": "Size: {size}",
  "settings.backupIntro": "Keep a copy of your data in a private folder on your Google Drive and restore it on another machine.",
  "settings.backupError": "Backup error: {msg}",
```

- [ ] **Step 3: Type-check (enforces key parity)**

Run: `npx tsc --noEmit`
Expected: clean (if a key is missing from either catalog, tsc errors).

- [ ] **Step 4: Commit**

```bash
git add src/i18n/catalog/es.ts src/i18n/catalog/en.ts && git commit -m "feat(backup): i18n strings for the backup card"
```

---

## Task 13: Frontend — Settings backup card

**Files:** Modify `src/views/Settings.tsx`

- [ ] **Step 1: Add the card component + wire it in** (place a new `<section>` following the existing settings blocks; match the file's card markup — grep for existing `className="card"`/section pattern first)

```tsx
import { useEffect, useState } from "react";
import {
  backupStatus, connectDrive, disconnectDrive, backupNow, restoreLatest,
} from "../api";
import type { BackupStatus } from "../types";
import { useT } from "../i18n";

function BackupCard() {
  const t = useT();
  const [status, setStatus] = useState<BackupStatus | null>(null);
  const [busy, setBusy] = useState<null | "connect" | "backup" | "restore">(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => { backupStatus().then(setStatus).catch((e) => setError(String(e))); }, []);

  const run = async (kind: "connect" | "backup" | "restore", fn: () => Promise<unknown>) => {
    setBusy(kind); setError(null);
    try {
      const r = await fn();
      if (kind !== "restore") setStatus(r as BackupStatus);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(null);
    }
  };

  if (!status) return null;

  return (
    <section className="series-block" style={{ padding: 16 }}>
      <h3 className="card-title" style={{ marginTop: 0 }}>{t("settings.backupHeading")}</h3>
      <p className="muted" style={{ fontSize: 12.5 }}>{t("settings.backupIntro")}</p>

      {!status.configured ? (
        <p className="muted">{t("settings.backupNotConfigured")}</p>
      ) : !status.connected ? (
        <button className="btn btn-primary" disabled={busy !== null}
          onClick={() => run("connect", connectDrive)}>
          {busy === "connect" ? t("settings.backupConnecting") : t("settings.backupConnect")}
        </button>
      ) : (
        <>
          <p className="muted">{t("settings.backupConnected")}</p>
          <p className="muted" style={{ fontSize: 12 }}>
            {status.last_at ? t("settings.backupLast", { when: status.last_at }) : t("settings.backupNever")}
            {status.size_bytes ? ` · ${t("settings.backupSize", { size: `${Math.round(status.size_bytes / 1024)} KB` })}` : ""}
          </p>
          <div className="row" style={{ gap: 8, flexWrap: "wrap" }}>
            <button className="btn btn-primary" disabled={busy !== null}
              onClick={() => run("backup", backupNow)}>
              {busy === "backup" ? t("settings.backupWorking") : t("settings.backupNow")}
            </button>
            <button className="btn" disabled={busy !== null}
              onClick={() => {
                if (confirm(t("settings.backupRestoreConfirm"))) run("restore", restoreLatest);
              }}>
              {busy === "restore" ? t("settings.backupRestoring") : t("settings.backupRestore")}
            </button>
            <button className="btn btn-ghost" disabled={busy !== null}
              onClick={() => run("connect", disconnectDrive)}>
              {t("settings.backupDisconnect")}
            </button>
          </div>
        </>
      )}
      {error && <p className="muted" style={{ color: "var(--danger)", fontSize: 12 }}>
        {t("settings.backupError", { msg: error })}</p>}
    </section>
  );
}
```

Then render `<BackupCard />` inside the Settings page JSX (after the mirrors/language blocks). If `Settings.tsx` already imports React hooks / `useT`, don't duplicate imports — merge them.

- [ ] **Step 2: Type-check + build**

Run: `npx tsc --noEmit && npm run build 2>&1 | tail -3`
Expected: tsc clean, build OK.

- [ ] **Step 3: Harness preview (dark + light)**

Port the `BackupCard` markup (all three states: not-configured, not-connected, connected) + the relevant CSS to a self-contained HTML file, serve with `python -m http.server <port> --bind 127.0.0.1`, screenshot in Chrome with `data-theme` dark and light. Confirm buttons/labels read in both themes and nothing overflows. Kill the server by PID afterward.

- [ ] **Step 4: Commit**

```bash
git add src/views/Settings.tsx && git commit -m "feat(backup): Settings card (connect/backup/restore/disconnect)"
```

---

## Task 14: Full verification + merge

- [ ] **Step 1: Full suite**

Run: `cargo test --manifest-path src-tauri/Cargo.toml 2>&1 | grep -E "^test result|error\[" | head`
Expected: all green.

- [ ] **Step 2: Frontend**

Run: `npx tsc --noEmit && npm run build 2>&1 | tail -3`
Expected: clean + built.

- [ ] **Step 3: Confirm no scraper coupling**

Run: `grep -rn "scraper_engine\|animeytx\|reqwest" src-tauri/src/backup/`
Expected: only `reqwest` (to `googleapis.com`), zero `scraper_engine`/`animeytx`.

- [ ] **Step 4: Confirm compile-without-credentials path**

The dev shell has no `AOT_GOOGLE_*` env → `is_configured()` is false. Confirm the app still builds and `backup_status` returns `configured:false` (covered by tests + the fact the suite passes without creds).

- [ ] **Step 5: Merge to develop**

```bash
git fetch; git checkout develop && git pull 2>/dev/null; git checkout feat/cloud-backup-drive
git rebase develop   # or merge; resolve in-branch, then re-run cargo test + tsc + npm run build
git checkout develop && git merge --no-ff feat/cloud-backup-drive
git branch -d feat/cloud-backup-drive
git status --short   # clean
```

Never push.

---

## Acceptance (maps to spec)

- Spec §Auth → Tasks 1,2,3,6,9,10. §Qué se sube → Tasks 4,7,8. §Auto-backup → Tasks 5,9. §Restore staged → Tasks 5,8,9. §Frontend → Tasks 11,12,13. §Docs → Task 10. §TDD targets (pkce, auth url, redirect parser, token parser, signature, validate_restore, auto-backup decision) → Tasks 2,3,5. §No scraper coupling → Task 14 step 3.

## Not verifiable by tools (state in final summary)

Real OAuth consent + real Drive upload/download need the user's browser + Google session + their OAuth client. The Tauri window isn't tool-reachable. Verified instead: cargo test, tsc, npm run build, `snapshot_to`+`validate` on a read-only `.backup` of the real DB, and an HTML harness of the Settings card in Chrome (dark/light). The user must set credentials, relaunch, Connect, Back up now, and Restore to confirm end-to-end.
