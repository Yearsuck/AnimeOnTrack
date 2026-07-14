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
