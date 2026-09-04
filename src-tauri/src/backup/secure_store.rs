use base64::Engine;

const PREFIX: &str = "dpapi1:";

/// Encrypt `plain` for at-rest storage in the `settings` table, using Windows
/// DPAPI (current-user scope, no extra entropy) so the stored ciphertext is
/// only usable by the same Windows user account on the same machine — unlike
/// the plain text this replaces, reading the `.sqlite` file alone (a copy, a
/// backup, another user on the same PC) is no longer enough to extract the
/// Google OAuth client secret or refresh token.
///
/// Falls back to storing the value unprotected if DPAPI itself fails for any
/// reason, and on non-Windows targets (the whole scraper is Windows-only —
/// see CLAUDE.md — so this only ever needs to satisfy `cargo build`/`test`
/// on the CI matrix's mac/linux legs, not protect real user data there).
pub fn protect(plain: &str) -> String {
    #[cfg(windows)]
    {
        if let Some(cipher) = win::protect(plain.as_bytes()) {
            return format!("{PREFIX}{}", base64::engine::general_purpose::STANDARD.encode(cipher));
        }
    }
    plain.to_string()
}

/// Reverse of `protect`. A value with no `dpapi1:` prefix is returned as-is —
/// this is what makes the change backward compatible: an existing install's
/// already-stored plaintext token still reads back correctly (there is
/// nothing to migrate up front), and the very next `protect()` call on that
/// same setting key upgrades it in place. A prefixed value that fails to
/// decrypt (wrong Windows user, corrupted bytes) returns an empty string
/// rather than the raw ciphertext, so callers see "not connected" instead of
/// garbage.
pub fn unprotect(stored: &str) -> String {
    let Some(b64) = stored.strip_prefix(PREFIX) else { return stored.to_string() };
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
    String::new()
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
