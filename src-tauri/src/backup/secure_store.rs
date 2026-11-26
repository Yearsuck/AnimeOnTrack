#[cfg(any(windows, target_os = "android"))]
use base64::Engine;

const DPAPI_PREFIX: &str = "dpapi1:";
const AKS_PREFIX: &str = "aks1:";
#[allow(dead_code)]
const PREFIX: &str = DPAPI_PREFIX;

/// Encrypt `plain` for at-rest storage in the `settings` table.
///
/// On Windows, uses DPAPI (current-user scope, no extra entropy) with prefix
/// `dpapi1:` so the stored ciphertext is only usable by the same Windows user
/// account on the same machine.
///
/// On Android, uses AES-256-GCM with prefix `aks1:`, keyed by a random data key
/// unwrapped from the Android Keystore at startup. If the data key has not yet
/// been installed, waits up to 5 s for it before falling back.
///
/// Falls back to storing the value unprotected if encryption fails or on
/// unsupported platforms (e.g. macOS/Linux CI builds).
pub fn protect(plain: &str) -> String {
    #[cfg(windows)]
    {
        if let Some(cipher) = win::protect(plain.as_bytes()) {
            return format!("{DPAPI_PREFIX}{}", base64::engine::general_purpose::STANDARD.encode(cipher));
        }
    }
    #[cfg(target_os = "android")]
    {
        if let Some(key) = get_data_key_or_wait() {
            if let Some(sealed) = aks::seal(key, plain) {
                return sealed;
            }
            eprintln!("[secure_store] failed to seal secret with data key");
        }
    }
    plain.to_string()
}

/// Reverse of `protect`.
///
/// Values with prefix `dpapi1:` are decrypted via Windows DPAPI on Windows; on
/// other platforms they return an empty string.
///
/// Values with prefix `aks1:` are decrypted via AES-256-GCM using the Android
/// Keystore data key on Android; on other platforms they return an empty string.
///
/// Values with no known prefix are returned as-is — this provides backward
/// compatibility for legacy plaintext secrets stored before encryption was
/// introduced.
///
/// Any prefixed ciphertext that fails decryption (wrong user/keystore,
/// corrupted bytes, tampered payload) returns an empty string rather than the
/// raw ciphertext, so callers treat it as disconnected rather than corrupted.
pub fn unprotect(stored: &str) -> String {
    if let Some(b64) = stored.strip_prefix(DPAPI_PREFIX) {
        #[cfg(windows)]
        {
            if let Ok(cipher) = base64::engine::general_purpose::STANDARD.decode(b64) {
                if let Some(plain) = win::unprotect(&cipher) {
                    if let Ok(s) = String::from_utf8(plain) {
                        return s;
                    }
                }
            }
        }
        #[cfg(not(windows))]
        let _ = b64;
        return String::new();
    }

    if stored.starts_with(AKS_PREFIX) {
        #[cfg(target_os = "android")]
        {
            if let Some(key) = get_data_key_or_wait() {
                if let Some(plain) = aks::open(key, stored) {
                    return plain;
                }
            }
        }
        return String::new();
    }

    stored.to_string()
}

/// True when `stored` is non-empty legacy plain text (neither `dpapi1:` nor
/// `aks1:`) on a platform that is currently able to protect it (Windows, or
/// Android with the data key installed). Only Android's startup upgrade calls it;
/// the tests pin the rule on every host.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub fn needs_upgrade(stored: &str) -> bool {
    let legacy = !(stored.is_empty() || stored.starts_with(DPAPI_PREFIX) || stored.starts_with(AKS_PREFIX));
    #[cfg(target_os = "android")]
    let can_protect = DATA_KEY.get().is_some();
    #[cfg(not(target_os = "android"))]
    let can_protect = cfg!(windows);
    legacy && can_protect
}

#[cfg(target_os = "android")]
static DATA_KEY: std::sync::OnceLock<[u8; 32]> = std::sync::OnceLock::new();
/// Set when startup could not get the key (or a waiter already gave up once).
#[cfg(target_os = "android")]
static DATA_KEY_FAILED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Startup could not fetch the data key: stop making callers wait for it.
#[cfg(target_os = "android")]
pub fn mark_data_key_unavailable() {
    DATA_KEY_FAILED.store(true, std::sync::atomic::Ordering::Relaxed);
}

#[cfg(target_os = "android")]
pub fn install_data_key(b64: &str) -> Result<(), String> {
    if DATA_KEY.get().is_some() {
        return Ok(());
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(b64.trim())
        .map_err(|e| format!("invalid base64 data key: {e}"))?;
    let key: [u8; 32] = bytes
        .try_into()
        .map_err(|v: Vec<u8>| format!("expected 32-byte data key, got {}", v.len()))?;
    let _ = DATA_KEY.set(key);
    Ok(())
}

#[cfg(target_os = "android")]
fn get_data_key_or_wait() -> Option<&'static [u8; 32]> {
    use std::sync::atomic::Ordering;
    if let Some(key) = DATA_KEY.get() {
        return Some(key);
    }
    // Callers hold the DB mutex: once startup has definitely failed to get the key,
    // answer at once instead of stalling every caller for 5 s.
    for _ in 0..100 {
        if DATA_KEY_FAILED.load(Ordering::Relaxed) {
            return None;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
        if let Some(key) = DATA_KEY.get() {
            return Some(key);
        }
    }
    DATA_KEY_FAILED.store(true, Ordering::Relaxed);
    eprintln!("[secure_store] data key not available after waiting 5 s; falling back to unencrypted storage");
    None
}

#[cfg(any(test, target_os = "android"))]
pub mod aks {
    use base64::Engine;
    use ring::aead::{Aad, LessSafeKey, Nonce, UnboundKey, AES_256_GCM};

    pub const PREFIX: &str = "aks1:";
    const NONCE_LEN: usize = 12;
    const TAG_LEN: usize = 16;

    pub fn seal(key: &[u8; 32], plain: &str) -> Option<String> {
        let unbound = UnboundKey::new(&AES_256_GCM, key).ok()?;
        let less_safe = LessSafeKey::new(unbound);

        let mut nonce_bytes = [0u8; NONCE_LEN];
        getrandom::fill(&mut nonce_bytes).ok()?;
        let nonce = Nonce::try_assume_unique_for_key(&nonce_bytes).ok()?;

        let mut payload = Vec::with_capacity(NONCE_LEN + plain.len() + TAG_LEN);
        payload.extend_from_slice(&nonce_bytes);
        payload.extend_from_slice(plain.as_bytes());

        let tag = less_safe
            .seal_in_place_separate_tag(nonce, Aad::empty(), &mut payload[NONCE_LEN..])
            .ok()?;
        payload.extend_from_slice(tag.as_ref());

        let b64 = base64::engine::general_purpose::STANDARD.encode(&payload);
        Some(format!("{PREFIX}{b64}"))
    }

    pub fn open(key: &[u8; 32], stored: &str) -> Option<String> {
        let b64 = stored.strip_prefix(PREFIX)?;
        let mut raw = base64::engine::general_purpose::STANDARD.decode(b64).ok()?;
        if raw.len() < NONCE_LEN + TAG_LEN {
            return None;
        }

        let unbound = UnboundKey::new(&AES_256_GCM, key).ok()?;
        let less_safe = LessSafeKey::new(unbound);

        let (nonce_bytes, ciphertext_and_tag) = raw.split_at_mut(NONCE_LEN);
        let nonce = Nonce::try_assume_unique_for_key(nonce_bytes).ok()?;

        let plain_bytes = less_safe
            .open_in_place(nonce, Aad::empty(), ciphertext_and_tag)
            .ok()?;
        std::str::from_utf8(plain_bytes).ok().map(|s| s.to_string())
    }
}

#[cfg(windows)]
mod win {
    use windows::Win32::Foundation::LocalFree;
    use windows::Win32::Security::Cryptography::{
        CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };

    pub fn protect(plain: &[u8]) -> Option<Vec<u8>> {
        unsafe {
            let input = CRYPT_INTEGER_BLOB { cbData: plain.len() as u32, pbData: plain.as_ptr() as *mut u8 };
            let mut output = CRYPT_INTEGER_BLOB::default();
            CryptProtectData(&input, None, None, None, None, CRYPTPROTECT_UI_FORBIDDEN, &mut output).ok()?;
            let bytes = std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
            let _ = LocalFree(Some(windows::Win32::Foundation::HLOCAL(output.pbData as *mut core::ffi::c_void)));
            Some(bytes)
        }
    }

    pub fn unprotect(cipher: &[u8]) -> Option<Vec<u8>> {
        unsafe {
            let input = CRYPT_INTEGER_BLOB { cbData: cipher.len() as u32, pbData: cipher.as_ptr() as *mut u8 };
            let mut output = CRYPT_INTEGER_BLOB::default();
            CryptUnprotectData(&input, None, None, None, None, CRYPTPROTECT_UI_FORBIDDEN, &mut output).ok()?;
            let bytes = std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
            let _ = LocalFree(Some(windows::Win32::Foundation::HLOCAL(output.pbData as *mut core::ffi::c_void)));
            Some(bytes)
        }
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn protect_then_unprotect_round_trips() {
        let plain = "1//09-super-secret-refresh-token";
        let stored = protect(plain);
        assert!(stored.starts_with(PREFIX));
        assert_eq!(unprotect(&stored), plain);
    }

    #[test]
    fn unprotect_passes_through_legacy_plaintext_unchanged() {
        // Pre-encryption installs have raw plaintext sitting in the settings
        // table with no prefix — must keep reading back correctly.
        assert_eq!(unprotect("1//legacy-plaintext-token"), "1//legacy-plaintext-token");
    }

    #[test]
    fn unprotect_returns_empty_for_corrupted_ciphertext() {
        assert_eq!(unprotect("dpapi1:not-valid-base64!!!"), "");
    }
}

#[cfg(test)]
mod aks_tests {
    use super::*;
    use base64::Engine;

    const TEST_KEY: [u8; 32] = [
        0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08,
        0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f, 0x10,
        0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18,
        0x19, 0x1a, 0x1b, 0x1c, 0x1d, 0x1e, 0x1f, 0x20,
    ];

    #[test]
    fn aks_open_sealed_round_trips() {
        let plain = "1//token";
        let sealed = aks::seal(&TEST_KEY, plain).expect("seal should succeed");
        assert_eq!(aks::open(&TEST_KEY, &sealed), Some(plain.to_string()));
    }

    #[test]
    fn two_seals_of_same_text_differ_and_both_open() {
        let plain = "1//token-same-text";
        let sealed1 = aks::seal(&TEST_KEY, plain).expect("seal 1 should succeed");
        let sealed2 = aks::seal(&TEST_KEY, plain).expect("seal 2 should succeed");
        assert_ne!(sealed1, sealed2, "two seals must differ due to random nonce");
        assert_eq!(aks::open(&TEST_KEY, &sealed1), Some(plain.to_string()));
        assert_eq!(aks::open(&TEST_KEY, &sealed2), Some(plain.to_string()));
    }

    #[test]
    fn aks_open_with_wrong_key_fails() {
        let sealed = aks::seal(&TEST_KEY, "1//token").expect("seal should succeed");
        let other_key: [u8; 32] = [0x99; 32];
        assert_eq!(aks::open(&other_key, &sealed), None);
    }

    #[test]
    fn aks_flipping_one_byte_of_payload_fails() {
        let sealed = aks::seal(&TEST_KEY, "1//token").expect("seal should succeed");
        let b64 = sealed.strip_prefix(AKS_PREFIX).expect("has prefix");
        let mut raw = base64::engine::general_purpose::STANDARD
            .decode(b64)
            .expect("valid base64");
        raw[15] ^= 0x01;
        let tampered_b64 = base64::engine::general_purpose::STANDARD.encode(&raw);
        let tampered = format!("{AKS_PREFIX}{tampered_b64}");
        assert_eq!(aks::open(&TEST_KEY, &tampered), None);
    }

    #[test]
    fn aks_invalid_format_and_missing_prefix_fails() {
        assert_eq!(aks::open(&TEST_KEY, "aks1:!!!"), None);
        assert_eq!(aks::open(&TEST_KEY, "aks1:"), None);
        assert_eq!(aks::open(&TEST_KEY, "1//token"), None);
        assert_eq!(aks::open(&TEST_KEY, "dpapi1:different-prefix"), None);
    }

    #[test]
    fn unprotect_legacy_plaintext_on_any_platform() {
        assert_eq!(unprotect("plain-legacy"), "plain-legacy");
    }

    #[test]
    #[cfg(not(target_os = "android"))]
    fn unprotect_aks_on_non_android_hosts_returns_empty() {
        assert_eq!(unprotect("aks1:anything"), "");
        assert_eq!(unprotect("aks1:"), "");
    }

    #[test]
    fn needs_upgrade_behavior() {
        assert!(!needs_upgrade(""), "empty string never needs upgrade");
        assert!(!needs_upgrade("dpapi1:already-protected"), "dpapi1 does not need upgrade");
        assert!(!needs_upgrade("aks1:already-protected"), "aks1 does not need upgrade");
        #[cfg(windows)]
        {
            assert!(needs_upgrade("legacy-secret"), "legacy secret on Windows needs upgrade");
        }
    }
}
