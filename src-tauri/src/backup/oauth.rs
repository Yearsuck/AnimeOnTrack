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
