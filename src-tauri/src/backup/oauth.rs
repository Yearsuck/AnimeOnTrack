use base64::Engine;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

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

/// A random `state` value for the OAuth authorization request — same 32
/// random bytes / base64url shape as the PKCE verifier, just a distinct
/// value so a leftover `state` can never accidentally equal a leftover PKCE
/// verifier.
pub fn random_state() -> String {
    let mut bytes = [0u8; 32];
    getrandom_bytes(&mut bytes);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// Fill `buf` with OS-provided randomness (the `getrandom` crate — on
/// Windows this calls `BCryptGenRandom`). A hand-rolled PRNG seeded from the
/// system clock used to live here — its entropy was bounded by clock
/// resolution (an attacker who can bound when the OAuth flow started can
/// narrow the seed space), which defeats the point of PKCE/state values
/// whose entire job is to be unguessable.
fn getrandom_bytes(buf: &mut [u8]) {
    getrandom::fill(buf).expect("OS RNG must succeed for a security-sensitive nonce");
}

pub fn build_auth_url(client_id: &str, redirect_uri: &str, challenge: &str, state: &str) -> String {
    format!(
        "{AUTH_ENDPOINT}?client_id={cid}&redirect_uri={ruri}&response_type=code\
&scope={scope}&code_challenge={chal}&code_challenge_method=S256\
&access_type=offline&prompt=consent&state={st}",
        cid = urlencoding::encode(client_id),
        ruri = urlencoding::encode(redirect_uri),
        scope = urlencoding::encode(SCOPE),
        chal = urlencoding::encode(challenge),
        st = urlencoding::encode(state),
    )
}

#[derive(Debug, PartialEq)]
pub enum RedirectResult {
    Code { code: String, state: Option<String> },
    Error(String),
}

/// Parse the first request line of the loopback redirect. Extracts `code`,
/// `state` or `error` from the query string. A pair without `=` (e.g. a bare
/// flag param) is skipped rather than aborting the whole parse — Google's
/// real redirect only ever sends `code`/`state`/`scope`/`error`, but a `code`
/// that arrived before some hypothetical future malformed param must not be
/// discarded along with it.
pub fn parse_redirect_line(line: &str) -> Option<RedirectResult> {
    let path = line.split_whitespace().nth(1)?; // "/?code=..."
    let query = path.split_once('?')?.1;
    let mut code = None;
    let mut state = None;
    let mut error = None;
    for pair in query.split('&') {
        let Some((k, v)) = pair.split_once('=') else { continue };
        match k {
            "code" => code = Some(v.to_string()),
            "state" => state = Some(v.to_string()),
            "error" => error = Some(v.to_string()),
            _ => {}
        }
    }
    if let Some(e) = error { return Some(RedirectResult::Error(e)); }
    code.map(|code| RedirectResult::Code { code, state })
}

#[derive(Debug, serde::Deserialize)]
pub struct TokenSet {
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: Option<String>,
    // Google returns this; we refresh on demand rather than tracking expiry,
    // so it's deserialized but unused.
    #[serde(default)]
    #[allow(dead_code)]
    pub expires_in: i64,
}

pub fn parse_token_response(json: &str) -> Result<TokenSet, String> {
    serde_json::from_str(json).map_err(|e| format!("token parse: {e}"))
}

const REDIRECT_HTML: &str = "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\n\r\n\
<html><body style='font-family:sans-serif;background:#0d1117;color:#e6edf3'>\
<h2>AnimeOnTrack</h2><p>Autenticación completada. Puedes cerrar esta pestaña.</p></body></html>";

/// Bind a loopback listener, open the system browser to Google's consent
/// screen, and block until Google redirects back with `?code=`. Returns the
/// authorization code and the `redirect_uri` actually used (needed verbatim
/// in the token exchange).
///
/// Generates and checks a random `state` value (RFC 6749 §10.12): without
/// it, any local process that wins the race to connect to the ephemeral
/// loopback port before the real browser redirect arrives could hand this
/// function a `code` of its own choosing. `state` doesn't stop that
/// connection from being accepted (a single-shot loopback listener taking
/// whoever connects first is inherent to this flow), but it does mean a
/// connection carrying the wrong `state` — or none — is rejected instead of
/// silently accepted as if it were Google's redirect.
pub async fn run_loopback_and_get_code<F: Fn(&str)>(
    client_id: &str,
    challenge: &str,
    open_browser: F,
) -> Result<(String, String), String> {
    let listener = TcpListener::bind("127.0.0.1:0").await.map_err(|e| format!("bind: {e}"))?;
    let port = listener.local_addr().map_err(|e| format!("addr: {e}"))?.port();
    let redirect_uri = format!("http://127.0.0.1:{port}");
    let expected_state = random_state();
    open_browser(&build_auth_url(client_id, &redirect_uri, challenge, &expected_state));

    let (mut stream, _) = listener.accept().await.map_err(|e| format!("accept: {e}"))?;
    let mut buf = [0u8; 2048];
    let n = stream.read(&mut buf).await.map_err(|e| format!("read: {e}"))?;
    let request = String::from_utf8_lossy(&buf[..n]);
    let first_line = request.lines().next().unwrap_or("");
    let result = parse_redirect_line(first_line);
    stream.write_all(REDIRECT_HTML.as_bytes()).await.ok();
    stream.flush().await.ok();

    match result {
        Some(RedirectResult::Code { code, state }) => {
            if state.as_deref() != Some(expected_state.as_str()) {
                return Err("redirect state mismatch — rejecting a possibly forged authorization code".into());
            }
            Ok((code, redirect_uri))
        }
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
        let url = build_auth_url("cid.apps.googleusercontent.com", "http://127.0.0.1:5000", "CHAL", "STATE1");
        assert!(url.starts_with(AUTH_ENDPOINT));
        assert!(url.contains("client_id=cid.apps.googleusercontent.com"));
        assert!(url.contains("code_challenge=CHAL"));
        assert!(url.contains("code_challenge_method=S256"));
        assert!(url.contains("response_type=code"));
        assert!(url.contains("access_type=offline"));
        assert!(url.contains("scope=https%3A%2F%2Fwww.googleapis.com%2Fauth%2Fdrive.appdata"));
        assert!(url.contains("redirect_uri=http%3A%2F%2F127.0.0.1%3A5000"));
        assert!(url.contains("state=STATE1"));
    }

    #[test]
    fn random_state_is_url_safe_and_unpredictable_across_calls() {
        let a = random_state();
        let b = random_state();
        assert!(a.len() >= 43);
        assert_ne!(a, b);
    }

    #[test]
    fn parse_redirect_extracts_code_and_state() {
        let line = "GET /?state=xyz&code=4/abcDEF&scope=https://www.googleapis.com/auth/drive.appdata HTTP/1.1";
        assert_eq!(
            parse_redirect_line(line),
            Some(RedirectResult::Code { code: "4/abcDEF".into(), state: Some("xyz".into()) })
        );
    }

    #[test]
    fn parse_redirect_extracts_code_without_state() {
        let line = "GET /?code=4/abcDEF HTTP/1.1";
        assert_eq!(
            parse_redirect_line(line),
            Some(RedirectResult::Code { code: "4/abcDEF".into(), state: None })
        );
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

    /// A malformed query pair (no `=`) must not discard a `code` that arrived
    /// alongside it — the old implementation used `?` on every pair, so one
    /// bad segment anywhere in the query silently dropped the whole parse.
    #[test]
    fn parse_redirect_skips_a_malformed_pair_without_losing_the_code() {
        let line = "GET /?bareflag&code=4/abcDEF&state=xyz HTTP/1.1";
        assert_eq!(
            parse_redirect_line(line),
            Some(RedirectResult::Code { code: "4/abcDEF".into(), state: Some("xyz".into()) })
        );
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
