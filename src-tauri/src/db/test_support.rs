#![cfg(test)]

use super::*;

pub(crate) fn mk_airing(slug: &str, title: &str, next_episode_at: Option<i64>) -> crate::models::Series {
    crate::models::Series {
        id: 0,
        slug: slug.into(),
        title: title.into(),
        url: format!("https://site/tv/{slug}/"),
        cover_url: None,
        is_airing: true,
        followed: false,
        next_episode_at,
        site_episode_count: None,
    }
}

pub(crate) fn insert_eps_seen_up_to(db: &Db, series_id: i64, total: i64, seen_up_to: i64) {
    for i in 1..=total {
        db.insert_episode(&crate::models::Episode {
            id: 0, series_id, number: i.to_string(), title: None,
            url: format!("https://site/{series_id}-{i}"), released_at: None, seen: false,
        }).unwrap();
    }
    if seen_up_to >= 1 {
        db.set_seen_cascade(series_id, &seen_up_to.to_string(), true).unwrap();
    }
}

pub(crate) fn catalog_anime(id: i64, title: &str, genres: &[&str]) -> crate::anilist::CatalogAnime {
    catalog_anime_with_popularity(id, title, genres, None)
}

pub(crate) fn catalog_anime_with_popularity(
    id: i64,
    title: &str,
    genres: &[&str],
    popularity: Option<i64>,
) -> crate::anilist::CatalogAnime {
    crate::anilist::CatalogAnime {
        id,
        title: title.into(),
        title_romaji: None,
        title_english: None,
        cover_url: Some(format!("https://cdn/{id}.jpg")),
        format: Some("TV".into()),
        genres: genres.iter().map(|g| g.to_string()).collect(),
        episodes: Some(12),
        average_score: Some(80),
        popularity,
        url: format!("https://anilist.co/anime/{id}"),
        status: None,
        duration: None,
        studio: None,
    }
}

pub(crate) fn catalog_anime_full(
    id: i64,
    title: &str,
    genres: &[&str],
    format: &str,
    episodes: Option<i64>,
    average_score: Option<i64>,
) -> crate::anilist::CatalogAnime {
    crate::anilist::CatalogAnime {
        id,
        title: title.into(),
        title_romaji: None,
        title_english: None,
        cover_url: None,
        format: Some(format.into()),
        genres: genres.iter().map(|g| g.to_string()).collect(),
        episodes,
        average_score,
        popularity: Some(100 - id),
        url: format!("https://anilist.co/anime/{id}"),
        status: None,
        duration: None,
        studio: None,
    }
}

pub(crate) fn seed_filter_catalog(db: &Db) {
    db.upsert_catalog_anime(
        &catalog_anime_full(1, "Monster", &["Drama", "Mystery"], "TV", Some(74), Some(90)),
        0,
    )
    .unwrap();
    db.upsert_catalog_anime(
        &catalog_anime_full(2, "Monster Girl", &["Comedy"], "TV", Some(12), Some(65)),
        1,
    )
    .unwrap();
    db.upsert_catalog_anime(
        &catalog_anime_full(3, "Fullmetal Alchemist", &["Drama", "Action"], "TV", Some(64), Some(88)),
        2,
    )
    .unwrap();
    db.upsert_catalog_anime(
        &catalog_anime_full(4, "OnlyOne", &["Drama"], "MOVIE", Some(1), Some(70)),
        3,
    )
    .unwrap();
    db.upsert_catalog_anime(
        &catalog_anime_full(5, "ShortRun", &["Action"], "OVA", Some(6), Some(55)),
        4,
    )
    .unwrap();
    db.upsert_catalog_anime(
        &catalog_anime_full(6, "MidRun", &["Action"], "TV", Some(24), Some(72)),
        5,
    )
    .unwrap();
    db.upsert_catalog_anime(
        &catalog_anime_full(7, "LongRun", &["Action"], "TV", Some(500), Some(60)),
        6,
    )
    .unwrap();
    db.upsert_catalog_anime(
        &catalog_anime_full(8, "Unknown Length", &["Drama", "Action"], "TV", None, Some(50)),
        7,
    )
    .unwrap();
}
