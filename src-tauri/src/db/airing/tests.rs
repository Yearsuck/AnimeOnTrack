use super::*;
use crate::db::test_support::*;

#[test]
fn shows_needing_episode_source_selection_and_expiry() {
    let db = Db::open(":memory:").unwrap();
    let src_a = db.upsert_source("AnimeYT", "https://a", "animeytx").unwrap();
    let src_b = db.upsert_source("TioAnime", "https://b", "tioanime").unwrap();
    let now = 10_000_000;

    let mk = |slug: &str, title: &str| crate::models::Series {
        id: 0, slug: slug.into(), title: title.into(), url: format!("u-{slug}"),
        cover_url: None, is_airing: true, followed: false, next_episode_at: None, site_episode_count: None,
    };

    // Helper to insert a catalog anime
    let insert_catalog = |id, status| {
        db.upsert_catalog_anime(&crate::db::test_support::catalog_anime(id, "Title", &[]), 0).unwrap();
        db.conn.execute("UPDATE anilist_catalog SET status=?1 WHERE id=?2", (status, id)).unwrap();
    };

    // Candidate 1: Followed, 0 eps, RELEASING -> Candidate
    insert_catalog(10, "RELEASING");
    let c1 = db.upsert_series(src_a, &mk("c1", "Candidate One")).unwrap();
    db.set_anilist_id(c1, 10).unwrap();
    db.set_followed(c1, true).unwrap();

    // Followed, 0 eps on A, but has eps on B -> NOT a Candidate
    insert_catalog(20, "RELEASING");
    let c2a = db.upsert_series(src_a, &mk("c2a", "Candidate Two")).unwrap();
    db.set_anilist_id(c2a, 20).unwrap();
    db.set_followed(c2a, true).unwrap();
    let c2b = db.upsert_series(src_b, &mk("c2b", "Candidate Two")).unwrap();
    db.set_anilist_id(c2b, 20).unwrap();
    db.insert_episode(&crate::models::Episode {
        id: 0, series_id: c2b, number: "1".into(), title: None, url: "u/1".into(), released_at: None, seen: false,
        download_path: None, download_status: None,
    }).unwrap();

    // Not followed -> NOT a Candidate
    insert_catalog(30, "RELEASING");
    let c3 = db.upsert_series(src_a, &mk("c3", "Candidate Three")).unwrap();
    db.set_anilist_id(c3, 30).unwrap();

    // Followed, 0 eps, RELEASING, but fresh miss marker -> NOT a Candidate
    insert_catalog(40, "RELEASING");
    let c4 = db.upsert_series(src_a, &mk("c4", "Candidate Four")).unwrap();
    db.set_anilist_id(c4, 40).unwrap();
    db.set_followed(c4, true).unwrap();
    db.record_episode_fallback_miss("al:40", now - 3600).unwrap(); // 1 hour ago

    // Followed, 0 eps, RELEASING, miss marker is expired (4 days ago) -> Candidate again
    insert_catalog(50, "RELEASING");
    let c5 = db.upsert_series(src_a, &mk("c5", "Candidate Five")).unwrap();
    db.set_anilist_id(c5, 50).unwrap();
    db.set_followed(c5, true).unwrap();
    db.record_episode_fallback_miss("al:50", now - 4 * 24 * 3600).unwrap();

    // Followed, 0 eps, but NOT_YET_RELEASED -> NOT a Candidate
    insert_catalog(60, "NOT_YET_RELEASED");
    let c6 = db.upsert_series(src_a, &mk("c6", "Candidate Six")).unwrap();
    db.set_anilist_id(c6, 60).unwrap();
    db.set_followed(c6, true).unwrap();

    // Followed, 0 eps, RELEASING, but watched_externally -> NOT a Candidate
    insert_catalog(70, "RELEASING");
    let c7 = db.upsert_series(src_a, &mk("c7", "Candidate Seven")).unwrap();
    db.set_anilist_id(c7, 70).unwrap();
    db.set_followed(c7, true).unwrap();
    db.conn.execute("UPDATE series SET watched_externally=1 WHERE id=?", [c7]).unwrap();

    // Followed from the AniList catalog (synthetic anilist-N row), 0 eps, RELEASING: a
    // Candidate (the real case: Kanojo no Tomodachi was followed this way).
    insert_catalog(80, "RELEASING");
    let c8 = db.upsert_series(src_a, &mk("anilist-80", "Candidate Eight")).unwrap();
    db.set_anilist_id(c8, 80).unwrap();
    db.set_followed(c8, true).unwrap();

    let candidates = db.shows_needing_episode_source(src_a, now).unwrap();
    let titles: Vec<&str> = candidates.iter().map(|c| c.title.as_str()).collect();
    assert_eq!(titles, vec!["Candidate Eight", "Candidate Five", "Candidate One"]);
}

#[test]
fn shows_needing_episode_source_releasing_behind_ani_list_is_candidate() {
    let db = Db::open(":memory:").unwrap();
    let src_a = db.upsert_source("AnimeYT", "https://a.example", "animeytx").unwrap();
    let now = 10_000_000;

    let mk = |slug: &str, title: &str, url: &str| crate::models::Series {
        id: 0, slug: slug.into(), title: title.into(), url: url.into(),
        cover_url: None, is_airing: true, followed: false, next_episode_at: None, site_episode_count: None,
    };

    // Catalog: RELEASING, next_episode = 6 (expected 5)
    db.upsert_catalog_anime(&crate::db::test_support::catalog_anime(100, "Airing Show", &[]), 0).unwrap();
    db.conn.execute(
        "UPDATE anilist_catalog SET status = 'RELEASING', next_episode = 6 WHERE id = 100",
        [],
    ).unwrap();

    // Series on site A, followed, linked to 100
    let s_a = db.upsert_series(src_a, &mk("airing-show", "Airing Show", "https://a.example/tv/airing-show/")).unwrap();
    db.set_anilist_id(s_a, 100).unwrap();
    db.set_followed(s_a, true).unwrap();

    // Episodes 1 to 4 on site A (highest is 4 < 5)
    for ep in 1..=4 {
        db.insert_episode(&crate::models::Episode {
            id: 0, series_id: s_a, number: ep.to_string(), title: None,
            url: format!("https://a.example/ep-{ep}"), released_at: None, seen: false,
            download_path: None, download_status: None,
        }).unwrap();
    }

    let candidates = db.shows_needing_episode_source(src_a, now).unwrap();
    assert_eq!(candidates.len(), 1);
    let c = &candidates[0];
    assert_eq!(c.canon_key, "al:100");
    assert_eq!(c.anilist_id, Some(100));
    assert_eq!(c.title, "Airing Show");
    assert_eq!(c.known_rows, vec![(s_a, "animeytx".to_string(), "https://a.example/tv/airing-show/".to_string())]);
}

#[test]
fn shows_needing_episode_source_caught_up_is_not_candidate() {
    let db = Db::open(":memory:").unwrap();
    let src_a = db.upsert_source("AnimeYT", "https://a.example", "animeytx").unwrap();
    let now = 10_000_000;

    let mk = |slug: &str, title: &str| crate::models::Series {
        id: 0, slug: slug.into(), title: title.into(), url: format!("u-{slug}"),
        cover_url: None, is_airing: true, followed: false, next_episode_at: None, site_episode_count: None,
    };

    db.upsert_catalog_anime(&crate::db::test_support::catalog_anime(100, "Airing Show", &[]), 0).unwrap();
    db.conn.execute(
        "UPDATE anilist_catalog SET status = 'RELEASING', next_episode = 6 WHERE id = 100",
        [],
    ).unwrap();

    let s_a = db.upsert_series(src_a, &mk("airing-show", "Airing Show")).unwrap();
    db.set_anilist_id(s_a, 100).unwrap();
    db.set_followed(s_a, true).unwrap();

    // Episodes 1 to 5 on A (5 == expected 5)
    for ep in 1..=5 {
        db.insert_episode(&crate::models::Episode {
            id: 0, series_id: s_a, number: ep.to_string(), title: None,
            url: format!("https://a.example/ep-{ep}"), released_at: None, seen: false,
            download_path: None, download_status: None,
        }).unwrap();
    }

    let candidates = db.shows_needing_episode_source(src_a, now).unwrap();
    assert!(candidates.is_empty(), "caught-up show (ep 5 == expected 5) must not be candidate");
}

#[test]
fn shows_needing_episode_source_finished_or_null_next_with_episodes_not_candidate() {
    let db = Db::open(":memory:").unwrap();
    let src_a = db.upsert_source("AnimeYT", "https://a.example", "animeytx").unwrap();
    let now = 10_000_000;

    let mk = |slug: &str, title: &str| crate::models::Series {
        id: 0, slug: slug.into(), title: title.into(), url: format!("u-{slug}"),
        cover_url: None, is_airing: true, followed: false, next_episode_at: None, site_episode_count: None,
    };

    // FINISHED show with episodes
    db.upsert_catalog_anime(&crate::db::test_support::catalog_anime(201, "Finished Show", &[]), 0).unwrap();
    db.conn.execute(
        "UPDATE anilist_catalog SET status = 'FINISHED', next_episode = 6 WHERE id = 201",
        [],
    ).unwrap();
    let s1 = db.upsert_series(src_a, &mk("finished-show", "Finished Show")).unwrap();
    db.set_anilist_id(s1, 201).unwrap();
    db.set_followed(s1, true).unwrap();
    db.insert_episode(&crate::models::Episode {
        id: 0, series_id: s1, number: "1".into(), title: None,
        url: "u/1".into(), released_at: None, seen: false, download_path: None, download_status: None,
    }).unwrap();

    // RELEASING show with next_episode NULL and episodes present
    db.upsert_catalog_anime(&crate::db::test_support::catalog_anime(202, "Null Next Show", &[]), 0).unwrap();
    db.conn.execute(
        "UPDATE anilist_catalog SET status = 'RELEASING', next_episode = NULL WHERE id = 202",
        [],
    ).unwrap();
    let s2 = db.upsert_series(src_a, &mk("null-next-show", "Null Next Show")).unwrap();
    db.set_anilist_id(s2, 202).unwrap();
    db.set_followed(s2, true).unwrap();
    db.insert_episode(&crate::models::Episode {
        id: 0, series_id: s2, number: "1".into(), title: None,
        url: "u/2".into(), released_at: None, seen: false, download_path: None, download_status: None,
    }).unwrap();

    let candidates = db.shows_needing_episode_source(src_a, now).unwrap();
    assert!(candidates.is_empty(), "FINISHED with eps and NULL next_episode with eps must not be candidates");
}

#[test]
fn shows_needing_episode_source_fresh_group_miss_marker_skips_candidate() {
    let db = Db::open(":memory:").unwrap();
    let src_a = db.upsert_source("AnimeYT", "https://a.example", "animeytx").unwrap();
    let now = 10_000_000;

    let mk = |slug: &str, title: &str| crate::models::Series {
        id: 0, slug: slug.into(), title: title.into(), url: format!("u-{slug}"),
        cover_url: None, is_airing: true, followed: false, next_episode_at: None, site_episode_count: None,
    };

    db.upsert_catalog_anime(&crate::db::test_support::catalog_anime(300, "Miss Show", &[]), 0).unwrap();
    db.conn.execute(
        "UPDATE anilist_catalog SET status = 'RELEASING', next_episode = 6 WHERE id = 300",
        [],
    ).unwrap();
    let s = db.upsert_series(src_a, &mk("miss-show", "Miss Show")).unwrap();
    db.set_anilist_id(s, 300).unwrap();
    db.set_followed(s, true).unwrap();
    db.insert_episode(&crate::models::Episode {
        id: 0, series_id: s, number: "1".into(), title: None,
        url: "u/1".into(), released_at: None, seen: false, download_path: None, download_status: None,
    }).unwrap();

    db.record_episode_fallback_miss("al:300", now - 3600).unwrap();

    let candidates = db.shows_needing_episode_source(src_a, now).unwrap();
    assert!(candidates.is_empty(), "show with fresh miss marker must not be candidate");
}

#[test]
fn shows_needing_episode_source_zero_episodes_old_behaviour_intact() {
    let db = Db::open(":memory:").unwrap();
    let src_a = db.upsert_source("AnimeYT", "https://a.example", "animeytx").unwrap();
    let now = 10_000_000;

    let mk = |slug: &str, title: &str| crate::models::Series {
        id: 0, slug: slug.into(), title: title.into(), url: format!("u-{slug}"),
        cover_url: None, is_airing: true, followed: false, next_episode_at: None, site_episode_count: None,
    };

    db.upsert_catalog_anime(&crate::db::test_support::catalog_anime(400, "Zero Eps Show", &[]), 0).unwrap();
    db.conn.execute(
        "UPDATE anilist_catalog SET status = 'FINISHED' WHERE id = 400",
        [],
    ).unwrap();
    let s = db.upsert_series(src_a, &mk("zero-eps", "Zero Eps Show")).unwrap();
    db.set_anilist_id(s, 400).unwrap();
    db.set_followed(s, true).unwrap();

    let candidates = db.shows_needing_episode_source(src_a, now).unwrap();
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].canon_key, "al:400");
    assert!(candidates[0].known_rows.is_empty(), "old behaviour: empty known_rows");
}

#[test]
fn shows_needing_episode_source_unfollowed_or_watched_externally_never_candidate() {
    let db = Db::open(":memory:").unwrap();
    let src_a = db.upsert_source("AnimeYT", "https://a.example", "animeytx").unwrap();
    let now = 10_000_000;

    let mk = |slug: &str, title: &str| crate::models::Series {
        id: 0, slug: slug.into(), title: title.into(), url: format!("u-{slug}"),
        cover_url: None, is_airing: true, followed: false, next_episode_at: None, site_episode_count: None,
    };

    // Unfollowed behind show
    db.upsert_catalog_anime(&crate::db::test_support::catalog_anime(501, "Unfollowed", &[]), 0).unwrap();
    db.conn.execute(
        "UPDATE anilist_catalog SET status = 'RELEASING', next_episode = 6 WHERE id = 501",
        [],
    ).unwrap();
    let s1 = db.upsert_series(src_a, &mk("unfollowed", "Unfollowed")).unwrap();
    db.set_anilist_id(s1, 501).unwrap();
    db.insert_episode(&crate::models::Episode {
        id: 0, series_id: s1, number: "1".into(), title: None,
        url: "u/1".into(), released_at: None, seen: false, download_path: None, download_status: None,
    }).unwrap();

    // Followed but watched_externally show
    db.upsert_catalog_anime(&crate::db::test_support::catalog_anime(502, "Watched Ext", &[]), 0).unwrap();
    db.conn.execute(
        "UPDATE anilist_catalog SET status = 'RELEASING', next_episode = 6 WHERE id = 502",
        [],
    ).unwrap();
    let s2 = db.upsert_series(src_a, &mk("watched-ext", "Watched Ext")).unwrap();
    db.set_anilist_id(s2, 502).unwrap();
    db.set_followed(s2, true).unwrap();
    db.conn.execute("UPDATE series SET watched_externally = 1 WHERE id = ?1", [s2]).unwrap();
    db.insert_episode(&crate::models::Episode {
        id: 0, series_id: s2, number: "1".into(), title: None,
        url: "u/2".into(), released_at: None, seen: false, download_path: None, download_status: None,
    }).unwrap();

    let candidates = db.shows_needing_episode_source(src_a, now).unwrap();
    assert!(candidates.is_empty(), "unfollowed or watched_externally must never be a candidate");
}

#[test]
fn list_airing_orders_newest_first_nulls_last_title_tiebreak() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
    let base = chrono::Utc::now().timestamp() - 5 * 86400;
    db.upsert_series(src, &mk_airing("older", "Older", Some(base))).unwrap();
    db.upsert_series(src, &mk_airing("newer", "Newer", Some(base + 86400))).unwrap();
    db.upsert_series(src, &mk_airing("nodate", "NoDate", None)).unwrap();
    db.upsert_series(src, &mk_airing("tie-b", "Tie B", Some(base))).unwrap();

    let titles: Vec<String> = db.list_airing(src).unwrap().into_iter().map(|s| s.title).collect();
    assert_eq!(titles, vec!["Newer", "Older", "Tie B", "NoDate"]);
}

#[test]
fn pending_count_and_list_pending_fallback_to_other_sites() {
    let db = Db::open(":memory:").unwrap();
    let src_a = db.upsert_source("AnimeYT", "https://a.example", "animeytx").unwrap(); // active
    let src_b = db.upsert_source("TioAnime", "https://b.example", "tioanime").unwrap();
    let src_c = db.upsert_source("JKAnime", "https://c.example", "jkanime").unwrap();

    let mk = |slug: &str, title: &str| crate::models::Series {
        id: 0, slug: slug.into(), title: title.into(), url: format!("u-{slug}"),
        cover_url: None, is_airing: true, followed: false, next_episode_at: None, site_episode_count: None,
    };

    // 1. show on active site with episodes (unchanged)
    let active_has_eps = db.upsert_series(src_a, &mk("act", "Active Has Eps")).unwrap();
    db.set_followed(active_has_eps, true).unwrap();
    db.insert_episode(&crate::models::Episode {
        id: 0, series_id: active_has_eps, number: "1".into(), title: None,
        url: "u/1".into(), released_at: None, seen: false, download_path: None, download_status: None,
    }).unwrap();

    // 2. show only on another site
    let only_other = db.upsert_series(src_b, &mk("only-b", "Only Other")).unwrap();
    db.set_followed(only_other, true).unwrap();
    db.insert_episode(&crate::models::Episode {
        id: 0, series_id: only_other, number: "1".into(), title: None,
        url: "u/1".into(), released_at: None, seen: false, download_path: None, download_status: None,
    }).unwrap();

    // 3. show on two other sites (the one with more episodes wins)
    let two_b = db.upsert_series(src_b, &mk("two-b", "Two Others")).unwrap();
    db.set_anilist_id(two_b, 100).unwrap();
    db.set_followed(two_b, true).unwrap();
    db.insert_episode(&crate::models::Episode { id: 0, series_id: two_b, number: "1".into(), title: None, url: "u/1".into(), released_at: None, seen: false, download_path: None, download_status: None }).unwrap();

    let two_c = db.upsert_series(src_c, &mk("two-c", "Two Others")).unwrap();
    db.set_anilist_id(two_c, 100).unwrap();
    // C has 2 episodes, so C wins
    db.insert_episode(&crate::models::Episode { id: 0, series_id: two_c, number: "1".into(), title: None, url: "u/1".into(), released_at: None, seen: false, download_path: None, download_status: None }).unwrap();
    db.insert_episode(&crate::models::Episode { id: 0, series_id: two_c, number: "2".into(), title: None, url: "u/2".into(), released_at: None, seen: false, download_path: None, download_status: None }).unwrap();

    // 4. show on active site with 0 eps, but another site has eps
    let active_zero = db.upsert_series(src_a, &mk("zero-a", "Active Zero")).unwrap();
    db.set_anilist_id(active_zero, 200).unwrap();
    db.set_followed(active_zero, true).unwrap();

    let zero_b = db.upsert_series(src_b, &mk("zero-b", "Active Zero")).unwrap();
    db.set_anilist_id(zero_b, 200).unwrap();
    db.insert_episode(&crate::models::Episode { id: 0, series_id: zero_b, number: "1".into(), title: None, url: "u/1".into(), released_at: None, seen: false, download_path: None, download_status: None }).unwrap();

    // 5. unfollowed rows never appear
    let unfollowed = db.upsert_series(src_b, &mk("unf", "Unfollowed")).unwrap();
    db.insert_episode(&crate::models::Episode { id: 0, series_id: unfollowed, number: "1".into(), title: None, url: "u/1".into(), released_at: None, seen: false, download_path: None, download_status: None }).unwrap();

    // Expected for active source (A), merged per episode number across sites:
    // Active Has Eps: 1 (A)
    // Only Other: 1 (B)
    // Two Others: 2 (episode 1 from B, the lowest id on a priority tie; episode 2 only exists on C)
    // Active Zero: 1 (B, because A has none)
    // Total pending count = 1 + 1 + 2 + 1 = 5.
    assert_eq!(db.pending_count(src_a, &[]).unwrap(), 5);

    let pending = db.list_pending(src_a, PendingSort::RemainingAsc, &[]).unwrap();
    assert_eq!(pending.len(), 5);
    // The episode keeps its own row; the series is the group's representative.
    let owners: Vec<i64> = pending.iter().map(|(_, e)| e.series_id).collect();
    assert!(owners.contains(&active_has_eps));
    assert!(owners.contains(&only_other));
    assert!(owners.contains(&two_c), "episode 2 only exists on the third site");
    assert!(owners.contains(&two_b), "episode 1 comes from the lowest id on a tie");
    assert!(owners.contains(&zero_b));
    assert!(!owners.contains(&active_zero));
    assert!(!owners.contains(&unfollowed));
    let two_others: Vec<&str> = pending
        .iter()
        .filter(|(s, _)| s.title == "Two Others")
        .map(|(_, e)| e.number.as_str())
        .collect();
    assert_eq!(two_others, vec!["1", "2"], "one entry per episode number");
}

#[test]
fn list_pending_orders_groups_by_remaining_count() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("AnimeYT", "https://a.example", "animeytx").unwrap();
    let mk = |slug: &str| crate::models::Series {
        id: 0, slug: slug.into(), title: slug.into(), url: format!("u-{slug}"),
        cover_url: None, is_airing: true, followed: false, next_episode_at: None, site_episode_count: None,
    };
    // "few" has 1 pending episode, "many" has 3.
    let few = db.upsert_series(src, &mk("few")).unwrap();
    db.set_followed(few, true).unwrap();
    let many = db.upsert_series(src, &mk("many")).unwrap();
    db.set_followed(many, true).unwrap();
    db.insert_episode(&crate::models::Episode {
        id: 0, series_id: few, number: "1".into(), title: None,
        url: "few/1".into(), released_at: None, seen: false, download_path: None, download_status: None,
    }).unwrap();
    for n in 1..=3 {
        db.insert_episode(&crate::models::Episode {
            id: 0, series_id: many, number: n.to_string(), title: None,
            url: format!("many/{n}"), released_at: None, seen: false, download_path: None, download_status: None,
        }).unwrap();
    }

    // Ascending: the 1-episode series' rows come before the 3-episode one.
    let asc = db.list_pending(src, PendingSort::RemainingAsc, &[]).unwrap();
    assert_eq!(asc.first().unwrap().0.id, few);
    assert_eq!(asc.last().unwrap().0.id, many);
    // Descending: reversed.
    let desc = db.list_pending(src, PendingSort::RemainingDesc, &[]).unwrap();
    assert_eq!(desc.first().unwrap().0.id, many);
    assert_eq!(desc.last().unwrap().0.id, few);
    // Each series' episodes stay contiguous (no interleaving).
    let ids: Vec<i64> = asc.iter().map(|(s, _)| s.id).collect();
    assert_eq!(ids, vec![few, many, many, many]);
}

#[test]
fn list_pending_orders_episodes_numerically_within_a_series_not_by_insertion_order() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("AnimeYT", "https://a.example", "animeytx").unwrap();
    let mk = |slug: &str| crate::models::Series {
        id: 0, slug: slug.into(), title: slug.into(), url: format!("u-{slug}"),
        cover_url: None, is_airing: true, followed: false, next_episode_at: None, site_episode_count: None,
    };
    let sid = db.upsert_series(src, &mk("show")).unwrap();
    db.set_followed(sid, true).unwrap();
    // Inserted out of numeric order — a re-scan/backfill shape. If the
    // ordering were still keyed on insertion time (or a plain string
    // sort), "10" would surface before "2".
    for n in ["10", "2", "1"] {
        db.insert_episode(&crate::models::Episode {
            id: 0, series_id: sid, number: n.into(), title: None,
            url: format!("show/{n}"), released_at: None, seen: false, download_path: None, download_status: None,
        }).unwrap();
    }

    let pending = db.list_pending(src, PendingSort::RemainingAsc, &[]).unwrap();
    let numbers: Vec<&str> = pending.iter().map(|(_, e)| e.number.as_str()).collect();
    assert_eq!(numbers, vec!["1", "2", "10"], "numeric order, not insertion order or string order");
}

#[test]
fn list_airing_is_a_canonical_union_deduped_across_sites() {
    let db = Db::open(":memory:").unwrap();
    let a = db.upsert_source("AnimeYT", "https://a", "animeytx").unwrap();
    let b = db.upsert_source("TioAnime", "https://b", "tioanime").unwrap();
    let mk = |slug: &str, title: &str| crate::models::Series {
        id: 0, slug: slug.into(), title: title.into(), url: format!("u-{slug}"),
        cover_url: None, is_airing: true, followed: false, next_episode_at: None, site_episode_count: None,
    };
    // Catalog provides the canonical AniList display title for both sites.
    db.upsert_catalog_anime(&crate::db::test_support::catalog_anime(21, "One Piece", &["Action"]), 0).unwrap();
    db.upsert_catalog_anime(&crate::db::test_support::catalog_anime(99, "AnimeYT Only", &["Action"]), 0).unwrap();
    // Same show (same anilist id) airing on both sites + one site-only show.
    let one_a = db.upsert_series(a, &mk("op-a", "ONE PIECE")).unwrap();
    db.set_anilist_id(one_a, 21).unwrap();
    let one_b = db.upsert_series(b, &mk("op-b", "one piece latino")).unwrap();
    db.set_anilist_id(one_b, 21).unwrap();
    let solo = db.upsert_series(a, &mk("solo", "AnimeYT Only")).unwrap();
    db.set_anilist_id(solo, 99).unwrap();

    // Identical set regardless of the active site: One Piece (once) + solo.
    let from_a = db.list_airing(a).unwrap();
    let from_b = db.list_airing(b).unwrap();
    assert_eq!(from_a.len(), 2, "deduped: One Piece appears once, not twice");
    let titles_a: Vec<&str> = from_a.iter().map(|s| s.title.as_str()).collect();
    let titles_b: Vec<&str> = from_b.iter().map(|s| s.title.as_str()).collect();
    assert_eq!(titles_a, titles_b, "same airing set on either active site");
    // Representative prefers the active site (so its episodes open there).
    let op_from_b = from_b.iter().find(|s| s.title.eq_ignore_ascii_case("one piece")).unwrap();
    assert_eq!(op_from_b.id, one_b, "active-site member is the representative");
}

#[test]
fn list_airing_borrows_a_displayable_cover_from_a_sibling_site_when_the_representative_has_none() {
    // The reported bug: covers are only ever fetched for the currently
    // active site's own rows (refresh() is inherently one-active-site-
    // at-a-time), but list_airing is cross-site — a show airing on
    // several sites can pick a representative from whichever site is
    // active even when THAT site's own copy has never had its cover
    // fetched, while a sibling site's copy already has a real one. The
    // card showed the fallback tile forever even after the active
    // site's own backlog had long since converged.
    let db = Db::open(":memory:").unwrap();
    let a = db.upsert_source("AnimeYT", "https://a", "animeytx").unwrap();
    let b = db.upsert_source("TioAnime", "https://b", "tioanime").unwrap();
    let mk = |slug: &str, cover: &str| crate::models::Series {
        id: 0, slug: slug.into(), title: "Shared Show".into(), url: format!("u-{slug}"),
        cover_url: Some(cover.into()), is_airing: true, followed: false, next_episode_at: None, site_episode_count: None,
    };
    // Unlinked on both sides (no anilist_id) — same franchise_dedup_key
    // from the identical title is what groups them.
    let on_a = db.upsert_series(a, &mk("shared-a", "data:image/png;base64,AAAA")).unwrap();
    let on_b = db.upsert_series(b, &mk("shared-b", "https://tioanime.com/portadas/1.jpg")).unwrap();

    // B is active, so B's own row (blocked cover) is the representative
    // for url/id purposes — but the displayed cover must come from A.
    let out = db.list_airing(b).unwrap();
    assert_eq!(out.len(), 1, "deduped to one canonical entry");
    assert_eq!(out[0].id, on_b, "active-site row is still the representative for opening episodes");
    assert_eq!(out[0].cover_url.as_deref(), Some("data:image/png;base64,AAAA"), "cover borrowed from sibling site A");

    // If the representative's OWN cover is already displayable, nothing
    // is borrowed — its own value wins unchanged.
    let out_from_a = db.list_airing(a).unwrap();
    assert_eq!(out_from_a[0].id, on_a);
    assert_eq!(out_from_a[0].cover_url.as_deref(), Some("data:image/png;base64,AAAA"));
}

#[test]
fn list_airing_uses_the_freshest_next_episode_at_across_merged_members() {
    let db = Db::open(":memory:").unwrap();
    let a = db.upsert_source("AnimeYT", "https://a", "animeytx").unwrap();
    let b = db.upsert_source("TioAnime", "https://b", "tioanime").unwrap();
    let mk = |slug: &str, title: &str, next: Option<i64>| crate::models::Series {
        id: 0, slug: slug.into(), title: title.into(), url: format!("u-{slug}"),
        cover_url: None, is_airing: true, followed: false, next_episode_at: next, site_episode_count: None,
    };
    // Active site's member (a) is stale/missing next_episode_at (e.g. its
    // mirror has been down and hasn't rescanned successfully); site b's
    // member has fresh data. The merged entry must surface b's timestamp
    // instead of silently falling back to a's None just because a is the
    // representative (active-site) member — otherwise a stale site drags
    // an otherwise-current show to the bottom of "En emisión".
    let one_a = db.upsert_series(a, &mk("op-a", "One Piece", None)).unwrap();
    db.set_anilist_id(one_a, 21).unwrap();
    let one_b = db.upsert_series(b, &mk("op-b", "One Piece", Some(1_800_000_000))).unwrap();
    db.set_anilist_id(one_b, 21).unwrap();

    let airing = db.list_airing(a).unwrap();
    assert_eq!(airing.len(), 1);
    let entry = &airing[0];
    assert_eq!(entry.id, one_a, "active-site member is still the representative for id/url");
    assert_eq!(
        entry.next_episode_at,
        Some(1_800_000_000),
        "but the sort-relevant timestamp is the freshest across the whole group"
    );
}

#[test]
fn list_airing_dedups_unlinked_season_variants_across_sites() {
    let db = Db::open(":memory:").unwrap();
    let a = db.upsert_source("AnimeYT", "https://a", "animeytx").unwrap();
    let b = db.upsert_source("TioAnime", "https://b", "tioanime").unwrap();
    let mk = |slug: &str, title: &str| crate::models::Series {
        id: 0, slug: slug.into(), title: title.into(), url: format!("u-{slug}"),
        cover_url: None, is_airing: true, followed: false, next_episode_at: None, site_episode_count: None,
    };
    // Same show, NO anilist id on either, titles differ only by the season
    // marker's language — must still collapse to one airing entry.
    db.upsert_series(a, &mk("hm-a", "Hell Mode: Yarikomizuki no Gamer Temporada 2")).unwrap();
    db.upsert_series(b, &mk("hm-b", "Hell Mode: Yarikomizuki no Gamer 2nd Season")).unwrap();
    let airing = db.list_airing(a).unwrap();
    assert_eq!(airing.len(), 1, "unlinked season variants dedup to one entry");
}

#[test]
fn list_airing_marks_followed_when_any_site_follows_it() {
    let db = Db::open(":memory:").unwrap();
    let a = db.upsert_source("AnimeYT", "https://a", "animeytx").unwrap();
    let b = db.upsert_source("TioAnime", "https://b", "tioanime").unwrap();
    let mk = |slug: &str, title: &str| crate::models::Series {
        id: 0, slug: slug.into(), title: title.into(), url: format!("u-{slug}"),
        cover_url: None, is_airing: true, followed: false, next_episode_at: None, site_episode_count: None,
    };
    let one_a = db.upsert_series(a, &mk("op-a", "ONE PIECE")).unwrap();
    db.set_anilist_id(one_a, 21).unwrap();
    let one_b = db.upsert_series(b, &mk("op-b", "One Piece")).unwrap();
    db.set_anilist_id(one_b, 21).unwrap();
    // Followed only on AnimeYT.
    db.set_followed(one_a, true).unwrap();

    // From TioAnime's perspective the canonical entry is still "followed".
    let op = db.list_airing(b).unwrap().into_iter().find(|s| s.title.eq_ignore_ascii_case("one piece")).unwrap();
    assert!(op.followed, "followed on any site => followed in the canonical list");
}

#[test]
fn set_followed_canonical_follows_the_show_on_every_site() {
    let db = Db::open(":memory:").unwrap();
    let a = db.upsert_source("AnimeYT", "https://a", "animeytx").unwrap();
    let b = db.upsert_source("TioAnime", "https://b", "tioanime").unwrap();
    let mk = |slug: &str, title: &str| crate::models::Series {
        id: 0, slug: slug.into(), title: title.into(), url: format!("u-{slug}"),
        cover_url: None, is_airing: true, followed: false, next_episode_at: None, site_episode_count: None,
    };
    let one_a = db.upsert_series(a, &mk("op-a", "ONE PIECE")).unwrap();
    db.set_anilist_id(one_a, 21).unwrap();
    let one_b = db.upsert_series(b, &mk("op-b", "One Piece")).unwrap();
    db.set_anilist_id(one_b, 21).unwrap();

    // Following on B follows the AnimeYT row too (same anilist id).
    let changed = db.set_followed_canonical(one_b, true).unwrap();
    assert_eq!(changed, 2);
    assert!(db.list_followed(a).unwrap().iter().any(|s| s.id == one_a));
    assert!(db.list_followed(b).unwrap().iter().any(|s| s.id == one_b));

    // No-anilist rows fall back to normalized-title matching.
    let x1 = db.upsert_series(a, &mk("x1", "Some  Show")).unwrap();
    let x2 = db.upsert_series(b, &mk("x2", "some show")).unwrap();
    assert_eq!(db.set_followed_canonical(x1, true).unwrap(), 2);
    assert!(db.list_followed(b).unwrap().iter().any(|s| s.id == x2));
}

#[test]
fn is_csp_displayable_cover_handles_all_supported_schemes() {
    assert!(!is_csp_displayable_cover(None));
    assert!(!is_csp_displayable_cover(Some("")));
    assert!(is_csp_displayable_cover(Some("data:image/jpeg;base64,1234")));
    assert!(is_csp_displayable_cover(Some("file:covers/abc.jpg")));
    assert!(is_csp_displayable_cover(Some("file:///C:/Users/app/covers/abc.jpg")));
    assert!(is_csp_displayable_cover(Some("asset://localhost/covers/abc.jpg")));
    assert!(is_csp_displayable_cover(Some("https://s4.anilist.co/file/anilistcdn/media/anime/cover/medium/bx123.jpg")));
    assert!(is_csp_displayable_cover(Some("https://anilist.co/img/cover.jpg")));

    // Scraped remote covers must NOT be considered CSP displayable
    assert!(!is_csp_displayable_cover(Some("https://i0.wp.com/animeflv.net/cover.jpg")));
    assert!(!is_csp_displayable_cover(Some("https://cdn.jkdesa.com/cover.jpg")));
    assert!(!is_csp_displayable_cover(Some("https://animeflv.net/cover.jpg")));
    assert!(!is_csp_displayable_cover(Some("https://tioanime.com/cover.jpg")));
    assert!(!is_csp_displayable_cover(Some("https://w7.animeland.tv/cover.jpg")));
    assert!(!is_csp_displayable_cover(Some("https://wwv.animeytx.net/cover.jpg")));
}

#[test]
fn list_airing_uses_catalog_next_airing_at_when_linked_and_future() {
    let db = Db::open(":memory:").unwrap();
    let a = db.upsert_source("AnimeYT", "https://a", "animeytx").unwrap();
    let now = chrono::Utc::now().timestamp();

    db.upsert_catalog_anime(&crate::db::test_support::catalog_anime(21, "One Piece", &[]), 0).unwrap();
    db.conn.execute("UPDATE anilist_catalog SET next_airing_at = ?1 WHERE id = 21", [now + 86400]).unwrap();

    let id = db.upsert_series(a, &mk_airing("op", "One Piece", Some(now + 100))).unwrap();
    db.set_anilist_id(id, 21).unwrap();

    db.upsert_series(a, &mk_airing("other", "Other", Some(now + 50000))).unwrap();

    let airing = db.list_airing(a).unwrap();
    assert_eq!(airing.len(), 2);
    assert_eq!(airing[0].title, "One Piece");
    assert_eq!(airing[0].next_episode_at, Some(now + 86400));
    assert_eq!(airing[1].title, "Other");
    assert_eq!(airing[1].next_episode_at, Some(now + 50000));
}

#[test]
fn list_airing_keeps_scraped_value_when_catalog_value_is_null_or_unlinked() {
    let db = Db::open(":memory:").unwrap();
    let a = db.upsert_source("AnimeYT", "https://a", "animeytx").unwrap();
    let now = chrono::Utc::now().timestamp();

    db.upsert_catalog_anime(&crate::db::test_support::catalog_anime(21, "One Piece", &[]), 0).unwrap();

    let id1 = db.upsert_series(a, &mk_airing("op", "One Piece", Some(now + 100))).unwrap();
    db.set_anilist_id(id1, 21).unwrap();

    db.upsert_series(a, &mk_airing("other", "Other", Some(now + 200))).unwrap();

    let airing = db.list_airing(a).unwrap();
    let op = airing.iter().find(|s| s.title == "One Piece").unwrap();
    assert_eq!(op.next_episode_at, Some(now + 100));

    let other = airing.iter().find(|s| s.title == "Other").unwrap();
    assert_eq!(other.next_episode_at, Some(now + 200));
}

#[test]
fn list_airing_merges_two_sites_with_catalog_value() {
    let db = Db::open(":memory:").unwrap();
    let a = db.upsert_source("AnimeYT", "https://a", "animeytx").unwrap();
    let b = db.upsert_source("TioAnime", "https://b", "tioanime").unwrap();
    let now = chrono::Utc::now().timestamp();

    db.upsert_catalog_anime(&crate::db::test_support::catalog_anime(21, "One Piece", &[]), 0).unwrap();
    db.conn.execute("UPDATE anilist_catalog SET next_airing_at = ?1 WHERE id = 21", [now + 86400]).unwrap();

    let id_a = db.upsert_series(a, &mk_airing("op-a", "One Piece", Some(now + 100))).unwrap();
    db.set_anilist_id(id_a, 21).unwrap();

    let id_b = db.upsert_series(b, &mk_airing("op-b", "One Piece", Some(now + 200))).unwrap();
    db.set_anilist_id(id_b, 21).unwrap();

    let airing = db.list_airing(a).unwrap();
    assert_eq!(airing.len(), 1);
    assert_eq!(airing[0].next_episode_at, Some(now + 86400));
}

#[test]
fn list_airing_falls_back_to_scraped_when_catalog_value_is_stale() {
    let db = Db::open(":memory:").unwrap();
    let a = db.upsert_source("AnimeYT", "https://a", "animeytx").unwrap();
    let now = chrono::Utc::now().timestamp();

    db.upsert_catalog_anime(&crate::db::test_support::catalog_anime(21, "One Piece", &[]), 0).unwrap();
    db.conn.execute("UPDATE anilist_catalog SET next_airing_at = ?1 WHERE id = 21", [now - 3600]).unwrap();

    let id1 = db.upsert_series(a, &mk_airing("op", "One Piece", Some(now + 100))).unwrap();
    db.set_anilist_id(id1, 21).unwrap();

    let airing = db.list_airing(a).unwrap();
    assert_eq!(airing[0].next_episode_at, Some(now + 100));
}

#[test]
fn list_airing_ordering_with_mixed_entries() {
    let db = Db::open(":memory:").unwrap();
    let a = db.upsert_source("AnimeYT", "https://a", "animeytx").unwrap();
    let now = chrono::Utc::now().timestamp();

    db.upsert_catalog_anime(&crate::db::test_support::catalog_anime(1, "Linked Future", &[]), 0).unwrap();
    db.conn.execute("UPDATE anilist_catalog SET next_airing_at = ?1 WHERE id = 1", [now + 10000]).unwrap();
    let id1 = db.upsert_series(a, &mk_airing("s1", "Linked Future", Some(0))).unwrap();
    db.set_anilist_id(id1, 1).unwrap();

    db.upsert_catalog_anime(&crate::db::test_support::catalog_anime(2, "Linked Null", &[]), 0).unwrap();
    let id2 = db.upsert_series(a, &mk_airing("s2", "Linked Null", Some(now + 5000))).unwrap();
    db.set_anilist_id(id2, 2).unwrap();

    db.upsert_series(a, &mk_airing("s3", "Unlinked Future", Some(now + 20000))).unwrap();
    db.upsert_series(a, &mk_airing("s4", "Unlinked Null", None)).unwrap();

    let airing = db.list_airing(a).unwrap();
    let titles: Vec<&str> = airing.iter().map(|s| s.title.as_str()).collect();
    assert_eq!(titles, vec!["Unlinked Future", "Linked Future", "Linked Null", "Unlinked Null"]);
}

#[test]
fn list_airing_ignores_a_catalog_time_for_a_different_season() {
    let db = Db::open(":memory:").unwrap();
    let a = db.upsert_source("AnimeYT", "https://a", "animeytx").unwrap();
    let now = chrono::Utc::now().timestamp();

    // Only season 1 exists in the catalog; the site row is season 2.
    db.upsert_catalog_anime(&crate::db::test_support::catalog_anime(7, "Show", &[]), 0).unwrap();
    db.conn.execute("UPDATE anilist_catalog SET next_airing_at = ?1 WHERE id = 7", [now + 86400]).unwrap();
    let id = db.upsert_series(a, &mk_airing("show-2", "Show Temporada 2", Some(now + 100))).unwrap();
    db.set_anilist_id(id, 7).unwrap();

    let airing = db.list_airing(a).unwrap();
    assert_eq!(airing[0].next_episode_at, Some(now + 100), "a wrong-season link must not lend its countdown");
}

#[test]
fn list_airing_prefers_a_newer_scraped_time_over_a_catalog_time_already_passed() {
    let db = Db::open(":memory:").unwrap();
    let a = db.upsert_source("AnimeYT", "https://a", "animeytx").unwrap();
    let now = chrono::Utc::now().timestamp();

    db.upsert_catalog_anime(&crate::db::test_support::catalog_anime(21, "One Piece", &[]), 0).unwrap();
    // episode aired 3 h ago; the catalog snapshot is stale, the site already shows next week
    db.conn.execute("UPDATE anilist_catalog SET next_airing_at = ?1 WHERE id = 21", [now - 3 * 3600]).unwrap();
    let id = db.upsert_series(a, &mk_airing("op", "One Piece", Some(now + 6 * 86400))).unwrap();
    db.set_anilist_id(id, 21).unwrap();

    let airing = db.list_airing(a).unwrap();
    assert_eq!(airing[0].next_episode_at, Some(now + 6 * 86400));
}

#[test]
fn list_airing_keeps_the_scraped_time_of_an_unlinked_row_even_if_a_catalog_row_shares_its_title() {
    let db = Db::open(":memory:").unwrap();
    let a = db.upsert_source("AnimeYT", "https://a", "animeytx").unwrap();
    let now = chrono::Utc::now().timestamp();

    db.upsert_catalog_anime(&crate::db::test_support::catalog_anime(21, "One Piece", &[]), 0).unwrap();
    db.conn.execute("UPDATE anilist_catalog SET next_airing_at = ?1 WHERE id = 21", [now + 86400]).unwrap();
    db.upsert_series(a, &mk_airing("op", "One Piece", Some(now + 100))).unwrap(); // not linked

    let airing = db.list_airing(a).unwrap();
    assert_eq!(airing[0].next_episode_at, Some(now + 100));
}

#[test]
fn list_airing_hides_an_unfollowed_live_action_without_a_catalog_entry() {
    let db = Db::open(":memory:").unwrap();
    let a = db.upsert_source("AnimeYT", "https://a", "animeytx").unwrap();
    db.upsert_series(a, &mk_airing("op-live", "One Piece: Live Action (2023)", None)).unwrap();
    let anime = db.upsert_series(a, &mk_airing("op", "Some Anime", None)).unwrap();
    let followed_live = db.upsert_series(a, &mk_airing("fl", "Another: Live Action", None)).unwrap();
    db.set_followed(followed_live, true).unwrap();

    let titles: Vec<String> = db.list_airing(a).unwrap().into_iter().map(|s| s.title).collect();
    assert!(titles.contains(&"Some Anime".to_string()));
    assert!(titles.contains(&"Another: Live Action".to_string()), "a followed one stays");
    assert!(!titles.iter().any(|t| t.contains("One Piece: Live Action")));
    let _ = anime;
}

fn set_catalog_format(db: &Db, id: i64, format: &str) {
    db.conn.execute("UPDATE anilist_catalog SET format = ?1 WHERE id = ?2", (format, id)).unwrap();
}

fn airing_titles(db: &Db, source: i64) -> Vec<String> {
    db.list_airing(source).unwrap().into_iter().map(|s| s.title).collect()
}

#[test]
fn list_airing_hides_unfollowed_ova_without_an_upcoming_episode() {
    let db = Db::open(":memory:").unwrap();
    let a = db.upsert_source("AnimeYT", "https://a", "animeytx").unwrap();
    db.upsert_catalog_anime(&crate::db::test_support::catalog_anime(1, "Noble Rose", &[]), 0).unwrap();
    set_catalog_format(&db, 1, "OVA");
    let sid = db.upsert_series(a, &mk_airing("nr", "Noble Rose", None)).unwrap();
    db.set_anilist_id(sid, 1).unwrap();
    assert!(airing_titles(&db, a).is_empty(), "OVA with no future catalog airing is hidden");

    db.set_followed(sid, true).unwrap();
    assert_eq!(airing_titles(&db, a), vec!["Noble Rose".to_string()], "followed stays");
}

#[test]
fn list_airing_keeps_an_ova_that_has_a_future_catalog_airing() {
    let db = Db::open(":memory:").unwrap();
    let a = db.upsert_source("AnimeYT", "https://a", "animeytx").unwrap();
    db.upsert_catalog_anime(&crate::db::test_support::catalog_anime(1, "Noble Rose", &[]), 0).unwrap();
    set_catalog_format(&db, 1, "OVA");
    let now = chrono::Utc::now().timestamp();
    db.conn.execute("UPDATE anilist_catalog SET next_airing_at = ?1 WHERE id = 1", [now + 86400]).unwrap();
    let sid = db.upsert_series(a, &mk_airing("nr", "Noble Rose", None)).unwrap();
    db.set_anilist_id(sid, 1).unwrap();
    assert_eq!(airing_titles(&db, a).len(), 1);
}

#[test]
fn list_airing_hides_unlinked_specials_and_stale_unfollowed_rows() {
    let db = Db::open(":memory:").unwrap();
    let a = db.upsert_source("AnimeYT", "https://a", "animeytx").unwrap();
    let now = chrono::Utc::now().timestamp();
    let day = 86400;
    db.upsert_series(a, &mk_airing("sp", "Slime 3rd Season Specials", Some(now - day))).unwrap();
    db.upsert_series(a, &mk_airing("stale", "Stale Show", Some(now - 30 * day))).unwrap();
    db.upsert_series(a, &mk_airing("fresh", "Fresh Show", Some(now - 6 * day))).unwrap();
    db.upsert_series(a, &mk_airing("nodate", "Undated Show", None)).unwrap();
    let followed = db.upsert_series(a, &mk_airing("fstale", "Followed Stale", Some(now - 60 * day))).unwrap();
    db.set_followed(followed, true).unwrap();

    let mut titles = airing_titles(&db, a);
    titles.sort();
    assert_eq!(titles, vec!["Followed Stale", "Fresh Show", "Undated Show"]);
}

#[test]
fn list_airing_keeps_a_finished_entry_when_the_site_lists_episodes_past_its_total() {
    // Split cour: the site keeps one page for both cours and it got linked to
    // cour 1 (12 eps, FINISHED). Episode 14 on the site proves it is still airing.
    let db = Db::open(":memory:").unwrap();
    let a = db.upsert_source("AnimeYT", "https://a", "animeytx").unwrap();
    let now = chrono::Utc::now().timestamp();
    let mut anime = crate::db::test_support::catalog_anime(5, "Split Show", &[]);
    anime.status = Some("FINISHED".into());
    anime.episodes = Some(12);
    db.upsert_catalog_anime(&anime, 0).unwrap();
    let mut row = mk_airing("split", "Split Show", Some(now + 3 * 86400));
    row.site_episode_count = Some(14);
    let sid = db.upsert_series(a, &row).unwrap();
    db.set_anilist_id(sid, 5).unwrap();
    assert_eq!(airing_titles(&db, a).len(), 1, "episodes past the AniList total keep it");
}

#[test]
fn list_airing_hides_unfollowed_row_the_catalog_says_finished() {
    let db = Db::open(":memory:").unwrap();
    let a = db.upsert_source("AnimeYT", "https://a", "animeytx").unwrap();
    let now = chrono::Utc::now().timestamp();
    let mut anime = crate::db::test_support::catalog_anime(4, "Done Show", &[]);
    anime.status = Some("FINISHED".into());
    db.upsert_catalog_anime(&anime, 0).unwrap();
    // The site lists it with a future date, so the staleness rule alone keeps it.
    let sid = db.upsert_series(a, &mk_airing("done", "Done Show", Some(now + 3 * 86400))).unwrap();
    db.set_anilist_id(sid, 4).unwrap();
    assert!(airing_titles(&db, a).is_empty());
    db.set_followed(sid, true).unwrap();
    assert_eq!(airing_titles(&db, a).len(), 1, "followed stays");
}

#[test]
fn list_airing_keeps_a_stale_scraped_row_when_the_catalog_says_releasing() {
    let db = Db::open(":memory:").unwrap();
    let a = db.upsert_source("AnimeYT", "https://a", "animeytx").unwrap();
    let now = chrono::Utc::now().timestamp();
    let mut anime = crate::db::test_support::catalog_anime(3, "Biweekly Show", &[]);
    anime.status = Some("RELEASING".into());
    db.upsert_catalog_anime(&anime, 0).unwrap();
    let sid = db.upsert_series(a, &mk_airing("bw", "Biweekly Show", Some(now - 40 * 86400))).unwrap();
    db.set_anilist_id(sid, 3).unwrap();
    assert_eq!(airing_titles(&db, a).len(), 1);
}

#[test]
fn list_airing_keeps_a_stale_scraped_row_when_the_catalog_has_a_future_airing() {
    let db = Db::open(":memory:").unwrap();
    let a = db.upsert_source("AnimeYT", "https://a", "animeytx").unwrap();
    let now = chrono::Utc::now().timestamp();
    db.upsert_catalog_anime(&crate::db::test_support::catalog_anime(2, "Slow Show", &[]), 0).unwrap();
    db.conn.execute("UPDATE anilist_catalog SET next_airing_at = ?1 WHERE id = 2", [now + 3 * 86400]).unwrap();
    let sid = db.upsert_series(a, &mk_airing("slow", "Slow Show", Some(now - 40 * 86400))).unwrap();
    db.set_anilist_id(sid, 2).unwrap();
    assert_eq!(airing_titles(&db, a).len(), 1);
}

#[test]
fn list_airing_prefers_row_with_episodes_over_active_site_with_none() {
    let db = Db::open(":memory:").unwrap();
    let a = db.upsert_source("AnimeYT", "https://a", "animeytx").unwrap();
    let b = db.upsert_source("TioAnime", "https://b", "tioanime").unwrap();

    // Active site row has no episodes
    let _id_a = db.upsert_series(a, &mk_airing("op-a", "One Piece", None)).unwrap();
    // Non-active site row has an episode
    let id_b = db.upsert_series(b, &mk_airing("op-b", "One Piece", None)).unwrap();
    db.insert_episode(&crate::models::Episode {
        id: 0, series_id: id_b, number: "1".into(), title: None, url: "e1".into(), released_at: None, seen: false,
        download_path: None, download_status: None,
    }).unwrap();

    let airing = db.list_airing(a).unwrap();
    assert_eq!(airing.len(), 1);
    assert_eq!(airing[0].url, "https://site/tv/op-b/", "representative should be the one with episodes");
}

#[test]
fn active_row_with_all_seen_stays_authoritative_over_other_site_with_unseen() {
    let db = Db::open(":memory:").unwrap();
    let src_a = db.upsert_source("AnimeYT", "https://a.example", "animeytx").unwrap(); // active
    let src_b = db.upsert_source("TioAnime", "https://b.example", "tioanime").unwrap();

    let mk = |slug: &str, title: &str| crate::models::Series {
        id: 0, slug: slug.into(), title: title.into(), url: format!("u-{slug}"),
        cover_url: None, is_airing: true, followed: false, next_episode_at: None, site_episode_count: None,
    };

    let row_a = db.upsert_series(src_a, &mk("act", "Show")).unwrap();
    db.set_followed(row_a, true).unwrap();
    db.insert_episode(&crate::models::Episode {
        id: 0, series_id: row_a, number: "1".into(), title: None,
        url: "u/1".into(), released_at: None, seen: true, // SEEN!
        download_path: None, download_status: None,
    }).unwrap();

    let row_b = db.upsert_series(src_b, &mk("oth", "Show")).unwrap();
    db.set_followed(row_b, true).unwrap();
    db.insert_episode(&crate::models::Episode {
        id: 0, series_id: row_b, number: "1".into(), title: None,
        url: "u/1b".into(), released_at: None, seen: false, // UNSEEN!
        download_path: None, download_status: None,
    }).unwrap();

    // Active site has episodes, so it stays authoritative. Since its eps are seen, pending_count is 0.
    let priority = vec!["animeytx".to_string(), "tioanime".to_string()];
    assert_eq!(db.pending_count(src_a, &priority).unwrap(), 0);
}

#[test]
fn watched_externally_is_excluded_from_pending() {
    let db = Db::open(":memory:").unwrap();
    let src_a = db.upsert_source("AnimeYT", "https://a.example", "animeytx").unwrap();

    let row = db.upsert_series(src_a, &mk_airing("act", "Show", None)).unwrap();
    db.set_followed(row, true).unwrap();
    db.conn.execute("UPDATE series SET watched_externally = 1 WHERE id = ?", [row]).unwrap();

    db.insert_episode(&crate::models::Episode {
        id: 0, series_id: row, number: "1".into(), title: None,
        url: "u/1".into(), released_at: None, seen: false,
        download_path: None, download_status: None,
    }).unwrap();

    let priority = vec!["animeytx".to_string()];
    assert_eq!(db.pending_count(src_a, &priority).unwrap(), 0, "watched_externally should yield 0 pending");
}

#[test]
fn group_where_only_non_winner_row_is_followed() {
    let db = Db::open(":memory:").unwrap();
    let src_a = db.upsert_source("AnimeYT", "https://a.example", "animeytx").unwrap();
    let src_b = db.upsert_source("TioAnime", "https://b.example", "tioanime").unwrap();

    let mk = |slug: &str, title: &str| crate::models::Series {
        id: 0, slug: slug.into(), title: title.into(), url: format!("u-{slug}"),
        cover_url: None, is_airing: true, followed: false, next_episode_at: None, site_episode_count: None,
    };

    // B has episodes but is NOT followed.
    let row_b = db.upsert_series(src_b, &mk("oth", "Show")).unwrap();
    db.insert_episode(&crate::models::Episode {
        id: 0, series_id: row_b, number: "1".into(), title: None,
        url: "u/1b".into(), released_at: None, seen: false,
        download_path: None, download_status: None,
    }).unwrap();

    // A has NO episodes but IS followed.
    let row_a = db.upsert_series(src_a, &mk("act", "Show")).unwrap();
    db.set_followed(row_a, true).unwrap(); // Marks canonical group followed!

    let priority = vec!["animeytx".to_string(), "tioanime".to_string()];
    // Since the canonical group is followed, and A has no episodes, it falls back to B.
    assert_eq!(db.pending_count(src_a, &priority).unwrap(), 1);
    let pending = db.list_pending(src_a, PendingSort::RemainingAsc, &priority).unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].0.id, row_b);
}

#[test]
fn unlinked_catalog_rows_never_contribute_pending_episodes() {
    // A row still on its synthetic `anilist-<id>` slug has no page on any
    // site, so whatever episodes it holds were scraped from a wrong page
    // (animeyt.cc answers /anime/<anilist id> with a feed of recent
    // episodes of other shows). They must not reach Pendientes, while the
    // real row of the same show still does.
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("AnimeYT", "https://a.example", "animeytx").unwrap();
    let mk_ep = |sid: i64, n: &str| crate::models::Episode {
        id: 0, series_id: sid, number: n.into(), title: None,
        url: format!("u/{sid}/{n}"), released_at: None, seen: false, download_path: None, download_status: None,
    };

    let synthetic = db.upsert_series(src, &mk_airing("anilist-77", "Some Show", None)).unwrap();
    db.set_anilist_id(synthetic, 77).unwrap();
    db.set_followed(synthetic, true).unwrap();
    db.insert_episode(&mk_ep(synthetic, "7")).unwrap();
    db.insert_episode(&mk_ep(synthetic, "8")).unwrap();
    assert_eq!(db.pending_count(src, &[]).unwrap(), 0);

    let real = db.upsert_series(src, &mk_airing("some-show", "Some Show", None)).unwrap();
    db.set_anilist_id(real, 77).unwrap();
    db.insert_episode(&mk_ep(real, "1")).unwrap();
    assert_eq!(db.pending_count(src, &[]).unwrap(), 1, "only the real row's episode");
}

#[test]
fn pending_count_equals_list_pending_length() {
    let db = Db::open(":memory:").unwrap();
    let src_a = db.upsert_source("AnimeYT", "https://a.example", "animeytx").unwrap();

    let row = db.upsert_series(src_a, &mk_airing("act", "Show", None)).unwrap();
    db.set_followed(row, true).unwrap();
    db.insert_episode(&crate::models::Episode {
        id: 0, series_id: row, number: "1".into(), title: None,
        url: "u/1".into(), released_at: None, seen: false,
        download_path: None, download_status: None,
    }).unwrap();
    db.insert_episode(&crate::models::Episode {
        id: 0, series_id: row, number: "2".into(), title: None,
        url: "u/2".into(), released_at: None, seen: false,
        download_path: None, download_status: None,
    }).unwrap();

    let priority = vec!["animeytx".to_string()];
    let count = db.pending_count(src_a, &priority).unwrap();
    let pending = db.list_pending(src_a, PendingSort::RemainingAsc, &priority).unwrap();
    assert_eq!(count as usize, pending.len());
}

/// Guards the same regression the old wall-clock test did, without a clock:
/// `merged_pending_episodes_at` runs one lookup per candidate series / group
/// (episodes by series_id, episode map by anilist_id, series by id). With a
/// real library (6k series, 120k episodes) each of those must be an index
/// SEARCH; a full SCAN per row makes the whole thing quadratic (the freeze
/// that `stale_status_ids` hit). The SQL strings mirror the ones in
/// `merged_pending_episodes_at` and `row_align`.
#[test]
fn pending_per_row_lookups_use_an_index_not_a_scan() {
    let db = Db::open(":memory:").unwrap();
    let lookups = [
        "SELECT id, series_id, number, seen FROM episodes WHERE series_id IN (1,2,3)",
        "SELECT number FROM episodes WHERE series_id = ?1",
        "SELECT COUNT(*) FROM anilist_episode_map WHERE anilist_id = ?1 AND air_at IS NOT NULL AND air_at <= ?2",
        "SELECT anilist_id FROM series WHERE id = ?1",
    ];
    for sql in lookups {
        let mut stmt = db.conn.prepare(&format!("EXPLAIN QUERY PLAN {sql}")).unwrap();
        let params: Vec<i64> = vec![0; stmt.parameter_count()];
        let plan: Vec<String> = stmt
            .query_map(rusqlite::params_from_iter(params), |r| r.get::<_, String>(3))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert!(!plan.is_empty(), "no plan for {sql}");
        assert!(
            plan.iter().all(|d| d.trim_start().starts_with("SEARCH")),
            "per-row lookup must be an index SEARCH, plan was {plan:?} for {sql}"
        );
    }
}

/// Scaled correctness of the pending pipeline over a multi-site library
/// (no timing): the count and the list agree and match the fixture.
#[test]
fn pending_count_matches_list_pending_on_a_scaled_library() {
    let db = Db::open(":memory:").unwrap();
    let src_a = db.upsert_source("AnimeYT", "https://a.example", "animeytx").unwrap();
    let src_b = db.upsert_source("TioAnime", "https://b.example", "tioanime").unwrap();

    let tx = db.conn.unchecked_transaction().unwrap();
    let mut followed_n = 0usize;
    for i in 0..300 {
        let followed = i % 3 == 0;
        if followed {
            followed_n += 1;
        }
        let src = if i % 2 == 0 { src_a } else { src_b };
        tx.execute(
            "INSERT INTO series(source_id, slug, title, url, is_airing, followed, watched_externally)
             VALUES(?1, ?2, ?3, ?4, 0, ?5, 0)",
            (src, format!("s-{i}"), format!("Show {i}"), format!("u/{i}"), followed as i64),
        ).unwrap();
        let sid = tx.last_insert_rowid();
        for j in 1..=20 {
            tx.execute(
                "INSERT INTO episodes(series_id, number, url, seen) VALUES(?1, ?2, ?3, 0)",
                (sid, j.to_string(), format!("u/{i}/{j}")),
            ).unwrap();
        }
    }
    tx.commit().unwrap();

    let priority = vec!["animeytx".to_string(), "tioanime".to_string()];
    let count = db.pending_count(src_a, &priority).unwrap();
    let list = db.list_pending(src_a, PendingSort::RemainingAsc, &priority).unwrap();
    assert_eq!(count as usize, list.len());
    assert_eq!(list.len(), followed_n * 20);
}

#[test]
fn list_pending_prefers_catalog_title_and_cover_for_linked_series() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("AnimeYT", "https://a.example", "animeytx").unwrap();

    db.upsert_catalog_anime(&catalog_anime(1001, "As a Reincarnated Aristocrat", &[]), 0).unwrap();

    let s = crate::models::Series {
        id: 0,
        slug: "tensei-kizoku".into(),
        title: "Tensei Kizoku".into(),
        url: "https://site/tensei-kizoku".into(),
        cover_url: Some("file:C:/covers/tensei.jpg".into()),
        is_airing: true,
        followed: true,
        next_episode_at: None,
        site_episode_count: None,
    };
    let sid = db.upsert_series(src, &s).unwrap();
    db.set_followed(sid, true).unwrap();
    db.set_anilist_id(sid, 1001).unwrap();
    db.insert_episode(&crate::models::Episode {
        id: 0,
        series_id: sid,
        number: "1".into(),
        title: None,
        url: "https://site/tensei-kizoku/1".into(),
        released_at: None,
        seen: false,
        download_path: None,
        download_status: None,
    }).unwrap();

    let priority = vec!["animeytx".to_string()];
    let pending = db.list_pending(src, PendingSort::RemainingAsc, &priority).unwrap();
    assert_eq!(pending.len(), 1);
    let (series, _) = &pending[0];
    assert_eq!(series.title, "As a Reincarnated Aristocrat");
    assert_eq!(series.cover_url.as_deref(), Some("https://cdn/1001.jpg"));
}

#[test]
fn list_pending_keeps_site_title_and_cover_for_unlinked_series() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("AnimeYT", "https://a.example", "animeytx").unwrap();

    let s = crate::models::Series {
        id: 0,
        slug: "unlinked-show".into(),
        title: "Unlinked Site Title".into(),
        url: "https://site/unlinked".into(),
        cover_url: Some("file:C:/covers/unlinked.jpg".into()),
        is_airing: true,
        followed: true,
        next_episode_at: None,
        site_episode_count: None,
    };
    let sid = db.upsert_series(src, &s).unwrap();
    db.set_followed(sid, true).unwrap();
    db.insert_episode(&crate::models::Episode {
        id: 0,
        series_id: sid,
        number: "1".into(),
        title: None,
        url: "https://site/unlinked/1".into(),
        released_at: None,
        seen: false,
        download_path: None,
        download_status: None,
    }).unwrap();

    let priority = vec!["animeytx".to_string()];
    let pending = db.list_pending(src, PendingSort::RemainingAsc, &priority).unwrap();
    assert_eq!(pending.len(), 1);
    let (series, _) = &pending[0];
    assert_eq!(series.title, "Unlinked Site Title");
    assert_eq!(series.cover_url.as_deref(), Some("file:C:/covers/unlinked.jpg"));
}

#[test]
fn list_pending_linked_series_with_null_catalog_cover_falls_back_to_site_cover() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("AnimeYT", "https://a.example", "animeytx").unwrap();

    let mut cat = catalog_anime(1002, "Catalog Title", &[]);
    cat.cover_url = None;
    db.upsert_catalog_anime(&cat, 0).unwrap();

    let s = crate::models::Series {
        id: 0,
        slug: "null-cat-cover".into(),
        title: "Site Title".into(),
        url: "https://site/null-cat-cover".into(),
        cover_url: Some("file:C:/covers/site.jpg".into()),
        is_airing: true,
        followed: true,
        next_episode_at: None,
        site_episode_count: None,
    };
    let sid = db.upsert_series(src, &s).unwrap();
    db.set_followed(sid, true).unwrap();
    db.set_anilist_id(sid, 1002).unwrap();
    db.insert_episode(&crate::models::Episode {
        id: 0,
        series_id: sid,
        number: "1".into(),
        title: None,
        url: "https://site/null-cat-cover/1".into(),
        released_at: None,
        seen: false,
        download_path: None,
        download_status: None,
    }).unwrap();

    let priority = vec!["animeytx".to_string()];
    let pending = db.list_pending(src, PendingSort::RemainingAsc, &priority).unwrap();
    assert_eq!(pending.len(), 1);
    let (series, _) = &pending[0];
    assert_eq!(series.title, "Catalog Title");
    assert_eq!(series.cover_url.as_deref(), Some("file:C:/covers/site.jpg"));
}

#[test]
fn merged_pending_priority_and_link_selection() {
    let db = Db::open(":memory:").unwrap();
    let src_a = db.upsert_source("AnimeYT", "https://a.example", "animeytx").unwrap();
    let src_b = db.upsert_source("TioAnime", "https://b.example", "tioanime").unwrap();

    let mk = |slug: &str, title: &str| crate::models::Series {
        id: 0, slug: slug.into(), title: title.into(), url: format!("https://site/{slug}"),
        cover_url: None, is_airing: true, followed: true, next_episode_at: None, site_episode_count: None,
    };

    let row_a = db.upsert_series(src_a, &mk("act", "Show")).unwrap();
    db.set_followed(row_a, true).unwrap();
    for i in 1..=3 {
        db.insert_episode(&crate::models::Episode {
            id: 0, series_id: row_a, number: i.to_string(), title: None,
            url: format!("https://a/{i}"), released_at: None, seen: false,
            download_path: None, download_status: None,
        }).unwrap();
    }

    let row_b = db.upsert_series(src_b, &mk("oth", "Show")).unwrap();
    db.set_followed(row_b, true).unwrap();
    for i in 1..=4 {
        db.insert_episode(&crate::models::Episode {
            id: 0, series_id: row_b, number: i.to_string(), title: None,
            url: format!("https://b/{i}"), released_at: None, seen: false,
            download_path: None, download_status: None,
        }).unwrap();
    }

    // Priority [A, B]: pending = A1, A2, A3, B4 under representative A
    let p_ab = vec!["animeytx".to_string(), "tioanime".to_string()];
    let pending_ab = db.list_pending(src_a, PendingSort::RemainingAsc, &p_ab).unwrap();
    assert_eq!(pending_ab.len(), 4);
    assert_eq!(pending_ab[0].0.id, row_a); // representative is A
    assert_eq!(pending_ab[1].0.id, row_a);
    assert_eq!(pending_ab[2].0.id, row_a);
    assert_eq!(pending_ab[3].0.id, row_a);

    assert_eq!(pending_ab[0].1.url, "https://a/1");
    assert_eq!(pending_ab[1].1.url, "https://a/2");
    assert_eq!(pending_ab[2].1.url, "https://a/3");
    assert_eq!(pending_ab[3].1.url, "https://b/4");

    // Priority [B, A]: pending = B1, B2, B3, B4 under representative B
    let p_ba = vec!["tioanime".to_string(), "animeytx".to_string()];
    let pending_ba = db.list_pending(src_a, PendingSort::RemainingAsc, &p_ba).unwrap();
    assert_eq!(pending_ba.len(), 4);
    assert_eq!(pending_ba[0].0.id, row_b); // representative is B
    assert_eq!(pending_ba[0].1.url, "https://b/1");
    assert_eq!(pending_ba[1].1.url, "https://b/2");
    assert_eq!(pending_ba[2].1.url, "https://b/3");
    assert_eq!(pending_ba[3].1.url, "https://b/4");
}

#[test]
fn merged_pending_with_seen_watermark() {
    let db = Db::open(":memory:").unwrap();
    let src_a = db.upsert_source("AnimeYT", "https://a.example", "animeytx").unwrap();
    let src_b = db.upsert_source("TioAnime", "https://b.example", "tioanime").unwrap();

    let mk = |slug: &str, title: &str| crate::models::Series {
        id: 0, slug: slug.into(), title: title.into(), url: format!("https://site/{slug}"),
        cover_url: None, is_airing: true, followed: true, next_episode_at: None, site_episode_count: None,
    };

    let row_a = db.upsert_series(src_a, &mk("act", "Show")).unwrap();
    db.set_followed(row_a, true).unwrap();
    for i in 1..=3 {
        db.insert_episode(&crate::models::Episode {
            id: 0, series_id: row_a, number: i.to_string(), title: None,
            url: format!("https://a/{i}"), released_at: None, seen: i <= 2, // 1 and 2 seen!
            download_path: None, download_status: None,
        }).unwrap();
    }

    let row_b = db.upsert_series(src_b, &mk("oth", "Show")).unwrap();
    db.set_followed(row_b, true).unwrap();
    for i in 1..=4 {
        db.insert_episode(&crate::models::Episode {
            id: 0, series_id: row_b, number: i.to_string(), title: None,
            url: format!("https://b/{i}"), released_at: None, seen: false, // unseen on B!
            download_path: None, download_status: None,
        }).unwrap();
    }

    // Priority [A, B]: user has seen 1-2 (on A).
    // Pending must be 3 (from A) and 4 (from B).
    // Neither 1 nor 2 should reappear even though unseen on B!
    let priority = vec!["animeytx".to_string(), "tioanime".to_string()];
    let pending = db.list_pending(src_a, PendingSort::RemainingAsc, &priority).unwrap();
    assert_eq!(pending.len(), 2);
    assert_eq!(pending[0].1.number, "3");
    assert_eq!(pending[0].1.url, "https://a/3");
    assert_eq!(pending[1].1.number, "4");
    assert_eq!(pending[1].1.url, "https://b/4");
}

#[test]
fn merged_pending_only_other_site_has_episodes() {
    let db = Db::open(":memory:").unwrap();
    let src_a = db.upsert_source("AnimeYT", "https://a.example", "animeytx").unwrap();
    let src_b = db.upsert_source("TioAnime", "https://b.example", "tioanime").unwrap();

    let mk = |slug: &str, title: &str| crate::models::Series {
        id: 0, slug: slug.into(), title: title.into(), url: format!("https://site/{slug}"),
        cover_url: None, is_airing: true, followed: true, next_episode_at: None, site_episode_count: None,
    };

    let row_a = db.upsert_series(src_a, &mk("act", "Show")).unwrap();
    db.set_followed(row_a, true).unwrap();

    let row_b = db.upsert_series(src_b, &mk("oth", "Show")).unwrap();
    db.set_followed(row_b, true).unwrap();
    db.insert_episode(&crate::models::Episode {
        id: 0, series_id: row_b, number: "1".into(), title: None,
        url: "https://b/1".into(), released_at: None, seen: false,
        download_path: None, download_status: None,
    }).unwrap();

    let priority = vec!["animeytx".to_string(), "tioanime".to_string()];
    let pending = db.list_pending(src_a, PendingSort::RemainingAsc, &priority).unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].0.id, row_b);
    assert_eq!(pending[0].1.url, "https://b/1");
}

#[test]
fn missing_site_in_priority_ranks_last() {
    let db = Db::open(":memory:").unwrap();
    let src_a = db.upsert_source("AnimeYT", "https://a.example", "animeytx").unwrap();
    let src_b = db.upsert_source("Other", "https://b.example", "unknown_site").unwrap();

    let mk = |slug: &str, title: &str| crate::models::Series {
        id: 0, slug: slug.into(), title: title.into(), url: format!("https://site/{slug}"),
        cover_url: None, is_airing: true, followed: true, next_episode_at: None, site_episode_count: None,
    };

    let row_a = db.upsert_series(src_a, &mk("act", "Show")).unwrap();
    db.set_followed(row_a, true).unwrap();
    db.insert_episode(&crate::models::Episode {
        id: 0, series_id: row_a, number: "1".into(), title: None,
        url: "https://a/1".into(), released_at: None, seen: false,
        download_path: None, download_status: None,
    }).unwrap();

    let row_b = db.upsert_series(src_b, &mk("oth", "Show")).unwrap();
    db.set_followed(row_b, true).unwrap();
    db.insert_episode(&crate::models::Episode {
        id: 0, series_id: row_b, number: "1".into(), title: None,
        url: "https://b/1".into(), released_at: None, seen: false,
        download_path: None, download_status: None,
    }).unwrap();

    // Priority only contains animeytx. unknown_site is missing, so ranks last.
    let priority = vec!["animeytx".to_string()];
    let pending = db.list_pending(src_a, PendingSort::RemainingAsc, &priority).unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].1.url, "https://a/1");
}

#[test]
fn no_duplicate_episode_numbers() {
    let db = Db::open(":memory:").unwrap();
    let src_a = db.upsert_source("AnimeYT", "https://a.example", "animeytx").unwrap();
    let src_b = db.upsert_source("TioAnime", "https://b.example", "tioanime").unwrap();

    let mk = |slug: &str, title: &str| crate::models::Series {
        id: 0, slug: slug.into(), title: title.into(), url: format!("https://site/{slug}"),
        cover_url: None, is_airing: true, followed: true, next_episode_at: None, site_episode_count: None,
    };

    let row_a = db.upsert_series(src_a, &mk("act", "Show")).unwrap();
    db.set_followed(row_a, true).unwrap();
    db.insert_episode(&crate::models::Episode {
        id: 0, series_id: row_a, number: "1".into(), title: None,
        url: "https://a/1".into(), released_at: None, seen: false,
        download_path: None, download_status: None,
    }).unwrap();

    let row_b = db.upsert_series(src_b, &mk("oth", "Show")).unwrap();
    db.set_followed(row_b, true).unwrap();
    db.insert_episode(&crate::models::Episode {
        id: 0, series_id: row_b, number: "1".into(), title: None,
        url: "https://b/1".into(), released_at: None, seen: false,
        download_path: None, download_status: None,
    }).unwrap();

    let priority = vec!["animeytx".to_string(), "tioanime".to_string()];
    let pending = db.list_pending(src_a, PendingSort::RemainingAsc, &priority).unwrap();
    assert_eq!(pending.len(), 1, "Must never have the same episode number twice");
}

#[test]
fn unfollowed_and_watched_externally_shows_never_appear() {
    let db = Db::open(":memory:").unwrap();
    let src_a = db.upsert_source("AnimeYT", "https://a.example", "animeytx").unwrap();

    let mk = |slug: &str, title: &str| crate::models::Series {
        id: 0, slug: slug.into(), title: title.into(), url: format!("https://site/{slug}"),
        cover_url: None, is_airing: true, followed: false, next_episode_at: None, site_episode_count: None,
    };

    // Unfollowed
    let s_unfollowed = db.upsert_series(src_a, &mk("unf", "Unfollowed")).unwrap();
    db.insert_episode(&crate::models::Episode {
        id: 0, series_id: s_unfollowed, number: "1".into(), title: None,
        url: "https://a/unf/1".into(), released_at: None, seen: false,
        download_path: None, download_status: None,
    }).unwrap();

    // Watched externally
    let s_ext = db.upsert_series(src_a, &mk("ext", "Watched Externally")).unwrap();
    db.set_followed(s_ext, true).unwrap();
    db.conn.execute("UPDATE series SET watched_externally=1 WHERE id=?1", [s_ext]).unwrap();
    db.insert_episode(&crate::models::Episode {
        id: 0, series_id: s_ext, number: "1".into(), title: None,
        url: "https://a/ext/1".into(), released_at: None, seen: false,
        download_path: None, download_status: None,
    }).unwrap();

    let priority = vec!["animeytx".to_string()];
    assert_eq!(db.pending_count(src_a, &priority).unwrap(), 0);
    let pending = db.list_pending(src_a, PendingSort::RemainingAsc, &priority).unwrap();
    assert!(pending.is_empty());
}

#[test]
fn merged_pending_canonical_alignment_tensei_fixture() {
    let db = Db::open(":memory:").unwrap();
    let now = 1_700_000_000;
    let src_a = db.upsert_source("AnimeYT", "https://animeyt.cc", "animeytx").unwrap();
    let src_b = db.upsert_source("TioAnime", "https://tioanime.com", "tioanime").unwrap();

    let entries = vec![
        crate::anizip::MapEntry { episode: 1, season: 3, absolute: Some(25), air_at: Some(now - 14 * 86400) },
        crate::anizip::MapEntry { episode: 2, season: 3, absolute: Some(26), air_at: Some(now - 7 * 86400) },
        crate::anizip::MapEntry { episode: 3, season: 3, absolute: Some(27), air_at: Some(now) },
        crate::anizip::MapEntry { episode: 4, season: 3, absolute: Some(28), air_at: Some(now + 7 * 86400) },
        crate::anizip::MapEntry { episode: 5, season: 3, absolute: Some(29), air_at: Some(now + 14 * 86400) },
        crate::anizip::MapEntry { episode: 6, season: 3, absolute: Some(30), air_at: Some(now + 21 * 86400) },
        crate::anizip::MapEntry { episode: 7, season: 3, absolute: Some(31), air_at: Some(now + 28 * 86400) },
        crate::anizip::MapEntry { episode: 8, season: 3, absolute: Some(32), air_at: Some(now + 35 * 86400) },
    ];
    db.replace_episode_map(185756, &entries, now).unwrap();

    let mk = |slug: &str, title: &str| crate::models::Series {
        id: 0, slug: slug.into(), title: title.into(), url: format!("https://site/{slug}"),
        cover_url: None, is_airing: true, followed: true, next_episode_at: None, site_episode_count: None,
    };

    let row_a = db.upsert_series(src_a, &mk("tensei-a", "Tensei")).unwrap();
    db.set_anilist_id(row_a, 185756).unwrap();
    db.set_followed(row_a, true).unwrap();
    for i in 1..=3 {
        db.insert_episode(&crate::models::Episode {
            id: 0, series_id: row_a, number: i.to_string(), title: None,
            url: format!("https://animeyt.cc/tv/tensei/{i}"), released_at: None, seen: false,
            download_path: None, download_status: None,
        }).unwrap();
    }

    let row_b = db.upsert_series(src_b, &mk("tensei-b", "Tensei")).unwrap();
    db.set_anilist_id(row_b, 185756).unwrap();
    db.set_followed(row_b, true).unwrap();
    for i in 1..=27 {
        db.insert_episode(&crate::models::Episode {
            id: 0, series_id: row_b, number: i.to_string(), title: None,
            url: format!("https://tioanime.com/ver/tensei-{i}"), released_at: None, seen: false,
            download_path: None, download_status: None,
        }).unwrap();
    }

    let priority = vec!["animeytx".to_string(), "tioanime".to_string()];
    let pending = db.merged_pending_episodes_at(&priority, now).unwrap();
    assert_eq!(pending.len(), 1);
    let (rep_id, ep_ids) = &pending[0];
    assert_eq!(*rep_id, row_a);
    assert_eq!(ep_ids.len(), 3, "exactly 3 pending episodes (canonical 1..3), all from animeyt");
    for ep_id in ep_ids {
        let ep = db.get_episode(*ep_id).unwrap();
        assert_eq!(ep.series_id, row_a);
    }

    // Now if TioAnime also has "28" (canonical 4, aired later), it appears as the fourth, from TioAnime
    db.insert_episode(&crate::models::Episode {
        id: 0, series_id: row_b, number: "28".into(), title: None,
        url: "https://tioanime.com/ver/tensei-28".into(), released_at: None, seen: false,
        download_path: None, download_status: None,
    }).unwrap();

    let pending2 = db.merged_pending_episodes_at(&priority, now).unwrap();
    assert_eq!(pending2.len(), 1);
    let (rep_id2, ep_ids2) = &pending2[0];
    assert_eq!(*rep_id2, row_a);
    assert_eq!(ep_ids2.len(), 4, "four pending episodes now");
    let ep4 = db.get_episode(ep_ids2[3]).unwrap();
    assert_eq!(ep4.series_id, row_b);
    assert_eq!(ep4.number, "28");
    assert_eq!(ep4.url, "https://tioanime.com/ver/tensei-28");
}

#[test]
fn linked_and_unlinked_same_title_rows_collapse_into_one_airing_card() {
    let db = Db::open(":memory:").unwrap();
    let src_a = db.upsert_source("AnimeYT", "https://animeyt.cc", "animeytx").unwrap();
    let src_b = db.upsert_source("TioAnime", "https://tioanime.com", "tioanime").unwrap();

    let mk = |slug: &str, title: &str| crate::models::Series {
        id: 0,
        slug: slug.into(),
        title: title.into(),
        url: format!("https://site/{slug}"),
        cover_url: None,
        is_airing: true,
        followed: true,
        next_episode_at: None,
        site_episode_count: None,
    };

    // row_a is linked to AniList 9999
    let row_a = db.upsert_series(src_a, &mk("frieren-a", "Sousou no Frieren")).unwrap();
    db.set_anilist_id(row_a, 9999).unwrap();

    // row_b is unlinked (None) but shares the exact same title
    let _row_b = db.upsert_series(src_b, &mk("frieren-b", "Sousou no Frieren")).unwrap();

    let airing = db.list_airing(src_a).unwrap();
    assert_eq!(airing.len(), 1, "linked and unlinked same-title rows collapse into one airing card");
    assert_eq!(airing[0].title, "Sousou no Frieren");
}

#[test]
fn list_airing_rollover_when_no_scraped_upcoming_date() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("AnimeYT", "https://a.example", "animeytx").unwrap();
    let now = chrono::Utc::now().timestamp();
    let day = 86400;

    // 1. RELEASING, next_airing_at = now - 2 days, no scraped date
    // -> effective_next == next_airing_at + 7 days
    let mut a1 = crate::db::test_support::catalog_anime(101, "Rolled Show", &[]);
    a1.status = Some("RELEASING".into());
    a1.next_airing_at = Some(now - 2 * day);
    a1.next_episode = Some(5);
    db.upsert_catalog_anime(&a1, 0).unwrap();
    let s1 = db.upsert_series(src, &mk_airing("rolled", "Rolled Show", None)).unwrap();
    db.set_anilist_id(s1, 101).unwrap();

    let airing1 = db.list_airing(src).unwrap();
    assert_eq!(airing1.len(), 1);
    assert_eq!(airing1[0].next_episode_at, Some(now - 2 * day + 7 * day));

    // 2. now - 9 days (>= 7 days ago) -> no rolled date (None)
    db.conn.execute("UPDATE anilist_catalog SET next_airing_at = ?1 WHERE id = 101", [now - 9 * day]).unwrap();
    let airing2 = db.list_airing(src).unwrap();
    assert_eq!(airing2.len(), 1);
    assert_eq!(airing2[0].next_episode_at, None);

    // 3. FINISHED status -> no rolled date (None)
    // (followed, so the FINISHED row is still listed; see ...hides_unfollowed_row_the_catalog_says_finished)
    db.set_followed(s1, true).unwrap();
    db.conn.execute(
        "UPDATE anilist_catalog SET status = 'FINISHED', next_airing_at = ?1 WHERE id = 101",
        [now - 2 * day],
    ).unwrap();
    let airing3 = db.list_airing(src).unwrap();
    assert_eq!(airing3.len(), 1);
    assert_eq!(airing3[0].next_episode_at, None);

    // 4. Must NOT happen: catalog date > now is replaced by a rolled date
    db.conn.execute(
        "UPDATE anilist_catalog SET status = 'RELEASING', next_airing_at = ?1 WHERE id = 101",
        [now + 2 * day],
    ).unwrap();
    let airing4 = db.list_airing(src).unwrap();
    assert_eq!(airing4.len(), 1);
    assert_eq!(airing4[0].next_episode_at, Some(now + 2 * day));

    // 5. Must NOT happen: a scraped upcoming date is overridden by the roll-over
    db.conn.execute("UPDATE anilist_catalog SET next_airing_at = ?1 WHERE id = 101", [now - 2 * day]).unwrap();
    db.conn.execute("UPDATE series SET next_episode_at = ?1 WHERE id = ?2", [now + 3 * day, s1]).unwrap();
    let airing5 = db.list_airing(src).unwrap();
    assert_eq!(airing5.len(), 1);
    assert_eq!(airing5[0].next_episode_at, Some(now + 3 * day));

    // 6. catalog_upcoming is false for rolled date: unfollowed OVA is hidden
    db.conn.execute("UPDATE series SET next_episode_at = NULL, followed = 0 WHERE id = ?1", [s1]).unwrap();
    db.conn.execute(
        "UPDATE anilist_catalog SET format = 'OVA', next_airing_at = ?1 WHERE id = 101",
        [now - 2 * day],
    ).unwrap();
    let airing_ova_rolled = db.list_airing(src).unwrap();
    assert!(airing_ova_rolled.is_empty(), "unfollowed OVA with rolled date must stay hidden (catalog_upcoming false)");

    // But with real future catalog date (catalog_upcoming true), unfollowed OVA is shown
    db.conn.execute("UPDATE anilist_catalog SET next_airing_at = ?1 WHERE id = 101", [now + 2 * day]).unwrap();
    let airing_ova_future = db.list_airing(src).unwrap();
    assert_eq!(airing_ova_future.len(), 1, "unfollowed OVA with future date must show (catalog_upcoming true)");
}

#[test]
fn a_rolled_date_confirmed_by_a_scraped_one_is_not_estimated() {
    let db = Db::open(":memory:").unwrap();
    let a = db.upsert_source("AnimeYT", "https://a.example", "animeytx").unwrap();
    let b = db.upsert_source("TioAnime", "https://b.example", "tioanime").unwrap();
    let now = chrono::Utc::now().timestamp();
    let day = 86400;
    let mut cat = crate::db::test_support::catalog_anime(101, "Twin Show", &[]);
    cat.status = Some("RELEASING".into());
    cat.next_airing_at = Some(now - 2 * day);
    cat.next_episode = Some(5);
    db.upsert_catalog_anime(&cat, 0).unwrap();
    // Site A has no date of its own (its date is the +7 d roll); site B scraped exactly that moment.
    let sa = db.upsert_series(a, &crate::db::test_support::mk_airing("twin-a", "Twin Show", None)).unwrap();
    db.set_anilist_id(sa, 101).unwrap();
    let sb = db
        .upsert_series(b, &crate::db::test_support::mk_airing("twin-b", "Twin Show", Some(now + 5 * day)))
        .unwrap();
    db.set_anilist_id(sb, 101).unwrap();

    let (series, rolled) = db.list_airing_with_rolled(a).unwrap();
    assert_eq!(series.len(), 1);
    assert_eq!(series[0].next_episode_at, Some(now + 5 * day));
    assert!(rolled.is_empty(), "a real date confirms the rolled one");
}

#[test]
fn test_list_airing_next_episode_estimated() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("AnimeYT", "https://a.example", "animeytx").unwrap();
    let now = chrono::Utc::now().timestamp();
    let day = 86400;

    // RELEASING, next_airing_at = now - 2 days, no scraped date -> next_episode_estimated == true
    let mut a1 = crate::db::test_support::catalog_anime(101, "Rolled Show", &[]);
    a1.status = Some("RELEASING".into());
    a1.next_airing_at = Some(now - 2 * day);
    a1.next_episode = Some(5);
    db.upsert_catalog_anime(&a1, 0).unwrap();
    let s1 = db.upsert_series(src, &crate::db::test_support::mk_airing("rolled", "Rolled Show", None)).unwrap();
    db.set_anilist_id(s1, 101).unwrap();

    let (series1, rolled1) = db.list_airing_with_rolled(src).unwrap();
    assert_eq!(series1.len(), 1);
    assert!(rolled1.contains(&s1));

    // Future catalog date -> rolled does not contain s1
    db.conn.execute(
        "UPDATE anilist_catalog SET next_airing_at = ?1 WHERE id = 101",
        [now + 2 * day],
    ).unwrap();
    let (series2, rolled2) = db.list_airing_with_rolled(src).unwrap();
    assert_eq!(series2.len(), 1);
    assert!(!rolled2.contains(&s1));
}

#[test]
fn pending_skips_finished_show_with_any_watched_externally_seihantai() {
    // Seihantai case: catalog 210031 FINISHED episodes=13; row A (animeytx, wx=1, eps 1..13
    // with 1..12 seen), row B (tioanime, wx=0, followed, eps 1..13 with 1..12 seen), row C
    // synthetic slug `anilist-210031` wx=1. Must skip the whole group (0 episodes in pending).
    let db = Db::open(":memory:").unwrap();
    let src_a = db.upsert_source("AnimeYT", "https://a.example", "animeytx").unwrap();
    let src_b = db.upsert_source("TioAnime", "https://b.example", "tioanime").unwrap();
    let now = 10_000_000;

    let mut cat = crate::db::test_support::catalog_anime(210031, "Seihantai na Kimi to Boku 2nd Season", &[]);
    cat.status = Some("FINISHED".into());
    cat.episodes = Some(13);
    db.upsert_catalog_anime(&cat, 0).unwrap();

    let mk = |slug: &str, title: &str| crate::models::Series {
        id: 0, slug: slug.into(), title: title.into(), url: format!("https://site/{slug}"),
        cover_url: None, is_airing: true, followed: false, next_episode_at: None, site_episode_count: None,
    };

    // Row A: site animeytx, watched_externally=1, followed=false, eps 1..13 with 1..12 seen
    let s_a = db.upsert_series(src_a, &mk("seihantai-a", "Seihantai na Kimi to Boku 2nd Season")).unwrap();
    db.set_anilist_id(s_a, 210031).unwrap();
    db.set_watched_externally(s_a, true).unwrap();
    for i in 1..=13 {
        db.insert_episode(&crate::models::Episode {
            id: 0, series_id: s_a, number: i.to_string(), title: None,
            url: format!("https://a/{i}"), released_at: None, seen: i <= 12,
            download_path: None, download_status: None,
        }).unwrap();
    }

    // Row B: site tioanime, watched_externally=0, followed=true, eps 1..13 with 1..12 seen
    let s_b = db.upsert_series(src_b, &mk("seihantai-b", "Seihantai na Kimi to Boku 2nd Season")).unwrap();
    db.set_anilist_id(s_b, 210031).unwrap();
    db.set_followed(s_b, true).unwrap();
    for i in 1..=13 {
        db.insert_episode(&crate::models::Episode {
            id: 0, series_id: s_b, number: i.to_string(), title: None,
            url: format!("https://b/{i}"), released_at: None, seen: i <= 12,
            download_path: None, download_status: None,
        }).unwrap();
    }

    // Row C: synthetic slug `anilist-210031` wx=1
    let s_c = db.upsert_series(src_a, &mk("anilist-210031", "Seihantai na Kimi to Boku 2nd Season")).unwrap();
    db.set_anilist_id(s_c, 210031).unwrap();
    db.set_watched_externally(s_c, true).unwrap();

    let priority = vec!["animeytx".to_string(), "tioanime".to_string()];
    let pending = db.merged_pending_episodes_at(&priority, now).unwrap();
    assert!(pending.is_empty(), "FINISHED show marked watched_externally on any row must not appear in pending");
    assert_eq!(db.pending_count(src_a, &priority).unwrap(), 0);
}

#[test]
fn pending_releasing_guard_keeps_unseen_episodes_when_another_row_watched_externally() {
    // Airing guard: same rows as Seihantai case but catalog status is RELEASING
    // (next_episode=14, next_airing_at=now+3600). Episode 13 from row B must stay pending.
    let db = Db::open(":memory:").unwrap();
    let src_a = db.upsert_source("AnimeYT", "https://a.example", "animeytx").unwrap();
    let src_b = db.upsert_source("TioAnime", "https://b.example", "tioanime").unwrap();
    let now = 10_000_000;

    let mut cat = crate::db::test_support::catalog_anime(210031, "Seihantai na Kimi to Boku 2nd Season", &[]);
    cat.status = Some("RELEASING".into());
    cat.next_episode = Some(14);
    cat.next_airing_at = Some(now + 3600);
    db.upsert_catalog_anime(&cat, 0).unwrap();

    let mk = |slug: &str, title: &str| crate::models::Series {
        id: 0, slug: slug.into(), title: title.into(), url: format!("https://site/{slug}"),
        cover_url: None, is_airing: true, followed: false, next_episode_at: None, site_episode_count: None,
    };

    // Row A: site animeytx, watched_externally=1, followed=false, eps 1..13 with 1..12 seen
    let s_a = db.upsert_series(src_a, &mk("seihantai-a", "Seihantai na Kimi to Boku 2nd Season")).unwrap();
    db.set_anilist_id(s_a, 210031).unwrap();
    db.set_watched_externally(s_a, true).unwrap();
    for i in 1..=13 {
        db.insert_episode(&crate::models::Episode {
            id: 0, series_id: s_a, number: i.to_string(), title: None,
            url: format!("https://a/{i}"), released_at: None, seen: i <= 12,
            download_path: None, download_status: None,
        }).unwrap();
    }

    // Row B: site tioanime, watched_externally=0, followed=true, eps 1..13 with 1..12 seen
    let s_b = db.upsert_series(src_b, &mk("seihantai-b", "Seihantai na Kimi to Boku 2nd Season")).unwrap();
    db.set_anilist_id(s_b, 210031).unwrap();
    db.set_followed(s_b, true).unwrap();
    for i in 1..=13 {
        db.insert_episode(&crate::models::Episode {
            id: 0, series_id: s_b, number: i.to_string(), title: None,
            url: format!("https://b/{i}"), released_at: None, seen: i <= 12,
            download_path: None, download_status: None,
        }).unwrap();
    }

    // Row C: synthetic slug `anilist-210031` wx=1
    let s_c = db.upsert_series(src_a, &mk("anilist-210031", "Seihantai na Kimi to Boku 2nd Season")).unwrap();
    db.set_anilist_id(s_c, 210031).unwrap();
    db.set_watched_externally(s_c, true).unwrap();

    let priority = vec!["animeytx".to_string(), "tioanime".to_string()];
    let pending = db.merged_pending_episodes_at(&priority, now).unwrap();
    assert_eq!(pending.len(), 1);
    let (rep_id, ep_ids) = &pending[0];
    assert_eq!(*rep_id, s_b);
    assert_eq!(ep_ids.len(), 1);
    let ep = db.get_episode(ep_ids[0]).unwrap();
    assert_eq!(ep.number, "13");
    assert_eq!(ep.series_id, s_b);
    assert_eq!(db.pending_count(src_a, &priority).unwrap(), 1);
}

#[test]
fn pending_caps_unaired_episodes_per_anilist() {
    // Unaired per AniList: catalog RELEASING next_episode=9 next_airing_at=now+3600,
    // map has 7 aired; site row has unseen 7, 8, 9 -> pending is [7, 8] (cap = max(7, 8) = 8).
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("AnimeYT", "https://a.example", "animeytx").unwrap();
    let now = 10_000_000;

    let mut cat = crate::db::test_support::catalog_anime(3001, "Tensei Goblin", &[]);
    cat.status = Some("RELEASING".into());
    cat.next_episode = Some(9);
    cat.next_airing_at = Some(now + 3600);
    db.upsert_catalog_anime(&cat, 0).unwrap();

    let entries: Vec<crate::anizip::MapEntry> = (1..=7)
        .map(|i| crate::anizip::MapEntry {
            episode: i,
            season: 1,
            absolute: Some(i),
            air_at: Some(now - (8 - i) * 86400),
        })
        .collect();
    db.replace_episode_map(3001, &entries, now).unwrap();

    let mk = |slug: &str, title: &str| crate::models::Series {
        id: 0, slug: slug.into(), title: title.into(), url: format!("https://site/{slug}"),
        cover_url: None, is_airing: true, followed: true, next_episode_at: None, site_episode_count: None,
    };

    let sid = db.upsert_series(src, &mk("goblin", "Tensei Goblin")).unwrap();
    db.set_anilist_id(sid, 3001).unwrap();
    db.set_followed(sid, true).unwrap();

    for ep in [7, 8, 9] {
        db.insert_episode(&crate::models::Episode {
            id: 0, series_id: sid, number: ep.to_string(), title: None,
            url: format!("https://a/{ep}"), released_at: None, seen: false,
            download_path: None, download_status: None,
        }).unwrap();
    }

    let priority = vec!["animeytx".to_string()];
    let pending = db.merged_pending_episodes_at(&priority, now).unwrap();
    assert_eq!(pending.len(), 1);
    let (_, ep_ids) = &pending[0];
    let nums: Vec<String> = ep_ids.iter().map(|id| db.get_episode(*id).unwrap().number).collect();
    assert_eq!(nums, vec!["7", "8"], "episode 9 has not aired yet per AniList");
}

#[test]
fn pending_includes_stale_catalog_aired_episode() {
    // Stale catalog: same as above but next_airing_at=now-60 -> cap 9 -> pending [7, 8, 9].
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("AnimeYT", "https://a.example", "animeytx").unwrap();
    let now = 10_000_000;

    let mut cat = crate::db::test_support::catalog_anime(3002, "Tensei Goblin 2", &[]);
    cat.status = Some("RELEASING".into());
    cat.next_episode = Some(9);
    cat.next_airing_at = Some(now - 60);
    db.upsert_catalog_anime(&cat, 0).unwrap();

    let entries: Vec<crate::anizip::MapEntry> = (1..=7)
        .map(|i| crate::anizip::MapEntry {
            episode: i,
            season: 1,
            absolute: Some(i),
            air_at: Some(now - (8 - i) * 86400),
        })
        .collect();
    db.replace_episode_map(3002, &entries, now).unwrap();

    let mk = |slug: &str, title: &str| crate::models::Series {
        id: 0, slug: slug.into(), title: title.into(), url: format!("https://site/{slug}"),
        cover_url: None, is_airing: true, followed: true, next_episode_at: None, site_episode_count: None,
    };

    let sid = db.upsert_series(src, &mk("goblin2", "Tensei Goblin 2")).unwrap();
    db.set_anilist_id(sid, 3002).unwrap();
    db.set_followed(sid, true).unwrap();

    for ep in [7, 8, 9] {
        db.insert_episode(&crate::models::Episode {
            id: 0, series_id: sid, number: ep.to_string(), title: None,
            url: format!("https://a/{ep}"), released_at: None, seen: false,
            download_path: None, download_status: None,
        }).unwrap();
    }

    let priority = vec!["animeytx".to_string()];
    let pending = db.merged_pending_episodes_at(&priority, now).unwrap();
    assert_eq!(pending.len(), 1);
    let (_, ep_ids) = &pending[0];
    let nums: Vec<String> = ep_ids.iter().map(|id| db.get_episode(*id).unwrap().number).collect();
    assert_eq!(nums, vec!["7", "8", "9"], "stale catalog countdown means ep 9 has aired");
}

#[test]
fn pending_caps_finished_season_numbered_past_total_preserving_fractional() {
    // Finished season numbered past total: FINISHED episodes=12, unseen 11, 12, 12.5, 13
    // -> pending [11, 12, 12.5] (cap = 12.0; 12.5.floor() == 12.0 <= 12.0).
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("AnimeYT", "https://a.example", "animeytx").unwrap();
    let now = 10_000_000;

    let mut cat = crate::db::test_support::catalog_anime(4001, "Finished Season", &[]);
    cat.status = Some("FINISHED".into());
    cat.episodes = Some(12);
    db.upsert_catalog_anime(&cat, 0).unwrap();

    let mk = |slug: &str, title: &str| crate::models::Series {
        id: 0, slug: slug.into(), title: title.into(), url: format!("https://site/{slug}"),
        cover_url: None, is_airing: true, followed: true, next_episode_at: None, site_episode_count: None,
    };

    let sid = db.upsert_series(src, &mk("finished-season", "Finished Season")).unwrap();
    db.set_anilist_id(sid, 4001).unwrap();
    db.set_followed(sid, true).unwrap();

    for ep in ["11", "12", "12.5", "13"] {
        db.insert_episode(&crate::models::Episode {
            id: 0, series_id: sid, number: ep.to_string(), title: None,
            url: format!("https://a/{ep}"), released_at: None, seen: false,
            download_path: None, download_status: None,
        }).unwrap();
    }

    let priority = vec!["animeytx".to_string()];
    let pending = db.merged_pending_episodes_at(&priority, now).unwrap();
    assert_eq!(pending.len(), 1);
    let (_, ep_ids) = &pending[0];
    let nums: Vec<String> = ep_ids.iter().map(|id| db.get_episode(*id).unwrap().number).collect();
    assert_eq!(nums, vec!["11", "12", "12.5"], "12.5 recap stays under cap 12; 13 is dropped");
}

#[test]
fn pending_without_anilist_data_keeps_all_unseen_episodes() {
    // No AniList data (series without anilist_id, or catalog row with status NULL,
    // episodes NULL, next_episode NULL and empty map) -> every unseen episode stays.
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("AnimeYT", "https://a.example", "animeytx").unwrap();
    let now = 10_000_000;

    let mk = |slug: &str, title: &str| crate::models::Series {
        id: 0, slug: slug.into(), title: title.into(), url: format!("https://site/{slug}"),
        cover_url: None, is_airing: true, followed: true, next_episode_at: None, site_episode_count: None,
    };

    // Series A: no anilist_id
    let s_a = db.upsert_series(src, &mk("no-anilist", "No AniList Show")).unwrap();
    db.set_followed(s_a, true).unwrap();
    for ep in ["1", "2", "3"] {
        db.insert_episode(&crate::models::Episode {
            id: 0, series_id: s_a, number: ep.to_string(), title: None,
            url: format!("https://a/no-al/{ep}"), released_at: None, seen: false,
            download_path: None, download_status: None,
        }).unwrap();
    }

    // Series B: catalog row with status NULL, episodes NULL, next_episode NULL, empty map
    let mut cat = crate::db::test_support::catalog_anime(5001, "Null Catalog Show", &[]);
    cat.status = None;
    cat.episodes = None;
    cat.next_episode = None;
    cat.next_airing_at = None;
    db.upsert_catalog_anime(&cat, 0).unwrap();

    let s_b = db.upsert_series(src, &mk("null-catalog", "Null Catalog Show")).unwrap();
    db.set_anilist_id(s_b, 5001).unwrap();
    db.set_followed(s_b, true).unwrap();
    for ep in ["1", "2"] {
        db.insert_episode(&crate::models::Episode {
            id: 0, series_id: s_b, number: ep.to_string(), title: None,
            url: format!("https://a/null-cat/{ep}"), released_at: None, seen: false,
            download_path: None, download_status: None,
        }).unwrap();
    }

    let priority = vec!["animeytx".to_string()];
    let pending = db.merged_pending_episodes_at(&priority, now).unwrap();
    assert_eq!(pending.len(), 2);
    let total_eps: usize = pending.iter().map(|(_, eps)| eps.len()).sum();
    assert_eq!(total_eps, 5, "all 3 + 2 unseen episodes stay when AniList data is absent");
}

#[test]
fn anilist_aired_cap_calculation_rules() {
    let now = 10_000_000;

    // No catalog data -> None
    assert_eq!(anilist_aired_cap(None, None, None, None, None, now), None);
    assert_eq!(anilist_aired_cap(None, None, None, None, Some(7), now), None);

    // FINISHED with total episodes
    assert_eq!(anilist_aired_cap(Some("FINISHED"), Some(12), None, None, None, now), Some(12.0));
    assert_eq!(anilist_aired_cap(Some("FINISHED"), Some(12), None, None, Some(14), now), Some(14.0));

    // RELEASING upcoming
    assert_eq!(anilist_aired_cap(Some("RELEASING"), None, Some(9), Some(now + 3600), Some(7), now), Some(8.0));
    // RELEASING stale (air date passed)
    assert_eq!(anilist_aired_cap(Some("RELEASING"), None, Some(9), Some(now - 60), Some(7), now), Some(9.0));
    // Countdown for ep 9 passed 15 days ago: weekly eps 10 and 11 have aired too.
    assert_eq!(
        anilist_aired_cap(Some("RELEASING"), None, Some(9), Some(now - 15 * 86_400), Some(7), now),
        Some(11.0)
    );

    // Map alone (no countdown, not finished) undercounts when dates are
    // missing: no cap rather than hiding aired episodes.
    assert_eq!(anilist_aired_cap(Some("RELEASING"), None, None, None, Some(5), now), None);
}

#[test]
fn pending_does_not_cap_a_row_numbered_past_the_season_total() {
    // FINISHED entry with 12 episodes, but the site numbers this season
    // 13..24 (cumulative) and no ani.zip offset is known: the cap does not
    // describe that row, so its unseen episodes stay pending.
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("TioAnime", "https://b.example", "tioanime").unwrap();
    let now = 10_000_000;
    let mut cat = crate::db::test_support::catalog_anime(4001, "Some Show 2nd Season", &[]);
    cat.status = Some("FINISHED".into());
    cat.episodes = Some(12);
    db.upsert_catalog_anime(&cat, 0).unwrap();
    let sid = db.upsert_series(src, &crate::models::Series {
        id: 0, slug: "some-show-2".into(), title: "Some Show 2nd Season".into(),
        url: "https://site/some-show-2".into(), cover_url: None, is_airing: false,
        followed: true, next_episode_at: None, site_episode_count: None,
    }).unwrap();
    db.set_anilist_id(sid, 4001).unwrap();
    db.set_followed(sid, true).unwrap();
    for i in 13..=24 {
        db.insert_episode(&crate::models::Episode {
            id: 0, series_id: sid, number: i.to_string(), title: None,
            url: format!("https://b/{i}"), released_at: None, seen: false,
            download_path: None, download_status: None,
        }).unwrap();
    }
    let pending = db.merged_pending_episodes_at(&["tioanime".to_string()], now).unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].1.len(), 12);
}

static STMT_COUNT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
fn count_stmt(_sql: &str) {
    STMT_COUNT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
}

/// One Pendientes call must cost a bounded number of SQL statements, not
/// O(members): 50 followed groups x 2 members, all linked to AniList with an
/// episode map (cumulative row on one site, per-season on the other).
#[test]
fn merged_pending_statement_count_is_bounded() {
    let mut db = Db::open(":memory:").unwrap();
    let src_a = db.upsert_source("AnimeYT", "https://a.example", "animeytx").unwrap();
    let src_b = db.upsert_source("TioAnime", "https://b.example", "tioanime").unwrap();
    let now = 1_700_000_000;
    for g in 0..50i64 {
        let al = 9000 + g;
        db.upsert_catalog_anime(&catalog_anime(al, &format!("Group {g}"), &[]), 0).unwrap();
        let entries: Vec<crate::anizip::MapEntry> = (1..=4)
            .map(|e| crate::anizip::MapEntry {
                episode: e, season: 2, absolute: Some(e + 12),
                air_at: Some(now - (5 - e) * 86400),
            })
            .collect();
        db.replace_episode_map(al, &entries, now).unwrap();
        for (src, from, to) in [(src_a, 1, 4), (src_b, 1, 16)] {
            let sid = db
                .upsert_series(src, &mk_airing(&format!("g{g}-{src}"), &format!("Group {g}"), None))
                .unwrap();
            db.set_anilist_id(sid, al).unwrap();
            db.set_followed(sid, true).unwrap();
            for n in from..=to {
                db.insert_episode(&crate::models::Episode {
                    id: 0, series_id: sid, number: n.to_string(), title: None,
                    url: format!("u/{g}/{src}/{n}"), released_at: None, seen: false,
                    download_path: None, download_status: None,
                }).unwrap();
            }
        }
    }
    let priority = vec!["animeytx".to_string(), "tioanime".to_string()];
    STMT_COUNT.store(0, std::sync::atomic::Ordering::SeqCst);
    db.conn.trace(Some(count_stmt));
    let res = db.merged_pending_episodes_at(&priority, now).unwrap();
    db.conn.trace(None);
    let n = STMT_COUNT.load(std::sync::atomic::Ordering::SeqCst);
    eprintln!("merged_pending_episodes_at statements: {n}");
    assert_eq!(res.len(), 50);
    assert!(n <= 12, "statement count must not grow with members, got {n}");
}
