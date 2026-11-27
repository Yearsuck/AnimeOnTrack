use super::*;
use super::super::test_support::*;

// ---- is_unique_violation must name ONE constraint, not a whole class ----

/// The real thing it is meant to tolerate: two episodes of the same series
/// on the same URL.
#[test]
fn is_unique_violation_accepts_the_episodes_url_collision() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
    let sid = db.upsert_series(src, &mk_airing("a", "A", None)).unwrap();
    let err: anyhow::Error = db
        .conn
        .execute(
            "INSERT INTO episodes(series_id, number, url) VALUES(?1, '1', 'u'), (?1, '2', 'u')",
            [sid],
        )
        .unwrap_err()
        .into();
    assert!(is_unique_violation(&err), "expected a UNIQUE(series_id, url) hit, got: {err}");
}

/// Everything else that raises `ErrorCode::ConstraintViolation` must NOT be
/// swallowed as "url collides with another episode" — a scraped episode
/// with a NULL number, or one pointing at a series that doesn't exist, is a
/// genuine data bug and has to surface.
#[test]
fn is_unique_violation_rejects_other_constraint_failures() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
    let sid = db.upsert_series(src, &mk_airing("a", "A", None)).unwrap();

    // NOT NULL on episodes.number.
    let not_null: anyhow::Error = db
        .conn
        .execute("INSERT INTO episodes(series_id, number, url) VALUES(?1, NULL, 'n')", [sid])
        .unwrap_err()
        .into();
    assert!(!is_unique_violation(&not_null), "NOT NULL must not read as a url collision");

    // A different table's UNIQUE constraint (settings.key is the PK) is
    // also not this one — the message check, not just the extended code.
    db.set_setting("k", "v").unwrap();
    let other_table: anyhow::Error = db
        .conn
        .execute("INSERT INTO settings(key, value) VALUES('k', 'v2')", [])
        .unwrap_err()
        .into();
    assert!(!is_unique_violation(&other_table), "another table's constraint is not ours");

    // And a plain non-SQLite error is never a match.
    assert!(!is_unique_violation(&anyhow::anyhow!("something else entirely")));
}

#[test]
fn apply_episode_diff_refreshes_known_inserts_new_and_reports_the_new_count() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("AnimeYT", "old.example", "animeytx").unwrap();
    let sid = db.upsert_series(src, &mk_airing("a", "A", None)).unwrap();
    db.insert_episode(&crate::models::Episode {
        id: 0, series_id: sid, number: "1".into(), title: None,
        url: "https://old.example/a-1".into(), released_at: None, seen: true, download_path: None, download_status: None,
    }).unwrap();

    let scraped = vec![
        crate::models::Episode {
            id: 0, series_id: 0, number: "1".into(), title: Some("Ep 1".into()),
            url: "https://new.example/a-1".into(), released_at: Some("hoy".into()), seen: false, download_path: None, download_status: None,
        },
        crate::models::Episode {
            id: 0, series_id: 0, number: "2".into(), title: None,
            url: "https://new.example/a-2".into(), released_at: None, seen: false, download_path: None, download_status: None,
        },
    ];
    let inserted = db.apply_episode_diff(sid, Some("https://new.example"), &scraped).unwrap();
    assert_eq!(inserted, 1, "only episode 2 is new");

    let eps = db.list_series_episodes(sid).unwrap();
    assert_eq!(eps.len(), 2);
    assert!(eps[0].seen, "refreshing episode 1's link must not touch its seen state");
    assert_eq!(eps[0].url, "https://new.example/a-1", "known episode's link is refreshed in place");
    assert_eq!(db.get_series_url(sid).unwrap().as_deref(), Some("https://new.example"));
    assert!(db.last_checked_age_secs(sid).unwrap().is_some(), "last_checked_at must be stamped");
}

#[test]
fn apply_episode_diff_repro_5006_constraint_clash() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("AnimeYT", "tio.example", "tioanime").unwrap();
    let sid = db.upsert_series(src, &mk_airing("shingeki", "Shingeki", None)).unwrap();
    db.insert_episode(&crate::models::Episode {
        id: 0, series_id: sid, number: "135".into(), title: None,
        url: "https://tioanime.com/ver/shingeki-no-kyojin-13.5".into(), released_at: None, seen: true, download_path: None, download_status: None,
    }).unwrap();
    db.insert_episode(&crate::models::Episode {
        id: 0, series_id: sid, number: "13".into(), title: None,
        url: "https://tioanime.com/ver/shingeki-no-kyojin-13".into(), released_at: None, seen: true, download_path: None, download_status: None,
    }).unwrap();

    let scraped = vec![
        crate::models::Episode {
            id: 0, series_id: 0, number: "135".into(), title: None,
            url: "https://tioanime.com/ver/shingeki-no-kyojin-13.5".into(), released_at: None, seen: false, download_path: None, download_status: None,
        },
        crate::models::Episode {
            id: 0, series_id: 0, number: "13".into(), title: None,
            // Same url the "135" episode already owns — the exact
            // constraint clash observed live for series 5006.
            url: "https://tioanime.com/ver/shingeki-no-kyojin-13.5".into(), released_at: None, seen: false, download_path: None, download_status: None,
        },
    ];
    // Must skip the colliding episode and finish normally — this used
    // to propagate the constraint error and abort the whole `refresh()`
    // cycle for every series still queued behind this one.
    let inserted = db.apply_episode_diff(sid, None, &scraped).unwrap();
    assert_eq!(inserted, 0, "both numbers were already known, nothing new to insert");
}

/// The counting bug: `new_episodes()` keys on episode *number*, the
/// UNIQUE constraint keys on URL. When a stored episode's number no
/// longer matches the site's label (here: "13.5" was stored as "135" by
/// an older `digits_in`, and the adapter now emits "13.5"), the diff
/// offers it as new, the INSERT conflicts on the unchanged URL and writes
/// nothing — and `apply_episode_diff` used to count it as one new episode
/// anyway, on every refresh, forever, for an episode it never inserted.
#[test]
fn apply_episode_diff_does_not_count_an_episode_whose_url_already_exists() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("TioAnime", "tio.example", "tioanime").unwrap();
    let sid = db.upsert_series(src, &mk_airing("shingeki", "Shingeki", None)).unwrap();
    // Stored under the OLD number for this URL.
    db.insert_episode(&crate::models::Episode {
        id: 0, series_id: sid, number: "135".into(), title: None,
        url: "https://tioanime.com/ver/shingeki-no-kyojin-13.5".into(),
        released_at: None, seen: false, download_path: None, download_status: None,
    }).unwrap();

    // The site now labels that same URL "13.5" — an unknown *number*, so
    // the diff offers it as new, but its URL is already taken.
    let scraped = vec![crate::models::Episode {
        id: 0, series_id: 0, number: "13.5".into(), title: None,
        url: "https://tioanime.com/ver/shingeki-no-kyojin-13.5".into(),
        released_at: None, seen: false, download_path: None, download_status: None,
    }];
    let inserted = db.apply_episode_diff(sid, None, &scraped).unwrap();
    assert_eq!(inserted, 0, "nothing was actually written, so nothing is new");
    assert_eq!(db.episode_count(sid).unwrap(), 1, "and no duplicate row appeared");

    // Repeating the refresh must stay at zero — this is the part that
    // used to report a phantom new episode on every single cycle.
    assert_eq!(db.apply_episode_diff(sid, None, &scraped).unwrap(), 0);
}

/// A genuinely-new fractional episode (its own number AND its own URL)
/// still counts, and lands as its own row next to the integer episode it
/// used to collide with once `digits_in` stopped truncating it.
#[test]
fn apply_episode_diff_stores_a_fractional_episode_as_its_own_row() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("TioAnime", "tio.example", "tioanime").unwrap();
    let sid = db.upsert_series(src, &mk_airing("shingeki", "Shingeki", None)).unwrap();
    db.insert_episode(&crate::models::Episode {
        id: 0, series_id: sid, number: "13".into(), title: None,
        url: "https://tioanime.com/ver/shingeki-no-kyojin-13".into(),
        released_at: None, seen: true, download_path: None, download_status: None,
    }).unwrap();

    let scraped = vec![
        crate::models::Episode {
            id: 0, series_id: 0, number: "13".into(), title: None,
            url: "https://tioanime.com/ver/shingeki-no-kyojin-13".into(),
            released_at: None, seen: false, download_path: None, download_status: None,
        },
        crate::models::Episode {
            id: 0, series_id: 0, number: "13.5".into(), title: None,
            url: "https://tioanime.com/ver/shingeki-no-kyojin-13.5".into(),
            released_at: None, seen: false, download_path: None, download_status: None,
        },
    ];
    let inserted = db.apply_episode_diff(sid, None, &scraped).unwrap();
    assert_eq!(inserted, 1, "only the half-episode is new");

    let eps = db.list_series_episodes(sid).unwrap();
    assert_eq!(eps.len(), 2);
    let thirteen = eps.iter().find(|e| e.number == "13").unwrap();
    assert_eq!(
        thirteen.url, "https://tioanime.com/ver/shingeki-no-kyojin-13",
        "episode 13's url must not be rewritten to the half-episode's href"
    );
    assert!(thirteen.seen, "and its watch state is untouched");
    assert!(eps.iter().any(|e| e.number == "13.5"));

    // Re-scraping the same list is a no-op — the fractional number is now
    // a known number, so it is neither re-inserted nor re-counted.
    assert_eq!(db.apply_episode_diff(sid, None, &scraped).unwrap(), 0);
    assert_eq!(db.episode_count(sid).unwrap(), 2);

    // The cascade orders it strictly between 13 and 14: marking 13.5 seen
    // must not touch a later episode.
    db.insert_episode(&crate::models::Episode {
        id: 0, series_id: sid, number: "14".into(), title: None,
        url: "https://tioanime.com/ver/shingeki-no-kyojin-14".into(),
        released_at: None, seen: false, download_path: None, download_status: None,
    }).unwrap();
    db.set_seen_cascade(sid, "13.5", true).unwrap();
    let eps = db.list_series_episodes(sid).unwrap();
    assert!(eps.iter().find(|e| e.number == "13.5").unwrap().seen);
    assert!(!eps.iter().find(|e| e.number == "14").unwrap().seen);
}

#[test]
fn apply_episode_diff_leaves_series_url_alone_when_none_is_given() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
    let sid = db.upsert_series(src, &mk_airing("a", "A", None)).unwrap();
    let original_url = db.get_series_url(sid).unwrap();

    db.apply_episode_diff(sid, None, &[]).unwrap();

    assert_eq!(db.get_series_url(sid).unwrap(), original_url);
}

#[test]
fn episode_count_counts_only_this_series() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
    let sid_a = db.upsert_series(src, &mk_airing("a", "A", None)).unwrap();
    let sid_b = db.upsert_series(src, &mk_airing("b", "B", None)).unwrap();
    for i in 1..=3 {
        db.insert_episode(&crate::models::Episode {
            id: 0, series_id: sid_a, number: i.to_string(), title: None,
            url: format!("https://site/a-{i}"), released_at: None, seen: false, download_path: None, download_status: None,
        }).unwrap();
    }
    assert_eq!(db.episode_count(sid_a).unwrap(), 3);
    assert_eq!(db.episode_count(sid_b).unwrap(), 0);
}

/// `max_episode_number` must reflect the highest real episode number, not
/// the row count — recap/"0"/version-reupload rows must never inflate it
/// (Bug B in the 2026-07-12 airing-refresh-missing-episodes fix: a row
/// COUNT was being compared against the site's next-episode-number badge,
/// so a recap row cancelled the detection margin and a real new episode
/// got silently skipped).
#[test]
fn max_episode_number_ignores_recap_and_version_rows() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();

    // Wistoria-shaped: "0|Recap", "1".."12" -> 13 rows, highest real = 12.
    let sid_a = db.upsert_series(src, &mk_airing("a", "A", None)).unwrap();
    db.insert_episode(&crate::models::Episode {
        id: 0, series_id: sid_a, number: "0 | Recap".to_string(), title: None,
        url: "https://site/a-recap".to_string(), released_at: None, seen: false, download_path: None, download_status: None,
    }).unwrap();
    for i in 1..=12 {
        db.insert_episode(&crate::models::Episode {
            id: 0, series_id: sid_a, number: i.to_string(), title: None,
            url: format!("https://site/a-{i}"), released_at: None, seen: false, download_path: None, download_status: None,
        }).unwrap();
    }
    assert_eq!(db.episode_count(sid_a).unwrap(), 13, "sanity: row count is 13");
    assert_eq!(db.max_episode_number(sid_a).unwrap(), 12);

    // Tomb Raider King-shaped: "1 | v2", "2 | v1" -> highest real = 2.
    let sid_b = db.upsert_series(src, &mk_airing("b", "B", None)).unwrap();
    db.insert_episode(&crate::models::Episode {
        id: 0, series_id: sid_b, number: "1 | v2".to_string(), title: None,
        url: "https://site/b-1".to_string(), released_at: None, seen: false, download_path: None, download_status: None,
    }).unwrap();
    db.insert_episode(&crate::models::Episode {
        id: 0, series_id: sid_b, number: "2 | v1".to_string(), title: None,
        url: "https://site/b-2".to_string(), released_at: None, seen: false, download_path: None, download_status: None,
    }).unwrap();
    assert_eq!(db.max_episode_number(sid_b).unwrap(), 2);

    // No episodes at all -> 0.
    let sid_c = db.upsert_series(src, &mk_airing("c", "C", None)).unwrap();
    assert_eq!(db.max_episode_number(sid_c).unwrap(), 0);
}

/// The batched form must agree with `max_episode_number` per series, be
/// scoped to followed rows of the given source, and simply omit a
/// series with zero episode rows (callers default a missing key to 0).
#[test]
fn max_episode_numbers_for_source_matches_the_per_series_version() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
    let other_src = db.upsert_source("TioAnime", "t", "tioanime").unwrap();

    let sid_a = db.upsert_series(src, &mk_airing("a", "A", None)).unwrap();
    db.set_followed(sid_a, true).unwrap();
    db.insert_episode(&crate::models::Episode {
        id: 0, series_id: sid_a, number: "5".into(), title: None,
        url: "https://site/a-5".into(), released_at: None, seen: false, download_path: None, download_status: None,
    }).unwrap();

    // Followed but no episodes yet -> absent from the map.
    let sid_b = db.upsert_series(src, &mk_airing("b", "B", None)).unwrap();
    db.set_followed(sid_b, true).unwrap();

    // Not followed -> excluded even though it has episodes.
    let sid_c = db.upsert_series(src, &mk_airing("c", "C", None)).unwrap();
    db.insert_episode(&crate::models::Episode {
        id: 0, series_id: sid_c, number: "9".into(), title: None,
        url: "https://site/c-9".into(), released_at: None, seen: false, download_path: None, download_status: None,
    }).unwrap();

    // Followed on a different source -> excluded from this source's map.
    let sid_other = db.upsert_series(other_src, &mk_airing("d", "D", None)).unwrap();
    db.set_followed(sid_other, true).unwrap();
    db.insert_episode(&crate::models::Episode {
        id: 0, series_id: sid_other, number: "3".into(), title: None,
        url: "https://site/d-3".into(), released_at: None, seen: false, download_path: None, download_status: None,
    }).unwrap();

    let map = db.max_episode_numbers_for_source(src).unwrap();
    assert_eq!(map.get(&sid_a).copied(), Some(5));
    assert_eq!(map.get(&sid_b), None, "no episode rows -> absent, not 0");
    assert_eq!(map.get(&sid_c), None, "not followed -> excluded");
    assert_eq!(map.get(&sid_other), None, "different source -> excluded");
}

// ---- "Esta temporada" first-episode date (2026-07-13) ----

/// The lowest-numbered episode must win regardless of insertion order —
/// the site's episode list isn't always scraped/inserted in number order.
#[test]
fn first_episode_dates_picks_lowest_numbered_episode_out_of_order() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
    let sid = db.upsert_series(src, &mk_airing("a", "A", None)).unwrap();

    // Inserted out of order: 3, 1, 2.
    db.insert_episode(&crate::models::Episode {
        id: 0, series_id: sid, number: "3".into(), title: None,
        url: "https://site/a-3".into(), released_at: Some("agosto 1, 2026".into()), seen: false, download_path: None, download_status: None,
    }).unwrap();
    db.insert_episode(&crate::models::Episode {
        id: 0, series_id: sid, number: "1".into(), title: None,
        url: "https://site/a-1".into(), released_at: Some("junio 29, 2026".into()), seen: false, download_path: None, download_status: None,
    }).unwrap();
    db.insert_episode(&crate::models::Episode {
        id: 0, series_id: sid, number: "2".into(), title: None,
        url: "https://site/a-2".into(), released_at: Some("julio 15, 2026".into()), seen: false, download_path: None, download_status: None,
    }).unwrap();

    let dates = db.first_episode_dates(src).unwrap();
    assert_eq!(dates.get(&sid), Some(&"junio 29, 2026".to_string()));
}

#[test]
fn first_episode_dates_omits_series_with_null_released_at() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
    let sid = db.upsert_series(src, &mk_airing("a", "A", None)).unwrap();
    db.insert_episode(&crate::models::Episode {
        id: 0, series_id: sid, number: "1".into(), title: None,
        url: "https://site/a-1".into(), released_at: None, seen: false, download_path: None, download_status: None,
    }).unwrap();

    let dates = db.first_episode_dates(src).unwrap();
    assert!(!dates.contains_key(&sid));
}

#[test]
fn first_episode_dates_omits_non_airing_and_other_source_series() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
    let other_src = db.upsert_source("Other", "c", "othersite").unwrap();

    let mut not_airing = mk_airing("na", "NotAiring", None);
    not_airing.is_airing = false;
    let sid_not_airing = db.upsert_series(src, &not_airing).unwrap();
    db.insert_episode(&crate::models::Episode {
        id: 0, series_id: sid_not_airing, number: "1".into(), title: None,
        url: "https://site/na-1".into(), released_at: Some("mayo 1, 2026".into()), seen: false, download_path: None, download_status: None,
    }).unwrap();

    let sid_other = db.upsert_series(other_src, &mk_airing("o", "Other", None)).unwrap();
    db.insert_episode(&crate::models::Episode {
        id: 0, series_id: sid_other, number: "1".into(), title: None,
        url: "https://other/o-1".into(), released_at: Some("mayo 1, 2026".into()), seen: false, download_path: None, download_status: None,
    }).unwrap();

    let dates = db.first_episode_dates(src).unwrap();
    assert!(!dates.contains_key(&sid_not_airing));
    assert!(!dates.contains_key(&sid_other));
}

#[test]
fn first_episode_dates_includes_unfollowed_series_with_episodes() {
    // The DB layer has no followed requirement: unfollowed airing series
    // with episode data should be included in first_episode_dates results.
    // The filter logic lives in the frontend (AiringGrid.tsx), not here.
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();

    // Create an unfollowed airing series (never call set_followed).
    let sid = db.upsert_series(src, &mk_airing("unfollowed", "Unfollowed", None)).unwrap();
    assert_eq!(
        db.conn
            .query_row("SELECT followed FROM series WHERE id=?1", [sid], |r| r.get::<_, i64>(0))
            .unwrap(),
        0,
        "series must be unfollowed by default"
    );

    // Insert an episode with a non-null released_at.
    db.insert_episode(&crate::models::Episode {
        id: 0, series_id: sid, number: "1".into(), title: None,
        url: "https://site/unfollowed-1".into(), released_at: Some("julio 15, 2026".into()), seen: false, download_path: None, download_status: None,
    }).unwrap();

    // Unfollowed series with episode data IS included in first_episode_dates.
    let dates = db.first_episode_dates(src).unwrap();
    assert_eq!(
        dates.get(&sid),
        Some(&"julio 15, 2026".to_string()),
        "unfollowed series with episodes must be included"
    );
}

#[test]
fn seen_cascade_handles_non_integer_episode_numbers() {
    // Regression: episode numbers like "1x05" (season-prefixed) used to
    // fail Rust's strict i64 parse (-> 0) while SQLite's CAST still read
    // the leading digit, so un-marking one used to wipe every episode's
    // seen flag in the series instead of just the later ones.
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("AnimeYT", "https://wwv.animeytx.net", "animeytx").unwrap();
    let s = crate::models::Series {
        id: 0, slug: "x".into(), title: "X".into(),
        url: "u".into(), cover_url: None, is_airing: true, followed: false, next_episode_at: None, site_episode_count: None,
    };
    let sid = db.upsert_series(src, &s).unwrap();

    let mk = |n: &str, url: &str| crate::models::Episode {
        id: 0, series_id: sid, number: n.into(), title: None,
        url: url.into(), released_at: None, seen: false, download_path: None, download_status: None,
    };
    db.insert_episode(&mk("1x01", "https://site/e1")).unwrap();
    db.insert_episode(&mk("1x02", "https://site/e2")).unwrap();
    db.insert_episode(&mk("1x03", "https://site/e3")).unwrap();

    // marking "1x02" seen cascades to "1x01" too, but not "1x03"
    db.set_seen_cascade(sid, "1x02", true).unwrap();
    let eps = db.list_series_episodes(sid).unwrap();
    assert!(eps.iter().find(|e| e.number == "1x01").unwrap().seen);
    assert!(eps.iter().find(|e| e.number == "1x02").unwrap().seen);
    assert!(!eps.iter().find(|e| e.number == "1x03").unwrap().seen);

    // un-marking "1x02" un-marks "1x02"/"1x03" but must leave "1x01" alone
    db.set_seen_cascade(sid, "1x02", false).unwrap();
    let eps = db.list_series_episodes(sid).unwrap();
    assert!(eps.iter().find(|e| e.number == "1x01").unwrap().seen);
    assert!(!eps.iter().find(|e| e.number == "1x02").unwrap().seen);
    assert!(!eps.iter().find(|e| e.number == "1x03").unwrap().seen);
}

#[test]
fn set_seen_cascade_stamps_and_clears_seen_at() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("AnimeYT", "https://wwv.animeytx.net", "animeytx").unwrap();
    let s = crate::models::Series {
        id: 0, slug: "x".into(), title: "X".into(),
        url: "u".into(), cover_url: None, is_airing: true, followed: false, next_episode_at: None, site_episode_count: None,
    };
    let sid = db.upsert_series(src, &s).unwrap();
    let mk = |n: &str, url: &str| crate::models::Episode {
        id: 0, series_id: sid, number: n.into(), title: None,
        url: url.into(), released_at: None, seen: false, download_path: None, download_status: None,
    };
    let (e1, _) = db.insert_episode(&mk("1", "https://site/e1")).unwrap();
    let (e2, _) = db.insert_episode(&mk("2", "https://site/e2")).unwrap();

    let seen_at = |id: i64| -> Option<String> {
        db.conn
            .query_row("SELECT seen_at FROM episodes WHERE id=?1", [id], |r| r.get(0))
            .unwrap()
    };
    assert!(seen_at(e1).is_none());
    assert!(seen_at(e2).is_none());

    // marking "2" seen cascades to "1" — both rows get seen_at, not just
    // the one explicitly clicked.
    db.set_seen_cascade(sid, "2", true).unwrap();
    assert!(seen_at(e1).is_some());
    assert!(seen_at(e2).is_some());

    // un-marking "2" clears seen_at on the cascade target too
    db.set_seen_cascade(sid, "2", false).unwrap();
    assert!(seen_at(e1).is_some(), "e1 stays seen, keeps its seen_at");
    assert!(seen_at(e2).is_none(), "e2 un-marked, seen_at cleared");
}

#[test]
fn set_seen_cascade_preserves_seen_at_of_episodes_already_seen() {
    // The reported bug: watching episodes 1-11 over separate days, each
    // with its own real seen_at, then marking episode 12 seen used to
    // stamp every one of the 12 rows with today's date — flattening the
    // whole series' watch history and corrupting every date-bucketed
    // stat (30-day heatmap, yearly activity, hourly distribution).
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("AnimeYT", "https://wwv.animeytx.net", "animeytx").unwrap();
    let s = crate::models::Series {
        id: 0, slug: "x".into(), title: "X".into(),
        url: "u".into(), cover_url: None, is_airing: true, followed: false, next_episode_at: None, site_episode_count: None,
    };
    let sid = db.upsert_series(src, &s).unwrap();
    let (e1, _) = db
        .insert_episode(&crate::models::Episode {
            id: 0, series_id: sid, number: "1".into(), title: None,
            url: "https://site/e1".into(), released_at: None, seen: false, download_path: None, download_status: None,
        })
        .unwrap();
    let (e2, _) = db
        .insert_episode(&crate::models::Episode {
            id: 0, series_id: sid, number: "2".into(), title: None,
            url: "https://site/e2".into(), released_at: None, seen: false, download_path: None, download_status: None,
        })
        .unwrap();

    // Episode 1 was watched three weeks ago — give it a real, old seen_at.
    db.set_seen_cascade(sid, "1", true).unwrap();
    db.conn
        .execute(
            "UPDATE episodes SET seen_at = datetime('now', '-21 days') WHERE id=?1",
            [e1],
        )
        .unwrap();
    let old_seen_at: String = db
        .conn
        .query_row("SELECT seen_at FROM episodes WHERE id=?1", [e1], |r| r.get(0))
        .unwrap();

    // Marking episode 2 seen today re-touches episode 1 via the cascade
    // (1 <= 2) — its original watch date must survive unchanged.
    db.set_seen_cascade(sid, "2", true).unwrap();
    let seen_at_after: String = db
        .conn
        .query_row("SELECT seen_at FROM episodes WHERE id=?1", [e1], |r| r.get(0))
        .unwrap();
    assert_eq!(seen_at_after, old_seen_at, "cascade must not reset an already-seen episode's watch date");
    let e2_seen_at: Option<String> = db
        .conn
        .query_row("SELECT seen_at FROM episodes WHERE id=?1", [e2], |r| r.get(0))
        .unwrap();
    assert!(e2_seen_at.is_some(), "the newly-seen episode still gets stamped");
}

#[test]
fn set_seen_stamps_and_clears_seen_at() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("AnimeYT", "https://wwv.animeytx.net", "animeytx").unwrap();
    let s = crate::models::Series {
        id: 0, slug: "x".into(), title: "X".into(),
        url: "u".into(), cover_url: None, is_airing: true, followed: false, next_episode_at: None, site_episode_count: None,
    };
    let sid = db.upsert_series(src, &s).unwrap();
    let (e1, _) = db
        .insert_episode(&crate::models::Episode {
            id: 0, series_id: sid, number: "1".into(), title: None,
            url: "https://site/e1".into(), released_at: None, seen: false, download_path: None, download_status: None,
        })
        .unwrap();

    db.set_seen(e1, true).unwrap();
    let seen_at: Option<String> = db
        .conn
        .query_row("SELECT seen_at FROM episodes WHERE id=?1", [e1], |r| r.get(0))
        .unwrap();
    assert!(seen_at.is_some());

    db.set_seen(e1, false).unwrap();
    let seen_at: Option<String> = db
        .conn
        .query_row("SELECT seen_at FROM episodes WHERE id=?1", [e1], |r| r.get(0))
        .unwrap();
    assert!(seen_at.is_none());
}

#[test]
fn library_next_episode_agrees_with_series_detail_ordering() {
    // "9", "10", "10.5" — the design doc's canonical case for why next-
    // episode ordering must reuse list_series_episodes' ORDER BY
    // (CAST(number AS INTEGER) ASC, id ASC) rather than a numeric parse:
    // a naive string sort would put "10" before "9".
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("AnimeYT", "https://wwv.animeytx.net", "animeytx").unwrap();
    let s = crate::models::Series {
        id: 0, slug: "x".into(), title: "X".into(),
        url: "u".into(), cover_url: None, is_airing: true, followed: true, next_episode_at: None, site_episode_count: None,
    };
    let sid = db.upsert_series(src, &s).unwrap();
    // upsert_series never sets `followed` (scan path must not un-follow);
    // list_library filters on followed=1, so follow it explicitly.
    db.set_followed(sid, true).unwrap();
    let mk = |n: &str, url: &str| crate::models::Episode {
        id: 0, series_id: sid, number: n.into(), title: None,
        url: url.into(), released_at: None, seen: false, download_path: None, download_status: None,
    };
    // Insert out of numeric order to make sure ordering isn't just
    // reflecting insertion order.
    db.insert_episode(&mk("10", "https://site/e10")).unwrap();
    db.insert_episode(&mk("9", "https://site/e9")).unwrap();
    db.insert_episode(&mk("10.5", "https://site/e10.5")).unwrap();

    let detail_order = db.list_series_episodes(sid).unwrap();
    let first_unseen_in_detail_order = detail_order.iter().find(|e| !e.seen).unwrap();

    let items = db.list_library(src).unwrap();
    let item = items.iter().find(|it| it.series.id == sid).unwrap();
    let next = item.next_episode.as_ref().expect("has an unseen episode");
    assert_eq!(next.number, first_unseen_in_detail_order.number);
    assert_eq!(next.number, "9", "lowest by SeriesDetail's own ordering, not string sort");

    // Mark "9" seen; next should become "10" (not "10.5" — 10 sorts
    // before 10.5 under CAST(number AS INTEGER) since both cast to 10,
    // then id ASC breaks the tie in insertion order: "10" was inserted
    // before "10.5").
    db.set_seen_cascade(sid, "9", true).unwrap();
    let items = db.list_library(src).unwrap();
    let item = items.iter().find(|it| it.series.id == sid).unwrap();
    assert_eq!(item.next_episode.as_ref().unwrap().number, "10");
}

#[test]
fn library_next_episode_none_when_fully_seen() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("AnimeYT", "https://wwv.animeytx.net", "animeytx").unwrap();
    let s = crate::models::Series {
        id: 0, slug: "x".into(), title: "X".into(),
        url: "u".into(), cover_url: None, is_airing: true, followed: true, next_episode_at: None, site_episode_count: None,
    };
    let sid = db.upsert_series(src, &s).unwrap();
    db.set_followed(sid, true).unwrap();
    db.insert_episode(&crate::models::Episode {
        id: 0, series_id: sid, number: "1".into(), title: None,
        url: "https://site/e1".into(), released_at: None, seen: false, download_path: None, download_status: None,
    })
    .unwrap();
    db.set_seen_cascade(sid, "1", true).unwrap();

    let items = db.list_library(src).unwrap();
    let item = items.iter().find(|it| it.series.id == sid).unwrap();
    assert!(item.next_episode.is_none());
    assert_eq!(item.total_episodes, 1);
    assert_eq!(item.seen_episodes, 1);
}

#[test]
fn library_next_episode_none_and_no_panic_with_zero_episodes() {
    // A followed-but-never-scraped (or scrape-failed) series: total=0.
    // Must not divide by zero anywhere and must yield next_episode=None.
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("AnimeYT", "https://wwv.animeytx.net", "animeytx").unwrap();
    let s = crate::models::Series {
        id: 0, slug: "x".into(), title: "X".into(),
        url: "u".into(), cover_url: None, is_airing: true, followed: true, next_episode_at: None, site_episode_count: None,
    };
    let sid = db.upsert_series(src, &s).unwrap();
    db.set_followed(sid, true).unwrap();

    let items = db.list_library(src).unwrap();
    assert_eq!(items.len(), 1);
    assert!(items[0].next_episode.is_none());
    assert_eq!(items[0].total_episodes, 0);
    assert_eq!(items[0].seen_episodes, 0);
}

#[test]
fn insert_episode_dedups_and_marks_seen() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("AnimeYT", "https://wwv.animeytx.net", "animeytx").unwrap();
    let s = crate::models::Series {
        id: 0, slug: "x".into(), title: "X".into(),
        url: "u".into(), cover_url: None, is_airing: true, followed: false, next_episode_at: None, site_episode_count: None,
    };
    let sid = db.upsert_series(src, &s).unwrap();
    db.set_followed(sid, true).unwrap();

    let ep = crate::models::Episode {
        id: 0, series_id: sid, number: "1".into(), title: None,
        url: "https://site/ep1".into(), released_at: None, seen: false, download_path: None, download_status: None,
    };
    let (eid, was_new) = db.insert_episode(&ep).unwrap();
    assert!(was_new, "the first insert really does write a row");
    // same url again => no new row
    let (eid_dup, was_new_again) = db.insert_episode(&ep).unwrap();
    assert_eq!(eid, eid_dup);
    assert!(
        !was_new_again,
        "a conflicting insert writes nothing and must report itself as not-new"
    );

    assert_eq!(db.pending_count(src, &[]).unwrap(), 1);
    db.set_seen(eid, true).unwrap();
    assert_eq!(db.pending_count(src, &[]).unwrap(), 0);
}

#[test]
fn existing_episode_numbers_and_refresh_meta() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
    let s = crate::models::Series {
        id: 0, slug: "x".into(), title: "X".into(),
        url: "u".into(), cover_url: None, is_airing: true, followed: true, next_episode_at: None, site_episode_count: None,
    };
    let sid = db.upsert_series(src, &s).unwrap();
    db.insert_episode(&crate::models::Episode {
        id: 0, series_id: sid, number: "1".into(), title: None,
        url: "https://wwv.animeytx.net/anime/x-capitulo-1/".into(), released_at: None, seen: true, download_path: None, download_status: None,
    }).unwrap();

    let nums = db.existing_episode_numbers(sid).unwrap();
    assert!(nums.contains("1"));
    assert_eq!(nums.len(), 1);

    // A re-scan on a new domain refreshes the URL in place, leaving seen.
    db.refresh_episode_meta(sid, "1", "https://animeyt.cc/999/anime/x-capitulo-1/", None, None).unwrap();
    let (url, seen): (String, i64) = db.conn.query_row(
        "SELECT url, seen FROM episodes WHERE series_id=?1 AND number='1'",
        [sid], |r| Ok((r.get(0)?, r.get(1)?)),
    ).unwrap();
    assert_eq!(url, "https://animeyt.cc/999/anime/x-capitulo-1/");
    assert_eq!(seen, 1, "refreshing the link must not clear seen");
    // Still exactly one row for episode 1 — no duplicate.
    let n: i64 = db.conn.query_row(
        "SELECT COUNT(*) FROM episodes WHERE series_id=?1 AND number='1'", [sid], |r| r.get(0),
    ).unwrap();
    assert_eq!(n, 1);
}

// ---- unaired placeholders stored before the adapter skipped them ----

fn ep(sid: i64, n: &str, seen: bool) -> crate::models::Episode {
    crate::models::Episode {
        id: 0, series_id: sid, number: n.into(), title: None,
        url: format!("https://animeyt.cc/x-capitulo-{n}/"), released_at: None, seen,
        download_path: None, download_status: None,
    }
}

fn numbers(db: &Db, sid: i64) -> Vec<String> {
    let mut st = db.conn.prepare("SELECT number FROM episodes WHERE series_id=?1 ORDER BY CAST(number AS REAL)").unwrap();
    st.query_map([sid], |r| r.get(0)).unwrap().map(|r| r.unwrap()).collect()
}

#[test]
fn drops_unseen_rows_ahead_of_the_fresh_animeytx_listing_only() {
    let db = Db::open(":memory:").unwrap();
    let yt = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
    let sid = db.upsert_series(yt, &mk_airing("a", "A", None)).unwrap();
    for (n, seen) in [("1", true), ("2", true), ("3", false), ("4", false)] {
        db.insert_episode(&ep(sid, n, seen)).unwrap();
    }
    // The page now lists 1..=3; 4 was the unaired placeholder.
    let listed = vec![ep(sid, "3", false), ep(sid, "2", true), ep(sid, "1", true)];
    assert_eq!(db.drop_unaired_placeholders(sid, &listed).unwrap(), 1);
    assert_eq!(numbers(&db, sid), ["1", "2", "3"]);

    // A seen row is never touched, even ahead of the listing.
    db.insert_episode(&ep(sid, "9", true)).unwrap();
    assert_eq!(db.drop_unaired_placeholders(sid, &listed).unwrap(), 0);
    assert_eq!(numbers(&db, sid), ["1", "2", "3", "9"]);

    // Only the slot right after the highest listed number is a placeholder:
    // a row further ahead (a partly rendered page) is kept.
    db.insert_episode(&ep(sid, "5", false)).unwrap();
    assert_eq!(db.drop_unaired_placeholders(sid, &listed).unwrap(), 0);
    assert_eq!(numbers(&db, sid), ["1", "2", "3", "5", "9"]);
    db.conn.execute("DELETE FROM episodes WHERE series_id=?1 AND number='5'", [sid]).unwrap();
    // An empty or unparsable listing proves nothing: keep everything.
    db.insert_episode(&ep(sid, "10", false)).unwrap();
    assert_eq!(db.drop_unaired_placeholders(sid, &[]).unwrap(), 0);
    assert_eq!(db.drop_unaired_placeholders(sid, &[ep(sid, "x", false)]).unwrap(), 0);
    assert_eq!(numbers(&db, sid).len(), 5);
}

#[test]
fn placeholder_cleanup_ignores_other_sites_and_rows_with_a_date_or_download() {
    let db = Db::open(":memory:").unwrap();
    let tio = db.upsert_source("TioAnime", "b", "tioanime").unwrap();
    let other = db.upsert_series(tio, &mk_airing("a", "A", None)).unwrap();
    db.insert_episode(&ep(other, "1", true)).unwrap();
    db.insert_episode(&ep(other, "2", false)).unwrap();
    assert_eq!(db.drop_unaired_placeholders(other, &[ep(other, "1", true)]).unwrap(), 0);

    let yt = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
    let sid = db.upsert_series(yt, &mk_airing("b", "B", None)).unwrap();
    db.insert_episode(&ep(sid, "1", true)).unwrap();
    let mut dated = ep(sid, "2", false);
    dated.released_at = Some("2026-11-01".into());
    db.insert_episode(&dated).unwrap();
    db.insert_episode(&ep(sid, "3", false)).unwrap();
    db.conn.execute("UPDATE episodes SET download_path='C:/x.mp4' WHERE series_id=?1 AND number='3'", [sid]).unwrap();
    db.insert_episode(&ep(sid, "4", false)).unwrap();
    db.conn.execute("UPDATE episodes SET download_status='downloading' WHERE series_id=?1 AND number='4'", [sid]).unwrap();
    assert_eq!(db.drop_unaired_placeholders(sid, &[ep(sid, "1", true)]).unwrap(), 0);
    assert_eq!(numbers(&db, sid), ["1", "2", "3", "4"]);
}

#[test]
fn mark_all_episodes_seen_marks_every_episode_via_cascade() {
    // Pins the idempotence fix: a row marked "Ya lo vi" (catalog "Seen")
    // after episodes are scraped in for a title the user decided
    // `Seen` on the catalog deck, every episode must end up seen, via the
    // same gap-free `set_seen_cascade` the rest of the app's watch-tracking
    // uses (not a separate blanket UPDATE).
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
    let s = crate::models::Series {
        id: 0, slug: "x".into(), title: "X".into(),
        url: "u".into(), cover_url: None, is_airing: false, followed: false, next_episode_at: None, site_episode_count: None,
    };
    let sid = db.upsert_series(src, &s).unwrap();

    for n in 1..=3 {
        db.insert_episode(&crate::models::Episode {
            id: 0, series_id: sid, number: n.to_string(), title: None,
            url: format!("e{n}"), released_at: None, seen: false, download_path: None, download_status: None,
        }).unwrap();
    }

    db.mark_all_episodes_seen(sid).unwrap();

    let eps = db.list_series_episodes(sid).unwrap();
    assert_eq!(eps.len(), 3);
    assert!(eps.iter().all(|e| e.seen), "every episode must be marked seen");
}

#[test]
fn mark_all_episodes_seen_is_a_no_op_on_a_series_with_no_episodes() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
    let s = crate::models::Series {
        id: 0, slug: "x".into(), title: "X".into(),
        url: "u".into(), cover_url: None, is_airing: false, followed: false, next_episode_at: None, site_episode_count: None,
    };
    let sid = db.upsert_series(src, &s).unwrap();
    db.mark_all_episodes_seen(sid).unwrap(); // must not error on zero episodes
    assert!(db.list_series_episodes(sid).unwrap().is_empty());
}

#[test]
fn known_series_urls_reflects_any_decided_row() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
    let s = crate::models::Series {
        id: 0, slug: "x".into(), title: "X".into(),
        url: "https://site/tv/x/".into(), cover_url: None, is_airing: false, followed: false, next_episode_at: None, site_episode_count: None,
    };
    let sid = db.upsert_series(src, &s).unwrap();
    db.set_backlog_status(sid, Some("discarded")).unwrap();

    let known = db.known_series_urls(src).unwrap();
    assert!(known.contains("https://site/tv/x/"));
    assert_eq!(known.len(), 1);
}

#[test]
fn download_bookkeeping_and_cascade_detection() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
    let sid = db.upsert_series(src, &mk_airing("a", "A", None)).unwrap();

    let (ep1_id, _) = db.insert_episode(&crate::models::Episode {
        id: 0, series_id: sid, number: "1".into(), title: None,
        url: "https://site/a-1".into(), released_at: None, seen: false,
        download_path: None, download_status: None,
    }).unwrap();
    let (ep2_id, _) = db.insert_episode(&crate::models::Episode {
        id: 0, series_id: sid, number: "2".into(), title: None,
        url: "https://site/a-2".into(), released_at: None, seen: false,
        download_path: None, download_status: None,
    }).unwrap();

    // Initially no downloads
    assert_eq!(db.downloaded_paths(&[ep1_id, ep2_id]).unwrap().len(), 0);

    // Set ep1 downloaded
    let temp_file = std::env::temp_dir().join(format!("aot_test_dl_{}.mp4", ep1_id));
    std::fs::write(&temp_file, b"test-video").unwrap();
    let path_str = temp_file.to_str().unwrap();

    db.set_download_status(ep1_id, "downloaded", Some(path_str)).unwrap();

    let ep1 = db.get_episode(ep1_id).unwrap();
    assert_eq!(ep1.download_status.as_deref(), Some("downloaded"));
    assert_eq!(ep1.download_path.as_deref(), Some(path_str));

    let paths = db.downloaded_paths(&[ep1_id, ep2_id]).unwrap();
    assert_eq!(paths.len(), 1);
    assert_eq!(paths[0], (ep1_id, path_str.to_string()));

    // Check cascade affected IDs: marking episode 2 seen affects 1 and 2
    let cascade_ids = db.episode_ids_in_cascade(sid, "2", true).unwrap();
    assert!(cascade_ids.contains(&ep1_id));
    assert!(cascade_ids.contains(&ep2_id));

    // Clean up
    db.clear_download_columns(&[ep1_id]).unwrap();
    assert_eq!(db.downloaded_paths(&[ep1_id]).unwrap().len(), 0);
    let _ = std::fs::remove_file(&temp_file);
}

#[test]
fn set_seen_cascade_canonical_syncs_across_canonical_group() {
    let db = Db::open(":memory:").unwrap();
    let src_a = db.upsert_source("AnimeYT", "https://a.example", "animeytx").unwrap();
    let src_b = db.upsert_source("TioAnime", "https://b.example", "tioanime").unwrap();

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

    let row_a = db.upsert_series(src_a, &mk("show-a", "Shared Show")).unwrap();
    db.set_followed(row_a, true).unwrap();
    let row_b = db.upsert_series(src_b, &mk("show-b", "Shared Show")).unwrap();
    db.set_followed(row_b, true).unwrap();

    for i in 1..=3 {
        db.insert_episode(&crate::models::Episode {
            id: 0,
            series_id: row_a,
            number: i.to_string(),
            title: None,
            url: format!("https://a/{i}"),
            released_at: None,
            seen: false,
            download_path: None,
            download_status: None,
        })
        .unwrap();
    }
    for i in 1..=4 {
        db.insert_episode(&crate::models::Episode {
            id: 0,
            series_id: row_b,
            number: i.to_string(),
            title: None,
            url: format!("https://b/{i}"),
            released_at: None,
            seen: false,
            download_path: None,
            download_status: None,
        })
        .unwrap();
    }

    // Marking 3 seen on A cascades 1-3 on BOTH rows
    db.set_seen_cascade_canonical(row_a, "3", true).unwrap();
    let eps_a = db.list_series_episodes(row_a).unwrap();
    let eps_b = db.list_series_episodes(row_b).unwrap();
    assert!(eps_a.iter().all(|e| e.seen), "A has 1-3 seen");
    assert_eq!(eps_b.iter().filter(|e| e.seen).count(), 3, "B has 1-3 seen");
    assert!(!eps_b.iter().find(|e| e.number == "4").unwrap().seen, "B's ep 4 is still unseen");

    // Un-marking 2 marks 2-4 unseen on both
    db.set_seen_cascade_canonical(row_a, "2", false).unwrap();
    let eps_a = db.list_series_episodes(row_a).unwrap();
    let eps_b = db.list_series_episodes(row_b).unwrap();
    assert!(eps_a.iter().find(|e| e.number == "1").unwrap().seen, "A's ep 1 stays seen");
    assert!(!eps_a.iter().find(|e| e.number == "2").unwrap().seen, "A's ep 2 unseen");
    assert!(!eps_a.iter().find(|e| e.number == "3").unwrap().seen, "A's ep 3 unseen");

    assert!(eps_b.iter().find(|e| e.number == "1").unwrap().seen, "B's ep 1 stays seen");
    assert!(!eps_b.iter().find(|e| e.number == "2").unwrap().seen, "B's ep 2 unseen");
    assert!(!eps_b.iter().find(|e| e.number == "3").unwrap().seen, "B's ep 3 unseen");
    assert!(!eps_b.iter().find(|e| e.number == "4").unwrap().seen, "B's ep 4 unseen");
}

#[test]
fn set_seen_cascade_canonical_tensei_fixture() {
    let db = Db::open(":memory:").unwrap();
    let now = chrono::Utc::now().timestamp();
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
    for i in 1..=28 {
        db.insert_episode(&crate::models::Episode {
            id: 0, series_id: row_b, number: i.to_string(), title: None,
            url: format!("https://tioanime.com/ver/tensei-{i}"), released_at: None, seen: false,
            download_path: None, download_status: None,
        }).unwrap();
    }

    // Marking animeyt "2" seen marks TioAnime 1..26 seen (and animeyt 1..2)
    db.set_seen_cascade_canonical(row_a, "2", true).unwrap();
    let eps_a = db.list_series_episodes(row_a).unwrap();
    let eps_b = db.list_series_episodes(row_b).unwrap();
    assert!(eps_a.iter().filter(|e| e.seen).all(|e| e.number == "1" || e.number == "2"));
    assert_eq!(eps_a.iter().filter(|e| e.seen).count(), 2);
    assert_eq!(eps_b.iter().filter(|e| e.seen).count(), 26, "TioAnime 1..26 seen");
    assert!(!eps_b.iter().find(|e| e.number == "27").unwrap().seen);
    assert!(!eps_b.iter().find(|e| e.number == "28").unwrap().seen);

    // Un-marking animeyt "2" unseen marks TioAnime 26..28 unseen and leaves 1..25 seen
    db.set_seen_cascade_canonical(row_a, "2", false).unwrap();
    let eps_a = db.list_series_episodes(row_a).unwrap();
    let eps_b = db.list_series_episodes(row_b).unwrap();
    assert_eq!(eps_a.iter().filter(|e| e.seen).count(), 1, "animeyt has 1 seen");
    assert!(eps_a.iter().find(|e| e.number == "1").unwrap().seen);
    assert_eq!(eps_b.iter().filter(|e| e.seen).count(), 25, "TioAnime leaves 1..25 seen");
    assert!(!eps_b.iter().find(|e| e.number == "26").unwrap().seen, "TioAnime 26 unseen");
    assert!(!eps_b.iter().find(|e| e.number == "27").unwrap().seen, "TioAnime 27 unseen");
    assert!(!eps_b.iter().find(|e| e.number == "28").unwrap().seen, "TioAnime 28 unseen");

    // Marking TioAnime "27" seen marks animeyt 1..3 seen
    db.set_seen_cascade_canonical(row_b, "27", true).unwrap();
    let eps_a = db.list_series_episodes(row_a).unwrap();
    let eps_b = db.list_series_episodes(row_b).unwrap();
    assert_eq!(eps_a.iter().filter(|e| e.seen).count(), 3, "animeyt 1..3 seen");
    assert_eq!(eps_b.iter().filter(|e| e.seen).count(), 27, "TioAnime 1..27 seen");
    assert!(!eps_b.iter().find(|e| e.number == "28").unwrap().seen);

    // Marking an earlier-season number on cumulative row (<= offset 24) cascades ONLY that row
    db.set_seen_cascade_canonical(row_b, "20", false).unwrap();
    let eps_a = db.list_series_episodes(row_a).unwrap();
    let eps_b = db.list_series_episodes(row_b).unwrap();
    // Animeyt is completely untouched (still 1..3 seen)
    assert_eq!(eps_a.iter().filter(|e| e.seen).count(), 3, "animeyt untouched by earlier-season cascade");
    // TioAnime has 1..19 seen
    assert_eq!(eps_b.iter().filter(|e| e.seen).count(), 19);
}

#[test]
fn interrupted_downloads_are_reset_but_finished_ones_are_kept() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("T", "https://tioanime.com", "tioanime").unwrap();
    let series = crate::models::Series {
        id: 0, slug: "s".into(), title: "S".into(), url: "https://tioanime.com/anime/s".into(),
        cover_url: None, is_airing: true, followed: true, next_episode_at: None, site_episode_count: None,
    };
    let sid = db.upsert_series(src, &series).unwrap();
    let mk = |n: &str| crate::models::Episode {
        id: 0, series_id: sid, number: n.into(), title: None, url: format!("https://tioanime.com/ver/s-{n}"),
        released_at: None, seen: false, download_path: None, download_status: None,
    };
    let (a, _) = db.insert_episode(&mk("1")).unwrap();
    let (b, _) = db.insert_episode(&mk("2")).unwrap();
    let (c, _) = db.insert_episode(&mk("3")).unwrap();
    db.set_download_status(a, "downloading", None).unwrap();
    db.set_download_status(b, "downloaded", Some("/x/2.mp4")).unwrap();
    db.set_download_status(c, "failed", None).unwrap();
    assert_eq!(db.clear_interrupted_downloads().unwrap(), 1);
    assert_eq!(db.get_episode(a).unwrap().download_status, None);
    assert_eq!(db.get_episode(b).unwrap().download_status.as_deref(), Some("downloaded"));
    assert_eq!(db.get_episode(c).unwrap().download_status.as_deref(), Some("failed"));
}

#[test]
fn sibling_episodes_unlinked_two_sources_reciprocal() {
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

    let row_a = db.upsert_series(src_a, &mk("show-a", "Shared Show")).unwrap();
    let row_b = db.upsert_series(src_b, &mk("show-b", "Shared Show")).unwrap();

    let (ep_a1, _) = db.insert_episode(&crate::models::Episode {
        id: 0,
        series_id: row_a,
        number: "1".into(),
        title: None,
        url: "https://a/1".into(),
        released_at: None,
        seen: false,
        download_path: None,
        download_status: None,
    }).unwrap();

    let (ep_b1, _) = db.insert_episode(&crate::models::Episode {
        id: 0,
        series_id: row_b,
        number: "1".into(),
        title: None,
        url: "https://b/1".into(),
        released_at: None,
        seen: false,
        download_path: None,
        download_status: None,
    }).unwrap();

    let sibs_of_a = db.sibling_episodes(ep_a1).unwrap();
    assert_eq!(
        sibs_of_a,
        vec![SiblingEpisode {
            series_id: row_b,
            source_id: src_b,
            site_id: "tioanime".into(),
            episode_id: ep_b1,
            url: "https://b/1".into(),
        }]
    );

    let sibs_of_b = db.sibling_episodes(ep_b1).unwrap();
    assert_eq!(
        sibs_of_b,
        vec![SiblingEpisode {
            series_id: row_a,
            source_id: src_a,
            site_id: "animeytx".into(),
            episode_id: ep_a1,
            url: "https://a/1".into(),
        }]
    );
}

#[test]
fn sibling_episodes_cumulative_offset_tensei_fixture() {
    let db = Db::open(":memory:").unwrap();
    let now = chrono::Utc::now().timestamp();
    let src_a = db.upsert_source("AnimeYT", "https://animeyt.cc", "animeytx").unwrap();
    let src_b = db.upsert_source("TioAnime", "https://tioanime.com", "tioanime").unwrap();

    let entries = vec![
        crate::anizip::MapEntry { episode: 1, season: 3, absolute: Some(25), air_at: Some(now - 14 * 86400) },
        crate::anizip::MapEntry { episode: 2, season: 3, absolute: Some(26), air_at: Some(now - 7 * 86400) },
        crate::anizip::MapEntry { episode: 3, season: 3, absolute: Some(27), air_at: Some(now) },
        crate::anizip::MapEntry { episode: 4, season: 3, absolute: Some(28), air_at: Some(now + 7 * 86400) },
    ];
    db.replace_episode_map(185756, &entries, now).unwrap();

    let mk = |slug: &str, title: &str| crate::models::Series {
        id: 0, slug: slug.into(), title: title.into(), url: format!("https://site/{slug}"),
        cover_url: None, is_airing: true, followed: true, next_episode_at: None, site_episode_count: None,
    };

    let row_a = db.upsert_series(src_a, &mk("tensei-a", "Tensei")).unwrap();
    db.set_anilist_id(row_a, 185756).unwrap();
    let (ep_a3, _) = db.insert_episode(&crate::models::Episode {
        id: 0, series_id: row_a, number: "3".into(), title: None,
        url: "https://animeyt.cc/tv/tensei/3".into(), released_at: None, seen: false,
        download_path: None, download_status: None,
    }).unwrap();

    let row_b = db.upsert_series(src_b, &mk("tensei-b", "Tensei")).unwrap();
    db.set_anilist_id(row_b, 185756).unwrap();
    let mut ep_b20_id = 0;
    let mut ep_b27_id = 0;
    for i in 1..=28 {
        let (id, _) = db.insert_episode(&crate::models::Episode {
            id: 0, series_id: row_b, number: i.to_string(), title: None,
            url: format!("https://tioanime.com/ver/tensei-{i}"), released_at: None, seen: false,
            download_path: None, download_status: None,
        }).unwrap();
        if i == 20 { ep_b20_id = id; }
        if i == 27 { ep_b27_id = id; }
    }

    // Ep 3 on AnimeYT aligns to Ep 27 on TioAnime
    let sibs = db.sibling_episodes(ep_a3).unwrap();
    assert_eq!(
        sibs,
        vec![SiblingEpisode {
            series_id: row_b,
            source_id: src_b,
            site_id: "tioanime".into(),
            episode_id: ep_b27_id,
            url: "https://tioanime.com/ver/tensei-27".into(),
        }]
    );

    // Ep 20 on TioAnime is earlier season (<= offset 24) -> no canonical alignment -> empty vec
    let sibs_earlier = db.sibling_episodes(ep_b20_id).unwrap();
    assert!(sibs_earlier.is_empty());
}

#[test]
fn sibling_episodes_no_matching_episode_returns_empty() {
    let db = Db::open(":memory:").unwrap();
    let src_a = db.upsert_source("AnimeYT", "https://animeyt.cc", "animeytx").unwrap();
    let src_b = db.upsert_source("TioAnime", "https://tioanime.com", "tioanime").unwrap();

    let mk = |slug: &str, title: &str| crate::models::Series {
        id: 0, slug: slug.into(), title: title.into(), url: format!("https://site/{slug}"),
        cover_url: None, is_airing: true, followed: true, next_episode_at: None, site_episode_count: None,
    };

    let row_a = db.upsert_series(src_a, &mk("show-a", "Unfinished Show")).unwrap();
    let row_b = db.upsert_series(src_b, &mk("show-b", "Unfinished Show")).unwrap();

    // Episode 5 on row_a, but row_b only has episode 1
    let (ep_a5, _) = db.insert_episode(&crate::models::Episode {
        id: 0, series_id: row_a, number: "5".into(), title: None,
        url: "https://a/5".into(), released_at: None, seen: false,
        download_path: None, download_status: None,
    }).unwrap();
    let (_ep_b1, _) = db.insert_episode(&crate::models::Episode {
        id: 0, series_id: row_b, number: "1".into(), title: None,
        url: "https://b/1".into(), released_at: None, seen: false,
        download_path: None, download_status: None,
    }).unwrap();

    let sibs = db.sibling_episodes(ep_a5).unwrap();
    assert!(sibs.is_empty());

    // Sibling series with zero episodes also yields empty
    let src_c = db.upsert_source("JKAnime", "https://jkanime.net", "jkanime").unwrap();
    let _row_c = db.upsert_series(src_c, &mk("show-c", "Unfinished Show")).unwrap();
    let sibs_again = db.sibling_episodes(ep_a5).unwrap();
    assert!(sibs_again.is_empty());
}

#[test]
fn sibling_episodes_bridged_different_titles_rejected() {
    let db = Db::open(":memory:").unwrap();
    let src_a = db.upsert_source("AnimeYT", "https://animeyt.cc", "animeytx").unwrap();
    let src_b = db.upsert_source("TioAnime", "https://tioanime.com", "tioanime").unwrap();

    // Seed catalog row with title "Bridged Title"
    db.conn.execute(
        "INSERT INTO anilist_catalog (id, title, url, sort_order) VALUES (?1, ?2, ?3, ?4)",
        rusqlite::params![500, "Bridged Title", "https://anilist.co/anime/500", 0],
    ).unwrap();

    let mk = |slug: &str, title: &str| crate::models::Series {
        id: 0, slug: slug.into(), title: title.into(), url: format!("https://site/{slug}"),
        cover_url: None, is_airing: true, followed: true, next_episode_at: None, site_episode_count: None,
    };

    // row_a is unlinked (None) but its title matches the catalog row ("Bridged Title")
    let row_a = db.upsert_series(src_a, &mk("show-a", "Bridged Title")).unwrap();
    // row_b is linked to AniList 500 but has a different title
    let row_b = db.upsert_series(src_b, &mk("show-b", "Different Local Title")).unwrap();
    db.set_anilist_id(row_b, 500).unwrap();

    // Verify canonical_series_ids groups them
    let group = db.canonical_series_ids(row_a).unwrap();
    assert!(group.contains(&row_a) && group.contains(&row_b));

    let (ep_a1, _) = db.insert_episode(&crate::models::Episode {
        id: 0, series_id: row_a, number: "1".into(), title: None,
        url: "https://a/1".into(), released_at: None, seen: false,
        download_path: None, download_status: None,
    }).unwrap();
    let (ep_b1, _) = db.insert_episode(&crate::models::Episode {
        id: 0, series_id: row_b, number: "1".into(), title: None,
        url: "https://b/1".into(), released_at: None, seen: false,
        download_path: None, download_status: None,
    }).unwrap();

    // Even though canonical_series_ids grouped row_a and row_b,
    // sibling_episodes rejects row_b because row_a is unlinked and titles differ.
    let sibs_a = db.sibling_episodes(ep_a1).unwrap();
    assert!(sibs_a.is_empty(), "different titles with unlinked row must be rejected");

    let sibs_b = db.sibling_episodes(ep_b1).unwrap();
    assert!(sibs_b.is_empty(), "reciprocal rejection for linked row with different title");
}

#[test]
fn sibling_episodes_same_anilist_id_different_titles_are_siblings() {
    let db = Db::open(":memory:").unwrap();
    let src_a = db.upsert_source("AnimeYT", "https://animeyt.cc", "animeytx").unwrap();
    let src_b = db.upsert_source("TioAnime", "https://tioanime.com", "tioanime").unwrap();

    let mk = |slug: &str, title: &str| crate::models::Series {
        id: 0, slug: slug.into(), title: title.into(), url: format!("https://site/{slug}"),
        cover_url: None, is_airing: true, followed: true, next_episode_at: None, site_episode_count: None,
    };

    let row_a = db.upsert_series(src_a, &mk("show-a", "Attack on Titan")).unwrap();
    db.set_anilist_id(row_a, 999).unwrap();
    let row_b = db.upsert_series(src_b, &mk("show-b", "Shingeki no Kyojin")).unwrap();
    db.set_anilist_id(row_b, 999).unwrap();

    let (ep_a1, _) = db.insert_episode(&crate::models::Episode {
        id: 0, series_id: row_a, number: "1".into(), title: None,
        url: "https://a/1".into(), released_at: None, seen: false,
        download_path: None, download_status: None,
    }).unwrap();
    let (ep_b1, _) = db.insert_episode(&crate::models::Episode {
        id: 0, series_id: row_b, number: "1".into(), title: None,
        url: "https://b/1".into(), released_at: None, seen: false,
        download_path: None, download_status: None,
    }).unwrap();

    let sibs_a = db.sibling_episodes(ep_a1).unwrap();
    assert_eq!(sibs_a.len(), 1);
    assert_eq!(sibs_a[0].series_id, row_b);
    assert_eq!(sibs_a[0].episode_id, ep_b1);

    let sibs_b = db.sibling_episodes(ep_b1).unwrap();
    assert_eq!(sibs_b.len(), 1);
    assert_eq!(sibs_b[0].series_id, row_a);
    assert_eq!(sibs_b[0].episode_id, ep_a1);
}

#[test]
fn sibling_episodes_in_matches_sibling_episodes() {
    let db = Db::open(":memory:").unwrap();
    let src_a = db.upsert_source("AnimeYT", "https://animeyt.cc", "animeytx").unwrap();
    let src_b = db.upsert_source("TioAnime", "https://tioanime.com", "tioanime").unwrap();

    let mk = |slug: &str, title: &str| crate::models::Series {
        id: 0, slug: slug.into(), title: title.into(), url: format!("https://site/{slug}"),
        cover_url: None, is_airing: true, followed: true, next_episode_at: None, site_episode_count: None,
    };

    let row_a = db.upsert_series(src_a, &mk("show-a", "Same Title")).unwrap();
    let row_b = db.upsert_series(src_b, &mk("show-b", "Same Title")).unwrap();

    let (ep_a1, _) = db.insert_episode(&crate::models::Episode {
        id: 0, series_id: row_a, number: "1".into(), title: None,
        url: "https://a/1".into(), released_at: None, seen: false,
        download_path: None, download_status: None,
    }).unwrap();
    let (ep_b1, _) = db.insert_episode(&crate::models::Episode {
        id: 0, series_id: row_b, number: "1".into(), title: None,
        url: "https://b/1".into(), released_at: None, seen: false,
        download_path: None, download_status: None,
    }).unwrap();

    let sids = db.canonical_series_ids(row_a).unwrap();
    let sibs_via_in = db.sibling_episodes_in(ep_a1, &sids).unwrap();
    let sibs_direct = db.sibling_episodes(ep_a1).unwrap();
    assert_eq!(sibs_via_in, sibs_direct);
    assert_eq!(sibs_via_in.len(), 1);
    assert_eq!(sibs_via_in[0].episode_id, ep_b1);
}

#[test]
fn siblings_to_scrape_steel_ball_run_shape_finds_empty_siblings() {
    let db = Db::open(":memory:").unwrap();
    let src_tio = db.upsert_source("TioAnime", "https://tioanime.com", "tioanime").unwrap();
    let src_flv = db.upsert_source("AnimeFLV", "https://animeflv.or.at", "animeflv").unwrap();
    let src_gogo = db.upsert_source("GogoAnime", "https://gogoanime.by", "gogoanime").unwrap();

    let mk = |slug: &str, title: &str, url: &str| crate::models::Series {
        id: 0, slug: slug.into(), title: title.into(), url: url.into(),
        cover_url: None, is_airing: true, followed: true, next_episode_at: None, site_episode_count: None,
    };

    // Three rows: same title and same anilist_id 190327
    let row_tio = db.upsert_series(src_tio, &mk("sbr-tio", "Steel Ball Run", "https://tioanime.com/anime/steel-ball-run")).unwrap();
    db.set_anilist_id(row_tio, 190327).unwrap();
    let row_flv = db.upsert_series(src_flv, &mk("sbr-flv", "Steel Ball Run", "https://animeflv.or.at/anime/steel-ball-run")).unwrap();
    db.set_anilist_id(row_flv, 190327).unwrap();
    let row_gogo = db.upsert_series(src_gogo, &mk("sbr-gogo", "Steel Ball Run", "https://gogoanime.by/steel-ball-run")).unwrap();
    db.set_anilist_id(row_gogo, 190327).unwrap();

    // Only row_tio has episode 1
    let (ep_tio1, _) = db.insert_episode(&crate::models::Episode {
        id: 0, series_id: row_tio, number: "1".into(), title: None,
        url: "https://tioanime.com/ver/steel-ball-run-1".into(), released_at: None, seen: false,
        download_path: None, download_status: None,
    }).unwrap();

    // Sibling episodes already in DB: empty
    let sib_eps = db.sibling_episodes(ep_tio1).unwrap();
    assert!(sib_eps.is_empty());

    // siblings_to_scrape: returns the other two with their series urls and site ids, ordered by series_id
    let to_scrape = db.siblings_to_scrape(ep_tio1).unwrap();
    assert_eq!(to_scrape.len(), 2);

    let mut expected = vec![
        SiblingToScrape {
            series_id: row_flv,
            site_id: "animeflv".into(),
            url: "https://animeflv.or.at/anime/steel-ball-run".into(),
        },
        SiblingToScrape {
            series_id: row_gogo,
            site_id: "gogoanime".into(),
            url: "https://gogoanime.by/steel-ball-run".into(),
        },
    ];
    expected.sort_by_key(|s| s.series_id);
    assert_eq!(to_scrape, expected);
}

#[test]
fn siblings_to_scrape_omits_sibling_that_already_has_episode() {
    let db = Db::open(":memory:").unwrap();
    let src_a = db.upsert_source("TioAnime", "https://tioanime.com", "tioanime").unwrap();
    let src_b = db.upsert_source("AnimeFLV", "https://animeflv.or.at", "animeflv").unwrap();
    let src_c = db.upsert_source("GogoAnime", "https://gogoanime.by", "gogoanime").unwrap();

    let mk = |slug: &str, title: &str, url: &str| crate::models::Series {
        id: 0, slug: slug.into(), title: title.into(), url: url.into(),
        cover_url: None, is_airing: true, followed: true, next_episode_at: None, site_episode_count: None,
    };

    let row_a = db.upsert_series(src_a, &mk("show-a", "Same Show", "https://tioanime.com/anime/show-a")).unwrap();
    let row_b = db.upsert_series(src_b, &mk("show-b", "Same Show", "https://animeflv.or.at/anime/show-b")).unwrap();
    let row_c = db.upsert_series(src_c, &mk("show-c", "Same Show", "https://gogoanime.by/show-c")).unwrap();

    let (ep_a1, _) = db.insert_episode(&crate::models::Episode {
        id: 0, series_id: row_a, number: "1".into(), title: None,
        url: "https://tioanime.com/ver/show-a-1".into(), released_at: None, seen: false,
        download_path: None, download_status: None,
    }).unwrap();

    // row_b already has episode 1
    let (ep_b1, _) = db.insert_episode(&crate::models::Episode {
        id: 0, series_id: row_b, number: "1".into(), title: None,
        url: "https://animeflv.or.at/ver/show-b-1".into(), released_at: None, seen: false,
        download_path: None, download_status: None,
    }).unwrap();

    // row_c only has episode 2 (so episode 1 is missing on row_c)
    let (_ep_c2, _) = db.insert_episode(&crate::models::Episode {
        id: 0, series_id: row_c, number: "2".into(), title: None,
        url: "https://gogoanime.by/ver/show-c-2".into(), released_at: None, seen: false,
        download_path: None, download_status: None,
    }).unwrap();

    // sibling_episodes finds row_b's ep 1
    let sib_eps = db.sibling_episodes(ep_a1).unwrap();
    assert_eq!(sib_eps.len(), 1);
    assert_eq!(sib_eps[0].episode_id, ep_b1);

    // siblings_to_scrape omits row_b because it already has episode 1, but returns row_c
    let to_scrape = db.siblings_to_scrape(ep_a1).unwrap();
    assert_eq!(to_scrape.len(), 1);
    assert_eq!(to_scrape[0].series_id, row_c);
    assert_eq!(to_scrape[0].site_id, "gogoanime");
    assert_eq!(to_scrape[0].url, "https://gogoanime.by/show-c");
}

#[test]
fn siblings_to_scrape_rejects_different_title_and_unmatched_anilist() {
    let db = Db::open(":memory:").unwrap();
    let src_a = db.upsert_source("AnimeYT", "https://animeyt.cc", "animeytx").unwrap();
    let src_b = db.upsert_source("TioAnime", "https://tioanime.com", "tioanime").unwrap();

    // Seed catalog row with title "Bridged Title"
    db.conn.execute(
        "INSERT INTO anilist_catalog (id, title, url, sort_order) VALUES (?1, ?2, ?3, ?4)",
        rusqlite::params![500, "Bridged Title", "https://anilist.co/anime/500", 0],
    ).unwrap();

    let mk = |slug: &str, title: &str, url: &str| crate::models::Series {
        id: 0, slug: slug.into(), title: title.into(), url: url.into(),
        cover_url: None, is_airing: true, followed: true, next_episode_at: None, site_episode_count: None,
    };

    // row_a is unlinked (anilist_id is NULL) but title matches catalog row
    let row_a = db.upsert_series(src_a, &mk("show-a", "Bridged Title", "https://animeyt.cc/tv/show-a")).unwrap();
    // row_b is linked to AniList 500 but has a completely different title
    let row_b = db.upsert_series(src_b, &mk("show-b", "Completely Different Title", "https://tioanime.com/anime/show-b")).unwrap();
    db.set_anilist_id(row_b, 500).unwrap();

    // canonical_series_ids bridges them via catalog
    let group = db.canonical_series_ids(row_a).unwrap();
    assert!(group.contains(&row_a) && group.contains(&row_b));

    let (ep_a1, _) = db.insert_episode(&crate::models::Episode {
        id: 0, series_id: row_a, number: "1".into(), title: None,
        url: "https://animeyt.cc/tv/show-a/1".into(), released_at: None, seen: false,
        download_path: None, download_status: None,
    }).unwrap();

    // Neither sibling_episodes nor siblings_to_scrape should accept row_b
    // because row_a's anilist_id is NULL (so not same non-null anilist) and titles differ
    let to_scrape = db.siblings_to_scrape(ep_a1).unwrap();
    assert!(to_scrape.is_empty(), "different titles and unmatched anilist_id must be rejected");
}

#[test]
fn siblings_to_scrape_no_canonical_number_returns_empty() {
    let db = Db::open(":memory:").unwrap();
    let now = chrono::Utc::now().timestamp();
    let src_a = db.upsert_source("AnimeYT", "https://animeyt.cc", "animeytx").unwrap();
    let src_b = db.upsert_source("TioAnime", "https://tioanime.com", "tioanime").unwrap();

    let entries = vec![
        crate::anizip::MapEntry { episode: 1, season: 3, absolute: Some(25), air_at: Some(now - 14 * 86400) },
        crate::anizip::MapEntry { episode: 2, season: 3, absolute: Some(26), air_at: Some(now - 7 * 86400) },
        crate::anizip::MapEntry { episode: 3, season: 3, absolute: Some(27), air_at: Some(now) },
    ];
    db.replace_episode_map(185756, &entries, now).unwrap();

    let mk = |slug: &str, title: &str, url: &str| crate::models::Series {
        id: 0, slug: slug.into(), title: title.into(), url: url.into(),
        cover_url: None, is_airing: true, followed: true, next_episode_at: None, site_episode_count: None,
    };

    let row_a = db.upsert_series(src_a, &mk("tensei-a", "Tensei", "https://animeyt.cc/tv/tensei")).unwrap();
    db.set_anilist_id(row_a, 185756).unwrap();

    let row_b = db.upsert_series(src_b, &mk("tensei-b", "Tensei", "https://tioanime.com/anime/tensei")).unwrap();
    db.set_anilist_id(row_b, 185756).unwrap();

    // Episode 20 on TioAnime is an earlier season (<= offset 24)
    let (ep_b20_id, _) = db.insert_episode(&crate::models::Episode {
        id: 0, series_id: row_b, number: "20".into(), title: None,
        url: "https://tioanime.com/ver/tensei-20".into(), released_at: None, seen: false,
        download_path: None, download_status: None,
    }).unwrap();

    // Since ep 20 has no canonical alignment in season 3 (canonical number is None),
    // siblings_to_scrape must return empty
    let to_scrape = db.siblings_to_scrape(ep_b20_id).unwrap();
    assert!(to_scrape.is_empty());
}

#[test]
fn siblings_to_scrape_in_matches_siblings_to_scrape() {
    let db = Db::open(":memory:").unwrap();
    let src_a = db.upsert_source("AnimeYT", "https://animeyt.cc", "animeytx").unwrap();
    let src_b = db.upsert_source("TioAnime", "https://tioanime.com", "tioanime").unwrap();

    let mk = |slug: &str, title: &str, url: &str| crate::models::Series {
        id: 0, slug: slug.into(), title: title.into(), url: url.into(),
        cover_url: None, is_airing: true, followed: true, next_episode_at: None, site_episode_count: None,
    };

    let row_a = db.upsert_series(src_a, &mk("show-a", "Same Title", "https://animeyt.cc/tv/show-a")).unwrap();
    let row_b = db.upsert_series(src_b, &mk("show-b", "Same Title", "https://tioanime.com/anime/show-b")).unwrap();

    let (ep_a1, _) = db.insert_episode(&crate::models::Episode {
        id: 0, series_id: row_a, number: "1".into(), title: None,
        url: "https://animeyt.cc/tv/show-a/1".into(), released_at: None, seen: false,
        download_path: None, download_status: None,
    }).unwrap();

    let sids = db.canonical_series_ids(row_a).unwrap();
    let to_scrape_in = db.siblings_to_scrape_in(ep_a1, &sids).unwrap();
    let to_scrape_direct = db.siblings_to_scrape(ep_a1).unwrap();
    assert_eq!(to_scrape_in, to_scrape_direct);
    assert_eq!(to_scrape_in.len(), 1);
    assert_eq!(to_scrape_in[0].series_id, row_b);
    assert_eq!(to_scrape_in[0].site_id, "tioanime");
    assert_eq!(to_scrape_in[0].url, "https://tioanime.com/anime/show-b");
}

#[test]
fn download_search_context_resolves_titles_and_group_and_cumulative_none() {
    let db = Db::open(":memory:").unwrap();
    let src_a = db.upsert_source("AnimeYT", "https://animeyt.cc", "animeytx").unwrap();
    let src_b = db.upsert_source("TioAnime", "https://tioanime.com", "tioanime").unwrap();
    let src_c = db.upsert_source("GogoAnime", "https://anitaku.to", "gogoanime").unwrap();

    let now = chrono::Utc::now().timestamp();
    let aid = 12345;
    db.conn.execute(
        "INSERT INTO anilist_catalog (id, title, title_romaji, title_english, url, sort_order)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        rusqlite::params![
            aid,
            "JoJo English Title",
            "JoJo no Kimyou na Bouken",
            "JoJo's Bizarre Adventure",
            "https://anilist.co/anime/12345",
            1
        ],
    ).unwrap();

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

    let row_a = db.upsert_series(src_a, &mk("jojo-a", "JoJo AnimeYT")).unwrap();
    let row_b = db.upsert_series(src_b, &mk("jojo-b", "JoJo TioAnime")).unwrap();
    let row_c = db.upsert_series(src_c, &mk("jojo-c", "JoJo GogoAnime")).unwrap();

    db.set_anilist_id(row_a, aid).unwrap();
    db.set_anilist_id(row_b, aid).unwrap();
    db.set_anilist_id(row_c, aid).unwrap();

    let (ep_a1, _) = db.insert_episode(&crate::models::Episode {
        id: 0,
        series_id: row_a,
        number: "1".into(),
        title: None,
        url: "https://animeyt.cc/tv/jojo/1".into(),
        released_at: None,
        seen: false,
        download_path: None,
        download_status: None,
    }).unwrap();

    let ctx = db.download_search_context(ep_a1).unwrap().expect("context found");
    assert_eq!(ctx.canonical_number, 1.0);
    assert_eq!(
        ctx.titles,
        ("JoJo no Kimyou na Bouken".to_string(), Some("JoJo's Bizarre Adventure".to_string()))
    );
    let mut expected_sites = vec!["animeytx".to_string(), "tioanime".to_string(), "gogoanime".to_string()];
    expected_sites.sort();
    assert_eq!(ctx.group_site_ids, expected_sites);

    // Cumulative earlier-season number -> None
    let entries = vec![
        crate::anizip::MapEntry { episode: 1, season: 3, absolute: Some(25), air_at: Some(now - 14 * 86400) },
        crate::anizip::MapEntry { episode: 2, season: 3, absolute: Some(26), air_at: Some(now - 7 * 86400) },
    ];
    let tensei_aid = 185756;
    db.replace_episode_map(tensei_aid, &entries, now).unwrap();

    let row_tensei = db.upsert_series(src_b, &mk("tensei-cumul", "Tensei")).unwrap();
    db.set_anilist_id(row_tensei, tensei_aid).unwrap();
    let (ep_cumul_20, _) = db.insert_episode(&crate::models::Episode {
        id: 0,
        series_id: row_tensei,
        number: "20".into(),
        title: None,
        url: "https://tioanime.com/ver/tensei-20".into(),
        released_at: None,
        seen: false,
        download_path: None,
        download_status: None,
    }).unwrap();

    let ctx_earlier = db.download_search_context(ep_cumul_20).unwrap();
    assert!(ctx_earlier.is_none());
}

#[test]
fn series_downloaded_paths_and_canonical_paths() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("TioAnime", "https://tioanime.com", "tioanime").unwrap();
    let sid = db.upsert_series(src, &crate::models::Series {
        id: 0,
        slug: "test-dl".into(),
        title: "Test DL".into(),
        url: "https://tioanime.com/anime/test-dl".into(),
        cover_url: None,
        is_airing: false,
        followed: true,
        next_episode_at: None,
        site_episode_count: None,
    }).unwrap();

    let (ep1, _) = db.insert_episode(&crate::models::Episode {
        id: 0, series_id: sid, number: "1".into(), title: None,
        url: "https://tioanime.com/ver/test-1".into(), released_at: None, seen: false,
        download_path: None, download_status: None,
    }).unwrap();
    let (ep2, _) = db.insert_episode(&crate::models::Episode {
        id: 0, series_id: sid, number: "2".into(), title: None,
        url: "https://tioanime.com/ver/test-2".into(), released_at: None, seen: false,
        download_path: None, download_status: None,
    }).unwrap();

    let path1 = "/tmp/fake_dl_1.mp4";
    let path2 = "/tmp/fake_dl_2.mp4";
    db.set_download_status(ep1, "downloaded", Some(path1)).unwrap();
    db.set_download_status(ep2, "downloading", Some(path2)).unwrap();

    let paths = db.series_downloaded_paths(sid).unwrap();
    assert_eq!(paths.len(), 2);
    assert!(paths.contains(&path1.to_string()));
    assert!(paths.contains(&path2.to_string()));

    let canon_set = db.downloaded_episode_canonical_paths().unwrap();
    assert_eq!(canon_set.len(), 1);
    assert!(canon_set.contains(std::path::Path::new(path1)));
}
#[test]
fn clear_all_downloads_only_removes_files_inside_downloads_dir() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
    let sid = db.upsert_series(src, &mk_airing("a", "A", None)).unwrap();

    let (ep_inside, _) = db.insert_episode(&crate::models::Episode { id: 0, series_id: sid, number: "1".into(), title: None, url: "u1".into(), released_at: None, seen: false, download_path: None, download_status: None }).unwrap();
    let (ep_outside, _) = db.insert_episode(&crate::models::Episode { id: 0, series_id: sid, number: "2".into(), title: None, url: "u2".into(), released_at: None, seen: false, download_path: None, download_status: None }).unwrap();

    let temp_root = std::env::temp_dir().join(format!("aot_clear_all_test_{}_{}", std::process::id(), sid));
    let downloads_dir = temp_root.join("downloads");
    let outside_dir = temp_root.join("outside");
    std::fs::create_dir_all(&downloads_dir).unwrap();
    std::fs::create_dir_all(&outside_dir).unwrap();

    let inside_file = downloads_dir.join(format!("{}.mp4", ep_inside));
    let outside_file = outside_dir.join(format!("{}.mp4", ep_outside));
    std::fs::write(&inside_file, b"inside video bytes").unwrap();
    std::fs::write(&outside_file, b"outside video bytes").unwrap();

    db.set_download_status(ep_inside, "downloaded", Some(inside_file.to_str().unwrap())).unwrap();
    db.set_download_status(ep_outside, "downloaded", Some(outside_file.to_str().unwrap())).unwrap();

    // Run clear_all_downloads
    let cleaned = db.clear_all_downloads(&downloads_dir).unwrap();
    assert_eq!(cleaned, 1);

    // Inside file is deleted and its DB row cleared
    assert!(!inside_file.exists());
    let ep_in = db.get_episode(ep_inside).unwrap();
    assert_eq!(ep_in.download_path, None);
    assert_eq!(ep_in.download_status, None);

    // Outside file is NOT deleted and its DB row is left untouched
    assert!(outside_file.exists());
    let ep_out = db.get_episode(ep_outside).unwrap();
    assert_eq!(ep_out.download_path.as_deref(), Some(outside_file.to_str().unwrap()));
    assert_eq!(ep_out.download_status.as_deref(), Some("downloaded"));

    // Clean up
    let _ = std::fs::remove_dir_all(&temp_root);
}
