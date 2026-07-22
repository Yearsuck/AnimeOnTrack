//! Google OAuth client credentials for the Drive backup.
//!
//! Two sources, in priority order:
//!
//! 1. **Settings**, entered by the user in the backup card and stored in the
//!    `settings` table (`google_client_id` / `google_client_secret`).
//! 2. **Compile time**, injected from `.cargo/config.toml`'s `[env]` block
//!    (gitignored), for a build that ships with its own client.
//!
//! Settings win. An OAuth *Desktop* client's id and secret are not a secret in
//! the usual sense — Google documents them as non-confidential for installed
//! apps, which is exactly why this flow requires PKCE — so keeping them in the
//! local database alongside the refresh token adds no exposure the app didn't
//! already have, and it means a user can set the backup up without a rebuild.
//! With neither source present, the backup card shows a setup form and no
//! command panics.

pub fn built_in_client_id() -> Option<&'static str> {
    option_env!("AOT_GOOGLE_CLIENT_ID").filter(|s| !s.is_empty())
}

pub fn built_in_client_secret() -> Option<&'static str> {
    option_env!("AOT_GOOGLE_CLIENT_SECRET").filter(|s| !s.is_empty())
}

/// The credential pair to use, or `None` when neither source supplies both
/// halves. Blank/whitespace-only stored values are treated as absent, so
/// clearing a field in the UI falls back to the built-in pair rather than
/// authenticating with an empty client id.
pub fn resolve(
    stored_id: Option<String>,
    stored_secret: Option<String>,
) -> Option<(String, String)> {
    let pick = |stored: Option<String>, built_in: Option<&'static str>| {
        stored
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .or_else(|| built_in.map(str::to_string))
    };
    let id = pick(stored_id, built_in_client_id())?;
    let secret = pick(stored_secret, built_in_client_secret())?;
    Some((id, secret))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stored_pair_is_used_as_given() {
        assert_eq!(
            resolve(Some("id".into()), Some("secret".into())),
            Some(("id".to_string(), "secret".to_string()))
        );
    }

    #[test]
    fn stored_values_are_trimmed() {
        assert_eq!(
            resolve(Some("  id  ".into()), Some("\tsecret\n".into())),
            Some(("id".to_string(), "secret".to_string()))
        );
    }

    #[test]
    fn blank_stored_value_does_not_count_as_configured() {
        // With no compile-time pair (the CI/dev case) a blank field must read
        // as "not configured", never as an empty-string client id.
        if built_in_client_id().is_none() {
            assert_eq!(resolve(Some("   ".into()), Some("secret".into())), None);
            assert_eq!(resolve(Some("id".into()), Some("".into())), None);
        }
    }

    #[test]
    fn half_a_pair_is_not_configured() {
        if built_in_client_secret().is_none() {
            assert_eq!(resolve(Some("id".into()), None), None);
        }
        if built_in_client_id().is_none() {
            assert_eq!(resolve(None, Some("secret".into())), None);
        }
    }

    #[test]
    fn resolve_is_total_with_nothing_configured() {
        // Must not panic either way; the result depends on the build's env.
        let _ = resolve(None, None);
    }
}
