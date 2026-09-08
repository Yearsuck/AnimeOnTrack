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

/// Percent-decode one query-string value.
///
/// The authorization code Google hands back routinely contains `/` (codes look
/// like `4/0Ab...`), which arrives in the redirect URL as `%2F`. Handing that
/// raw to `exchange_code` — which posts it with `reqwest`'s `.form()`, i.e.
/// percent-encodes it *again* — turns the literal `%` into `%25` and Google
/// rejects the corrupted code with an opaque `invalid_grant`. Decoding an
/// already-unencoded code is a no-op (nothing in an unencoded code is a `%`
/// escape), so this is safe to do unconditionally.
///
/// `urlencoding::decode` only fails when the decoded bytes aren't valid UTF-8;
/// an OAuth code is ASCII, so falling back to the raw value there is the
/// conservative no-op rather than a reason to drop the code.
fn percent_decode(value: &str) -> String {
    urlencoding::decode(value).map(|c| c.into_owned()).unwrap_or_else(|_| value.to_string())
}

/// Parse the first request line of the loopback redirect. Extracts `code`,
/// `state` or `error` from the query string. A pair without `=` (e.g. a bare
/// flag param) is skipped rather than aborting the whole parse — Google's
/// real redirect only ever sends `code`/`state`/`scope`/`error`, but a `code`
/// that arrived before some hypothetical future malformed param must not be
/// discarded along with it.
///
/// Every extracted value is percent-decoded (see `percent_decode`). `state` is
/// base64url, so decoding it is always a no-op, but decoding all three keeps
/// one rule instead of a per-key exception.
pub fn parse_redirect_line(line: &str) -> Option<RedirectResult> {
    let path = line.split_whitespace().nth(1)?; // "/?code=..."
    let query = path.split_once('?')?.1;
    let mut code = None;
    let mut state = None;
    let mut error = None;
    for pair in query.split('&') {
        let Some((k, v)) = pair.split_once('=') else { continue };
        match k {
            "code" => code = Some(percent_decode(v)),
            "state" => state = Some(percent_decode(v)),
            "error" => error = Some(percent_decode(v)),
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

/// Google's OAuth error body shape (RFC 6749 §5.2): `{"error":"invalid_grant",
/// "error_description":"Token has been expired or revoked."}`.
#[derive(Debug, serde::Deserialize)]
struct TokenErrorBody {
    error: String,
    #[serde(default)]
    error_description: Option<String>,
}

/// Marker embedded in the error string when Google rejected the *grant* — the
/// stored refresh token (or the just-issued authorization code) is dead and no
/// retry will ever fix it; only reconnecting will. Callers detect it with
/// [`is_grant_rejected`] rather than string-matching Google's wording, which
/// isn't a stable API.
const GRANT_REJECTED_TAG: &str = "[reconnect-required]";

/// True when `err` came from a token request Google rejected because the grant
/// itself is expired/revoked. The caller's cue to drop the stored refresh
/// token so the UI stops claiming "connected" and offers the reconnect flow.
pub fn is_grant_rejected(err: &str) -> bool {
    err.contains(GRANT_REJECTED_TAG)
}

/// Interpret one response from Google's token endpoint.
///
/// Both token calls used to hand the raw body straight to
/// `parse_token_response`, so an HTTP 400 `{"error":"invalid_grant"}` — the
/// answer to a revoked or expired refresh token, the single most likely way
/// this breaks in the field — surfaced as `token parse: missing field
/// access_token`, indistinguishable from a truncated response or an API
/// change. Checking the body for an `error` field first (and only then the
/// status, since a non-2xx without a recognisable error body is still a
/// failure) makes "reconnect Google Drive" separable from "something else went
/// wrong, retry later".
pub fn parse_token_body(status: u16, body: &str) -> Result<TokenSet, String> {
    // A success body has no `error` field, so this only matches real errors.
    if let Ok(err) = serde_json::from_str::<TokenErrorBody>(body) {
        let detail = err
            .error_description
            .filter(|d| !d.is_empty())
            .map(|d| format!(": {d}"))
            .unwrap_or_default();
        return Err(match err.error.as_str() {
            "invalid_grant" => format!(
                "{GRANT_REJECTED_TAG} Google rejected the saved Drive authorization \
                 (invalid_grant{detail}). Reconnect Google Drive in Ajustes."
            ),
            other => format!("Google rejected the token request ({other}{detail})"),
        });
    }
    if !(200..300).contains(&status) {
        return Err(format!("token endpoint returned HTTP {status}: {}", snippet(body)));
    }
    parse_token_response(body)
}

/// First ~200 *characters* of a response body, for an error message. Sliced by
/// chars, never by bytes — a byte slice through a multi-byte character panics.
fn snippet(body: &str) -> String {
    body.chars().take(200).collect()
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
    let status = resp.status().as_u16();
    let text = resp.text().await.map_err(|e| format!("token body: {e}"))?;
    parse_token_body(status, &text)
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
    let status = resp.status().as_u16();
    let text = resp.text().await.map_err(|e| format!("refresh body: {e}"))?;
    Ok(parse_token_body(status, &text)?.access_token)
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

    /// A `/` in an authorization code arrives percent-encoded. Passing `%2F`
    /// through untouched let `reqwest`'s `.form()` re-encode the `%` to `%25`,
    /// corrupting the code and failing the exchange with an opaque error.
    #[test]
    fn parse_redirect_percent_decodes_the_code() {
        let line = "GET /?state=xyz&code=4%2F0AbC%2DdEf%2Bgh%3D%3D&scope=drive.appdata HTTP/1.1";
        assert_eq!(
            parse_redirect_line(line),
            Some(RedirectResult::Code { code: "4/0AbC-dEf+gh==".into(), state: Some("xyz".into()) })
        );
    }

    /// Decoding a code that was never encoded must not change it — that's what
    /// makes decoding safe to do unconditionally.
    #[test]
    fn parse_redirect_leaves_an_unencoded_code_alone() {
        let line = "GET /?code=4/0AbC-dEf_gh HTTP/1.1";
        assert_eq!(
            parse_redirect_line(line),
            Some(RedirectResult::Code { code: "4/0AbC-dEf_gh".into(), state: None })
        );
    }

    /// The bug this replaced: Google answers a revoked/expired refresh grant
    /// with HTTP 400 + `{"error":"invalid_grant"}`, which fell through to
    /// `parse_token_response` and surfaced as `missing field access_token` —
    /// unreadable, and indistinguishable from a truncated body.
    #[test]
    fn invalid_grant_is_reported_as_needing_a_reconnect() {
        let body = r#"{"error":"invalid_grant","error_description":"Token has been expired or revoked."}"#;
        let err = parse_token_body(400, body).unwrap_err();
        assert!(is_grant_rejected(&err), "invalid_grant must be flagged for reconnect: {err}");
        assert!(err.contains("invalid_grant"), "{err}");
        assert!(err.contains("Token has been expired or revoked."), "{err}");
        assert!(!err.contains("missing field"), "must not leak the old parse error: {err}");
    }

    /// Everything else that can go wrong must stay *distinct* from the
    /// reconnect case, or the token gets thrown away on a transient blip.
    #[test]
    fn other_failures_are_not_mistaken_for_a_rejected_grant() {
        // A different OAuth error code (client misconfiguration, not a dead grant).
        let other = parse_token_body(401, r#"{"error":"invalid_client"}"#).unwrap_err();
        assert!(!is_grant_rejected(&other), "{other}");
        assert!(other.contains("invalid_client"), "{other}");

        // A non-JSON 5xx (Google's LB, a captive portal): still an error, still
        // not a reason to drop the refresh token, and the status is surfaced.
        let server = parse_token_body(503, "<html>Service Unavailable</html>").unwrap_err();
        assert!(!is_grant_rejected(&server), "{server}");
        assert!(server.contains("503"), "{server}");

        // A 200 whose body isn't a token at all — a genuine parse failure.
        let parse = parse_token_body(200, "{\"unexpected\":1}").unwrap_err();
        assert!(!is_grant_rejected(&parse), "{parse}");
        assert!(parse.contains("token parse"), "{parse}");
    }

    #[test]
    fn parse_token_body_still_reads_a_normal_success() {
        let json = r#"{"access_token":"ya29.x","refresh_token":"1//rt","expires_in":3599}"#;
        let t = parse_token_body(200, json).unwrap();
        assert_eq!(t.access_token, "ya29.x");
        assert_eq!(t.refresh_token.as_deref(), Some("1//rt"));
    }

    /// Error messages get shown in Settings; a multi-byte body must not panic
    /// the truncation (the `&s[..n]` char-boundary class of crash).
    #[test]
    fn a_multibyte_error_body_is_truncated_without_panicking() {
        let body = "日".repeat(500);
        let err = parse_token_body(500, &body).unwrap_err();
        assert!(err.contains("500"));
    }
}
