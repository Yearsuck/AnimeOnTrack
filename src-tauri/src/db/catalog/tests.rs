use super::*;
use crate::db::test_support::*;

#[test]
fn catalog_upsert_and_list_orders_by_popularity_desc() {
    let db = Db::open(":memory:").unwrap();
    // Inserted out of popularity order and out of sort_order too, to
    // prove ordering follows `popularity`, not insertion or sort_order.
    db.upsert_catalog_anime(&catalog_anime_with_popularity(3, "LowPop", &["Drama"], Some(10)), 0).unwrap();
    db.upsert_catalog_anime(&catalog_anime_with_popularity(1, "HighPop", &["Action", "Fantasy"], Some(9000)), 1).unwrap();
    db.upsert_catalog_anime(&catalog_anime_with_popularity(2, "MidPop", &[], Some(500)), 2).unwrap();

    assert_eq!(db.catalog_count().unwrap(), 3);

    let page = db.list_catalog(1, 10).unwrap();
    assert_eq!(
        page.iter().map(|a| a.title.as_str()).collect::<Vec<_>>(),
        vec!["HighPop", "MidPop", "LowPop"]
    );
    assert_eq!(page[0].genres, vec!["Action".to_string(), "Fantasy".to_string()]);
}

#[test]
fn catalog_list_puts_null_popularity_last_and_tie_breaks_by_id() {
    let db = Db::open(":memory:").unwrap();
    db.upsert_catalog_anime(&catalog_anime_with_popularity(5, "NoPopB", &[], None), 0).unwrap();
    db.upsert_catalog_anime(&catalog_anime_with_popularity(2, "NoPopA", &[], None), 1).unwrap();
    db.upsert_catalog_anime(&catalog_anime_with_popularity(1, "HasPop", &[], Some(50)), 2).unwrap();

    let page = db.list_catalog(1, 10).unwrap();
    assert_eq!(
        page.iter().map(|a| a.title.as_str()).collect::<Vec<_>>(),
        vec!["HasPop", "NoPopA", "NoPopB"]
    );
}

#[test]
fn catalog_popularity_index_exists() {
    let db = Db::open(":memory:").unwrap();
    let name: String = db
        .conn
        .query_row(
            "SELECT name FROM sqlite_master WHERE type='index' AND name='idx_catalog_popularity'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(name, "idx_catalog_popularity");
}

#[test]
fn catalog_genre_index_exists() {
    let db = Db::open(":memory:").unwrap();
    let name: String = db
        .conn
        .query_row(
            "SELECT name FROM sqlite_master WHERE type='index' AND name='idx_catalog_genre'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(name, "idx_catalog_genre");
}

#[test]
fn catalog_upsert_is_idempotent_and_refreshes_genres() {
    let db = Db::open(":memory:").unwrap();
    db.upsert_catalog_anime(&catalog_anime(1, "First", &["Action"]), 0).unwrap();
    db.upsert_catalog_anime(&catalog_anime(1, "First (updated)", &["Comedy"]), 0).unwrap();

    assert_eq!(db.catalog_count().unwrap(), 1);
    let page = db.list_catalog(1, 10).unwrap();
    assert_eq!(page.len(), 1);
    assert_eq!(page[0].title, "First (updated)");
    assert_eq!(page[0].genres, vec!["Comedy".to_string()]);
}

#[test]
fn catalog_pagination_offsets_correctly() {
    let db = Db::open(":memory:").unwrap();
    for i in 0..5 {
        db.upsert_catalog_anime(&catalog_anime(i, &format!("Anime {i}"), &[]), i).unwrap();
    }
    let page1 = db.list_catalog(1, 2).unwrap();
    let page2 = db.list_catalog(2, 2).unwrap();
    assert_eq!(page1.iter().map(|a| a.id).collect::<Vec<_>>(), vec![0, 1]);
    assert_eq!(page2.iter().map(|a| a.id).collect::<Vec<_>>(), vec![2, 3]);
}

#[test]
fn has_synced_catalog_status_false_when_empty_or_all_null_true_once_one_row_has_status() {
    let db = Db::open(":memory:").unwrap();
    assert!(!db.has_synced_catalog_status().unwrap());

    db.upsert_catalog_anime(&catalog_anime_with_popularity(1, "Unsynced", &["Drama"], Some(1000)), 0).unwrap();
    assert!(!db.has_synced_catalog_status().unwrap(), "a NULL-status row alone must not count as synced");

    let mut synced = catalog_anime_with_popularity(2, "Synced", &["Drama"], Some(1000));
    synced.status = Some("RELEASING".into());
    db.upsert_catalog_anime(&synced, 1).unwrap();
    assert!(db.has_synced_catalog_status().unwrap());
}

/// Write a catalog row the way a *previous* build would have: straight
/// INSERT, leaving `metadata_version` at its `DEFAULT 0`. `romaji`/`status`
/// are parameterized so a test can reproduce the exact regression — a row
/// that already has a romaji title (so the old `title_romaji IS NULL`
/// marker considered it fresh) but none of the columns added after it.
fn insert_legacy_catalog_row(db: &Db, id: i64, romaji: Option<&str>, status: Option<&str>) {
    db.conn
        .execute(
            "INSERT INTO anilist_catalog(id, title, title_romaji, url, sort_order, popularity, format, status)
             VALUES(?1, ?2, ?3, ?4, 0, 1000, 'TV', ?5)",
            (id, format!("Legacy {id}"), romaji, format!("https://anilist.co/anime/{id}"), status),
        )
        .unwrap();
}

#[test]
fn stale_catalog_ids_flags_rows_written_with_an_older_field_set() {
    let db = Db::open(":memory:").unwrap();
    // The regression: romaji present (the old marker said "fresh"), but
    // status/duration/studio/start_date never fetched.
    insert_legacy_catalog_row(&db, 1, Some("Legacy One"), None);
    // The case the old marker did catch, still caught.
    insert_legacy_catalog_row(&db, 2, None, None);

    let stale = db.stale_catalog_ids().unwrap();
    assert!(stale.contains(&1), "a romaji-but-nothing-else row must be stale: {stale:?}");
    assert!(stale.contains(&2));
}

#[test]
fn stale_catalog_ids_ignores_rows_written_with_the_current_field_set() {
    let db = Db::open(":memory:").unwrap();
    // Written through the normal path, so stamped with the current version
    // — even though `studio`/`duration`/`start_date` are None here, which
    // is legitimate AniList data, not staleness.
    db.upsert_catalog_anime(&catalog_anime_with_popularity(1, "Fresh", &["Drama"], Some(1000)), 0).unwrap();
    assert!(db.stale_catalog_ids().unwrap().is_empty());
}

#[test]
fn backfilling_a_stale_row_makes_it_not_stale() {
    let db = Db::open(":memory:").unwrap();
    insert_legacy_catalog_row(&db, 7, Some("Legacy Seven"), None);
    assert_eq!(db.stale_catalog_ids().unwrap(), vec![7]);

    // What `backfill_catalog_metadata` does with what it re-fetched.
    let mut refetched = catalog_anime_with_popularity(7, "Legacy 7", &["Drama"], Some(1000));
    refetched.status = Some("FINISHED".into());
    db.upsert_catalog_anime(&refetched, db.catalog_sort_order(7).unwrap()).unwrap();

    assert!(db.stale_catalog_ids().unwrap().is_empty());
}

#[test]
fn metadata_version_migration_only_marks_the_genuinely_old_rows_stale() {
    // The whole point of seeding `metadata_version` at migration time:
    // defaulting every existing row to 0 would push a ~22k-row, >12h paced
    // backfill onto every user on first launch. A row that already carries
    // both a romaji title AND a status was written by a build with the
    // current field set, so it must come out non-stale; the rows the old
    // marker was blind to must come out stale.
    let path = std::env::temp_dir().join(format!(
        "aot_metaver_{}_{:?}.sqlite",
        std::process::id(),
        std::thread::current().id()
    ));
    std::fs::remove_file(&path).ok();
    {
        let db = Db::open(path.to_str().unwrap()).unwrap();
        insert_legacy_catalog_row(&db, 1, Some("Has Both"), Some("FINISHED"));
        insert_legacy_catalog_row(&db, 2, Some("Romaji Only"), None);
        insert_legacy_catalog_row(&db, 3, None, None);
        // Simulate a pre-migration database: no row has been stamped yet.
        db.conn.execute("UPDATE anilist_catalog SET metadata_version = 0", []).unwrap();
    }
    // Reopening re-runs init_schema, i.e. the migration.
    let db = Db::open(path.to_str().unwrap()).unwrap();
    let stale = db.stale_catalog_ids().unwrap();
    // Legacy rows are stamped as version 1, which is behind the current
    // version (2 added synonyms), so they backfill like any other stale row.
    assert!(stale.contains(&1), "a v1-stamped row must be re-backfilled for synonyms: {stale:?}");
    assert!(stale.contains(&2), "the romaji-only row is exactly what the old marker missed");
    assert!(stale.contains(&3));
    drop(db);
    std::fs::remove_file(&path).ok();
}

#[test]
fn random_catalog_anime_in_genre_none_when_empty_some_when_populated() {
    let db = Db::open(":memory:").unwrap();
    assert!(db.random_catalog_anime_in_genre("Drama", &[], &[], &std::collections::HashSet::new(), &std::collections::HashMap::new(), &std::collections::HashMap::new(), true, false, None, None, None).unwrap().is_none());
    db.upsert_catalog_anime(
        &catalog_anime_with_popularity(1, "Only", &["Drama"], Some(1000)),
        0,
    )
    .unwrap();
    let picked = db.random_catalog_anime_in_genre("Drama", &[], &[], &std::collections::HashSet::new(), &std::collections::HashMap::new(), &std::collections::HashMap::new(), true, false, None, None, None).unwrap().unwrap();
    assert_eq!(picked.id, 1);
    assert_eq!(picked.genres, vec!["Drama".to_string()]);
}

#[test]
fn random_catalog_pick_never_returns_an_excluded_id() {
    let db = Db::open(":memory:").unwrap();
    for id in 1..=3 {
        db.upsert_catalog_anime(&catalog_anime_with_popularity(id, &format!("Show {id}"), &["Drama"], Some(1000)), id).unwrap();
    }
    let no_titles = std::collections::HashSet::new();
    let none = std::collections::HashMap::new();
    let signals = crate::recommend::TasteSignals::default();
    let pick = |excluded: &std::collections::HashSet<i64>| {
        db.random_catalog_anime_in_genre_with_signals_and_excluded_ids(
            "Drama", &[], &[], &no_titles, excluded, &none, &none, &signals, false, false, None, None, None,
        )
        .unwrap()
    };
    // Whatever the random order, ids 1 and 2 excluded leaves exactly id 3.
    let excluded: std::collections::HashSet<i64> = [1, 2].into_iter().collect();
    for _ in 0..20 {
        assert_eq!(pick(&excluded).unwrap().id, 3);
    }
    // All three excluded: nothing left.
    let all: std::collections::HashSet<i64> = [1, 2, 3].into_iter().collect();
    assert!(pick(&all).is_none());
    // Repeated picks while excluding each result give three distinct cards, then none.
    let mut seen = std::collections::HashSet::new();
    while let Some(a) = pick(&seen) {
        assert!(seen.insert(a.id), "id {} offered twice", a.id);
    }
    assert_eq!(seen.len(), 3);
}

#[test]
fn random_catalog_anime_in_genre_only_matches_requested_genre() {
    let db = Db::open(":memory:").unwrap();
    db.upsert_catalog_anime(&catalog_anime_with_popularity(1, "Dramatic", &["Drama"], Some(1000)), 0).unwrap();
    db.upsert_catalog_anime(&catalog_anime_with_popularity(2, "Actiony", &["Action"], Some(1000)), 1).unwrap();
    let picked = db.random_catalog_anime_in_genre("Action", &[], &[], &std::collections::HashSet::new(), &std::collections::HashMap::new(), &std::collections::HashMap::new(), true, false, None, None, None).unwrap().unwrap();
    assert_eq!(picked.title, "Actiony");
}

#[test]
fn random_catalog_anime_in_genre_excludes_low_popularity_and_wrong_format() {
    let db = Db::open(":memory:").unwrap();
    // Below the 500 popularity floor -> excluded.
    db.upsert_catalog_anime(&catalog_anime_with_popularity(1, "TooObscure", &["Drama"], Some(50)), 0).unwrap();
    // MUSIC format -> excluded regardless of popularity.
    let mut music = catalog_anime_with_popularity(2, "MusicVideo", &["Drama"], Some(9000));
    music.format = Some("MUSIC".into());
    db.upsert_catalog_anime(&music, 1).unwrap();
    // Qualifies on both counts.
    db.upsert_catalog_anime(&catalog_anime_with_popularity(3, "Qualifies", &["Drama"], Some(1000)), 2).unwrap();

    let picked = db.random_catalog_anime_in_genre("Drama", &[], &[], &std::collections::HashSet::new(), &std::collections::HashMap::new(), &std::collections::HashMap::new(), true, false, None, None, None).unwrap().unwrap();
    assert_eq!(picked.title, "Qualifies");
}

#[test]
fn random_catalog_anime_in_genre_excludes_not_yet_released_when_hide_upcoming_true() {
    let db = Db::open(":memory:").unwrap();
    let mut upcoming = catalog_anime_with_popularity(1, "Upcoming", &["Drama"], Some(1000));
    upcoming.status = Some("NOT_YET_RELEASED".into());
    db.upsert_catalog_anime(&upcoming, 0).unwrap();
    db.upsert_catalog_anime(&catalog_anime_with_popularity(2, "OutNow", &["Drama"], Some(1000)), 1).unwrap();

    let picked = db
        .random_catalog_anime_in_genre("Drama", &[], &[], &std::collections::HashSet::new(), &std::collections::HashMap::new(), &std::collections::HashMap::new(), true, true, None, None, None)
        .unwrap()
        .unwrap();
    assert_eq!(picked.title, "OutNow");
}

#[test]
fn random_catalog_anime_in_genre_includes_null_status_when_hide_upcoming_true() {
    // Unsynced rows (status still NULL, e.g. before the first resync
    // after this column existed) must NOT vanish from the deck just
    // because the toggle is on — only a confirmed NOT_YET_RELEASED
    // status is excluded.
    let db = Db::open(":memory:").unwrap();
    db.upsert_catalog_anime(&catalog_anime_with_popularity(1, "UnsyncedStatus", &["Drama"], Some(1000)), 0).unwrap();

    let picked = db
        .random_catalog_anime_in_genre("Drama", &[], &[], &std::collections::HashSet::new(), &std::collections::HashMap::new(), &std::collections::HashMap::new(), true, true, None, None, None)
        .unwrap()
        .unwrap();
    assert_eq!(picked.title, "UnsyncedStatus");
}

#[test]
fn random_catalog_anime_in_genre_includes_upcoming_when_hide_upcoming_false() {
    let db = Db::open(":memory:").unwrap();
    let mut upcoming = catalog_anime_with_popularity(1, "Upcoming", &["Drama"], Some(1000));
    upcoming.status = Some("NOT_YET_RELEASED".into());
    db.upsert_catalog_anime(&upcoming, 0).unwrap();

    let picked = db
        .random_catalog_anime_in_genre("Drama", &[], &[], &std::collections::HashSet::new(), &std::collections::HashMap::new(), &std::collections::HashMap::new(), true, false, None, None, None)
        .unwrap()
        .unwrap();
    assert_eq!(picked.title, "Upcoming");
}

#[test]
fn random_catalog_anime_in_genre_date_range_excludes_before_min_and_after_max() {
    let db = Db::open(":memory:").unwrap();
    let mut too_old = catalog_anime_with_popularity(1, "TooOld", &["Drama"], Some(1000));
    too_old.start_date = Some(1_000);
    db.upsert_catalog_anime(&too_old, 0).unwrap();
    let mut too_new = catalog_anime_with_popularity(2, "TooNew", &["Drama"], Some(1000));
    too_new.start_date = Some(3_000);
    db.upsert_catalog_anime(&too_new, 1).unwrap();
    let mut in_range = catalog_anime_with_popularity(3, "InRange", &["Drama"], Some(1000));
    in_range.start_date = Some(2_000);
    db.upsert_catalog_anime(&in_range, 2).unwrap();

    let picked = db
        .random_catalog_anime_in_genre(
            "Drama", &[], &[], &std::collections::HashSet::new(),
            &std::collections::HashMap::new(), &std::collections::HashMap::new(),
            true, false, Some(1_500), Some(2_500), None,
        )
        .unwrap()
        .unwrap();
    assert_eq!(picked.title, "InRange");
}

#[test]
fn random_catalog_anime_in_genre_date_range_keeps_unsynced_start_date_regardless_of_bounds() {
    let db = Db::open(":memory:").unwrap();
    // start_date: None (unsynced) — same "don't punish unsynced data"
    // policy as hide_upcoming's NULL-status handling.
    db.upsert_catalog_anime(&catalog_anime_with_popularity(1, "Unsynced", &["Drama"], Some(1000)), 0).unwrap();

    let picked = db
        .random_catalog_anime_in_genre(
            "Drama", &[], &[], &std::collections::HashSet::new(),
            &std::collections::HashMap::new(), &std::collections::HashMap::new(),
            true, false, Some(1_500), Some(2_500), None,
        )
        .unwrap()
        .unwrap();
    assert_eq!(picked.title, "Unsynced");
}

#[test]
fn random_catalog_anime_in_genre_date_range_only_min_or_only_max_still_filters() {
    let db = Db::open(":memory:").unwrap();
    let mut low = catalog_anime_with_popularity(1, "Low", &["Drama"], Some(1000));
    low.start_date = Some(1_000);
    db.upsert_catalog_anime(&low, 0).unwrap();
    let mut high = catalog_anime_with_popularity(2, "High", &["Drama"], Some(1000));
    high.start_date = Some(3_000);
    db.upsert_catalog_anime(&high, 1).unwrap();

    // Only a min bound: excludes Low, keeps High.
    let picked = db
        .random_catalog_anime_in_genre(
            "Drama", &[], &[], &std::collections::HashSet::new(),
            &std::collections::HashMap::new(), &std::collections::HashMap::new(),
            true, false, Some(2_000), None, None,
        )
        .unwrap()
        .unwrap();
    assert_eq!(picked.title, "High");

    // Only a max bound: excludes High, keeps Low.
    let picked = db
        .random_catalog_anime_in_genre(
            "Drama", &[], &[], &std::collections::HashSet::new(),
            &std::collections::HashMap::new(), &std::collections::HashMap::new(),
            true, false, None, Some(2_000), None,
        )
        .unwrap()
        .unwrap();
    assert_eq!(picked.title, "Low");
}

#[test]
fn random_catalog_anime_in_genre_excludes_already_decided_titles() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
    db.upsert_catalog_anime(&catalog_anime_with_popularity(1, "Decided", &["Drama"], Some(1000)), 0).unwrap();
    db.upsert_catalog_anime(&catalog_anime_with_popularity(2, "Undecided", &["Drama"], Some(1000)), 1).unwrap();

    let series = crate::models::Series {
        id: 0, slug: "anilist-1".into(), title: "Decided".into(),
        url: "https://anilist.co/anime/1".into(), cover_url: None,
        is_airing: false, followed: false, next_episode_at: None, site_episode_count: None,
    };
    let sid = db.upsert_series(src, &series).unwrap();
    db.set_anilist_id(sid, 1).unwrap();
    db.set_backlog_status(sid, Some("discarded")).unwrap();

    for _ in 0..10 {
        let picked = db.random_catalog_anime_in_genre("Drama", &[], &[], &std::collections::HashSet::new(), &std::collections::HashMap::new(), &std::collections::HashMap::new(), true, false, None, None, None).unwrap().unwrap();
        assert_eq!(picked.title, "Undecided");
    }
}

#[test]
fn random_catalog_anime_in_genre_returns_none_when_all_decided() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
    db.upsert_catalog_anime(&catalog_anime_with_popularity(1, "OnlyOne", &["Drama"], Some(1000)), 0).unwrap();
    let series = crate::models::Series {
        id: 0, slug: "anilist-1".into(), title: "OnlyOne".into(),
        url: "https://anilist.co/anime/1".into(), cover_url: None,
        is_airing: false, followed: false, next_episode_at: None, site_episode_count: None,
    };
    let sid = db.upsert_series(src, &series).unwrap();
    db.set_anilist_id(sid, 1).unwrap();
    db.set_backlog_status(sid, Some("want")).unwrap();

    assert!(db.random_catalog_anime_in_genre("Drama", &[], &[], &std::collections::HashSet::new(), &std::collections::HashMap::new(), &std::collections::HashMap::new(), true, false, None, None, None).unwrap().is_none());
}

#[test]
fn random_catalog_anime_in_genre_excludes_a_banned_format() {
    let db = Db::open(":memory:").unwrap();
    let mut movie = catalog_anime_with_popularity(1, "AMovie", &["Drama"], Some(1000));
    movie.format = Some("MOVIE".into());
    db.upsert_catalog_anime(&movie, 0).unwrap();
    let mut tv = catalog_anime_with_popularity(2, "ATVShow", &["Drama"], Some(1000));
    tv.format = Some("TV".into());
    db.upsert_catalog_anime(&tv, 1).unwrap();

    // With MOVIE banned, only the TV entry can ever be picked.
    for _ in 0..10 {
        let picked = db
            .random_catalog_anime_in_genre("Drama", &["MOVIE".to_string()], &[], &std::collections::HashSet::new(), &std::collections::HashMap::new(), &std::collections::HashMap::new(), true, false, None, None, None)
            .unwrap()
            .unwrap();
        assert_eq!(picked.title, "ATVShow");
    }
}

#[test]
fn random_catalog_anime_in_genre_excludes_a_candidate_carrying_a_banned_secondary_genre() {
    // The bug this covers: banning a genre only kept it from being picked
    // as the round's OUTER genre (`filter_candidate_genres`). A title
    // tagged both an allowed genre and a banned one still passed every
    // card-level filter and got served whenever the round happened to
    // sample the allowed genre.
    let db = Db::open(":memory:").unwrap();
    db.upsert_catalog_anime(
        &catalog_anime_with_popularity(1, "AlsoEcchi", &["Accion", "Ecchi"], Some(1000)),
        0,
    )
    .unwrap();
    db.upsert_catalog_anime(
        &catalog_anime_with_popularity(2, "CleanPick", &["Accion"], Some(1000)),
        1,
    )
    .unwrap();

    // Queried under the ALLOWED genre — the banned one is only a secondary
    // tag on the candidate, which is exactly what used to slip through.
    for _ in 0..10 {
        let picked = db
            .random_catalog_anime_in_genre(
                "Accion",
                &[],
                &["Ecchi".to_string()],
                &std::collections::HashSet::new(),
                &std::collections::HashMap::new(),
                &std::collections::HashMap::new(),
                true,
                false,
                None,
                None,
                None,
            )
            .unwrap()
            .unwrap();
        assert_eq!(picked.title, "CleanPick");
    }
}

#[test]
fn random_catalog_anime_in_genre_banned_genre_match_is_case_insensitive_and_can_empty_the_deck() {
    // Case-insensitive, mirroring `filter_candidate_genres`'s
    // `eq_ignore_ascii_case` — the stored ban list is user-entered text.
    let db = Db::open(":memory:").unwrap();
    db.upsert_catalog_anime(
        &catalog_anime_with_popularity(1, "AlsoEcchi", &["Accion", "Ecchi"], Some(1000)),
        0,
    )
    .unwrap();

    assert!(db
        .random_catalog_anime_in_genre(
            "Accion",
            &[],
            &["ecchi".to_string()],
            &std::collections::HashSet::new(),
            &std::collections::HashMap::new(),
            &std::collections::HashMap::new(),
            true,
            false,
            None,
            None,
            None,
        )
        .unwrap()
        .is_none());

    // Control: with no ban list at all the same title is returnable, so
    // the exclusion above is the ban doing it, not some other filter.
    assert!(db
        .random_catalog_anime_in_genre(
            "Accion",
            &[],
            &[],
            &std::collections::HashSet::new(),
            &std::collections::HashMap::new(),
            &std::collections::HashMap::new(),
            true,
            false,
            None,
            None,
            None,
        )
        .unwrap()
        .is_some());
}

#[test]
fn random_catalog_anime_in_genre_none_when_every_format_banned() {
    let db = Db::open(":memory:").unwrap();
    let mut tv = catalog_anime_with_popularity(1, "ATVShow", &["Drama"], Some(1000));
    tv.format = Some("TV".into());
    db.upsert_catalog_anime(&tv, 0).unwrap();

    let all_banned: Vec<String> =
        ["TV", "MOVIE", "OVA", "ONA", "SPECIAL"].iter().map(|s| s.to_string()).collect();
    // An invalid empty SQL IN () would error here if not short-circuited.
    assert!(db
        .random_catalog_anime_in_genre("Drama", &all_banned, &[], &std::collections::HashSet::new(), &std::collections::HashMap::new(), &std::collections::HashMap::new(), true, false, None, None, None)
        .unwrap()
        .is_none());
}

#[test]
fn random_catalog_anime_in_genre_excludes_engaged_normalized_title() {
    // Root cause (docs/superpowers/specs/2026-07-12-discover-exclude-followed-design.md):
    // followed site series never get an anilist_id (linking is on-demand
    // and rare), so the pre-existing `anilist_id NOT IN series` clause
    // catches ~zero engaged series. The deck must also exclude by
    // normalized title so a followed site row with no anilist_id still
    // blocks the same-titled catalog entry.
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
    db.upsert_catalog_anime(&catalog_anime_with_popularity(1, "Overlord IV", &["Fantasy"], Some(1000)), 0)
        .unwrap();
    // Control: an un-followed catalog title in the same genre must stay
    // returnable, proving the exclusion is title-specific, not blanket.
    db.upsert_catalog_anime(&catalog_anime_with_popularity(2, "Some Other Show", &["Fantasy"], Some(1000)), 1)
        .unwrap();

    let followed = crate::models::Series {
        id: 0,
        slug: "overlord-iv".into(),
        title: "Overlord IV".into(),
        url: "https://example.com/tv/overlord-iv".into(),
        cover_url: None,
        is_airing: false,
        followed: true,
        next_episode_at: None,
        site_episode_count: None,
    };
    let sid = db.upsert_series(src, &followed).unwrap();
    db.set_followed(sid, true).unwrap();
    // Deliberately no set_anilist_id: this is the exact bug scenario —
    // followed=1, anilist_id NULL.

    let excluded: std::collections::HashSet<String> =
        db.engaged_series_titles().unwrap().iter().map(|t| crate::matching::normalize_title(t)).collect();
    assert!(excluded.contains(&crate::matching::normalize_title("Overlord IV")));

    for _ in 0..10 {
        let picked = db.random_catalog_anime_in_genre("Fantasy", &[], &[], &excluded, &std::collections::HashMap::new(), &std::collections::HashMap::new(), true, false, None, None, None).unwrap().unwrap();
        assert_eq!(picked.title, "Some Other Show");
    }
}

#[test]
fn random_catalog_anime_in_genre_returns_none_when_only_candidate_is_engaged_by_title() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
    db.upsert_catalog_anime(&catalog_anime_with_popularity(1, "Overlord IV", &["Fantasy"], Some(1000)), 0)
        .unwrap();
    let followed = crate::models::Series {
        id: 0,
        slug: "overlord-iv".into(),
        title: "Overlord IV".into(),
        url: "https://example.com/tv/overlord-iv".into(),
        cover_url: None,
        is_airing: false,
        followed: true,
        next_episode_at: None,
        site_episode_count: None,
    };
    let sid = db.upsert_series(src, &followed).unwrap();
    db.set_followed(sid, true).unwrap();

    let excluded: std::collections::HashSet<String> =
        db.engaged_series_titles().unwrap().iter().map(|t| crate::matching::normalize_title(t)).collect();
    assert!(db.random_catalog_anime_in_genre("Fantasy", &[], &[], &excluded, &std::collections::HashMap::new(), &std::collections::HashMap::new(), true, false, None, None, None).unwrap().is_none());
}

#[test]
fn random_catalog_anime_in_genre_end_to_end_biases_toward_secondary_genre_affinity() {
    // Integration check that the recommendation engine is actually wired
    // through the DB layer, not just correct in isolation
    // (`recommend::tests` covers the pure scorer). Both candidates match
    // the requested outer genre "Drama" (excluded from scoring — see
    // `score_candidate`'s doc comment), so only a real backfill of each
    // survivor's FULL genre list (via `list_catalog_genres`, not just
    // the queried column) lets the secondary-genre overlap with
    // "Fantasy" actually surface and bias the pick.
    let db = Db::open(":memory:").unwrap();
    db.upsert_catalog_anime(
        &catalog_anime_with_popularity(1, "TasteMatch", &["Drama", "Fantasy"], Some(1000)),
        0,
    )
    .unwrap();
    db.upsert_catalog_anime(&catalog_anime_with_popularity(2, "NoMatch", &["Drama"], Some(1000)), 1).unwrap();

    let mut genre_affinity = std::collections::HashMap::new();
    genre_affinity.insert("Fantasy".to_string(), 10.0);
    let format_affinity = std::collections::HashMap::new();

    let mut taste_match_wins = 0;
    // ~90% win rate per pick; 200 draws and a 150 floor sit ~7 sigma
    // below the mean, so this cannot flake (25/30 did, ~1 run in 12).
    for _ in 0..200 {
        let picked = db
            .random_catalog_anime_in_genre(
                "Drama",
                &[],
                &[],
                &std::collections::HashSet::new(),
                &genre_affinity,
                &format_affinity,
                true,
                false,
                None,
                None,
                None,
            )
            .unwrap()
            .unwrap();
        if picked.title == "TasteMatch" {
            taste_match_wins += 1;
        }
    }
    assert!(
        taste_match_wins >= 150,
        "expected the secondary-genre-matching candidate to dominate the weighted pick, won {taste_match_wins}/200"
    );
}

#[test]
fn random_catalog_anime_in_genre_recommended_false_ignores_affinity_bias() {
    // Mirrors `..._end_to_end_biases_toward_secondary_genre_affinity` but
    // with recommended=false: the same heavy Fantasy affinity that
    // dominates the recommended-mode pick must NOT bias the random-mode
    // pick — recommended=false bypasses `pick_recommended` entirely and
    // just returns the first (already `ORDER BY RANDOM()`) survivor.
    let db = Db::open(":memory:").unwrap();
    db.upsert_catalog_anime(
        &catalog_anime_with_popularity(1, "TasteMatch", &["Drama", "Fantasy"], Some(1000)),
        0,
    )
    .unwrap();
    db.upsert_catalog_anime(&catalog_anime_with_popularity(2, "NoMatch", &["Drama"], Some(1000)), 1).unwrap();

    let mut genre_affinity = std::collections::HashMap::new();
    genre_affinity.insert("Fantasy".to_string(), 10.0);
    let format_affinity = std::collections::HashMap::new();

    let mut taste_match_wins = 0;
    for _ in 0..30 {
        let picked = db
            .random_catalog_anime_in_genre(
                "Drama",
                &[],
                &[],
                &std::collections::HashSet::new(),
                &genre_affinity,
                &format_affinity,
                false,
                false,
                None,
                None,
                None,
            )
            .unwrap()
            .unwrap();
        if picked.title == "TasteMatch" {
            taste_match_wins += 1;
        }
    }
    // Wide bounds (should land near 15/30) purely to avoid flakiness —
    // the point is it must NOT dominate like the recommended-mode test
    // (which asserts >= 25/30).
    assert!(
        (5..=25).contains(&taste_match_wins),
        "expected roughly even split ignoring affinity, got {taste_match_wins}/30 for TasteMatch"
    );
}

#[test]
fn random_catalog_anime_in_genre_recommended_false_still_excludes_engaged_titles() {
    // recommended=false must skip scoring, not skip exclusion — the
    // batch-fetch/ban/quality-floor/exclusion logic is shared regardless
    // of mode.
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
    db.upsert_catalog_anime(&catalog_anime_with_popularity(1, "Overlord IV", &["Fantasy"], Some(1000)), 0)
        .unwrap();
    db.upsert_catalog_anime(&catalog_anime_with_popularity(2, "Some Other Show", &["Fantasy"], Some(1000)), 1)
        .unwrap();

    let followed = crate::models::Series {
        id: 0,
        slug: "overlord-iv".into(),
        title: "Overlord IV".into(),
        url: "https://example.com/tv/overlord-iv".into(),
        cover_url: None,
        is_airing: false,
        followed: true,
        next_episode_at: None,
        site_episode_count: None,
    };
    let sid = db.upsert_series(src, &followed).unwrap();
    db.set_followed(sid, true).unwrap();

    let excluded: std::collections::HashSet<String> =
        db.engaged_series_titles().unwrap().iter().map(|t| crate::matching::normalize_title(t)).collect();

    for _ in 0..10 {
        let picked = db
            .random_catalog_anime_in_genre(
                "Fantasy",
                &[],
                &[],
                &excluded,
                &std::collections::HashMap::new(),
                &std::collections::HashMap::new(),
                false,
                false,
                None,
                None,
                None,
            )
            .unwrap()
            .unwrap();
        assert_eq!(picked.title, "Some Other Show");
    }
}

#[test]
fn link_series_to_catalog_links_an_exact_case_insensitive_title_match() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("GogoAnime", "g", "gogoanime").unwrap();
    db.upsert_catalog_anime(&catalog_anime(187538, "BLACK TORCH", &["Action"]), 0).unwrap();

    let sid = db.upsert_series(src, &crate::models::Series {
        id: 0, slug: "black-torch".into(), title: "Black Torch".into(),
        url: "https://gogoanime.example/series/black-torch".into(),
        cover_url: None, is_airing: true, followed: true,
        next_episode_at: None, site_episode_count: None,
    }).unwrap();
    db.set_followed(sid, true).unwrap();

    let linked = db.link_series_to_catalog().unwrap();
    assert_eq!(linked, 1);
    assert_eq!(
        db.conn.query_row("SELECT anilist_id FROM series WHERE id=?1", [sid], |r| r.get::<_, Option<i64>>(0)).unwrap(),
        Some(187538)
    );
}

#[test]
fn link_series_to_catalog_prefers_the_matching_season_over_season_one() {
    // The reported bug: a site's "... Temporada 2" title has no exact
    // catalog hit (AniList stores "... 2nd Season"), and its franchise
    // base ("...", season markers stripped) exact-matches season 1 —
    // which used to win outright before the full title's fuzzy score
    // against the real season-2 entry was ever tried. Season 1 is
    // seeded with far higher popularity so a popularity tie-break bug
    // would also surface this as a failure.
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("TioAnime", "t", "tioanime").unwrap();
    db.upsert_catalog_anime(
        &catalog_anime_with_popularity(100, "Seihantai na Kimi to Boku", &["Comedy"], Some(50_000)),
        0,
    ).unwrap();
    db.upsert_catalog_anime(
        &catalog_anime_with_popularity(200, "Seihantai na Kimi to Boku 2nd Season", &["Comedy"], Some(5_000)),
        0,
    ).unwrap();

    let sid = db.upsert_series(src, &crate::models::Series {
        id: 0, slug: "seihantai-2".into(), title: "Seihantai na Kimi to Boku Temporada 2".into(),
        url: "https://tioanime.example/series/seihantai-2".into(),
        cover_url: None, is_airing: true, followed: true,
        next_episode_at: None, site_episode_count: None,
    }).unwrap();
    db.set_followed(sid, true).unwrap();

    assert_eq!(db.link_series_to_catalog().unwrap(), 1);
    assert_eq!(
        db.conn.query_row("SELECT anilist_id FROM series WHERE id=?1", [sid], |r| r.get::<_, Option<i64>>(0)).unwrap(),
        Some(200),
        "must link to season 2's own entry, not season 1's"
    );
}

#[test]
fn link_series_to_catalog_falls_back_to_the_franchise_base_for_an_arc_with_no_entry_of_its_own() {
    // An arc split ("One Piece: Arco de Elbaph") has no AniList entry of
    // its own — the full title correctly finds nothing (exact or
    // fuzzy), so this must still fall back to the parent show's entry.
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("TioAnime", "t", "tioanime").unwrap();
    // One Piece airs: no episode total yet (None), next episode in the 1000s: a long-runner.
    let mut op = catalog_anime(21, "One Piece", &["Action"]);
    op.episodes = None;
    op.next_episode = Some(1181);
    db.upsert_catalog_anime(&op, 0).unwrap();

    let sid = db.upsert_series(src, &crate::models::Series {
        id: 0, slug: "one-piece-elbaph".into(), title: "One Piece: Arco de Elbaph".into(),
        url: "https://tioanime.example/series/one-piece-elbaph".into(),
        cover_url: None, is_airing: true, followed: true,
        next_episode_at: None, site_episode_count: None,
    }).unwrap();
    db.set_followed(sid, true).unwrap();

    assert_eq!(db.link_series_to_catalog().unwrap(), 1);
    assert_eq!(
        db.conn.query_row("SELECT anilist_id FROM series WHERE id=?1", [sid], |r| r.get::<_, Option<i64>>(0)).unwrap(),
        Some(21)
    );
}

fn series_with(db: &Db, src: i64, slug: &str, title: &str) -> i64 {
    let sid = db.upsert_series(src, &crate::models::Series {
        id: 0, slug: slug.into(), title: title.into(),
        url: format!("https://example/series/{slug}"),
        cover_url: None, is_airing: true, followed: true,
        next_episode_at: None, site_episode_count: None,
    }).unwrap();
    db.set_followed(sid, true).unwrap();
    sid
}

fn linked_id(db: &Db, sid: i64) -> Option<i64> {
    db.conn.query_row("SELECT anilist_id FROM series WHERE id=?1", [sid], |r| r.get(0)).unwrap()
}

#[test]
fn link_never_falls_back_to_season_one_for_a_season_n_title() {
    // Real bug: animeytx "Kanojo, Okarishimasu Temporada 5" got linked to the
    // plain season-1 entry (FINISHED), so a show airing now was marked finished.
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("AnimeYT", "a", "animeytx").unwrap();
    db.upsert_catalog_anime(&catalog_anime(113813, "Kanojo, Okarishimasu", &[]), 0).unwrap();
    let s5 = series_with(&db, src, "k5", "Kanojo, Okarishimasu Temporada 5");
    let s1 = series_with(&db, src, "k1", "Kanojo, Okarishimasu");

    db.link_series_to_catalog().unwrap();
    assert_eq!(linked_id(&db, s5), None, "a season-5 title must stay unlinked rather than join season 1");
    assert_eq!(linked_id(&db, s1), Some(113813), "the plain title still links to season 1");
}

#[test]
fn link_does_not_attach_a_live_action_title_to_the_anime() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("AnimeYT", "a", "animeytx").unwrap();
    let mut op = catalog_anime(21, "One Piece", &[]);
    op.episodes = None;
    op.next_episode = Some(1181);
    db.upsert_catalog_anime(&op, 0).unwrap();
    let live = series_with(&db, src, "op-live", "One Piece: Live Action (2023)");
    let arc = series_with(&db, src, "op-arc", "One Piece: Arco de Elbaph");

    db.link_series_to_catalog().unwrap();
    assert_eq!(linked_id(&db, live), None, "live action is a different work");
    assert_eq!(linked_id(&db, arc), Some(21), "arcs still fall back to the parent show");
}

#[test]
fn repair_moves_a_wrong_season_link_to_the_right_entry_once_it_exists() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("AnimeYT", "a", "animeytx").unwrap();
    db.upsert_catalog_anime(&catalog_anime(113813, "Kanojo, Okarishimasu", &[]), 0).unwrap();
    let s5 = series_with(&db, src, "k5", "Kanojo, Okarishimasu Temporada 5");
    db.set_anilist_id(s5, 113813).unwrap(); // the stale wrong link

    // Season 5 enters the catalog later.
    db.upsert_catalog_anime(&catalog_anime(199029, "Kanojo, Okarishimasu 5th Season", &[]), 1).unwrap();

    assert_eq!(db.repair_season_mislinks().unwrap(), 1);
    assert_eq!(linked_id(&db, s5), Some(199029));
    assert_eq!(db.repair_season_mislinks().unwrap(), 0, "idempotent");
}

#[test]
fn repair_takes_a_subtitle_season_off_the_plain_entry() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("AnimeYT", "a", "animeytx").unwrap();
    db.upsert_catalog_anime(&catalog_anime(178869, "Clevatess", &[]), 0).unwrap();
    db.upsert_catalog_anime(
        &catalog_anime(198946, "Clevatess II: Majuu no Ou to Itsuwari no Yuusha Denshou", &[]),
        1,
    )
    .unwrap();

    let wrong = series_with(
        &db,
        src,
        "clevatess-t2",
        "Clevatess Temporada 2: Majuu no Ou to Itsuwari no Yuusha Denshou",
    );
    db.set_anilist_id(wrong, 178869).unwrap();
    let correct = series_with(
        &db,
        src,
        "clevatess-ii",
        "Clevatess II: Majuu no Ou to Itsuwari no Yuusha Denshou",
    );
    db.set_anilist_id(correct, 198946).unwrap();
    let plain = series_with(&db, src, "clevatess", "Clevatess");
    db.set_anilist_id(plain, 178869).unwrap();

    assert_eq!(db.repair_season_mislinks().unwrap(), 1);
    // Off the season-1 entry; without an unambiguous season-2 match the repair
    // leaves it unlinked (never wrong), as on the real database.
    assert_ne!(linked_id(&db, wrong), Some(178869));
    assert_eq!(linked_id(&db, correct), Some(198946));
    assert_eq!(linked_id(&db, plain), Some(178869));
}

#[test]
fn repair_unlinks_a_live_action_row_attached_to_the_anime() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("AnimeYT", "a", "animeytx").unwrap();
    db.upsert_catalog_anime(&catalog_anime(21, "One Piece", &[]), 0).unwrap();
    let live = series_with(&db, src, "op-live", "One Piece: Live Action");
    db.set_anilist_id(live, 21).unwrap();
    let anime = series_with(&db, src, "op", "One Piece");
    db.set_anilist_id(anime, 21).unwrap();

    assert_eq!(db.repair_season_mislinks().unwrap(), 1);
    assert_eq!(linked_id(&db, live), None);
    assert_eq!(linked_id(&db, anime), Some(21));
}

#[test]
fn an_ambiguous_final_season_title_never_joins_season_one() {
    let db = Db::open(":memory:").unwrap();
    let a = db.upsert_source("AnimeYT", "a", "animeytx").unwrap();
    let b = db.upsert_source("Gogo", "g", "gogoanime").unwrap();
    db.upsert_catalog_anime(&catalog_anime(16498, "Attack on Titan", &[]), 0).unwrap();
    let plain = series_with(&db, a, "aot", "Attack on Titan");
    let fin = series_with(&db, b, "aot-final", "Attack on Titan The Final Season");

    db.link_series_to_catalog().unwrap();
    assert_eq!(linked_id(&db, plain), Some(16498));
    assert_eq!(linked_id(&db, fin), None, "via base fallback or sibling it would join season 1");
}

#[test]
fn repair_keeps_a_link_whose_catalog_row_is_absent_or_final_season() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("AnimeYT", "a", "animeytx").unwrap();
    db.upsert_catalog_anime(&catalog_anime(110277, "Attack on Titan: The Final Season", &[]), 0).unwrap();
    let s4 = series_with(&db, src, "aot4", "Shingeki no Kyojin Temporada 4");
    db.set_anilist_id(s4, 110277).unwrap();
    let orphan = series_with(&db, src, "x3", "Some Show Temporada 3");
    db.set_anilist_id(orphan, 999_999).unwrap(); // catalog row not loaded

    assert_eq!(db.repair_season_mislinks().unwrap(), 0);
    assert_eq!(linked_id(&db, s4), Some(110277));
    assert_eq!(linked_id(&db, orphan), Some(999_999));
}

#[test]
fn repair_keeps_an_arc_link_of_a_long_runner_without_countdown() {
    // Between airings / on hiatus AniList has no countdown and the episode
    // map may be empty: the strict long-runner rule says "no", but the
    // repair pass must not wipe a correct arc link because of that.
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("TioAnime", "t", "tioanime").unwrap();
    let mut op = catalog_anime(21, "One Piece", &[]);
    op.episodes = None;
    op.next_episode = None;
    db.upsert_catalog_anime(&op, 0).unwrap();
    let arc = series_with(&db, src, "op-elbaph", "One Piece: Arco de Elbaph");
    db.set_anilist_id(arc, 21).unwrap();

    assert_eq!(db.repair_season_mislinks().unwrap(), 0);
    assert_eq!(linked_id(&db, arc), Some(21));
}

#[test]
fn repair_keeps_a_link_to_a_subtitled_sequel_entry() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("AnimeYT", "a", "animeytx").unwrap();
    db.upsert_catalog_anime(&catalog_anime(101921, "Kaguya-sama: Ultra Romantic", &[]), 0).unwrap();
    let s3 = series_with(&db, src, "k3", "Kaguya-sama wa Kokurasetai Temporada 3");
    db.set_anilist_id(s3, 101921).unwrap();

    assert_eq!(db.repair_season_mislinks().unwrap(), 0);
    assert_eq!(linked_id(&db, s3), Some(101921));
}

#[test]
fn test_repair_season_mislinks_slug_season() {
    let db = Db::open(":memory:").unwrap();
    let src_a = db.upsert_source("AnimeYT", "a", "animeytx").unwrap();
    let src_b = db.upsert_source("TioAnime", "b", "tioanime").unwrap();
    db.upsert_catalog_anime(&catalog_anime(97940, "Black Clover", &[]), 0).unwrap();
    db.upsert_catalog_anime(&catalog_anime(195604, "Black Clover Season 2", &[]), 1).unwrap();

    let s1 = series_with(&db, src_a, "black-clover", "Black Clover");
    db.set_anilist_id(s1, 97940).unwrap();

    let s2 = series_with(&db, src_b, "black-clover-temporada-2", "Black Clover");
    db.set_anilist_id(s2, 97940).unwrap();

    assert_eq!(db.repair_season_mislinks().unwrap(), 1);
    assert_eq!(linked_id(&db, s1), Some(97940), "S1 row must remain linked to S1");
    assert_eq!(linked_id(&db, s2), Some(195604), "S2 slug row must be repaired to S2 entry");
}

#[test]
fn test_repair_season_mislinks_bare_digit_season() {
    // Real case: series id 2222 "Farming Life in Another World 2",
    // slug "isekai-nonbiri-nouka-temporada-2" linked to AniList 197824
    // ("Farming Life in Another World 2") must NOT be selected or unlinked.
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("AnimeYT", "a", "animeytx").unwrap();
    db.upsert_catalog_anime(&catalog_anime(197824, "Farming Life in Another World 2", &[]), 0).unwrap();

    let s2 = series_with(&db, src, "isekai-nonbiri-nouka-temporada-2", "Farming Life in Another World 2");
    db.set_anilist_id(s2, 197824).unwrap();

    assert_eq!(db.repair_season_mislinks().unwrap(), 0, "correct S2 bare-number entry must not be selected for repair");
    assert_eq!(linked_id(&db, s2), Some(197824));
}

#[test]
fn a_colon_subtitle_never_inherits_a_short_shows_link() {
    // Real bug: "Kami no Tou: Ouji no Kikan" (season 2) was linked to the
    // season-1 entry through the colon-stripped base title.
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("AnimeYT", "a", "animeytx").unwrap();
    let mut tog = catalog_anime(115230, "Tower of God", &[]);
    tog.title_romaji = Some("Kami no Tou: Tower of God".into());
    tog.synonyms = vec!["Kami no Tou".into()];
    db.upsert_catalog_anime(&tog, 0).unwrap();
    let s2 = series_with(&db, src, "tog2", "Kami no Tou: Ouji no Kikan");

    db.link_series_to_catalog().unwrap();
    assert_eq!(linked_id(&db, s2), None);
}

#[test]
fn repair_unlinks_a_colon_subtitle_that_was_linked_through_the_base_title() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("AnimeYT", "a", "animeytx").unwrap();
    db.upsert_catalog_anime(&catalog_anime(115230, "Tower of God", &[]), 0).unwrap(); // 12 episodes
    let s2 = series_with(&db, src, "tog2", "Tower of God: Ouji no Kikan");
    db.set_anilist_id(s2, 115230).unwrap();
    let mut op = catalog_anime(21, "One Piece", &[]);
    op.episodes = None;
    op.next_episode = Some(1100);
    db.upsert_catalog_anime(&op, 1).unwrap();
    let arc = series_with(&db, src, "op-arc", "One Piece: Arco de Elbaph");
    db.set_anilist_id(arc, 21).unwrap();

    assert_eq!(db.repair_season_mislinks().unwrap(), 1);
    assert_eq!(linked_id(&db, s2), None);
    assert_eq!(linked_id(&db, arc), Some(21), "arcs of a long-runner keep the parent link");
}

#[test]
fn repair_keeps_a_colon_row_linked_to_an_entry_with_the_same_title_even_when_a_duplicate_is_more_popular() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("AnimeYT", "a", "animeytx").unwrap();
    // A series and its movie share the exact title; the movie is more popular.
    let mut tv = catalog_anime(1, "Foo: The Story", &[]);
    tv.popularity = Some(10);
    let mut movie = catalog_anime(2, "Foo: The Story", &[]);
    movie.popularity = Some(9000);
    db.upsert_catalog_anime(&tv, 0).unwrap();
    db.upsert_catalog_anime(&movie, 1).unwrap();
    let row = series_with(&db, src, "foo", "Foo: The Story");
    db.set_anilist_id(row, 1).unwrap();
    // and a row linked through its parenthetical spelling
    let mut inner = catalog_anime(3, "Baz Qux", &[]);
    inner.popularity = Some(5);
    db.upsert_catalog_anime(&inner, 2).unwrap();
    let paren = series_with(&db, src, "bq", "Foo: Bar (Baz Qux)");
    db.set_anilist_id(paren, 3).unwrap();

    assert_eq!(db.repair_season_mislinks().unwrap(), 0);
    assert_eq!(linked_id(&db, row), Some(1));
    assert_eq!(linked_id(&db, paren), Some(3));
}

#[test]
fn repair_clears_a_wrong_season_link_when_no_right_entry_exists() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("AnimeYT", "a", "animeytx").unwrap();
    db.upsert_catalog_anime(&catalog_anime(113813, "Kanojo, Okarishimasu", &[]), 0).unwrap();
    let s5 = series_with(&db, src, "k5", "Kanojo, Okarishimasu Temporada 5");
    db.set_anilist_id(s5, 113813).unwrap();

    assert_eq!(db.repair_season_mislinks().unwrap(), 1);
    assert_eq!(linked_id(&db, s5), None, "a wrong link is worse than none");
    // and a correct link is never touched
    let ok = series_with(&db, src, "k1", "Kanojo, Okarishimasu");
    db.set_anilist_id(ok, 113813).unwrap();
    assert_eq!(db.repair_season_mislinks().unwrap(), 0);
    assert_eq!(linked_id(&db, ok), Some(113813));
}

#[test]
fn a_sibling_on_another_site_with_the_same_season_lends_its_link() {
    // Site A's title matches the catalog entry; site B's spelling of the SAME
    // season matches nothing by itself, but shares franchise key and season.
    let db = Db::open(":memory:").unwrap();
    let a = db.upsert_source("AnimeYT", "a", "animeytx").unwrap();
    let b = db.upsert_source("Gogo", "g", "gogoanime").unwrap();
    db.upsert_catalog_anime(&catalog_anime(500, "Pretty Name 2nd Season", &[]), 0).unwrap();
    let site_a = series_with(&db, a, "pn-a", "Pretty Name 2nd Season");
    db.set_anilist_id(site_a, 500).unwrap();
    let site_b = series_with(&db, b, "pn-b", "Pretty Name Temporada 2");
    // a different season of the same franchise must NOT inherit it
    let other = series_with(&db, b, "pn-b3", "Pretty Name Temporada 3");

    db.link_series_to_catalog().unwrap();
    assert_eq!(linked_id(&db, site_b), Some(500));
    assert_eq!(linked_id(&db, other), None, "season 3 must not borrow season 2's link");
}

#[test]
fn link_series_to_catalog_leaves_a_title_with_no_catalog_match_unlinked() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("GogoAnime", "g", "gogoanime").unwrap();
    let sid = db.upsert_series(src, &crate::models::Series {
        id: 0, slug: "totally-obscure".into(), title: "Totally Obscure Show Nobody Synced".into(),
        url: "https://gogoanime.example/series/totally-obscure".into(),
        cover_url: None, is_airing: true, followed: true,
        next_episode_at: None, site_episode_count: None,
    }).unwrap();
    db.set_followed(sid, true).unwrap();

    assert_eq!(db.link_series_to_catalog().unwrap(), 0);
    assert_eq!(
        db.conn.query_row("SELECT anilist_id FROM series WHERE id=?1", [sid], |r| r.get::<_, Option<i64>>(0)).unwrap(),
        None
    );
}

#[test]
fn list_catalog_filtered_with_default_filter_matches_list_catalog() {
    let db = Db::open(":memory:").unwrap();
    seed_filter_catalog(&db);

    let default_filter = db.list_catalog_filtered(1, 100, &CatalogFilter::default()).unwrap();
    let legacy = db.list_catalog(1, 100).unwrap();
    assert_eq!(
        default_filter.iter().map(|a| a.id).collect::<Vec<_>>(),
        legacy.iter().map(|a| a.id).collect::<Vec<_>>()
    );
    assert_eq!(
        db.catalog_count_filtered(&CatalogFilter::default()).unwrap(),
        db.catalog_count().unwrap()
    );
}

#[test]
fn list_catalog_filtered_search_is_case_insensitive_substring() {
    let db = Db::open(":memory:").unwrap();
    seed_filter_catalog(&db);

    let filter = CatalogFilter { search: Some("mONSteR".into()), ..Default::default() };
    let rows = db.list_catalog_filtered(1, 100, &filter).unwrap();
    assert_eq!(
        rows.iter().map(|a| a.title.as_str()).collect::<Vec<_>>(),
        vec!["Monster", "Monster Girl"]
    );
    assert_eq!(db.catalog_count_filtered(&filter).unwrap(), 2);
}

#[test]
fn list_catalog_filtered_multi_genre_uses_and_semantics() {
    let db = Db::open(":memory:").unwrap();
    seed_filter_catalog(&db);

    // "Fullmetal Alchemist" (Drama+Action) and "Unknown Length" (Drama+Action)
    // have both genres; "Monster" (Drama+Mystery) and "ShortRun" (Action only)
    // have only one of the two and must be excluded.
    let filter = CatalogFilter {
        genres: vec!["Drama".into(), "Action".into()],
        ..Default::default()
    };
    let rows = db.list_catalog_filtered(1, 100, &filter).unwrap();
    let titles: Vec<&str> = rows.iter().map(|a| a.title.as_str()).collect();
    assert_eq!(titles, vec!["Fullmetal Alchemist", "Unknown Length"]);
    assert_eq!(db.catalog_count_filtered(&filter).unwrap(), 2);
}

#[test]
fn list_catalog_filtered_format_exact_match() {
    let db = Db::open(":memory:").unwrap();
    seed_filter_catalog(&db);

    let filter = CatalogFilter { format: Some("MOVIE".into()), ..Default::default() };
    let rows = db.list_catalog_filtered(1, 100, &filter).unwrap();
    assert_eq!(rows.iter().map(|a| a.title.as_str()).collect::<Vec<_>>(), vec!["OnlyOne"]);
}

#[test]
fn list_catalog_filtered_studio_exact_match() {
    let db = Db::open(":memory:").unwrap();
    let mut a = catalog_anime_with_popularity(1, "MadeByA", &["Drama"], Some(1000));
    a.studio = Some("Studio A".into());
    db.upsert_catalog_anime(&a, 0).unwrap();
    let mut b = catalog_anime_with_popularity(2, "MadeByB", &["Drama"], Some(1000));
    b.studio = Some("Studio B".into());
    db.upsert_catalog_anime(&b, 1).unwrap();
    // No studio at all — must not match either filter value.
    db.upsert_catalog_anime(&catalog_anime_with_popularity(3, "NoStudio", &["Drama"], Some(1000)), 2).unwrap();

    let filter = CatalogFilter { studio: Some("Studio A".into()), ..Default::default() };
    let rows = db.list_catalog_filtered(1, 100, &filter).unwrap();
    assert_eq!(rows.iter().map(|a| a.title.as_str()).collect::<Vec<_>>(), vec!["MadeByA"]);
    assert_eq!(db.catalog_count_filtered(&filter).unwrap(), 1);
}

#[test]
fn distinct_catalog_studios_returns_alphabetical_non_null() {
    let db = Db::open(":memory:").unwrap();
    let mut a = catalog_anime_with_popularity(1, "First", &[], Some(1000));
    a.studio = Some("Zeta Studio".into());
    db.upsert_catalog_anime(&a, 0).unwrap();
    let mut b = catalog_anime_with_popularity(2, "Second", &[], Some(1000));
    b.studio = Some("Alpha Studio".into());
    db.upsert_catalog_anime(&b, 1).unwrap();
    // No studio — must not produce a NULL entry in the vocabulary.
    db.upsert_catalog_anime(&catalog_anime_with_popularity(3, "Third", &[], Some(1000)), 2).unwrap();

    let studios = db.distinct_catalog_studios().unwrap();
    assert_eq!(studios, vec!["Alpha Studio".to_string(), "Zeta Studio".to_string()]);
}

#[test]
fn list_catalog_filtered_min_score_is_inclusive_floor() {
    let db = Db::open(":memory:").unwrap();
    seed_filter_catalog(&db);

    let filter = CatalogFilter { min_score: Some(70), ..Default::default() };
    let rows = db.list_catalog_filtered(1, 100, &filter).unwrap();
    // scores >= 70: Monster(90), Fullmetal(88), OnlyOne(70), MidRun(72)
    let mut titles: Vec<&str> = rows.iter().map(|a| a.title.as_str()).collect();
    titles.sort();
    assert_eq!(titles, vec!["Fullmetal Alchemist", "MidRun", "Monster", "OnlyOne"]);
}

#[test]
fn list_catalog_filtered_episode_bucket_boundaries() {
    let db = Db::open(":memory:").unwrap();
    seed_filter_catalog(&db);

    let bucket = |b: &str| {
        let filter = CatalogFilter { episodes: Some(b.into()), ..Default::default() };
        let mut titles: Vec<String> = db
            .list_catalog_filtered(1, 100, &filter)
            .unwrap()
            .iter()
            .map(|a| a.title.clone())
            .collect();
        titles.sort();
        titles
    };

    assert_eq!(bucket("1"), vec!["OnlyOne".to_string()]);
    assert_eq!(bucket("2-12"), vec!["Monster Girl".to_string(), "ShortRun".to_string()]);
    assert_eq!(bucket("13-26"), vec!["MidRun".to_string()]);
    assert_eq!(bucket("27+"), vec!["Fullmetal Alchemist".to_string(), "LongRun".to_string(), "Monster".to_string()]);
    assert_eq!(bucket("unknown"), vec!["Unknown Length".to_string()]);
}

#[test]
fn list_catalog_filtered_combines_all_filters() {
    let db = Db::open(":memory:").unwrap();
    seed_filter_catalog(&db);

    let filter = CatalogFilter {
        search: Some("run".into()),
        genres: vec!["Action".into()],
        format: Some("TV".into()),
        min_score: Some(60),
        episodes: Some("13-26".into()),
        ..Default::default()
    };
    let rows = db.list_catalog_filtered(1, 100, &filter).unwrap();
    assert_eq!(rows.iter().map(|a| a.title.as_str()).collect::<Vec<_>>(), vec!["MidRun"]);
    assert_eq!(db.catalog_count_filtered(&filter).unwrap(), 1);
}

#[test]
fn catalog_count_filtered_agrees_with_paginated_totals() {
    let db = Db::open(":memory:").unwrap();
    seed_filter_catalog(&db);

    let filter = CatalogFilter { genres: vec!["Action".into()], ..Default::default() };
    let total = db.catalog_count_filtered(&filter).unwrap();

    let mut seen = std::collections::HashSet::new();
    let mut page = 1;
    loop {
        let rows = db.list_catalog_filtered(page, 2, &filter).unwrap();
        if rows.is_empty() {
            break;
        }
        for r in &rows {
            seen.insert(r.id);
        }
        page += 1;
    }
    assert_eq!(seen.len() as i64, total);
}

#[test]
fn get_catalog_titles_reads_all_three_title_fields() {
    let db = Db::open(":memory:").unwrap();
    let anime = crate::anilist::CatalogAnime {
        id: 42,
        title: "Attack on Titan".into(),
        title_romaji: Some("Shingeki no Kyojin".into()),
        title_english: Some("Attack on Titan".into()),
        synonyms: Vec::new(), next_airing_at: None, next_episode: None, tags: Vec::new(), relations: Vec::new(),
        cover_url: None,
        banner_url: None,
        format: Some("TV".into()),
        genres: vec!["Action".into()],
        episodes: Some(25),
        average_score: Some(90),
        popularity: Some(1000),
        url: "https://anilist.co/anime/42".into(),
        status: None,
        duration: None,
        studio: None,
        start_date: None,
    };
    db.upsert_catalog_anime(&anime, 0).unwrap();

    let (title, romaji, english) = db.get_catalog_titles(42).unwrap().unwrap();
    assert_eq!(title, "Attack on Titan");
    assert_eq!(romaji.as_deref(), Some("Shingeki no Kyojin"));
    assert_eq!(english.as_deref(), Some("Attack on Titan"));
}

#[test]
fn upsert_catalog_anime_round_trips_duration() {
    let db = Db::open(":memory:").unwrap();

    // A row with a real synced duration.
    let with_duration = crate::anilist::CatalogAnime {
        id: 43,
        title: "Timed Show".into(),
        title_romaji: None,
        title_english: None, synonyms: Vec::new(), next_airing_at: None, next_episode: None, tags: Vec::new(), relations: Vec::new(),
        cover_url: None,
        banner_url: None,
        format: Some("TV".into()),
        genres: vec![],
        episodes: Some(12),
        average_score: None,
        popularity: None,
        url: "https://anilist.co/anime/43".into(),
        status: None,
        duration: Some(23),
        studio: None,
        start_date: None,
    };
    db.upsert_catalog_anime(&with_duration, 0).unwrap();

    // A row that never sets duration (mirrors a pre-existing/unsynced
    // row) — must default to None, not error or coerce to 0.
    let without_duration = crate::anilist::CatalogAnime {
        id: 44,
        title: "Undated Show".into(),
        title_romaji: None,
        title_english: None, synonyms: Vec::new(), next_airing_at: None, next_episode: None, tags: Vec::new(), relations: Vec::new(),
        cover_url: None,
        banner_url: None,
        format: Some("TV".into()),
        genres: vec![],
        episodes: Some(12),
        average_score: None,
        popularity: None,
        url: "https://anilist.co/anime/44".into(),
        status: None,
        duration: None,
        studio: None,
        start_date: None,
    };
    db.upsert_catalog_anime(&without_duration, 1).unwrap();

    let page = db.list_catalog(1, 10).unwrap();
    let read_with = page.iter().find(|a| a.id == 43).unwrap();
    assert_eq!(read_with.duration, Some(23), "real synced duration must round-trip");
    let read_without = page.iter().find(|a| a.id == 44).unwrap();
    assert_eq!(read_without.duration, None, "unset duration must read back as None, not 0");
}

#[test]
fn upsert_catalog_anime_round_trips_studio() {
    let db = Db::open(":memory:").unwrap();

    // A row with a real synced studio.
    let with_studio = crate::anilist::CatalogAnime {
        id: 45,
        title: "Studio Show".into(),
        title_romaji: None,
        title_english: None, synonyms: Vec::new(),
        cover_url: None,
        banner_url: None,
        format: Some("TV".into()),
        genres: vec![],
        episodes: Some(12),
        average_score: None,
        popularity: None,
        url: "https://anilist.co/anime/45".into(),
        status: None,
        duration: None,
        studio: Some("Studio Ghibli".into()),
        start_date: None,
        next_airing_at: None, next_episode: None, tags: Vec::new(), relations: Vec::new(),
    };
    db.upsert_catalog_anime(&with_studio, 0).unwrap();

    // A row that never sets studio (mirrors a pre-existing/unsynced row,
    // or a title AniList credits no studio for) — must default to None,
    // not error or coerce to an empty string.
    let without_studio = crate::anilist::CatalogAnime {
        id: 46,
        title: "No Studio Show".into(),
        title_romaji: None,
        title_english: None, synonyms: Vec::new(),
        cover_url: None,
        banner_url: None,
        format: Some("TV".into()),
        genres: vec![],
        episodes: Some(12),
        average_score: None,
        popularity: None,
        url: "https://anilist.co/anime/46".into(),
        status: None,
        duration: None,
        studio: None,
        start_date: None,
        next_airing_at: None, next_episode: None, tags: Vec::new(), relations: Vec::new(),
    };
    db.upsert_catalog_anime(&without_studio, 1).unwrap();

    let page = db.list_catalog(1, 10).unwrap();
    let read_with = page.iter().find(|a| a.id == 45).unwrap();
    assert_eq!(read_with.studio.as_deref(), Some("Studio Ghibli"), "real synced studio must round-trip");
    let read_without = page.iter().find(|a| a.id == 46).unwrap();
    assert_eq!(read_without.studio, None, "unset studio must read back as None, not empty string");
}

#[test]
fn upsert_catalog_anime_round_trips_start_date() {
    let db = Db::open(":memory:").unwrap();

    let with_date = crate::anilist::CatalogAnime {
        id: 47,
        title: "Dated Show".into(),
        title_romaji: None,
        title_english: None, synonyms: Vec::new(),
        cover_url: None,
        banner_url: None,
        format: Some("TV".into()),
        genres: vec![],
        episodes: Some(12),
        average_score: None,
        popularity: None,
        url: "https://anilist.co/anime/47".into(),
        status: None,
        duration: None,
        studio: None,
        start_date: Some(1_776_211_200), // 2026-04-15T00:00:00Z
        next_airing_at: None, next_episode: None, tags: Vec::new(), relations: Vec::new(),
    };
    db.upsert_catalog_anime(&with_date, 0).unwrap();

    let without_date = crate::anilist::CatalogAnime {
        id: 48,
        title: "Undated Show".into(),
        title_romaji: None,
        title_english: None, synonyms: Vec::new(),
        cover_url: None,
        banner_url: None,
        format: Some("TV".into()),
        genres: vec![],
        episodes: Some(12),
        average_score: None,
        popularity: None,
        url: "https://anilist.co/anime/48".into(),
        status: None,
        duration: None,
        studio: None,
        start_date: None,
        next_airing_at: None, next_episode: None, tags: Vec::new(), relations: Vec::new(),
    };
    db.upsert_catalog_anime(&without_date, 1).unwrap();

    let page = db.list_catalog(1, 10).unwrap();
    let read_with = page.iter().find(|a| a.id == 47).unwrap();
    assert_eq!(read_with.start_date, Some(1_776_211_200), "real synced start_date must round-trip");
    let read_without = page.iter().find(|a| a.id == 48).unwrap();
    assert_eq!(read_without.start_date, None, "unset start_date must read back as None, not 0");
}

#[test]
fn catalog_start_dates_by_normalized_title_covers_all_title_variants_and_skips_null() {
    let db = Db::open(":memory:").unwrap();

    // Matched under its romaji title on the (hypothetical) scraped site,
    // even though `title` itself collapsed to English.
    db.upsert_catalog_anime(
        &crate::anilist::CatalogAnime {
            id: 50, title: "Attack on Titan".into(), title_romaji: Some("Shingeki no Kyojin".into()),
            title_english: Some("Attack on Titan".into()), synonyms: Vec::new(), cover_url: None, banner_url: None, format: Some("TV".into()),
            genres: vec![], episodes: Some(25), average_score: None, popularity: None,
            url: "https://anilist.co/anime/50".into(), status: None, duration: None, studio: None,
            start_date: Some(1_776_211_200),
            next_airing_at: None, next_episode: None, tags: Vec::new(), relations: Vec::new(),
        },
        0,
    ).unwrap();

    // No start_date synced yet — must be entirely absent from the map,
    // not present with a None/0 value.
    db.upsert_catalog_anime(
        &crate::anilist::CatalogAnime {
            id: 51, title: "Unsynced Show".into(), title_romaji: None, title_english: None, synonyms: Vec::new(),
            cover_url: None, banner_url: None, format: Some("TV".into()), genres: vec![], episodes: None,
            average_score: None, popularity: None, url: "https://anilist.co/anime/51".into(),
            status: None, duration: None, studio: None, start_date: None,
            next_airing_at: None, next_episode: None, tags: Vec::new(), relations: Vec::new(),
        },
        1,
    ).unwrap();

    let map = db.catalog_start_dates_by_normalized_title().unwrap();
    assert_eq!(map.get(&crate::matching::normalize_title("Attack on Titan")), Some(&1_776_211_200));
    assert_eq!(map.get(&crate::matching::normalize_title("Shingeki no Kyojin")), Some(&1_776_211_200));
    assert_eq!(map.get(&crate::matching::normalize_title("Unsynced Show")), None);
    assert_eq!(map.len(), 2, "one entry per distinct title/romaji/english variant with a real start_date");
}

#[test]
fn upsert_catalog_anime_stores_and_rewrites_relations() {
    let db = Db::open(":memory:").unwrap();

    let mut anime = crate::anilist::CatalogAnime {
        id: 99,
        title: "Relations Show".into(),
        title_romaji: None,
        title_english: None,
        synonyms: Vec::new(),
        next_airing_at: None,
        next_episode: None,
        tags: Vec::new(),
        relations: vec![
            crate::anilist::CatalogRelation { related_id: 100, relation_type: "SEQUEL".into() },
            crate::anilist::CatalogRelation { related_id: 98, relation_type: "PREQUEL".into() },
        ],
        cover_url: None,
        banner_url: None,
        format: Some("TV".into()),
        genres: vec![],
        episodes: Some(12),
        average_score: None,
        popularity: None,
        url: "https://anilist.co/anime/99".into(),
        status: None,
        duration: None,
        studio: None,
        start_date: None,
    };
    db.upsert_catalog_anime(&anime, 0).unwrap();

    let stored = db.list_catalog_relations(99).unwrap();
    assert_eq!(
        stored,
        vec![(98, "PREQUEL".to_string()), (100, "SEQUEL".to_string())]
    );

    // Second upsert with fewer relations leaves only the new set.
    anime.relations = vec![
        crate::anilist::CatalogRelation { related_id: 100, relation_type: "SEQUEL".into() },
    ];
    db.upsert_catalog_anime(&anime, 0).unwrap();

    let updated = db.list_catalog_relations(99).unwrap();
    assert_eq!(updated, vec![(100, "SEQUEL".to_string())]);
}

fn mk_catalog_anime(id: i64, title: &str, url: &str) -> crate::anilist::CatalogAnime {
    crate::anilist::CatalogAnime {
        id, title: title.into(), title_romaji: None, title_english: None, synonyms: Vec::new(), cover_url: None,
        banner_url: None, format: Some("TV".into()), genres: vec![], episodes: Some(12), average_score: None,
        popularity: None, url: url.into(), status: None, duration: None, studio: None, start_date: None,
        next_airing_at: None, next_episode: None, tags: Vec::new(), relations: Vec::new(),
    }
}

#[test]
fn catalog_info_for_series_uses_direct_anilist_id_when_linked() {
    let db = Db::open(":memory:").unwrap();
    db.upsert_catalog_anime(&mk_catalog_anime(70, "Frieren", "https://anilist.co/anime/70"), 0).unwrap();
    let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
    let sid = db.upsert_series(src, &mk_airing("frieren-site", "Some Other Scraped Title", None)).unwrap();
    db.set_anilist_id(sid, 70).unwrap();

    // Direct id join wins even though the scraped title wouldn't
    // title-match — proves it doesn't fall through to the title lookup.
    let info = db.catalog_info_for_series(sid).unwrap().expect("linked match");
    assert_eq!(info.url, "https://anilist.co/anime/70");
}

#[test]
fn catalog_info_for_series_falls_back_to_normalized_title_match_when_unlinked() {
    let db = Db::open(":memory:").unwrap();
    db.upsert_catalog_anime(&mk_catalog_anime(71, "Bocchi the Rock!", "https://anilist.co/anime/71"), 0).unwrap();
    let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
    // Followed/scraped series never get anilist_id set in practice — no
    // set_anilist_id call here, matching real-world coverage.
    let sid = db.upsert_series(src, &mk_airing("bocchi", "Bocchi The Rock! Sub Español", None)).unwrap();

    let info = db.catalog_info_for_series(sid).unwrap().expect("title match");
    assert_eq!(info.url, "https://anilist.co/anime/71");
}

#[test]
fn catalog_info_for_series_uses_a_linked_sibling_on_another_site() {
    // The same show on two sites: one row was linked (e.g. by the fuzzy
    // linker), the other is unlinked and its title does not match any
    // catalog title — it must still resolve through its canonical twin
    // instead of reading as "not on AniList".
    let db = Db::open(":memory:").unwrap();
    db.upsert_catalog_anime(&mk_catalog_anime(70, "Sousou no Frieren", "https://anilist.co/anime/70"), 0).unwrap();
    let a = db.upsert_source("AnimeYT", "a", "animeytx").unwrap();
    let b = db.upsert_source("TioAnime", "b", "tioanime").unwrap();
    let linked = db.upsert_series(a, &mk_airing("frieren-a", "Frieren: Beyond Journey's End", None)).unwrap();
    db.set_anilist_id(linked, 70).unwrap();
    let unlinked = db.upsert_series(b, &mk_airing("frieren-b", "Frieren: Beyond Journey's End", None)).unwrap();

    let info = db.catalog_info_for_series(unlinked).unwrap().expect("resolved via sibling");
    assert_eq!(info.id, 70);
    assert_eq!(info.url, "https://anilist.co/anime/70");
}

#[test]
fn catalog_info_for_series_ignores_a_dangling_anilist_id() {
    // anilist_id points at a row that was never synced (or since removed)
    // — must fall through to the title-match path instead of erroring.
    let db = Db::open(":memory:").unwrap();
    db.upsert_catalog_anime(&mk_catalog_anime(72, "Real Match", "https://anilist.co/anime/72"), 0).unwrap();
    let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
    let sid = db.upsert_series(src, &mk_airing("real-match", "Real Match", None)).unwrap();
    db.set_anilist_id(sid, 9999).unwrap(); // never synced

    let info = db.catalog_info_for_series(sid).unwrap().expect("falls through to title match");
    assert_eq!(info.url, "https://anilist.co/anime/72");
}

fn mk_full_catalog_anime(id: i64, title: &str, genres: &[&str]) -> crate::anilist::CatalogAnime {
    crate::anilist::CatalogAnime {
        id, title: title.into(), title_romaji: None, title_english: None, synonyms: Vec::new(),
        cover_url: Some(format!("https://img/{id}")), banner_url: None, format: Some("TV".into()),
        genres: genres.iter().map(|g| g.to_string()).collect(), episodes: Some(24),
        average_score: Some(88), popularity: Some(5000), url: format!("https://anilist.co/anime/{id}"),
        status: Some("FINISHED".into()), duration: Some(24), studio: Some("Studio X".into()),
        start_date: None,
        next_airing_at: None, next_episode: None, tags: Vec::new(), relations: Vec::new(),
    }
}

#[test]
fn catalog_info_for_series_returns_full_matched_row_with_genres() {
    let db = Db::open(":memory:").unwrap();
    let anime = mk_full_catalog_anime(80, "Frieren", &["Adventure", "Drama"]);
    db.upsert_catalog_anime(&anime, 0).unwrap();
    let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
    let sid = db.upsert_series(src, &mk_airing("frieren", "Frieren Sub Español", None)).unwrap();

    let info = db.catalog_info_for_series(sid).unwrap().expect("should title-match");
    assert_eq!(info.id, 80);
    assert_eq!(info.studio.as_deref(), Some("Studio X"));
    assert_eq!(info.average_score, Some(88));
    assert_eq!(info.genres, vec!["Adventure".to_string(), "Drama".to_string()]);
}

#[test]
fn catalog_info_for_series_none_when_unmatched() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
    let sid = db.upsert_series(src, &mk_airing("obscure", "Totally Unmatched Title", None)).unwrap();

    assert_eq!(db.catalog_info_for_series(sid).unwrap(), None);
}

#[test]
fn link_series_to_catalog_links_squashed_spacing_and_punctuation_variants() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("TioAnime", "t", "tioanime").unwrap();

    // Seed catalog with target titles differing only by punctuation / whitespace
    db.upsert_catalog_anime(&catalog_anime(184356, "Dogul Wang", &["Action"]), 0).unwrap();
    db.upsert_catalog_anime(
        &catalog_anime(199066, "Hanaori-san wa Tensei Shite mo Kenka ga Shitai", &["Romance"]),
        1,
    )
    .unwrap();
    db.upsert_catalog_anime(&catalog_anime(300, "Let's Go Kaiki-gumi", &["Comedy"]), 2).unwrap();
    db.upsert_catalog_anime(&catalog_anime(400, "Hime Kishi wa Barbaroi no Yome", &["Fantasy"]), 3).unwrap();
    db.upsert_catalog_anime(&catalog_anime(500, "Ichijouma Mankitsu Gurashi!", &["Slice of Life"]), 4).unwrap();
    db.upsert_catalog_anime(
        &catalog_anime(600, "Taiari deshita: Ojousama wa Kakutou Game nante Shinai", &["Comedy"]),
        5,
    )
    .unwrap();

    let test_cases = [
        ("dogulwang", "Dogulwang", 184356),
        ("hanaori", "Hanaori-san wa Tensei shitemo Kenka ga Shitai", 199066),
        ("kaiki", "Let’s Go Kaiki-gumi", 300), // curly apostrophe
        ("himekishi", "Himekishi wa Barbaroi no Yome", 400),
        ("ichijouma", "Ichijouma Mankitsugurashi!", 500),
        ("taiari", "Tai-Ari deshita. Ojousama wa Kakutou Game nante Shinai", 600),
    ];

    let mut sids = Vec::new();
    for (slug, title, _) in &test_cases {
        let sid = db
            .upsert_series(
                src,
                &crate::models::Series {
                    id: 0,
                    slug: (*slug).into(),
                    title: (*title).into(),
                    url: format!("https://tioanime.example/series/{slug}"),
                    cover_url: None,
                    is_airing: true,
                    followed: true,
                    next_episode_at: None,
                    site_episode_count: None,
                },
            )
            .unwrap();
        db.set_followed(sid, true).unwrap();
        sids.push(sid);
    }

    let linked = db.link_series_to_catalog().unwrap();
    assert_eq!(linked, 6);

    for (sid, (_, _, expected_id)) in sids.iter().zip(test_cases.iter()) {
        let actual_id = db
            .conn
            .query_row(
                "SELECT anilist_id FROM series WHERE id=?1",
                [sid],
                |r| r.get::<_, Option<i64>>(0),
            )
            .unwrap();
        assert_eq!(actual_id, Some(*expected_id));
    }
}

#[test]
fn link_series_to_catalog_short_title_stays_unlinked() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("TioAnime", "t", "tioanime").unwrap();
    db.upsert_catalog_anime(&catalog_anime(700, "Hero", &["Action"]), 0).unwrap();

    // "He ro" squashes to "hero" (4 chars < 6), must not match
    let sid = db
        .upsert_series(
            src,
            &crate::models::Series {
                id: 0,
                slug: "he-ro".into(),
                title: "He ro".into(),
                url: "https://tioanime.example/series/he-ro".into(),
                cover_url: None,
                is_airing: true,
                followed: true,
                next_episode_at: None,
                site_episode_count: None,
            },
        )
        .unwrap();
    db.set_followed(sid, true).unwrap();

    let linked = db.link_series_to_catalog().unwrap();
    assert_eq!(linked, 0);
    let actual_id = db
        .conn
        .query_row(
            "SELECT anilist_id FROM series WHERE id=?1",
            [sid],
            |r| r.get::<_, Option<i64>>(0),
        )
        .unwrap();
    assert_eq!(actual_id, None, "short squashed key (< 6 chars) must stay unlinked");
}

#[test]
fn stale_status_ids_selects_airing_linked_releasing_and_recent_finished() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("TestSite", "https://site.example", "testsite").unwrap();
    let now = chrono::Utc::now().timestamp();

    // 1. Linked to a series with is_airing=1 (even if status is FINISHED and start_date is old)
    let mut anime1 = catalog_anime_with_popularity(1, "AiringLinked", &[], Some(100));
    anime1.status = Some("FINISHED".into());
    anime1.start_date = Some(now - 300 * 86400); // 300 days ago
    db.upsert_catalog_anime(&anime1, 0).unwrap();
    let mut s1 = mk_airing("s1", "AiringLinked", None);
    s1.is_airing = true;
    let sid1 = db.upsert_series(src, &s1).unwrap();
    db.set_anilist_id(sid1, 1).unwrap();

    // 2. Unlinked RELEASING
    let mut anime2 = catalog_anime_with_popularity(2, "Releasing", &[], Some(200));
    anime2.status = Some("RELEASING".into());
    db.upsert_catalog_anime(&anime2, 1).unwrap();

    // 3. Unlinked NOT_YET_RELEASED
    let mut anime3 = catalog_anime_with_popularity(3, "Upcoming", &[], Some(300));
    anime3.status = Some("NOT_YET_RELEASED".into());
    db.upsert_catalog_anime(&anime3, 2).unwrap();

    // 4. Unlinked FINISHED within last 120 days (e.g. 60 days ago)
    let mut anime4 = catalog_anime_with_popularity(4, "RecentFinished", &[], Some(400));
    anime4.status = Some("FINISHED".into());
    anime4.start_date = Some(now - 60 * 86400);
    db.upsert_catalog_anime(&anime4, 3).unwrap();

    // 5. Excluded: Unlinked FINISHED older than 120 days (e.g. 150 days ago)
    let mut anime5 = catalog_anime_with_popularity(5, "OldFinished", &[], Some(500));
    anime5.status = Some("FINISHED".into());
    anime5.start_date = Some(now - 150 * 86400);
    db.upsert_catalog_anime(&anime5, 4).unwrap();

    // 6. Excluded: Linked to series with is_airing=0, FINISHED and old start_date
    let mut anime6 = catalog_anime_with_popularity(6, "NonAiringLinkedOld", &[], Some(600));
    anime6.status = Some("FINISHED".into());
    anime6.start_date = Some(now - 200 * 86400);
    db.upsert_catalog_anime(&anime6, 5).unwrap();
    let mut s6 = mk_airing("s6", "NonAiringLinkedOld", None);
    s6.is_airing = false;
    let sid6 = db.upsert_series(src, &s6).unwrap();
    db.set_anilist_id(sid6, 6).unwrap();

    // 7. Excluded: Unlinked CANCELLED
    let mut anime7 = catalog_anime_with_popularity(7, "Cancelled", &[], Some(700));
    anime7.status = Some("CANCELLED".into());
    db.upsert_catalog_anime(&anime7, 6).unwrap();

    let ids = db.stale_status_ids().unwrap();
    assert!(ids.contains(&1), "must include linked airing row");
    assert!(ids.contains(&2), "must include RELEASING row");
    assert!(ids.contains(&3), "must include NOT_YET_RELEASED row");
    assert!(ids.contains(&4), "must include FINISHED within 120 days");
    assert!(!ids.contains(&5), "must exclude old FINISHED row");
    assert!(!ids.contains(&6), "must exclude non-airing old FINISHED row");
    assert!(!ids.contains(&7), "must exclude CANCELLED row");
}

/// Regression: `stale_status_ids` correlates on `series.anilist_id` once per
/// catalog row. Without an index on that column it scanned `series` for each
/// of the ~22k catalog rows (twice), ~220 s on a real library, all while
/// holding the DB mutex that the UI's synchronous commands need: the window
/// froze. The plan must use the index, and a realistically sized library
/// must answer in well under a second.
#[test]
fn stale_status_ids_uses_the_series_anilist_index_and_is_fast_at_real_size() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("TestSite", "https://site.example", "testsite").unwrap();

    // The plan: no full scan of `series` inside the correlated subqueries.
    let plan: Vec<String> = db
        .conn
        .prepare(&format!("EXPLAIN QUERY PLAN {STALE_STATUS_IDS_SQL}"))
        .unwrap()
        .query_map([0i64, 0i64], |r| r.get::<_, String>(3))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert!(
        plan.iter().any(|d| d.contains("idx_series_anilist_id")),
        "correlated subquery must use idx_series_anilist_id, plan was: {plan:?}"
    );
    // A full scan of `series` (any SQLite wording: "SCAN s", "SCAN TABLE s", with or without
    // an alias suffix) would be the quadratic behaviour. Scans of `c` are expected.
    assert!(
        !plan.iter().any(|d| {
            let d = d.trim();
            d.starts_with("SCAN") && !d.contains(" c") && !d.contains("INDEX") && (d.ends_with(" s") || d.contains("TABLE s") || d.contains("series"))
        }),
        "series must not be fully scanned per catalog row, plan was: {plan:?}"
    );

    // Real-library size: 22k catalog rows, 6k series, ~100 airing and linked.
    let tx = db.conn.unchecked_transaction().unwrap();
    for i in 1..=22_000i64 {
        tx.execute(
            "INSERT INTO anilist_catalog (id, title, cover_url, format, episodes, average_score, url, sort_order, popularity, status)
             VALUES (?1, ?2, NULL, 'TV', 12, 70, ?3, ?1, ?4, ?5)",
            (i, format!("Title {i}"), format!("https://anilist.co/anime/{i}"), i % 5000,
             if i % 200 == 0 { "RELEASING" } else { "FINISHED" }),
        )
        .unwrap();
    }
    for i in 0..6_000i64 {
        let linked = if i % 3 == 0 { Some(i + 1) } else { None };
        tx.execute(
            "INSERT INTO series (source_id, slug, title, url, is_airing, anilist_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            (src, format!("s{i}"), format!("S {i}"), format!("https://x/{i}"), (i % 60 == 0) as i64, linked),
        )
        .unwrap();
    }
    tx.commit().unwrap();

    let started = std::time::Instant::now();
    let ids = db.stale_status_ids().unwrap();
    let took = started.elapsed();
    assert!(!ids.is_empty() && ids.len() <= 500);
    // Without the index this takes minutes; allow generous slack for slow CI debug builds.
    assert!(took < std::time::Duration::from_secs(5), "stale_status_ids took {took:?}");
}

#[test]
fn stale_status_ids_orders_linked_first_and_respects_500_cap() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("TestSite", "https://site.example", "testsite").unwrap();

    // Create an unlinked RELEASING row with huge popularity
    let mut high_pop = catalog_anime_with_popularity(1001, "HighPopUnlinked", &[], Some(100_000));
    high_pop.status = Some("RELEASING".into());
    db.upsert_catalog_anime(&high_pop, 0).unwrap();

    // Create a linked row with low popularity
    let mut low_pop = catalog_anime_with_popularity(1002, "LowPopLinked", &[], Some(10));
    low_pop.status = Some("RELEASING".into());
    db.upsert_catalog_anime(&low_pop, 1).unwrap();
    let s = mk_airing("s1002", "LowPopLinked", None);
    let sid = db.upsert_series(src, &s).unwrap();
    db.set_anilist_id(sid, 1002).unwrap();

    let ids = db.stale_status_ids().unwrap();
    assert_eq!(ids[0], 1002, "linked row must come first despite lower popularity");
    assert_eq!(ids[1], 1001);

    // Populate > 500 rows and assert cap
    for i in 1..=550 {
        let mut a = catalog_anime_with_popularity(2000 + i, &format!("Batch{i}"), &[], Some(i));
        a.status = Some("RELEASING".into());
        db.upsert_catalog_anime(&a, i).unwrap();
    }
    let capped_ids = db.stale_status_ids().unwrap();
    assert_eq!(capped_ids.len(), 500, "must be capped at 500 rows");
}

#[test]
fn upsert_catalog_anime_synonyms_and_catalog_titles_for_index() {
    let db = Db::open(":memory:").unwrap();
    let mut a = catalog_anime(1, "Main Title", &[]);
    a.synonyms = vec!["Synonym One".to_string(), "Synonym Two".to_string()];
    db.upsert_catalog_anime(&a, 0).unwrap();

    let titles = db.catalog_titles_for_index().unwrap();
    assert_eq!(titles.len(), 1);
    assert_eq!(titles[0].titles, vec!["Main Title".to_string()]);
    let mut actual = titles[0].synonyms.clone();
    actual.sort();
    assert_eq!(actual, vec!["Synonym One".to_string(), "Synonym Two".to_string()]);
}

#[test]
fn upsert_catalog_anime_replaces_old_synonyms() {
    let db = Db::open(":memory:").unwrap();
    let mut a = catalog_anime(1, "Main Title", &[]);
    a.synonyms = vec!["Old Synonym".to_string()];
    db.upsert_catalog_anime(&a, 0).unwrap();

    let mut a_new = catalog_anime(1, "Main Title", &[]);
    a_new.synonyms = vec!["New Synonym".to_string()];
    db.upsert_catalog_anime(&a_new, 0).unwrap();

    let titles = db.catalog_titles_for_index().unwrap();
    assert_eq!(titles.len(), 1);
    assert_eq!(titles[0].synonyms, vec!["New Synonym".to_string()]);
}

#[test]
fn catalog_titles_for_index_skips_short_synonyms_and_duplicates() {
    let db = Db::open(":memory:").unwrap();
    let mut a = catalog_anime(1, "Main Title", &[]);
    // "abc" is 3 chars (skip), "Main Title" is a duplicate of the main title (case-insensitive)
    a.synonyms = vec!["abc".to_string(), "main title".to_string(), "Valid Synonym".to_string()];
    db.upsert_catalog_anime(&a, 0).unwrap();

    let titles = db.catalog_titles_for_index().unwrap();
    assert_eq!(titles.len(), 1);
    assert_eq!(titles[0].synonyms, vec!["Valid Synonym".to_string()]);
}

#[test]
fn link_series_to_catalog_matches_synonyms() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("TestSite", "https://site.example", "testsite").unwrap();

    let mut a = catalog_anime(4242, "Hyakunichi Kakumei", &[]);
    a.synonyms = vec!["Bai Ri Cheng Wang".to_string()];
    db.upsert_catalog_anime(&a, 0).unwrap();

    let sid = db.upsert_series(
        src,
        &crate::models::Series {
            id: 0,
            slug: "bai-ri-cheng-wang".into(),
            title: "Bai Ri Cheng Wang".into(),
            url: "https://site/bai-ri-cheng-wang".into(),
            cover_url: None,
            is_airing: true,
            followed: true,
            next_episode_at: None,
            site_episode_count: None,
        },
    ).unwrap();
    db.set_followed(sid, true).unwrap();

    let linked = db.link_series_to_catalog().unwrap();
    assert_eq!(linked, 1);

    let actual_id = db
        .conn
        .query_row(
            "SELECT anilist_id FROM series WHERE id=?1",
            [sid],
            |r| r.get::<_, Option<i64>>(0),
        )
        .unwrap();
    assert_eq!(actual_id, Some(4242));
}

#[test]
fn upsert_catalog_anime_round_trips_next_airing_and_tags() {
    let db = Db::open(":memory:").unwrap();

    let mut a = catalog_anime(100, "Show", &[]);
    a.next_airing_at = Some(12345);
    a.next_episode = Some(5);
    a.tags = vec![
        crate::anilist::CatalogTag { name: "TagA".into(), rank: 90 },
        crate::anilist::CatalogTag { name: "TagB".into(), rank: 80 },
    ];
    db.upsert_catalog_anime(&a, 0).unwrap();

    let mut b = catalog_anime(101, "Finished", &[]);
    b.next_airing_at = None;
    b.next_episode = None;
    b.tags = vec![];
    db.upsert_catalog_anime(&b, 1).unwrap();

    let page = db.list_catalog(1, 10).unwrap();

    let read_a = page.iter().find(|x| x.id == 100).unwrap();
    assert_eq!(read_a.next_airing_at, Some(12345));
    assert_eq!(read_a.next_episode, Some(5));
    assert_eq!(read_a.tags.len(), 2);
    assert_eq!(read_a.tags[0].name, "TagA"); // sorted by rank DESC
    assert_eq!(read_a.tags[1].name, "TagB");

    let read_b = page.iter().find(|x| x.id == 101).unwrap();
    assert_eq!(read_b.next_airing_at, None);
    assert_eq!(read_b.next_episode, None);
    assert!(read_b.tags.is_empty());
}

#[test]
fn upsert_catalog_anime_replaces_tags_and_clears_next_airing() {
    let db = Db::open(":memory:").unwrap();
    let mut a = catalog_anime(100, "Show", &[]);
    a.next_airing_at = Some(12345);
    a.next_episode = Some(5);
    a.tags = vec![crate::anilist::CatalogTag { name: "Old".into(), rank: 90 }];
    db.upsert_catalog_anime(&a, 0).unwrap();

    // Update: replace tags, clear next_airing_at to NULL (finished show)
    let mut a_new = catalog_anime(100, "Show", &[]);
    a_new.next_airing_at = None;
    a_new.next_episode = None;
    a_new.tags = vec![crate::anilist::CatalogTag { name: "New".into(), rank: 85 }];
    db.upsert_catalog_anime(&a_new, 0).unwrap();

    let page = db.list_catalog(1, 10).unwrap();
    let read = page.iter().find(|x| x.id == 100).unwrap();
    assert_eq!(read.next_airing_at, None);
    assert_eq!(read.next_episode, None);
    assert_eq!(read.tags.len(), 1);
    assert_eq!(read.tags[0].name, "New");
}

#[test]
fn improved_search_rules_forget_the_old_misses_once() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("TestSite", "https://site.example", "testsite").unwrap();
    series_with(&db, src, "m", "Missed Title");
    let norm = crate::matching::normalize_title("Missed Title");
    let now = 20_000_000;

    db.set_setting("anilist_search_rules", "1").unwrap(); // misses recorded under old rules
    db.record_search_miss(&norm, now).unwrap();
    assert_eq!(db.unlinked_for_search(10, now).unwrap().len(), 1, "old-rules miss is forgotten");

    db.record_search_miss(&norm, now).unwrap();
    assert!(db.unlinked_for_search(10, now).unwrap().is_empty(), "a miss under the current rules stands");
}

#[test]
fn unlinked_for_search_tests() {
    let db = Db::open(":memory:").unwrap();
    db.set_setting("anilist_search_rules", "2").unwrap(); // current rules: recorded misses stand
    let src = db.upsert_source("TestSite", "https://site.example", "testsite").unwrap();

    let now = 20_000_000;

    // s1: linked -> not offered
    let sid1 = db.upsert_series(src, &crate::models::Series {
        id: 0, slug: "s1".into(), title: "Linked Title".into(), url: "u1".into(), cover_url: None, is_airing: true, followed: true, next_episode_at: None, site_episode_count: None,
    }).unwrap();
    db.set_anilist_id(sid1, 123).unwrap();

    // s2: unlinked, no miss
    let sid2 = db.upsert_series(src, &crate::models::Series {
        id: 0, slug: "s2".into(), title: "Title 2".into(), url: "u2".into(), cover_url: None, is_airing: true, followed: true, next_episode_at: None, site_episode_count: None,
    }).unwrap();

    // s3: unlinked, missed 15 days ago -> offered
    let _sid3 = db.upsert_series(src, &crate::models::Series {
        id: 0, slug: "s3".into(), title: "Title 3".into(), url: "u3".into(), cover_url: None, is_airing: true, followed: true, next_episode_at: None, site_episode_count: None,
    }).unwrap();
    db.record_search_miss(&crate::matching::normalize_title("Title 3"), now - 15 * 24 * 3600).unwrap();

    // s4: unlinked, missed 13 days ago -> not offered
    let _sid4 = db.upsert_series(src, &crate::models::Series {
        id: 0, slug: "s4".into(), title: "Title 4".into(), url: "u4".into(), cover_url: None, is_airing: true, followed: true, next_episode_at: None, site_episode_count: None,
    }).unwrap();
    db.record_search_miss(&crate::matching::normalize_title("Title 4"), now - 13 * 24 * 3600).unwrap();

    let targets = db.unlinked_for_search(10, now).unwrap();
    assert_eq!(targets.len(), 2);

    // They should be "Title 2" and "Title 3"
    let t2 = targets.iter().find(|t| t.title == "Title 2");
    assert!(t2.is_some());
    let t3 = targets.iter().find(|t| t.title == "Title 3");
    assert!(t3.is_some());

    // Check link_search_result links all ids
    let a = catalog_anime(4242, "Linked Anime", &[]);
    db.link_search_result(&[sid2], &a).unwrap();

    // Check that sid2 is linked
    let actual_id: Option<i64> = db.conn.query_row("SELECT anilist_id FROM series WHERE id=?1", [sid2], |r| r.get(0)).unwrap();
    assert_eq!(actual_id, Some(4242));

    // Ensure catalog row is added
    let has_cat: i64 = db.conn.query_row("SELECT COUNT(*) FROM anilist_catalog WHERE id=4242", [], |r| r.get(0)).unwrap();
    assert_eq!(has_cat, 1);
}

#[test]
fn link_resolves_a_temporada_title_through_the_bare_number_spelling() {
    // Real case: AniList lists season 2 as "... desu 2"; the site row says "... Temporada 2".
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("AnimeYT", "a", "animeytx").unwrap();
    let mut s2 = catalog_anime(159309, "Trapped in a Dating Sim Season 2", &[]);
    s2.synonyms = vec!["Otome Game Sekai wa Mob ni Kibishii Sekai desu 2".into()];
    db.upsert_catalog_anime(&s2, 0).unwrap();
    let row = series_with(&db, src, "otome2", "Otome Game Sekai wa Mob ni Kibishii Sekai desu Temporada 2");

    db.link_series_to_catalog().unwrap();
    assert_eq!(linked_id(&db, row), Some(159309));
}

#[test]
fn a_temporada_title_never_takes_the_bare_number_of_an_unrelated_franchise() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("AnimeYT", "a", "animeytx").unwrap();
    // "Something Else 2" exists, but it is another show, not "Mob Show"'s season 2.
    db.upsert_catalog_anime(&catalog_anime(1, "Something Else 2", &[]), 0).unwrap();
    let row = series_with(&db, src, "mob2", "Mob Show Temporada 2");
    db.link_series_to_catalog().unwrap();
    assert_eq!(linked_id(&db, row), None);
}

#[test]
fn query_catalog_matches_tag_isekai_and_genre_filter() {
    let db = Db::open(":memory:").unwrap();
    let mut a1 = catalog_anime(1001, "Tensei Shitara Slime Datta Ken", &["Fantasy"]);
    a1.tags = vec![crate::anilist::CatalogTag { name: "Isekai".into(), rank: 90 }];
    db.upsert_catalog_anime(&a1, 0).unwrap();

    let a2 = catalog_anime(1002, "Ordinary School Life", &["Action"]);
    db.upsert_catalog_anime(&a2, 0).unwrap();

    // Search text "isekai" matches a1 via tag even though title has "Slime" and genre is "Fantasy"
    let search_filter = CatalogFilter {
        search: Some("isekai".into()),
        ..Default::default()
    };
    let results = db.list_catalog_filtered(1, 10, &search_filter).unwrap();
    let total = db.catalog_count_filtered(&search_filter).unwrap();
    assert_eq!(total, 1);
    assert_eq!(results[0].id, 1001);

    // Genre filter with "Isekai" also matches a1 because genre filter checks tags as well
    let genre_filter = CatalogFilter {
        genres: vec!["Isekai".into()],
        ..Default::default()
    };
    let results2 = db.list_catalog_filtered(1, 10, &genre_filter).unwrap();
    let total2 = db.catalog_count_filtered(&genre_filter).unwrap();
    assert_eq!(total2, 1);
    assert_eq!(results2[0].id, 1001);

    // distinct_catalog_genres includes "Isekai"
    let genres = db.distinct_catalog_genres().unwrap();
    assert!(genres.contains(&"Isekai".to_string()));
}

#[test]
fn test_catalog_filter_hide_watched_excludes_watched_externally_and_seen_episodes() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("AnimeYT", "https://animeyt.es", "animeytx").unwrap();

    let a1 = catalog_anime(1001, "Seen Externally Show", &[]);
    let a2 = catalog_anime(1002, "Watched In App Show", &[]);
    let a3 = catalog_anime(1003, "Unwatched Show", &[]);
    let a4 = catalog_anime(1004, "Unlinked Watched Show", &[]);

    db.upsert_catalog_anime(&a1, 0).unwrap();
    db.upsert_catalog_anime(&a2, 1).unwrap();
    db.upsert_catalog_anime(&a3, 2).unwrap();
    db.upsert_catalog_anime(&a4, 3).unwrap();

    // s1: linked to 1001, watched_externally = 1
    let s1_item = mk_airing("s1", "Seen Externally Show", None);
    let s1 = db.upsert_series(src, &s1_item).unwrap();
    db.set_anilist_id(s1, 1001).unwrap();
    db.set_watched_externally(s1, true).unwrap();

    // s2: linked to 1002, seen episode
    let s2_item = mk_airing("s2", "Watched In App Show", None);
    let s2 = db.upsert_series(src, &s2_item).unwrap();
    db.set_anilist_id(s2, 1002).unwrap();
    db.conn.execute(
        "INSERT INTO episodes (series_id, number, title, url, seen) VALUES (?1, '1', 'Ep 1', 'https://ep1', 1)",
        [s2],
    ).unwrap();

    // s3: linked to 1003, followed=1, 0 episodes seen, watched_externally=0
    let s3_item = mk_airing("s3", "Unwatched Show", None);
    let s3 = db.upsert_series(src, &s3_item).unwrap();
    db.set_anilist_id(s3, 1003).unwrap();

    // s4: unlinked (anilist_id is NULL), title matches a4, watched_externally=1
    let s4_item = mk_airing("s4", "Unlinked Watched Show", None);
    let s4 = db.upsert_series(src, &s4_item).unwrap();
    db.set_watched_externally(s4, true).unwrap();

    // When hide_watched is false: all 4 are returned
    let default_filter = CatalogFilter::default();
    let results = db.list_catalog_filtered(1, 10, &default_filter).unwrap();
    assert_eq!(results.len(), 4);

    // When hide_watched is true: only a3 (1003) is returned!
    let hide_watched_filter = CatalogFilter {
        hide_watched: true,
        ..Default::default()
    };
    let filtered = db.list_catalog_filtered(1, 10, &hide_watched_filter).unwrap();
    let total = db.catalog_count_filtered(&hide_watched_filter).unwrap();
    assert_eq!(total, 1);
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].id, 1003);
}

#[test]
fn test_catalog_filter_max_score_excludes_higher_and_null() {
    let db = Db::open(":memory:").unwrap();
    let mut a1 = catalog_anime_with_popularity(1, "Low Score", &[], Some(100));
    a1.average_score = Some(45);
    let mut a2 = catalog_anime_with_popularity(2, "Boundary Score", &[], Some(100));
    a2.average_score = Some(59);
    let mut a3 = catalog_anime_with_popularity(3, "Just Over", &[], Some(100));
    a3.average_score = Some(60);
    let mut a4 = catalog_anime_with_popularity(4, "High Score", &[], Some(100));
    a4.average_score = Some(85);
    let mut a5 = catalog_anime_with_popularity(5, "Null Score", &[], Some(100));
    a5.average_score = None;

    db.upsert_catalog_anime(&a1, 0).unwrap();
    db.upsert_catalog_anime(&a2, 1).unwrap();
    db.upsert_catalog_anime(&a3, 2).unwrap();
    db.upsert_catalog_anime(&a4, 3).unwrap();
    db.upsert_catalog_anime(&a5, 4).unwrap();

    let filter = CatalogFilter {
        max_score: Some(59),
        ..Default::default()
    };
    let rows = db.list_catalog_filtered(1, 100, &filter).unwrap();
    let ids: Vec<i64> = rows.iter().map(|a| a.id).collect();
    assert_eq!(ids, vec![1, 2]);
    assert_eq!(db.catalog_count_filtered(&filter).unwrap(), 2);
}

#[test]
fn test_catalog_filter_min_0_max_100_returns_all_scored_rows() {
    let db = Db::open(":memory:").unwrap();
    let mut a1 = catalog_anime_with_popularity(1, "Zero Score", &[], Some(100));
    a1.average_score = Some(0);
    let mut a2 = catalog_anime_with_popularity(2, "Mid Score", &[], Some(100));
    a2.average_score = Some(50);
    let mut a3 = catalog_anime_with_popularity(3, "Perfect Score", &[], Some(100));
    a3.average_score = Some(100);
    let mut a4 = catalog_anime_with_popularity(4, "Unscored", &[], Some(100));
    a4.average_score = None;

    db.upsert_catalog_anime(&a1, 0).unwrap();
    db.upsert_catalog_anime(&a2, 1).unwrap();
    db.upsert_catalog_anime(&a3, 2).unwrap();
    db.upsert_catalog_anime(&a4, 3).unwrap();

    let filter = CatalogFilter {
        min_score: Some(0),
        max_score: Some(100),
        ..Default::default()
    };
    let rows = db.list_catalog_filtered(1, 100, &filter).unwrap();
    let mut ids: Vec<i64> = rows.iter().map(|a| a.id).collect();
    ids.sort();
    assert_eq!(ids, vec![1, 2, 3]);
    assert_eq!(db.catalog_count_filtered(&filter).unwrap(), 3);
}

#[test]
fn test_catalog_filter_start_date_inclusive_range_excludes_null() {
    let db = Db::open(":memory:").unwrap();
    let mut a1 = catalog_anime_with_popularity(1, "Too Early", &[], Some(100));
    a1.start_date = Some(1000);
    let mut a2 = catalog_anime_with_popularity(2, "Min Boundary", &[], Some(100));
    a2.start_date = Some(1500);
    let mut a3 = catalog_anime_with_popularity(3, "Max Boundary", &[], Some(100));
    a3.start_date = Some(2000);
    let mut a4 = catalog_anime_with_popularity(4, "Too Late", &[], Some(100));
    a4.start_date = Some(2500);
    let mut a5 = catalog_anime_with_popularity(5, "Undated", &[], Some(100));
    a5.start_date = None;

    db.upsert_catalog_anime(&a1, 0).unwrap();
    db.upsert_catalog_anime(&a2, 1).unwrap();
    db.upsert_catalog_anime(&a3, 2).unwrap();
    db.upsert_catalog_anime(&a4, 3).unwrap();
    db.upsert_catalog_anime(&a5, 4).unwrap();

    let filter = CatalogFilter {
        min_start_date: Some(1500),
        max_start_date: Some(2000),
        ..Default::default()
    };
    let rows = db.list_catalog_filtered(1, 100, &filter).unwrap();
    let mut ids: Vec<i64> = rows.iter().map(|a| a.id).collect();
    ids.sort();
    assert_eq!(ids, vec![2, 3]);
    assert_eq!(db.catalog_count_filtered(&filter).unwrap(), 2);
}

#[test]
fn test_catalog_filter_status_finished_and_unknown_ignored() {
    let db = Db::open(":memory:").unwrap();
    let mut a1 = catalog_anime_with_popularity(1, "Finished Show", &[], Some(100));
    a1.status = Some("FINISHED".into());
    let mut a2 = catalog_anime_with_popularity(2, "Airing Show", &[], Some(100));
    a2.status = Some("RELEASING".into());
    let mut a3 = catalog_anime_with_popularity(3, "Upcoming Show", &[], Some(100));
    a3.status = Some("NOT_YET_RELEASED".into());
    let mut a4 = catalog_anime_with_popularity(4, "No Status Show", &[], Some(100));
    a4.status = None;

    db.upsert_catalog_anime(&a1, 0).unwrap();
    db.upsert_catalog_anime(&a2, 1).unwrap();
    db.upsert_catalog_anime(&a3, 2).unwrap();
    db.upsert_catalog_anime(&a4, 3).unwrap();

    // Exact match on FINISHED
    let finished_filter = CatalogFilter {
        status: Some("FINISHED".into()),
        ..Default::default()
    };
    let rows = db.list_catalog_filtered(1, 100, &finished_filter).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, 1);
    assert_eq!(db.catalog_count_filtered(&finished_filter).unwrap(), 1);

    // Unknown status value is ignored -> no filter applied
    let unknown_filter = CatalogFilter {
        status: Some("UNKNOWN_STATUS".into()),
        ..Default::default()
    };
    let all_rows = db.list_catalog_filtered(1, 100, &unknown_filter).unwrap();
    assert_eq!(all_rows.len(), 4);
    assert_eq!(db.catalog_count_filtered(&unknown_filter).unwrap(), 4);

    // SQL injection attempt in status: ignored
    let inj_filter = CatalogFilter {
        status: Some("' OR 1=1 --".into()),
        ..Default::default()
    };
    let inj_rows = db.list_catalog_filtered(1, 100, &inj_filter).unwrap();
    assert_eq!(inj_rows.len(), 4);
}

#[test]
fn test_catalog_filter_sort_score_desc_and_oldest_with_id_ties() {
    let db = Db::open(":memory:").unwrap();
    let mut a1 = catalog_anime_with_popularity(10, "Show 10", &[], Some(100));
    a1.average_score = Some(80);
    a1.start_date = Some(2000);

    let mut a2 = catalog_anime_with_popularity(20, "Show 20", &[], Some(200));
    a2.average_score = Some(95);
    a2.start_date = Some(1995);

    let mut a3 = catalog_anime_with_popularity(5, "Show 5", &[], Some(50));
    a3.average_score = Some(80); // tie with a1, but id 5 < 10
    a3.start_date = Some(2010);

    let mut a4 = catalog_anime_with_popularity(30, "Show 30", &[], Some(300));
    a4.average_score = None; // null score
    a4.start_date = None;    // null date

    db.upsert_catalog_anime(&a1, 0).unwrap();
    db.upsert_catalog_anime(&a2, 1).unwrap();
    db.upsert_catalog_anime(&a3, 2).unwrap();
    db.upsert_catalog_anime(&a4, 3).unwrap();

    // sort=score_desc: 95 (id 20), then 80 (id 5 before id 10), then null (id 30)
    let score_desc_filter = CatalogFilter {
        sort: Some("score_desc".into()),
        ..Default::default()
    };
    let rows = db.list_catalog_filtered(1, 100, &score_desc_filter).unwrap();
    let ids: Vec<i64> = rows.iter().map(|a| a.id).collect();
    assert_eq!(ids, vec![20, 5, 10, 30]);

    // sort=oldest: 1995 (id 20), 2000 (id 10), 2010 (id 5), then null (id 30)
    let oldest_filter = CatalogFilter {
        sort: Some("oldest".into()),
        ..Default::default()
    };
    let rows_oldest = db.list_catalog_filtered(1, 100, &oldest_filter).unwrap();
    let ids_oldest: Vec<i64> = rows_oldest.iter().map(|a| a.id).collect();
    assert_eq!(ids_oldest, vec![20, 10, 5, 30]);

    // Unknown sort falls back to default popularity DESC NULLS LAST, id
    let unknown_sort_filter = CatalogFilter {
        sort: Some("random_garbage; DROP TABLE x;".into()),
        ..Default::default()
    };
    let rows_def = db.list_catalog_filtered(1, 100, &unknown_sort_filter).unwrap();
    let ids_def: Vec<i64> = rows_def.iter().map(|a| a.id).collect();
    assert_eq!(ids_def, vec![30, 20, 10, 5]);
}

#[test]
fn test_catalog_filter_all_filters_combined_and_count_filtered_sync() {
    let db = Db::open(":memory:").unwrap();
    let mut a1 = catalog_anime_with_popularity(1, "Target Anime Movie", &["Action"], Some(500));
    a1.format = Some("MOVIE".into());
    a1.average_score = Some(75);
    a1.start_date = Some(1600000000);
    a1.status = Some("FINISHED".into());
    a1.studio = Some("Bones".into());
    a1.episodes = Some(1);

    let mut a2 = catalog_anime_with_popularity(2, "Other Target Anime", &["Action"], Some(400));
    a2.format = Some("MOVIE".into());
    a2.average_score = Some(80);
    a2.start_date = Some(1650000000);
    a2.status = Some("FINISHED".into());
    a2.studio = Some("Bones".into());
    a2.episodes = Some(1);

    let mut a3 = catalog_anime_with_popularity(3, "Fails Score Anime", &["Action"], Some(300));
    a3.format = Some("MOVIE".into());
    a3.average_score = Some(50); // score too low
    a3.start_date = Some(1600000000);
    a3.status = Some("FINISHED".into());
    a3.studio = Some("Bones".into());
    a3.episodes = Some(1);

    db.upsert_catalog_anime(&a1, 0).unwrap();
    db.upsert_catalog_anime(&a2, 1).unwrap();
    db.upsert_catalog_anime(&a3, 2).unwrap();

    let filter = CatalogFilter {
        search: Some("Target".into()),
        genres: vec!["Action".into()],
        format: Some("MOVIE".into()),
        min_score: Some(70),
        max_score: Some(85),
        min_start_date: Some(1500000000),
        max_start_date: Some(1700000000),
        status: Some("FINISHED".into()),
        studio: Some("Bones".into()),
        episodes: Some("1".into()),
        sort: Some("score_desc".into()),
        hide_watched: false,
    };

    let rows = db.list_catalog_filtered(1, 100, &filter).unwrap();
    let count = db.catalog_count_filtered(&filter).unwrap();
    assert_eq!(rows.len() as i64, count);
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].id, 2); // 80 > 75
    assert_eq!(rows[1].id, 1);
}

#[test]
fn test_deck_min_score_persistence_and_random_catalog_anime_filter() {
    let db = Db::open(":memory:").unwrap();
    assert_eq!(db.get_deck_min_score().unwrap(), None);
    db.set_deck_min_score(Some(75)).unwrap();
    assert_eq!(db.get_deck_min_score().unwrap(), Some(75));
    db.set_deck_min_score(None).unwrap();
    assert_eq!(db.get_deck_min_score().unwrap(), None);

    // Test random_catalog_anime_in_genre with min_score
    let mut low = catalog_anime_with_popularity(1, "LowScoreShow", &["Action"], Some(1000));
    low.average_score = Some(60);
    db.upsert_catalog_anime(&low, 0).unwrap();
    let mut high = catalog_anime_with_popularity(2, "HighScoreShow", &["Action"], Some(1000));
    high.average_score = Some(85);
    db.upsert_catalog_anime(&high, 1).unwrap();

    // Floor of 70 filters out LowScoreShow (60), picks HighScoreShow (85)
    for _ in 0..10 {
        let picked = db
            .random_catalog_anime_in_genre(
                "Action",
                &[],
                &[],
                &std::collections::HashSet::new(),
                &std::collections::HashMap::new(),
                &std::collections::HashMap::new(),
                false,
                false,
                None,
                None,
                Some(70),
            )
            .unwrap()
            .unwrap();
        assert_eq!(picked.title, "HighScoreShow");
    }
}


#[test]
fn test_replace_episode_map_replaces_previous_entries() {
    let db = Db::open(":memory:").unwrap();
    let entries_v1 = vec![
        crate::anizip::MapEntry { episode: 1, season: 1, absolute: Some(1), air_at: None },
        crate::anizip::MapEntry { episode: 2, season: 1, absolute: Some(2), air_at: None },
    ];
    db.replace_episode_map(100, &entries_v1, 1000).unwrap();
    assert_eq!(db.episode_offset(100).unwrap(), Some(0));

    let entries_v2 = vec![
        crate::anizip::MapEntry { episode: 1, season: 2, absolute: Some(25), air_at: None },
    ];
    db.replace_episode_map(100, &entries_v2, 2000).unwrap();
    assert_eq!(db.episode_offset(100).unwrap(), Some(24));

    let count: i64 = db.conn.query_row(
        "SELECT COUNT(*) FROM anilist_episode_map WHERE anilist_id = 100",
        [],
        |r| r.get(0),
    ).unwrap();
    assert_eq!(count, 1);

    let fetched: (i64, i64) = db.conn.query_row(
        "SELECT fetched_at, entries FROM anilist_episode_map_fetched WHERE anilist_id = 100",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    ).unwrap();
    assert_eq!(fetched, (2000, 1));
}

#[test]
fn test_episode_offset_cases() {
    let db = Db::open(":memory:").unwrap();

    // 185756 starting at absolute 25 -> 25 - 1 = 24
    let entries_185756 = vec![
        crate::anizip::MapEntry { episode: 1, season: 3, absolute: Some(25), air_at: None },
        crate::anizip::MapEntry { episode: 2, season: 3, absolute: Some(26), air_at: None },
    ];
    db.replace_episode_map(185756, &entries_185756, 1000).unwrap();
    assert_eq!(db.episode_offset(185756).unwrap(), Some(24));

    // season-1 map (absolute 1) -> 1 - 1 = 0
    let entries_s1 = vec![
        crate::anizip::MapEntry { episode: 1, season: 1, absolute: Some(1), air_at: None },
    ];
    db.replace_episode_map(164702, &entries_s1, 1000).unwrap();
    assert_eq!(db.episode_offset(164702).unwrap(), Some(0));

    // None with no map
    assert_eq!(db.episode_offset(999999).unwrap(), None);

    // None when absolute is NULL
    let entries_no_abs = vec![
        crate::anizip::MapEntry { episode: 1, season: 1, absolute: None, air_at: None },
    ];
    db.replace_episode_map(888888, &entries_no_abs, 1000).unwrap();
    assert_eq!(db.episode_offset(888888).unwrap(), None);
}

#[test]
fn test_episode_number_for_absolute_lookup() {
    let db = Db::open(":memory:").unwrap();
    let entries = vec![
        crate::anizip::MapEntry { episode: 1, season: 3, absolute: Some(25), air_at: None },
        crate::anizip::MapEntry { episode: 2, season: 3, absolute: Some(26), air_at: None },
        crate::anizip::MapEntry { episode: 3, season: 3, absolute: Some(27), air_at: None },
    ];
    db.replace_episode_map(185756, &entries, 1000).unwrap();
    assert_eq!(db.episode_number_for_absolute(185756, 27).unwrap(), Some(3));
    assert_eq!(db.episode_number_for_absolute(185756, 99).unwrap(), None);
    assert_eq!(db.episode_number_for_absolute(999999, 27).unwrap(), None);
}

#[test]
fn test_episode_map_stale_ids_conditions() {
    let db = Db::open(":memory:").unwrap();
    let now = 1_000_000;
    let one_day = 86400;

    db.conn.execute("INSERT INTO sources (id, name, base_url, site_id) VALUES (1, 's', 'https://s.example', 'animeytx')", []).unwrap();

    let insert_series = |id: i64, anilist_id: Option<i64>, followed: i64, is_airing: i64| {
        db.conn.execute(
            "INSERT INTO series (id, source_id, slug, title, url, followed, is_airing, anilist_id)
             VALUES (?1, 1, ?2, ?2, 'https://example.com', ?3, ?4, ?5)",
            rusqlite::params![id, format!("slug-{id}"), followed, is_airing, anilist_id],
        ).unwrap();
    };

    let insert_catalog = |id: i64, status: &str| {
        db.conn.execute(
            "INSERT INTO anilist_catalog (id, title, url, sort_order, status)
             VALUES (?1, 'Title', 'https://anilist.co', 0, ?2)",
            rusqlite::params![id, status],
        ).unwrap();
    };

    let insert_fetched = |id: i64, fetched_at: i64| {
        db.conn.execute(
            "INSERT INTO anilist_episode_map_fetched (anilist_id, fetched_at, entries)
             VALUES (?1, ?2, 1)",
            rusqlite::params![id, fetched_at],
        ).unwrap();
    };

    // 1. Followed linked series with no fetched row -> returned
    insert_series(1, Some(101), 1, 0);
    insert_catalog(101, "RELEASING");

    // 2. Fetched 1 day ago (RELEASING) -> NOT returned
    insert_series(2, Some(102), 1, 0);
    insert_catalog(102, "RELEASING");
    insert_fetched(102, now - one_day);

    // 3. Fetched 4 days ago (RELEASING) -> returned
    insert_series(3, Some(103), 1, 0);
    insert_catalog(103, "RELEASING");
    insert_fetched(103, now - 4 * one_day);

    // 4. Fetched 4 days ago (FINISHED) -> NOT returned
    insert_series(4, Some(104), 1, 0);
    insert_catalog(104, "FINISHED");
    insert_fetched(104, now - 4 * one_day);

    // 5. Fetched 31 days ago (FINISHED) -> returned
    insert_series(5, Some(105), 1, 0);
    insert_catalog(105, "FINISHED");
    insert_fetched(105, now - 31 * one_day);

    // 6. Unfollowed non-airing series -> NEVER returned
    insert_series(6, Some(106), 0, 0);
    insert_catalog(106, "RELEASING");

    // 7. Unlinked series (anilist_id NULL) -> NEVER returned
    insert_series(7, None, 1, 1);

    let stale = db.episode_map_stale_ids(now, 100).unwrap();
    assert!(stale.contains(&101), "no fetched row must be stale");
    assert!(!stale.contains(&102), "1 day ago RELEASING must not be stale");
    assert!(stale.contains(&103), "4 days ago RELEASING must be stale");
    assert!(!stale.contains(&104), "4 days ago FINISHED must not be stale");
    assert!(stale.contains(&105), "31 days ago FINISHED must be stale");
    assert!(!stale.contains(&106), "unfollowed non-airing must never be returned");
    assert_eq!(stale.len(), 3);

    // Limit honoured
    let limited = db.episode_map_stale_ids(now, 2).unwrap();
    assert_eq!(limited.len(), 2);
}

#[test]
fn test_next_airing_episode_cases() {
    let db = Db::open(":memory:").unwrap();
    let now = 1_000_000;

    // 1. Map with episodes 1..3 at air_at now-100, now+100, now+200 -> Some((2, now+100))
    let entries = vec![
        crate::anizip::MapEntry { episode: 1, season: 1, absolute: Some(1), air_at: Some(now - 100) },
        crate::anizip::MapEntry { episode: 2, season: 1, absolute: Some(2), air_at: Some(now + 100) },
        crate::anizip::MapEntry { episode: 3, season: 1, absolute: Some(3), air_at: Some(now + 200) },
    ];
    db.replace_episode_map(100, &entries, now).unwrap();
    assert_eq!(db.next_airing_episode(100, now).unwrap(), Some((2, now + 100)));

    // 2. All entries in the past -> None
    let entries_past = vec![
        crate::anizip::MapEntry { episode: 1, season: 1, absolute: Some(1), air_at: Some(now - 200) },
        crate::anizip::MapEntry { episode: 2, season: 1, absolute: Some(2), air_at: Some(now - 100) },
        crate::anizip::MapEntry { episode: 3, season: 1, absolute: Some(3), air_at: Some(now) },
    ];
    db.replace_episode_map(200, &entries_past, now).unwrap();
    assert_eq!(db.next_airing_episode(200, now).unwrap(), None);

    // 3. Unknown anilist_id -> None
    assert_eq!(db.next_airing_episode(999999, now).unwrap(), None);

    // 4. Entry with NULL air_at ignored (others still found)
    let entries_with_null = vec![
        crate::anizip::MapEntry { episode: 1, season: 1, absolute: Some(1), air_at: None },
        crate::anizip::MapEntry { episode: 2, season: 1, absolute: Some(2), air_at: Some(now + 300) },
    ];
    db.replace_episode_map(300, &entries_with_null, now).unwrap();
    assert_eq!(db.next_airing_episode(300, now).unwrap(), Some((2, now + 300)));

    // 5. Two entries with the same future air_at -> lowest episode wins
    let entries_tie = vec![
        crate::anizip::MapEntry { episode: 5, season: 1, absolute: Some(5), air_at: Some(now + 500) },
        crate::anizip::MapEntry { episode: 4, season: 1, absolute: Some(4), air_at: Some(now + 500) },
    ];
    db.replace_episode_map(400, &entries_tie, now).unwrap();
    assert_eq!(db.next_airing_episode(400, now).unwrap(), Some((4, now + 500)));

    // 6. Past air_at must never be returned as upcoming
    let entries_strictly_past = vec![
        crate::anizip::MapEntry { episode: 1, season: 1, absolute: Some(1), air_at: Some(now - 1) },
    ];
    db.replace_episode_map(500, &entries_strictly_past, now).unwrap();
    assert_eq!(db.next_airing_episode(500, now).unwrap(), None);
}

#[test]
fn test_import_catalog_snapshot_idempotent_and_no_overwrite() {
    let db = Db::open(":memory:").unwrap();
    // Seed an existing catalog row with id 1 and original title
    let existing = catalog_anime(1, "Original Title", &["Action"]);
    db.upsert_catalog_anime(&existing, 0).unwrap();

    // Snapshot items: id 1 with DIFFERENT title, and new id 2
    let item1 = catalog_anime(1, "Changed Title", &["Drama"]);
    let item2 = catalog_anime(2, "Brand New Anime", &["Comedy"]);

    let items = vec![item1, item2];

    // First import: id 1 skipped (not overwritten), id 2 inserted -> returns 1
    let inserted = db.import_catalog_snapshot(&items, 0).unwrap();
    assert_eq!(inserted, 1);
    assert_eq!(db.catalog_count().unwrap(), 2);

    // Check that id 1's title was NOT overwritten
    let (title, _, _) = db.get_catalog_titles(1).unwrap().unwrap();
    assert_eq!(title, "Original Title");

    // Second import with same items: returns 0 and nothing changes
    let second_inserted = db.import_catalog_snapshot(&items, 0).unwrap();
    assert_eq!(second_inserted, 0);
    assert_eq!(db.catalog_count().unwrap(), 2);
    let (title2, _, _) = db.get_catalog_titles(1).unwrap().unwrap();
    assert_eq!(title2, "Original Title");
}

#[test]
fn test_import_catalog_snapshot_preserves_sort_order() {
    let db = Db::open(":memory:").unwrap();
    // Three anime items in a specific sequence
    let a10 = catalog_anime(10, "Tenth Anime", &["Action"]);
    let a20 = catalog_anime(20, "Twentieth Anime", &["Sci-Fi"]);
    let a5 = catalog_anime(5, "Fifth Anime", &["Fantasy"]);

    let items = vec![a10, a20, a5];
    let inserted = db.import_catalog_snapshot(&items, 0).unwrap();
    assert_eq!(inserted, 3);

    // catalog_all_for_snapshot orders by sort_order, id
    let read_back = db.catalog_all_for_snapshot().unwrap();
    assert_eq!(read_back.len(), 3);
    assert_eq!(read_back[0].id, 10);
    assert_eq!(read_back[1].id, 20);
    assert_eq!(read_back[2].id, 5);
    assert_eq!(read_back[0].title, "Tenth Anime");
    assert_eq!(read_back[1].title, "Twentieth Anime");
    assert_eq!(read_back[2].title, "Fifth Anime");
}

#[test]
fn test_catalog_all_for_snapshot_full_fidelity_and_child_tables() {
    let db = Db::open(":memory:").unwrap();
    let full = crate::anilist::CatalogAnime {
        id: 42,
        title: "Main Title".into(),
        title_romaji: Some("Romaji Title".into()),
        title_english: Some("English Title".into()),
        synonyms: vec!["Synonym 1".into(), "Synonym 2".into()],
        cover_url: Some("https://example.com/cover.jpg".into()),
        banner_url: Some("https://example.com/banner.jpg".into()),
        format: Some("TV".into()),
        genres: vec!["Action".into(), "Adventure".into()],
        episodes: Some(24),
        average_score: Some(88),
        popularity: Some(12345),
        url: "https://anilist.co/anime/42".into(),
        status: Some("FINISHED".into()),
        duration: Some(25),
        studio: Some("Kyoto Animation".into()),
        start_date: Some(1609459200),
        next_airing_at: Some(1612137600),
        next_episode: Some(12),
        tags: vec![
            crate::anilist::CatalogTag { name: "Magic".into(), rank: 70 },
            crate::anilist::CatalogTag { name: "Isekai".into(), rank: 50 },
        ],
        relations: vec![
            crate::anilist::CatalogRelation { related_id: 100, relation_type: "PREQUEL".into() },
            crate::anilist::CatalogRelation { related_id: 101, relation_type: "SEQUEL".into() },
        ],
    };

    db.upsert_catalog_anime(&full, 0).unwrap();
    let snapshot = db.catalog_all_for_snapshot().unwrap();
    assert_eq!(snapshot.len(), 1);
    let item = &snapshot[0];
    assert_eq!(item.id, 42);
    assert_eq!(item.title, "Main Title");
    assert_eq!(item.title_romaji.as_deref(), Some("Romaji Title"));
    assert_eq!(item.title_english.as_deref(), Some("English Title"));
    assert_eq!(item.synonyms, vec!["Synonym 1", "Synonym 2"]);
    assert_eq!(item.cover_url.as_deref(), Some("https://example.com/cover.jpg"));
    assert_eq!(item.banner_url.as_deref(), Some("https://example.com/banner.jpg"));
    assert_eq!(item.format.as_deref(), Some("TV"));
    assert_eq!(item.genres, vec!["Action", "Adventure"]);
    assert_eq!(item.episodes, Some(24));
    assert_eq!(item.average_score, Some(88));
    assert_eq!(item.popularity, Some(12345));
    assert_eq!(item.url, "https://anilist.co/anime/42");
    assert_eq!(item.status.as_deref(), Some("FINISHED"));
    assert_eq!(item.duration, Some(25));
    assert_eq!(item.studio.as_deref(), Some("Kyoto Animation"));
    assert_eq!(item.start_date, Some(1609459200));
    assert_eq!(item.next_airing_at, Some(1612137600));
    assert_eq!(item.next_episode, Some(12));
    // Tags must be sorted by rank asc then name: 50 ("Isekai") before 70 ("Magic")
    assert_eq!(
        item.tags,
        vec![
            crate::anilist::CatalogTag { name: "Isekai".into(), rank: 50 },
            crate::anilist::CatalogTag { name: "Magic".into(), rank: 70 },
        ]
    );
    // Relations sorted by related_id asc then relation_type
    assert_eq!(
        item.relations,
        vec![
            crate::anilist::CatalogRelation { related_id: 100, relation_type: "PREQUEL".into() },
            crate::anilist::CatalogRelation { related_id: 101, relation_type: "SEQUEL".into() },
        ]
    );
}

#[test]
fn build_catalog_airing_meta_matches_old_functions_map_by_map() {
    let db = Db::open(":memory:").unwrap();

    // Row 1: has title, romaji, english, start_date, banner, cover, score.
    let mut a1 = catalog_anime(1, "Shingeki no Kyojin", &[]);
    a1.title_romaji = Some("Attack on Titan Romaji".into());
    a1.title_english = Some("Attack on Titan".into());
    a1.start_date = Some(1365292800);
    a1.banner_url = Some("https://cdn/banner1.jpg".into());
    a1.cover_url = Some("https://cdn/cover1.jpg".into());
    a1.average_score = Some(85);
    db.upsert_catalog_anime(&a1, 0).unwrap();

    // Row 2: shares normalized variant "attack on titan" with a1.
    // First variant encountered wins, so a1's values must win for that key.
    let mut a2 = catalog_anime(2, "Attack on Titan", &[]);
    a2.title_romaji = None;
    a2.title_english = Some("AoT Alternative".into());
    a2.start_date = Some(1400000000);
    a2.banner_url = Some("https://cdn/banner2.jpg".into());
    a2.cover_url = Some("https://cdn/cover2.jpg".into());
    a2.average_score = Some(90);
    db.upsert_catalog_anime(&a2, 1).unwrap();

    // Row 3: empty banner_url and empty cover_url (""), NULL start_date, has score.
    let mut a3 = catalog_anime(3, "Empty Banner Show", &[]);
    a3.title_romaji = Some("Romaji Variant".into());
    a3.title_english = None;
    a3.start_date = None;
    a3.banner_url = Some("".into());
    a3.cover_url = Some("".into());
    a3.average_score = Some(70);
    db.upsert_catalog_anime(&a3, 2).unwrap();

    // Row 4: NULL score, NULL banner_url, has start_date and cover_url.
    let mut a4 = catalog_anime(4, "No Score Show", &[]);
    a4.title_romaji = None;
    a4.title_english = Some("No Score English".into());
    a4.start_date = Some(1500000000);
    a4.banner_url = None;
    a4.cover_url = Some("https://cdn/cover4.jpg".into());
    a4.average_score = None;
    db.upsert_catalog_anime(&a4, 3).unwrap();

    let old_dates = db.catalog_start_dates_by_normalized_title().unwrap();
    let old_banners = db.catalog_banners_by_normalized_title().unwrap();
    let old_covers = db.catalog_covers_by_normalized_title().unwrap();
    let old_scores = db.catalog_scores_by_normalized_title().unwrap();

    let rows = db.catalog_airing_meta_rows().unwrap();
    assert_eq!(rows.len(), 4);
    let meta = build_catalog_airing_meta(rows);

    assert_eq!(meta.start_dates, old_dates);
    assert_eq!(meta.banners, old_banners);
    assert_eq!(meta.covers, old_covers);
    assert_eq!(meta.scores, old_scores);

    let norm_shared = crate::matching::normalize_title("Attack on Titan");
    assert_eq!(meta.start_dates.get(&norm_shared), Some(&1365292800));
    assert_eq!(meta.banners.get(&norm_shared).map(String::as_str), Some("https://cdn/banner1.jpg"));
    assert_eq!(meta.covers.get(&norm_shared).map(String::as_str), Some("https://cdn/cover1.jpg"));
    assert_eq!(meta.scores.get(&norm_shared), Some(&85.0));

    let norm_empty = crate::matching::normalize_title("Empty Banner Show");
    assert!(!meta.banners.contains_key(&norm_empty));
    assert!(!meta.covers.contains_key(&norm_empty));
    assert!(!meta.start_dates.contains_key(&norm_empty));
    assert_eq!(meta.scores.get(&norm_empty), Some(&70.0));
}

#[test]
fn series_banners_and_scores_matches_individual_queries() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("TestSite", "https://site.example", "testsite").unwrap();

    let mut c_a = catalog_anime(10, "Series A Catalog", &[]);
    c_a.banner_url = Some("https://cat/banner_a.jpg".into());
    c_a.average_score = Some(82);
    db.upsert_catalog_anime(&c_a, 0).unwrap();

    let sid_a = db.upsert_series(src, &mk_airing("series-a", "Series A", None)).unwrap();
    db.set_anilist_id(sid_a, 10).unwrap();
    db.conn.execute(
        "UPDATE series SET banner_url = ?1 WHERE id = ?2",
        rusqlite::params!["https://custom/banner_a.jpg", sid_a],
    ).unwrap();

    let mut c_b = catalog_anime(20, "Series B Catalog", &[]);
    c_b.banner_url = Some("https://cat/banner_b.jpg".into());
    c_b.average_score = Some(75);
    db.upsert_catalog_anime(&c_b, 1).unwrap();

    let sid_b = db.upsert_series(src, &mk_airing("series-b", "Series B", None)).unwrap();
    db.set_anilist_id(sid_b, 20).unwrap();

    let sid_c = db.upsert_series(src, &mk_airing("series-c", "Series C", None)).unwrap();

    let missing_id = 999_999i64;

    let series_ids = vec![sid_a, sid_b, sid_c, missing_id];
    let batch = db.series_banners_and_scores(&series_ids).unwrap();

    assert_eq!(
        batch.get(&sid_a),
        Some(&(db.get_series_banner(sid_a).unwrap(), db.get_series_score(sid_a).unwrap()))
    );
    assert_eq!(
        batch.get(&sid_a),
        Some(&(Some("https://custom/banner_a.jpg".to_string()), Some(82.0)))
    );

    assert_eq!(
        batch.get(&sid_b),
        Some(&(db.get_series_banner(sid_b).unwrap(), db.get_series_score(sid_b).unwrap()))
    );
    assert_eq!(
        batch.get(&sid_b),
        Some(&(Some("https://cat/banner_b.jpg".to_string()), Some(75.0)))
    );

    assert_eq!(
        batch.get(&sid_c),
        Some(&(db.get_series_banner(sid_c).unwrap(), db.get_series_score(sid_c).unwrap()))
    );
    assert_eq!(batch.get(&sid_c), Some(&(None, None)));

    assert_eq!(batch.get(&missing_id), None);
}

#[test]
fn series_banners_and_scores_chunking_large_id_list() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("TestSite", "https://site.example", "testsite").unwrap();
    let sid = db.upsert_series(src, &mk_airing("s-one", "Series One", None)).unwrap();
    db.conn.execute(
        "UPDATE series SET banner_url = 'https://banner.jpg' WHERE id = ?1",
        [sid],
    ).unwrap();

    let mut ids = vec![sid];
    for i in 1..=600 {
        ids.push(1000 + i);
    }
    let batch = db.series_banners_and_scores(&ids).unwrap();
    assert_eq!(batch.len(), 1);
    assert_eq!(batch.get(&sid), Some(&(Some("https://banner.jpg".to_string()), None)));
}

#[test]
fn list_catalog_genres_and_tags_for_matches_single_id_functions() {
    let db = Db::open(":memory:").unwrap();

    // Id 1: multiple genres, multiple tags with different and tied ranks
    let mut a1 = catalog_anime(1, "Anime One", &["Drama", "Action", "Comedy"]);
    a1.tags = vec![
        CatalogTag { name: "Magic".into(), rank: 70 },
        CatalogTag { name: "Isekai".into(), rank: 50 },
        CatalogTag { name: "Adventure".into(), rank: 50 },
        CatalogTag { name: "High Stakes".into(), rank: 90 },
    ];
    db.upsert_catalog_anime(&a1, 0).unwrap();

    // Id 2: single genre, single tag
    let mut a2 = catalog_anime(2, "Anime Two", &["Sci-Fi"]);
    a2.tags = vec![CatalogTag { name: "Cyberpunk".into(), rank: 80 }];
    db.upsert_catalog_anime(&a2, 1).unwrap();

    // Id 3: no genres, no tags
    let a3 = catalog_anime(3, "Anime Three", &[]);
    db.upsert_catalog_anime(&a3, 2).unwrap();

    // Id 4: not even in catalog
    let id4 = 9999i64;

    let ids = vec![1, 2, 3, id4];

    let genres_batch = db.list_catalog_genres_for(&ids).unwrap();
    let tags_batch = db.list_catalog_tags_for(&ids).unwrap();

    for &id in &ids {
        let single_genres = db.list_catalog_genres(id).unwrap();
        let single_tags = db.list_catalog_tags(id).unwrap();

        assert_eq!(
            genres_batch.get(&id),
            Some(&single_genres),
            "batch genres for {id} must match single-id"
        );
        assert_eq!(
            tags_batch.get(&id),
            Some(&single_tags),
            "batch tags for {id} must match single-id"
        );
    }

    // Specifically check that empty / nonexistent ids have empty vecs
    assert_eq!(genres_batch.get(&3), Some(&vec![]));
    assert_eq!(tags_batch.get(&3), Some(&vec![]));
    assert_eq!(genres_batch.get(&id4), Some(&vec![]));
    assert_eq!(tags_batch.get(&id4), Some(&vec![]));

    // Test chunking with > 500 ids
    let mut large_ids = vec![1, 2];
    for i in 1000..=1600 {
        large_ids.push(i);
    }
    let large_genres = db.list_catalog_genres_for(&large_ids).unwrap();
    let large_tags = db.list_catalog_tags_for(&large_ids).unwrap();
    assert_eq!(large_genres.len(), large_ids.len());
    assert_eq!(large_tags.len(), large_ids.len());
    assert_eq!(large_genres.get(&1), Some(&db.list_catalog_genres(1).unwrap()));
    assert_eq!(large_tags.get(&1), Some(&db.list_catalog_tags(1).unwrap()));
}

#[test]
fn batch_of_5_from_catalog_with_8_titles_returns_5_distinct_titles() {
    let db = Db::open(":memory:").unwrap();
    for i in 1..=8 {
        let mut a = catalog_anime_with_popularity(i, &format!("Show {i}"), &["Drama"], Some(1000));
        a.format = Some("TV".into());
        db.upsert_catalog_anime(&a, i).unwrap();
    }

    let mut picked_titles = std::collections::HashSet::new();
    let mut picked_ids = std::collections::HashSet::new();
    let mut excluded_norm_titles = std::collections::HashSet::new();
    let mut excluded_ids = std::collections::HashSet::new();

    for _ in 0..5 {
        let picked = db
            .random_catalog_anime_in_genre_with_signals_and_excluded_ids(
                "Drama",
                &[],
                &[],
                &excluded_norm_titles,
                &excluded_ids,
                &std::collections::HashMap::new(),
                &std::collections::HashMap::new(),
                &crate::recommend::TasteSignals::default(),
                false, // random pick
                false,
                None,
                None,
                None,
            )
            .unwrap()
            .expect("should pick an anime from eligible pool");

        assert!(picked.id >= 1 && picked.id <= 8, "membership check");
        assert!(!picked_ids.contains(&picked.id), "must be distinct id");
        assert!(!picked_titles.contains(&picked.title), "must be distinct title");

        picked_ids.insert(picked.id);
        picked_titles.insert(picked.title.clone());
        excluded_ids.insert(picked.id);
        excluded_norm_titles.insert(crate::matching::normalize_title(&picked.title));
    }

    assert_eq!(picked_titles.len(), 5);
    assert_eq!(picked_ids.len(), 5);
}

#[test]
fn test_catalog_next_airing_cases() {
    let db = Db::open(":memory:").unwrap();
    let now = 1_000_000;

    // 1. next_airing_at = now+3600, next_episode = 7 -> Some((7, now+3600))
    let mut a1 = catalog_anime(1, "Upcoming Anime", &[]);
    a1.next_airing_at = Some(now + 3600);
    a1.next_episode = Some(7);
    db.upsert_catalog_anime(&a1, 0).unwrap();
    assert_eq!(db.catalog_next_airing(1, now).unwrap(), Some((7, now + 3600)));

    // 2. next_airing_at = now-60 -> None
    let mut a2 = catalog_anime(2, "Past Anime", &[]);
    a2.next_airing_at = Some(now - 60);
    a2.next_episode = Some(7);
    db.upsert_catalog_anime(&a2, 1).unwrap();
    assert_eq!(db.catalog_next_airing(2, now).unwrap(), None);

    // 3. next_airing_at = now (strictly > now requirement) -> None
    let mut a3 = catalog_anime(3, "Now Anime", &[]);
    a3.next_airing_at = Some(now);
    a3.next_episode = Some(7);
    db.upsert_catalog_anime(&a3, 2).unwrap();
    assert_eq!(db.catalog_next_airing(3, now).unwrap(), None);

    // 4. next_episode = None -> None
    let mut a4 = catalog_anime(4, "No Ep Anime", &[]);
    a4.next_airing_at = Some(now + 3600);
    a4.next_episode = None;
    db.upsert_catalog_anime(&a4, 3).unwrap();
    assert_eq!(db.catalog_next_airing(4, now).unwrap(), None);

    // 5. next_airing_at = None -> None
    let mut a5 = catalog_anime(5, "No Airing Anime", &[]);
    a5.next_airing_at = None;
    a5.next_episode = Some(7);
    db.upsert_catalog_anime(&a5, 4).unwrap();
    assert_eq!(db.catalog_next_airing(5, now).unwrap(), None);

    // 6. Unknown anilist_id -> None
    assert_eq!(db.catalog_next_airing(999_999, now).unwrap(), None);
}

#[test]
fn test_catalog_next_airing_rolled_cases() {
    let db = Db::open(":memory:").unwrap();
    let now = 1_000_000;
    const SEVEN_DAYS_SECS: i64 = 7 * 86400;

    // 1. RELEASING, next_episode=5, next_airing_at = now+3600 -> Some((5, now+3600)) (unchanged).
    let mut a1 = catalog_anime(1, "Upcoming Releasing", &[]);
    a1.status = Some("RELEASING".into());
    a1.next_airing_at = Some(now + 3600);
    a1.next_episode = Some(5);
    db.upsert_catalog_anime(&a1, 0).unwrap();
    assert_eq!(
        db.catalog_next_airing_rolled(1, now).unwrap(),
        Some((5, now + 3600, false))
    );

    // 2. RELEASING, next_episode=5, next_airing_at = now-3600 -> Some((6, next_airing_at + 7*86400)).
    let mut a2 = catalog_anime(2, "Recent Expired Releasing", &[]);
    a2.status = Some("RELEASING".into());
    let a2_air = now - 3600;
    a2.next_airing_at = Some(a2_air);
    a2.next_episode = Some(5);
    db.upsert_catalog_anime(&a2, 1).unwrap();
    assert_eq!(
        db.catalog_next_airing_rolled(2, now).unwrap(),
        Some((6, a2_air + SEVEN_DAYS_SECS, true))
    );

    // 3. RELEASING, next_episode=5, next_airing_at = now-(7*86400-60) -> Some((6, next_airing_at + 7*86400)) (one week is enough: that is > now by 60 s).
    let mut a3 = catalog_anime(3, "Near Week Limit Releasing", &[]);
    a3.status = Some("RELEASING".into());
    let a3_air = now - (SEVEN_DAYS_SECS - 60);
    a3.next_airing_at = Some(a3_air);
    a3.next_episode = Some(5);
    db.upsert_catalog_anime(&a3, 2).unwrap();
    assert_eq!(
        db.catalog_next_airing_rolled(3, now).unwrap(),
        Some((6, a3_air + SEVEN_DAYS_SECS, true))
    );

    // 4. RELEASING, next_airing_at = now-(7*86400+1) -> None (too old).
    let mut a4 = catalog_anime(4, "Too Old Releasing", &[]);
    a4.status = Some("RELEASING".into());
    let a4_air = now - (SEVEN_DAYS_SECS + 1);
    a4.next_airing_at = Some(a4_air);
    a4.next_episode = Some(5);
    db.upsert_catalog_anime(&a4, 3).unwrap();
    assert_eq!(db.catalog_next_airing_rolled(4, now).unwrap(), None);

    // 5. Must NOT happen: status FINISHED with an expired date -> None.
    let mut a5 = catalog_anime(5, "Finished Expired", &[]);
    a5.status = Some("FINISHED".into());
    a5.next_airing_at = Some(now - 3600);
    a5.next_episode = Some(5);
    db.upsert_catalog_anime(&a5, 4).unwrap();
    assert_eq!(db.catalog_next_airing_rolled(5, now).unwrap(), None);

    // 6. Must NOT happen: next_episode NULL -> None.
    let mut a6 = catalog_anime(6, "No Ep Releasing", &[]);
    a6.status = Some("RELEASING".into());
    a6.next_airing_at = Some(now - 3600);
    a6.next_episode = None;
    db.upsert_catalog_anime(&a6, 5).unwrap();
    assert_eq!(db.catalog_next_airing_rolled(6, now).unwrap(), None);

    // 7. Must NOT happen: next_airing_at NULL -> None.
    let mut a7 = catalog_anime(7, "No Airing Releasing", &[]);
    a7.status = Some("RELEASING".into());
    a7.next_airing_at = None;
    a7.next_episode = Some(5);
    db.upsert_catalog_anime(&a7, 6).unwrap();
    assert_eq!(db.catalog_next_airing_rolled(7, now).unwrap(), None);

    // 8. Must NOT happen: no catalog row -> None.
    assert_eq!(db.catalog_next_airing_rolled(999_999, now).unwrap(), None);

    // 9. RELEASING, next_airing_at = now (strictly expired) -> rolled forward by 1 week.
    let mut a8 = catalog_anime(8, "Now Releasing", &[]);
    a8.status = Some("RELEASING".into());
    a8.next_airing_at = Some(now);
    a8.next_episode = Some(5);
    db.upsert_catalog_anime(&a8, 7).unwrap();
    assert_eq!(
        db.catalog_next_airing_rolled(8, now).unwrap(),
        Some((6, now + SEVEN_DAYS_SECS, true))
    );
}

#[test]
fn test_stale_status_ids_orders_aired_date_before_future_date_followed() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("TestSite", "https://site.example", "testsite").unwrap();
    let now = chrono::Utc::now().timestamp();

    // Id A (101): not followed, status RELEASING (matches WHERE), next_airing_at in past (already aired)
    let mut a = catalog_anime_with_popularity(101, "AiredAwaitingRefresh", &[], Some(10));
    a.status = Some("RELEASING".into());
    a.next_airing_at = Some(now - 3600);
    a.next_episode = Some(5);
    db.upsert_catalog_anime(&a, 0).unwrap();

    // Id B (102): followed series linked, is_airing = 1, status RELEASING, next_airing_at in future
    let mut b = catalog_anime_with_popularity(102, "FutureAirFollowed", &[], Some(100_000));
    b.status = Some("RELEASING".into());
    b.next_airing_at = Some(now + 3600);
    b.next_episode = Some(6);
    db.upsert_catalog_anime(&b, 1).unwrap();
    let sid = db.upsert_series(src, &crate::models::Series {
        id: 0,
        slug: "future-followed".into(),
        title: "FutureAirFollowed".into(),
        url: "https://example/future".into(),
        cover_url: None,
        is_airing: true,
        followed: true,
        next_episode_at: None,
        site_episode_count: None,
    }).unwrap();
    db.set_anilist_id(sid, 102).unwrap();
    db.set_followed(sid, true).unwrap();

    let ids = db.stale_status_ids().unwrap();
    assert!(ids.contains(&101));
    assert!(ids.contains(&102));
    assert_eq!(ids[0], 101, "aired-and-awaiting id A must come first despite id B being followed with higher popularity");
    assert_eq!(ids[1], 102);
}

#[test]
fn catalog_batch_count_clamping() {
    assert_eq!(0u32.clamp(1, 20), 1);
    assert_eq!(100u32.clamp(1, 20), 20);
    assert_eq!(5u32.clamp(1, 20), 5);
    assert_eq!(1u32.clamp(1, 20), 1);
    assert_eq!(20u32.clamp(1, 20), 20);
}

#[test]
fn is_long_runner_null_episodes_low_next_episode_is_false() {
    let db = Db::open(":memory:").unwrap();
    let mut anime = catalog_anime(1001, "Airing Short", &[]);
    anime.episodes = None;
    anime.next_episode = Some(4);
    db.upsert_catalog_anime(&anime, 0).unwrap();
    assert!(!db.is_long_runner(1001).unwrap());
}

#[test]
fn is_long_runner_null_episodes_high_next_episode_is_true() {
    let db = Db::open(":memory:").unwrap();
    let mut anime = catalog_anime(1002, "Airing Long", &[]);
    anime.episodes = None;
    anime.next_episode = Some(1100);
    db.upsert_catalog_anime(&anime, 0).unwrap();
    assert!(db.is_long_runner(1002).unwrap());
}

#[test]
fn is_long_runner_null_episodes_high_map_count_is_true() {
    let db = Db::open(":memory:").unwrap();
    let mut anime = catalog_anime(1003, "Mapped Long", &[]);
    anime.episodes = None;
    anime.next_episode = None;
    db.upsert_catalog_anime(&anime, 0).unwrap();
    for ep in 1..=70 {
        db.conn.execute(
            "INSERT INTO anilist_episode_map (anilist_id, episode, season) VALUES (?1, ?2, 1)",
            [1003, ep],
        ).unwrap();
    }
    assert!(db.is_long_runner(1003).unwrap());
}

#[test]
fn is_long_runner_short_episodes_is_false() {
    let db = Db::open(":memory:").unwrap();
    let mut anime = catalog_anime(1004, "Short Show", &[]);
    anime.episodes = Some(24);
    db.upsert_catalog_anime(&anime, 0).unwrap();
    assert!(!db.is_long_runner(1004).unwrap());
}

#[test]
fn is_long_runner_long_episodes_is_true() {
    let db = Db::open(":memory:").unwrap();
    let mut anime = catalog_anime(1005, "Long Show", &[]);
    anime.episodes = Some(1100);
    db.upsert_catalog_anime(&anime, 0).unwrap();
    assert!(db.is_long_runner(1005).unwrap());
}

#[test]
fn a_colon_subtitle_never_inherits_airing_short_show_link() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("AnimeYT", "a", "animeytx").unwrap();
    let mut tog = catalog_anime(115230, "Tower of God", &[]);
    tog.title_romaji = Some("Kami no Tou: Tower of God".into());
    tog.synonyms = vec!["Kami no Tou".into()];
    tog.episodes = None;
    tog.next_episode = Some(5);
    db.upsert_catalog_anime(&tog, 0).unwrap();
    let s2 = series_with(&db, src, "tog2", "Kami no Tou: Ouji no Kikan");

    db.link_series_to_catalog().unwrap();
    assert_eq!(linked_id(&db, s2), None);
}
