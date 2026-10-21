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
        "aventura" | "aventuras" | "adventure" => Some("Adventure"),
        "comedia" | "comedy" => Some("Comedy"),
        "drama" => Some("Drama"),
        "ecchi" => Some("Ecchi"),
        "fantasia" | "fantasy" => Some("Fantasy"),
        "hentai" => Some("Hentai"),
        "terror" | "horror" => Some("Horror"),
        "mahou shoujo" | "chicas magicas" | "chica magica" | "magical girl" | "magical girls" => {
            Some("Mahou Shoujo")
        }
        "mecha" | "robots" => Some("Mecha"),
        "musica" | "musical" | "music" => Some("Music"),
        "misterio" | "mystery" => Some("Mystery"),
        "psicologico" | "psicologica" | "psychological" => Some("Psychological"),
        "romance" | "romantico" | "romantica" => Some("Romance"),
        "ciencia ficcion" | "ciencia-ficcion" | "sci-fi" | "sci fi" | "scifi" | "science fiction" => {
            Some("Sci-Fi")
        }
        "recuentos de la vida" | "recuentos de vida" | "vida cotidiana" | "slice of life" => {
            Some("Slice of Life")
        }
        "deportes" | "deporte" | "sports" => Some("Sports"),
        "sobrenatural" | "supernatural" => Some("Supernatural"),
        // MAL's "Suspense" (and the sites' Spanish "Suspenso") is AniList's Thriller.
        "suspenso" | "suspense" | "thriller" => Some("Thriller"),
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
    fn maps_the_other_spanish_and_mal_labels_the_sites_publish() {
        // Labels seen in the adapters' fixtures (src-tauri/tests/fixtures/).
        // Stats drop any site label that does not map, so a miss here makes
        // an unlinked show lose a real genre.
        for (raw, canon) in [
            ("Sobrenatural", "Supernatural"),
            ("Suspenso", "Thriller"),
            ("Suspense", "Thriller"),
            ("Deporte", "Sports"),
            ("Chicas mágicas", "Mahou Shoujo"),
            ("Chicas magicas", "Mahou Shoujo"),
            ("Chica mágica", "Mahou Shoujo"),
            ("Magical Girl", "Mahou Shoujo"),
            ("Aventuras", "Adventure"),
            ("Musical", "Music"),
            ("Psicológica", "Psychological"),
            ("Ciencia-ficción", "Sci-Fi"),
            ("Sci Fi", "Sci-Fi"),
            ("Science Fiction", "Sci-Fi"),
            ("Recuentos de vida", "Slice of Life"),
            ("Vida cotidiana", "Slice of Life"),
            ("Romántico", "Romance"),
            ("Robots", "Mecha"),
        ] {
            assert_eq!(canonical_genre(raw), Some(canon), "{raw}");
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
