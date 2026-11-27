//! Typed application error (branch-by-abstraction foundation).
//!
//! Commands still return `Result<_, String>` so the frontend is unchanged;
//! `From<AppError> for String` lets them use `?` on an `AppError` meanwhile.

use std::fmt;

#[derive(Debug)]
pub enum AppError {
    NotFound(String),
    Db(rusqlite::Error),
    Network { message: String, retryable: bool },
    Parse(String),
    Config(String),
    Internal(String),
    /// An error from the anyhow-based `Db` layer, shown exactly as the old
    /// `e.to_string()` so command error texts do not change.
    Other(String),
}

impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AppError::NotFound(m) => write!(f, "not found: {m}"),
            // Same text as the old `e.to_string()` so command errors are unchanged.
            AppError::Db(e) => write!(f, "{e}"),
            AppError::Network { message, .. } => write!(f, "network error: {message}"),
            AppError::Parse(m) => write!(f, "parse error: {m}"),
            AppError::Config(m) => write!(f, "config error: {m}"),
            AppError::Internal(m) => write!(f, "internal error: {m}"),
            AppError::Other(m) => write!(f, "{m}"),
        }
    }
}

impl std::error::Error for AppError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            AppError::Db(e) => Some(e),
            _ => None,
        }
    }
}

impl From<rusqlite::Error> for AppError {
    fn from(e: rusqlite::Error) -> Self {
        AppError::Db(e)
    }
}

impl From<anyhow::Error> for AppError {
    fn from(e: anyhow::Error) -> Self {
        AppError::Other(e.to_string())
    }
}

impl From<AppError> for String {
    fn from(e: AppError) -> Self {
        e.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_variants() {
        assert_eq!(AppError::NotFound("x".into()).to_string(), "not found: x");
        assert_eq!(
            AppError::Network { message: "down".into(), retryable: true }.to_string(),
            "network error: down"
        );
        assert_eq!(AppError::Parse("p".into()).to_string(), "parse error: p");
        assert_eq!(AppError::Config("c".into()).to_string(), "config error: c");
    }

    #[test]
    fn from_rusqlite() {
        let e: AppError = rusqlite::Error::QueryReturnedNoRows.into();
        assert!(matches!(e, AppError::Db(_)));
        assert_eq!(e.to_string(), rusqlite::Error::QueryReturnedNoRows.to_string());
    }

    #[test]
    fn from_anyhow_keeps_the_old_text() {
        let src = anyhow::anyhow!("boom").context("outer");
        let old = src.to_string();
        let e: AppError = src.into();
        assert!(matches!(&e, AppError::Other(_)));
        assert_eq!(e.to_string(), old);
    }

    #[test]
    fn into_string_for_question_mark() {
        fn f() -> Result<(), String> {
            Err(AppError::NotFound("y".into()))?;
            Ok(())
        }
        assert_eq!(f().unwrap_err(), "not found: y");
    }
}
