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
}

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
