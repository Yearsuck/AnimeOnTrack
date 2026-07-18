//! Genre vocabulary normalization.
//!
//! The scraped site's genre tags are in Spanish (`series_genres`, e.g.
//! `Fantasía`, `Acción`); AniList's are a closed, fixed English vocabulary
//! (`anilist_catalog_genres`). Before this module existed, `get_genre_affinity`
//! grouped by the raw string, so `Acción` (site) and `Action` (catalog) scored
//! as two unrelated genres — splitting the user's taste signal in half across
//! sources. `canonical_genre` maps known Spanish (and already-canonical
//! English) spellings onto AniList's 19-value vocabulary; anything with no
//! AniList equivalent (site-only tags like `Shounen`, `Isekai`,
//! `Reencarnación`, `Escolar`, `Seinen`, `Josei`) returns `None` so callers can
//! keep it under its own raw name instead of losing the signal entirely.

/// Strip the handful of accented Latin-1 characters that show up in the
/// site's Spanish genre tags. Not a general Unicode NFD normalizer — just
/// enough to make `canonical_genre`'s lookup accent-insensitive without
/// pulling in a dependency for a fixed, small vocabulary. `pub(crate)` so
/// `matching.rs` can reuse it for title normalization instead of
/// duplicating the same accent table.
pub(crate) fn strip_accents(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            'á' | 'à' | 'ä' | 'â' => 'a',
            'é' | 'è' | 'ë' | 'ê' => 'e',
            'í' | 'ì' | 'ï' | 'î' => 'i',
            'ó' | 'ò' | 'ö' | 'ô' => 'o',
            'ú' | 'ù' | 'ü' | 'û' => 'u',
            'ñ' => 'n',
            'Á' | 'À' | 'Ä' | 'Â' => 'A',
            'É' | 'È' | 'Ë' | 'Ê' => 'E',
            'Í' | 'Ì' | 'Ï' | 'Î' => 'I',
            'Ó' | 'Ò' | 'Ö' | 'Ô' => 'O',
            'Ú' | 'Ù' | 'Ü' | 'Û' => 'U',
            'Ñ' => 'N',
            other => other,
        })
        .collect()
}

/// Map a raw genre tag (from either source, any case/accent) onto AniList's
/// canonical genre name, or `None` if it has no AniList equivalent.
pub fn canonical_genre(raw: &str) -> Option<&'static str> {
    let normalized = strip_accents(raw.trim()).to_lowercase();
    match normalized.as_str() {
        "accion" | "action" => Some("Action"),
        "aventura" | "adventure" => Some("Adventure"),
        "comedia" | "comedy" => Some("Comedy"),
        "drama" => Some("Drama"),
        "ecchi" => Some("Ecchi"),
        "fantasia" | "fantasy" => Some("Fantasy"),
        "hentai" => Some("Hentai"),
        "terror" | "horror" => Some("Horror"),
        "mahou shoujo" => Some("Mahou Shoujo"),
        "mecha" => Some("Mecha"),
        "musica" | "music" => Some("Music"),
        "misterio" | "mystery" => Some("Mystery"),
        "psicologico" | "psychological" => Some("Psychological"),
        "romance" => Some("Romance"),
        "ciencia ficcion" | "sci-fi" | "scifi" => Some("Sci-Fi"),
        "recuentos de la vida" | "slice of life" => Some("Slice of Life"),
        "deportes" | "sports" => Some("Sports"),
        "supernatural" => Some("Supernatural"),
        "thriller" => Some("Thriller"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::canonical_genre;

    #[test]
    fn maps_spanish_genres_onto_anilist_canon() {
        assert_eq!(canonical_genre("Acción"), Some("Action"));
        assert_eq!(canonical_genre("Fantasía"), Some("Fantasy"));
        assert_eq!(canonical_genre("Aventura"), Some("Adventure"));
        assert_eq!(canonical_genre("Comedia"), Some("Comedy"));
        assert_eq!(canonical_genre("Ciencia ficción"), Some("Sci-Fi"));
        assert_eq!(canonical_genre("Recuentos de la vida"), Some("Slice of Life"));
        assert_eq!(canonical_genre("Misterio"), Some("Mystery"));
        assert_eq!(canonical_genre("Terror"), Some("Horror"));
        assert_eq!(canonical_genre("Deportes"), Some("Sports"));
        assert_eq!(canonical_genre("Música"), Some("Music"));
        assert_eq!(canonical_genre("Psicológico"), Some("Psychological"));
    }

    #[test]
    fn is_case_and_accent_insensitive() {
        assert_eq!(canonical_genre("ACCIÓN"), Some("Action"));
        assert_eq!(canonical_genre("accion"), Some("Action"));
        assert_eq!(canonical_genre("  Acción  "), Some("Action"));
    }

    #[test]
    fn already_canonical_english_genres_pass_through() {
        for g in [
            "Drama", "Romance", "Supernatural", "Mecha", "Ecchi", "Horror", "Thriller", "Action",
            "Fantasy", "Adventure", "Comedy", "Mystery", "Sci-Fi", "Slice of Life", "Sports",
            "Music", "Psychological", "Hentai", "Mahou Shoujo",
        ] {
            assert_eq!(canonical_genre(g), Some(g), "expected {g} to pass through unchanged");
        }
    }

    #[test]
    fn site_only_tags_with_no_anilist_equivalent_return_none() {
        assert_eq!(canonical_genre("Shounen"), None);
        assert_eq!(canonical_genre("Isekai"), None);
        assert_eq!(canonical_genre("Reencarnación"), None);
        assert_eq!(canonical_genre("Escolar"), None);
        assert_eq!(canonical_genre("Seinen"), None);
        assert_eq!(canonical_genre("Josei"), None);
    }
}
