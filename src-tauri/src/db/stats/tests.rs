use super::*;
use crate::db::test_support::*;

#[test]
fn franchise_display_title_strips_season_markers_keeping_case() {
    assert_eq!(franchise_display_title("Kimetsu no Yaiba Temporada 2"), "Kimetsu no Yaiba");
    assert_eq!(franchise_display_title("Overlord IV"), "Overlord");
    assert_eq!(franchise_display_title("86 Eighty-Six Part 2"), "86 Eighty-Six");
    // Accents and internal punctuation survive — this is display text.
    assert_eq!(franchise_display_title("Shingeki no Kyojin: The Final Season"), "Shingeki no Kyojin:");
    // Nothing to strip: returned untouched.
    assert_eq!(franchise_display_title("Bleach"), "Bleach");
}

#[test]
fn franchise_display_title_keeps_a_colon_qualifier() {
    // A label must describe the group the title actually landed in, and a
    // colon title only merges into its base when that base exists in the
    // user's own data (see franchise_parent_key) — which this pure
    // function can't know.
    assert_eq!(franchise_display_title("One Piece: Arco de Elbaph"), "One Piece: Arco de Elbaph");
    assert_eq!(franchise_display_title("Re:Zero kara Hajimeru Isekai Seikatsu"), "Re:Zero kara Hajimeru Isekai Seikatsu");
}

#[test]
fn franchise_display_title_never_returns_empty() {
    assert_eq!(franchise_display_title("Temporada 2"), "Temporada 2");
    assert_eq!(franchise_display_title(": only a subtitle"), ": only a subtitle");
}

#[test]
fn franchise_base_title_drops_the_colon_qualifier_for_catalog_lookup() {
    assert_eq!(franchise_base_title("One Piece: Arco de Elbaph"), "One Piece");
    assert_eq!(franchise_base_title("One Piece: Live Action (2023) Temporada 2"), "One Piece");
    assert_eq!(franchise_base_title("Fate/stay night: Unlimited Blade Works"), "Fate/stay night");
    assert_eq!(franchise_base_title("Bleach"), "Bleach");
}

#[test]
fn franchise_key_does_not_merge_shows_that_merely_share_a_colon_head() {
    // Six unrelated shows previously collapsed into a single franchise
    // called "Re" because every colon was treated as an arc separator.
    let re_titles = [
        "Re:Zero kara Hajimeru Isekai Seikatsu",
        "Re:CREATORS",
        "Re:Monster",
        "Re: Hamatora",
        "Re:Stage! Dream Days",
    ];
    let keys: HashSet<String> = re_titles.iter().map(|t| franchise_key(t)).collect();
    assert_eq!(keys.len(), re_titles.len(), "each Re: show keeps its own key: {keys:?}");
    assert!(!keys.contains("re"), "nothing collapses to the bare colon head");
    assert_ne!(franchise_key("Code:Breaker"), franchise_key("Code: Realize ~Guardian of Rebirth~"));
}

#[test]
fn franchise_key_keeps_a_title_that_merely_ends_in_a_digit_distinct() {
    // "Steins;Gate 0" is a different work with its own AniList entry, not
    // a second season of "Steins;Gate".
    assert_ne!(franchise_key("Steins;Gate"), franchise_key("Steins;Gate 0"));
    // A digit introduced by a season marker still strips.
    assert_eq!(franchise_key("Kimetsu no Yaiba Temporada 2"), franchise_key("Kimetsu no Yaiba"));
    assert_eq!(franchise_key("Overlord IV"), franchise_key("Overlord"));
}

/// Regression guard for the char-boundary panic this bug caused in
/// `strip_season_markers` (a byte-for-byte duplicate of matching.rs'
/// `is_ordinal`, now fixed identically). Every title comes from the user's
/// live database and carries a multi-byte Unicode char (Roman numeral Ⅱ,
/// Cyrillic о, fraction ⅐, superscript ⁿ, fullwidth Ａ) that byte-index
/// slicing would have split on a non-char boundary and panicked — in a
/// main-thread `#[tauri::command]` that aborts the whole process
/// (FAST_FAIL_FATAL_APP_EXIT). `franchise_key`/`franchise_display_title`
/// feed raw site titles through this path.
/// The multi-byte char has to be in the **trailing** token: this loop reads
/// `tokens.last()` and breaks at the first non-marker, so a multi-byte char
/// in a leading token never reaches `is_ordinal` and never panicked — a
/// test built on one of those would pass against the buggy code too.
///
/// Second field is the ASCII core that must survive, asserted per title so
/// one entry can't satisfy the check on behalf of the other five.
const MULTIBYTE_TITLES: &[(&str, &str)] = &[
    // Real catalog rows: Ⅱ = U+2161 lowercases to ⅱ (U+2171), a trailing
    // token that is a single 3-byte char.
    ("Long Sword Ⅱ", "sword"),
    ("The Morose Mononokean Ⅱ", "mononokean"),
    ("Zhu Dick: Guguai Dao Da Maoxian Ⅰ", "maoxian"),
    // Other scripts from the catalog, moved into trailing position so they
    // exercise the loop: fullwidth Ａ, superscript ⁿ, fraction ⅐.
    ("Galaxy Angel Ａ", "angel"),
    ("Kuusou Episodeⁿ", "kuusou"),
    ("Tom Thumb 00⅐", "thumb"),
];

#[test]
fn multibyte_titles_do_not_panic_in_franchise_key() {
    for (title, core) in MULTIBYTE_TITLES {
        // Not panicking IS the assertion — the old byte-slicing version
        // aborted the process on every one of these. The multi-byte chars
        // are in none of the ordinal/ROMANS/marker lists, so they survive
        // intact: the fix removes a panic, it invents no stripping rule.
        let key = franchise_key(title);
        assert!(
            key.contains(core),
            "expected {core:?} to survive in {title:?} -> {key:?}"
        );
    }
}

#[test]
fn multibyte_titles_do_not_panic_in_franchise_display_title() {
    for (title, core) in MULTIBYTE_TITLES {
        let display = franchise_display_title(title);
        assert!(
            display.to_lowercase().contains(core),
            "expected {core:?} to survive in {title:?} -> {display:?}"
        );
        // Unlike franchise_key, the display path keeps the original casing.
        assert!(
            display.chars().any(|c| c.is_uppercase()),
            "display title lost its casing: {title:?} -> {display:?}"
        );
    }
}

#[test]
fn franchise_parent_key_only_offers_a_parent_for_a_real_qualifier() {
    assert_eq!(franchise_parent_key("One Piece: Arco de Elbaph").as_deref(), Some("one piece"));
    // No colon at all.
    assert_eq!(franchise_parent_key("Bleach"), None);
    // Colon with an empty head can't yield a parent.
    assert_eq!(franchise_parent_key(": subtitle only"), None);
    // A parent identical to the key itself is not a merge candidate.
    assert_eq!(franchise_parent_key("Overlord: Temporada 2"), None);
}

#[test]
fn arc_rows_only_merge_when_the_base_show_is_in_the_users_own_data() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("A", "a", "animeytx").unwrap();

    // Two colon-qualified titles. The user owns a plain "One Piece" row,
    // so its arc merges; nobody owns a series called "Re", so Re:Zero
    // stays its own franchise.
    for (slug, title) in [
        ("op", "One Piece"),
        ("op-elbaph", "One Piece: Arco de Elbaph"),
        ("rezero", "Re:Zero kara Hajimeru Isekai Seikatsu"),
    ] {
        let id = db.upsert_series(src, &mk_airing(slug, title, None)).unwrap();
        db.set_kind(id, "TV").unwrap();
        db.insert_episode(&crate::models::Episode {
            id: 0, series_id: id, number: "1".into(), title: None,
            url: format!("https://site/anime/{slug}-capitulo-1/"),
            released_at: None, seen: false, download_path: None, download_status: None,
        }).unwrap();
        db.set_seen_cascade(id, "1", true).unwrap();
    }

    let insights = db.get_watch_insights().unwrap();
    let titles: HashSet<&str> = insights.top_series.iter().map(|t| t.title.as_str()).collect();
    assert_eq!(insights.top_series.len(), 2, "One Piece merged, Re:Zero did not: {titles:?}");
    assert!(titles.contains("One Piece"));
    assert!(titles.contains("Re:Zero kara Hajimeru Isekai Seikatsu"));
    let one_piece = insights.top_series.iter().find(|t| t.title == "One Piece").unwrap();
    assert_eq!(one_piece.count, 2, "the arc's episode folded into the base show");
}

#[test]
fn top_series_reports_the_franchise_name_and_the_whole_franchise_count() {
    // The real-world One Piece shape: the site lists per-arc rows, each
    // with its own scraped episodes, and the user separately swiped "Ya lo
    // vi" on the AniList entry for the whole show.
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("A", "a", "animeytx").unwrap();

    let arc = db.upsert_series(src, &mk_airing("op-elbaph", "One Piece: Arco de Elbaph", None)).unwrap();
    db.set_followed(arc, true).unwrap();
    db.set_kind(arc, "TV").unwrap();
    for n in 1..=20 {
        db.insert_episode(&crate::models::Episode {
            id: 0, series_id: arc, number: n.to_string(), title: None,
            url: format!("https://site/anime/op-elbaph-capitulo-{n}/"),
            released_at: None, seen: false, download_path: None, download_status: None,
        }).unwrap();
    }
    db.set_seen_cascade(arc, "20", true).unwrap();

    let season2 = db.upsert_series(src, &mk_airing("op-t2", "One Piece Temporada 2", None)).unwrap();
    db.set_kind(season2, "TV").unwrap();
    for n in 1..=5 {
        db.insert_episode(&crate::models::Episode {
            id: 0, series_id: season2, number: n.to_string(), title: None,
            url: format!("https://site/anime/op-t2-capitulo-{n}/"),
            released_at: None, seen: false, download_path: None, download_status: None,
        }).unwrap();
    }
    db.set_seen_cascade(season2, "5", true).unwrap();

    db.upsert_catalog_anime(
        &crate::anilist::CatalogAnime {
            id: 21, title: "ONE PIECE".into(), title_romaji: None, title_english: None, synonyms: Vec::new(), next_airing_at: None, next_episode: None, tags: Vec::new(), relations: Vec::new(),
            cover_url: None, banner_url: None, format: Some("TV".into()), genres: vec![],
            episodes: Some(1140), average_score: None, popularity: None,
            url: "https://anilist.co/anime/21".into(), status: None, duration: Some(24),
            studio: None, start_date: None,
        },
        0,
    ).unwrap();
    let whole = db.upsert_series(src, &mk_airing("one-piece", "ONE PIECE", None)).unwrap();
    db.set_watched_externally(whole, true).unwrap();
    db.set_anilist_id(whole, 21).unwrap();

    let insights = db.get_watch_insights().unwrap();
    let top = insights.top_series.first().expect("one franchise expected");
    assert_eq!(top.title, "One Piece", "labelled with the franchise, not the arc");
    assert_eq!(top.count, 1140, "credited with the whole show, not one arc's 20 episodes");
    assert_eq!(insights.top_series.len(), 1, "all three rows are one franchise");
}

#[test]
fn franchise_external_estimate_never_double_counts_real_marks() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("A", "a", "animeytx").unwrap();

    db.upsert_catalog_anime(
        &crate::anilist::CatalogAnime {
            id: 21, title: "ONE PIECE".into(), title_romaji: None, title_english: None, synonyms: Vec::new(), next_airing_at: None, next_episode: None, tags: Vec::new(), relations: Vec::new(),
            cover_url: None, banner_url: None, format: Some("TV".into()), genres: vec![],
            episodes: Some(100), average_score: None, popularity: None,
            url: "https://anilist.co/anime/21".into(), status: None, duration: Some(24),
            studio: None, start_date: None,
        },
        0,
    ).unwrap();

    let arc = db.upsert_series(src, &mk_airing("op-arc", "One Piece: Arco", None)).unwrap();
    db.set_kind(arc, "TV").unwrap();
    for n in 1..=30 {
        db.insert_episode(&crate::models::Episode {
            id: 0, series_id: arc, number: n.to_string(), title: None,
            url: format!("https://site/anime/op-arc-capitulo-{n}/"),
            released_at: None, seen: false, download_path: None, download_status: None,
        }).unwrap();
    }
    db.set_seen_cascade(arc, "30", true).unwrap();

    let whole = db.upsert_series(src, &mk_airing("one-piece", "One Piece", None)).unwrap();
    db.set_watched_externally(whole, true).unwrap();
    db.set_anilist_id(whole, 21).unwrap();

    let summary = db.get_watch_summary().unwrap();
    assert_eq!(summary.episodes_watched, 30, "real marks stay exactly as marked");
    assert_eq!(
        summary.episodes_watched_external, 70,
        "only the 100-30 remainder is credited on top, so the two add up to 100 not 130"
    );

    let insights = db.get_watch_insights().unwrap();
    assert_eq!(insights.estimated_minutes_tracked, 30 * 24);
    assert_eq!(insights.estimated_minutes_external, 70 * 24, "remainder minutes only");
    assert_eq!(insights.external_titles_estimated, 1);
    assert_eq!(insights.external_titles_total, 1, "counted per franchise, not per row");
}

#[test]
fn franchise_key_keeps_distinct_shows_distinct_and_never_empty() {
    assert_ne!(franchise_key("Naruto"), franchise_key("Bleach"));
    // A title that is *only* a season marker must not collapse to "".
    assert!(!franchise_key("Season 2").is_empty());
    assert!(!franchise_key("IV").is_empty());
}

/// A title that is entirely punctuation/noise normalizes to "" even
/// before season-marker stripping — the key must not also be "" (two
/// such garbage titles would otherwise silently share one franchise).
#[test]
fn franchise_key_keeps_two_all_punctuation_titles_distinct() {
    assert!(!franchise_key("!!!").is_empty());
    assert!(!franchise_key("???").is_empty());
    assert_ne!(franchise_key("!!!"), franchise_key("???"));
}

#[test]
fn get_watch_summary_counts_distinct_animes_collapsing_seasons() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("A", "a", "animeytx").unwrap();

    // Three followed rows that are the same franchise (S1/S2/S3), each
    // with at least one real seen episode — distinct_anime now requires
    // watch evidence (see the episode/anime-counts-fix design doc), not
    // just `followed=1`.
    for (slug, title) in [
        ("slime1", "Tensei shitara Slime Datta Ken"),
        ("slime2", "Tensei shitara Slime Datta Ken Temporada 2"),
        ("slime3", "Tensei shitara Slime Datta Ken Temporada 3"),
    ] {
        let id = db.upsert_series(src, &mk_airing(slug, title, None)).unwrap();
        db.set_followed(id, true).unwrap();
        insert_eps_seen_up_to(&db, id, 1, 1);
    }
    // A different followed franchise, also with a seen episode.
    let n = db.upsert_series(src, &mk_airing("naruto", "Naruto", None)).unwrap();
    db.set_followed(n, true).unwrap();
    insert_eps_seen_up_to(&db, n, 1, 1);
    // A watched-externally-only franchise (counts too, even if not followed).
    let w = db.upsert_series(src, &mk_airing("frieren", "Frieren", None)).unwrap();
    db.set_watched_externally(w, true).unwrap();

    let summary = db.get_watch_summary().unwrap();
    assert_eq!(summary.followed_series, 4, "4 followed rows");
    assert_eq!(summary.distinct_anime, 3, "Slime(x3 seasons)=1, Naruto=1, Frieren=1");
}

// ---- Stats stale refresh + metric concepts (2026-07-13) ----

#[test]
fn get_watch_summary_computes_airing_followed_and_pending_to_watch() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("A", "a", "animeytx").unwrap();

    // Followed + still airing + one unseen episode: counts toward both
    // airing_followed and pending_to_watch.
    let mut airing = mk_airing("airing1", "Airing Show", None);
    airing.is_airing = true;
    let airing_id = db.upsert_series(src, &airing).unwrap();
    db.set_followed(airing_id, true).unwrap();
    db.insert_episode(&crate::models::Episode {
        id: 0,
        series_id: airing_id,
        number: "1".into(),
        title: None,
        url: "https://site/anime/airing1-capitulo-1/".into(),
        released_at: None,
        seen: false, download_path: None, download_status: None,
    })
    .unwrap();

    // Followed + finished + fully seen: counts toward neither.
    let mut finished = mk_airing("finished1", "Finished Show", None);
    finished.is_airing = false;
    let finished_id = db.upsert_series(src, &finished).unwrap();
    db.set_followed(finished_id, true).unwrap();
    db.insert_episode(&crate::models::Episode {
        id: 0,
        series_id: finished_id,
        number: "1".into(),
        title: None,
        url: "https://site/anime/finished1-capitulo-1/".into(),
        released_at: None,
        seen: true, download_path: None, download_status: None,
    })
    .unwrap();

    // Want-only (never followed): counts toward backlog_want only.
    let want = mk_airing("want1", "Want Show", None);
    let want_id = db.upsert_series(src, &want).unwrap();
    db.set_backlog_status(want_id, Some("want")).unwrap();

    let summary = db.get_watch_summary().unwrap();
    assert_eq!(summary.airing_followed, 1, "only the airing+followed row counts");
    assert_eq!(summary.pending_to_watch, 1, "only the followed row with an unseen episode counts");
    assert_eq!(summary.backlog_want, 1, "only the want-only row counts");
}

// ---- Stats new metrics: minutes_per_episode + get_watch_insights (2026-07-13) ----

#[test]
fn minutes_per_episode_maps_each_format_branch() {
    assert_eq!(minutes_per_episode(Some("MOVIE")), 100);
    assert_eq!(minutes_per_episode(Some("movie")), 100, "case-insensitive");
    assert_eq!(minutes_per_episode(Some("Pelicula")), 100);
    assert_eq!(minutes_per_episode(Some("Película")), 100, "accented site vocabulary");
    assert_eq!(minutes_per_episode(Some("MUSIC")), 5);
    assert_eq!(minutes_per_episode(Some("TV_SHORT")), 8);
    assert_eq!(minutes_per_episode(Some("OVA")), 26);
    assert_eq!(minutes_per_episode(Some("SPECIAL")), 26);
    assert_eq!(minutes_per_episode(Some("TV")), 24);
    assert_eq!(minutes_per_episode(Some("ONA")), 24);
}

#[test]
fn minutes_per_episode_defaults_to_24_for_real_site_noise_and_none() {
    // Real dirty `series.kind` vocabulary observed live on 2026-07-13
    // (see the design doc) must not crash or misclassify — it all falls
    // back to the plain-TV estimate.
    for noisy in ["4K", "Blu-Ray", "Resubido", "Sin Censura", "Yaoi"] {
        assert_eq!(minutes_per_episode(Some(noisy)), 24, "noisy kind: {noisy}");
    }
    assert_eq!(minutes_per_episode(None), 24, "no kind at all");
}

#[test]
fn get_watch_insights_estimates_minutes_from_seeded_db() {
    // Mirrors the design doc's acceptance test: a followed series with 3
    // seen episodes (kind=TV -> 24 min/ep = 72) plus a linked "Ya vista"
    // whose catalog row has 12 episodes at format TV (24 min/ep = 288).
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("A", "a", "animeytx").unwrap();

    // Followed, airing, TV, 4 episodes numbered 1..4, cascade-mark 1..3 seen.
    let mut airing = mk_airing("airing1", "Airing Show", None);
    airing.is_airing = true;
    let airing_id = db.upsert_series(src, &airing).unwrap();
    db.set_followed(airing_id, true).unwrap();
    db.set_kind(airing_id, "TV").unwrap();
    for n in 1..=4 {
        db.insert_episode(&crate::models::Episode {
            id: 0, series_id: airing_id, number: n.to_string(), title: None,
            url: format!("https://site/anime/airing1-capitulo-{n}/"),
            released_at: None, seen: false, download_path: None, download_status: None,
        }).unwrap();
    }
    db.set_seen_cascade(airing_id, "3", true).unwrap();

    // Followed, finished, MOVIE, 2 episodes already seen (no seen_at —
    // simulates pre-existing data from before the column existed).
    let mut finished = mk_airing("finished1", "Finished Show", None);
    finished.is_airing = false;
    let finished_id = db.upsert_series(src, &finished).unwrap();
    db.set_followed(finished_id, true).unwrap();
    db.set_kind(finished_id, "MOVIE").unwrap();
    for n in 1..=2 {
        db.insert_episode(&crate::models::Episode {
            id: 0, series_id: finished_id, number: n.to_string(), title: None,
            url: format!("https://site/anime/finished1-capitulo-{n}/"),
            released_at: None, seen: true, download_path: None, download_status: None,
        }).unwrap();
    }

    // Want-only.
    let want_id = db.upsert_series(src, &mk_airing("want1", "Want Show", None)).unwrap();
    db.set_backlog_status(want_id, Some("want")).unwrap();

    // Discarded-only.
    let disc_id = db.upsert_series(src, &mk_airing("disc1", "Discarded Show", None)).unwrap();
    db.set_backlog_status(disc_id, Some("discarded")).unwrap();

    // Watched-externally, linked to a catalog row with 12 TV episodes.
    db.upsert_catalog_anime(
        &crate::anilist::CatalogAnime {
            id: 100, title: "External Show".into(), title_romaji: None, title_english: None, synonyms: Vec::new(), next_airing_at: None, next_episode: None, tags: Vec::new(), relations: Vec::new(),
            cover_url: None, banner_url: None, format: Some("TV".into()), genres: vec![],
            episodes: Some(12), average_score: None, popularity: None,
            url: "https://anilist.co/anime/100".into(), status: None, duration: None,
            studio: None, start_date: None,
        },
        0,
    ).unwrap();
    let ext_id = db.upsert_series(src, &mk_airing("ext1", "External Show", None)).unwrap();
    db.set_watched_externally(ext_id, true).unwrap();
    db.set_anilist_id(ext_id, 100).unwrap();

    // A second watched-externally row, unlinked (no catalog data) — must
    // count toward the total but not toward the estimated minutes.
    let ext2_id = db.upsert_series(src, &mk_airing("ext2", "Unlinked External", None)).unwrap();
    db.set_watched_externally(ext2_id, true).unwrap();

    let insights = db.get_watch_insights().unwrap();

    // Both series are followed (one airing, one finished) — tracked
    // minutes sum across ALL followed series' seen episodes, not just
    // airing ones: 3 TV eps * 24 min + 2 MOVIE eps * 100 min.
    assert_eq!(insights.estimated_minutes_tracked, 272, "3*24 (TV) + 2*100 (MOVIE)");
    assert_eq!(insights.estimated_minutes_external, 288, "12 eps * 24 min (TV)");
    assert_eq!(insights.external_titles_estimated, 1, "only the linked row counted");
    assert_eq!(insights.external_titles_total, 2, "both watched-externally rows");
    assert_eq!(insights.avg_episodes_per_series, 3.0, "(4 + 2) / 2 followed series");
    assert_eq!(insights.followed_airing, 1);
    assert_eq!(insights.followed_finished, 1);
    assert_eq!(insights.want, 1);
    assert_eq!(insights.discarded, 1);
    assert_eq!(insights.watched_externally, 2);
    assert_eq!(
        insights.top_series,
        vec![
            crate::models::TitleCount { title: "External Show".into(), count: 12 },
            crate::models::TitleCount { title: "Airing Show".into(), count: 3 },
            crate::models::TitleCount { title: "Finished Show".into(), count: 2 },
        ],
        "ordered by credited episodes descending; a 'Ya lo vi' title is credited \
         with its catalog episode count, which is the only evidence it has"
    );
    // A full 30-day spine, zero-filled: only today has marks.
    assert_eq!(insights.marks_by_day.len(), 30, "always a contiguous 30-day window");
    assert_eq!(insights.marks_by_day.last().unwrap().count, 3, "today holds the cascade-marked episodes");
    assert_eq!(
        insights.marks_by_day.iter().map(|d| d.count).sum::<i64>(), 3,
        "every other day is an explicit zero, not a missing entry"
    );
    assert!(insights.marks_tracked_since.is_some());
}

#[test]
fn get_watch_insights_matches_design_doc_acceptance_example() {
    // Literal acceptance scenario from
    // docs/superpowers/specs/2026-07-13-stats-new-metrics-design.md:
    // one followed series with 3 seen episodes, plus one linked
    // watched-externally series whose catalog row has 12 TV episodes.
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("A", "a", "animeytx").unwrap();

    let sid = db.upsert_series(src, &mk_airing("s1", "S1", None)).unwrap();
    db.set_followed(sid, true).unwrap();
    db.set_kind(sid, "TV").unwrap();
    for n in 1..=3 {
        db.insert_episode(&crate::models::Episode {
            id: 0, series_id: sid, number: n.to_string(), title: None,
            url: format!("https://site/anime/s1-capitulo-{n}/"),
            released_at: None, seen: true, download_path: None, download_status: None,
        }).unwrap();
    }

    db.upsert_catalog_anime(
        &crate::anilist::CatalogAnime {
            id: 200, title: "S2".into(), title_romaji: None, title_english: None, synonyms: Vec::new(), next_airing_at: None, next_episode: None, tags: Vec::new(), relations: Vec::new(),
            cover_url: None, banner_url: None, format: Some("TV".into()), genres: vec![],
            episodes: Some(12), average_score: None, popularity: None,
            url: "https://anilist.co/anime/200".into(), status: None, duration: None,
            studio: None, start_date: None,
        },
        0,
    ).unwrap();
    let ext_id = db.upsert_series(src, &mk_airing("s2", "S2", None)).unwrap();
    db.set_watched_externally(ext_id, true).unwrap();
    db.set_anilist_id(ext_id, 200).unwrap();

    let insights = db.get_watch_insights().unwrap();
    assert_eq!(insights.estimated_minutes_tracked, 72);
    assert_eq!(insights.estimated_minutes_external, 288);
}

#[test]
fn get_watch_insights_uses_real_duration_when_synced() {
    // Same shape as the design-doc acceptance test, but the linked
    // catalog row has a real synced `duration` of 23 min/ep — that must
    // win over the format-based estimate (TV -> 24 min/ep, which would
    // give 288). 23 * 12 = 276.
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("A", "a", "animeytx").unwrap();

    db.upsert_catalog_anime(
        &crate::anilist::CatalogAnime {
            id: 201, title: "S3".into(), title_romaji: None, title_english: None, synonyms: Vec::new(), next_airing_at: None, next_episode: None, tags: Vec::new(), relations: Vec::new(),
            cover_url: None, banner_url: None, format: Some("TV".into()), genres: vec![],
            episodes: Some(12), average_score: None, popularity: None,
            url: "https://anilist.co/anime/201".into(), status: None, duration: Some(23),
            studio: None, start_date: None,
        },
        0,
    ).unwrap();
    let ext_id = db.upsert_series(src, &mk_airing("s3", "S3", None)).unwrap();
    db.set_watched_externally(ext_id, true).unwrap();
    db.set_anilist_id(ext_id, 201).unwrap();

    let insights = db.get_watch_insights().unwrap();
    assert_eq!(insights.estimated_minutes_external, 276, "23 min/ep * 12 eps beats the 24 min/ep TV estimate");
}

#[test]
fn get_watch_insights_falls_back_to_estimate_when_duration_is_null() {
    // Same shape, but `duration: None` (unsynced/no-duration catalog row)
    // — must fall back to today's format-based estimate: 24 * 12 = 288.
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("A", "a", "animeytx").unwrap();

    db.upsert_catalog_anime(
        &crate::anilist::CatalogAnime {
            id: 202, title: "S4".into(), title_romaji: None, title_english: None, synonyms: Vec::new(), next_airing_at: None, next_episode: None, tags: Vec::new(), relations: Vec::new(),
            cover_url: None, banner_url: None, format: Some("TV".into()), genres: vec![],
            episodes: Some(12), average_score: None, popularity: None,
            url: "https://anilist.co/anime/202".into(), status: None, duration: None,
            studio: None, start_date: None,
        },
        0,
    ).unwrap();
    let ext_id = db.upsert_series(src, &mk_airing("s4", "S4", None)).unwrap();
    db.set_watched_externally(ext_id, true).unwrap();
    db.set_anilist_id(ext_id, 202).unwrap();

    let insights = db.get_watch_insights().unwrap();
    assert_eq!(insights.estimated_minutes_external, 288, "falls back to 24 min/ep (TV) estimate, unchanged");
}

#[test]
fn get_watch_insights_empty_db_has_no_divide_by_zero_and_empty_collections() {
    let db = Db::open(":memory:").unwrap();
    let insights = db.get_watch_insights().unwrap();
    assert_eq!(insights.estimated_minutes_tracked, 0);
    assert_eq!(insights.estimated_minutes_external, 0);
    assert_eq!(insights.estimated_minutes_share, 0.0);
    assert_eq!(insights.external_titles_estimated, 0);
    assert_eq!(insights.external_titles_total, 0);
    assert_eq!(insights.avg_episodes_per_series, 0.0);
    assert!(insights.top_series.is_empty());
    // The day spine is always present and always 30 long, so a chart never
    // has to distinguish "no data yet" from "a gap in the middle".
    assert_eq!(insights.marks_by_day.len(), 30);
    assert!(insights.marks_by_day.iter().all(|d| d.count == 0));
    assert_eq!(insights.marks_tracked_since, None);
}

#[test]
fn top_series_sums_watched_episodes_across_a_franchises_split_arc_rows() {
    // Reproduces the reported bug: the site models a long-runner as
    // several `series` rows by arc, so a naive per-row GROUP BY reported
    // one arc's own count (114) instead of the show's real total.
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("A", "a", "animeytx").unwrap();

    let arcs = [
        ("op1", "One Piece", 500),
        ("op2", "One Piece: Wano", 400),
        ("op3", "One Piece: Arco de Elbaph", 114),
    ];
    for (slug, title, seen_count) in arcs {
        let id = db.upsert_series(src, &mk_airing(slug, title, None)).unwrap();
        db.set_followed(id, true).unwrap();
        insert_eps_seen_up_to(&db, id, seen_count, seen_count);
    }
    // An unrelated series must stay in its own bucket, not get folded in.
    let n = db.upsert_series(src, &mk_airing("naruto", "Naruto", None)).unwrap();
    db.set_followed(n, true).unwrap();
    insert_eps_seen_up_to(&db, n, 50, 50);

    let insights = db.get_watch_insights().unwrap();
    let one_piece = insights.top_series.iter().find(|t| t.title == "One Piece").expect("collapsed One Piece entry");
    assert_eq!(one_piece.count, 1014, "500 + 400 + 114 summed across all arc rows");
    assert!(
        !insights.top_series.iter().any(|t| t.title.contains("Wano") || t.title.contains("Elbaph")),
        "arc-specific rows must not surface as separate top-series entries: {:?}",
        insights.top_series
    );
    let naruto = insights.top_series.iter().find(|t| t.title == "Naruto").expect("Naruto entry");
    assert_eq!(naruto.count, 50);
}

#[test]
fn get_genre_affinity_weighs_followed_want_discarded_correctly() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();

    let followed = crate::models::Series {
        id: 0, slug: "f".into(), title: "F".into(),
        url: "u1".into(), cover_url: None, is_airing: false, followed: true, next_episode_at: None, site_episode_count: None,
    };
    let sid_f = db.upsert_series(src, &followed).unwrap();
    db.set_followed(sid_f, true).unwrap();
    db.insert_series_genres(sid_f, &["Seinen".to_string(), "Shared".to_string()]).unwrap();

    let want = crate::models::Series {
        id: 0, slug: "w".into(), title: "W".into(),
        url: "u2".into(), cover_url: None, is_airing: false, followed: false, next_episode_at: None, site_episode_count: None,
    };
    let sid_w = db.upsert_series(src, &want).unwrap();
    db.set_backlog_status(sid_w, Some("want")).unwrap();
    db.insert_series_genres(sid_w, &["Romance".to_string(), "Shared".to_string()]).unwrap();

    let discarded = crate::models::Series {
        id: 0, slug: "d".into(), title: "D".into(),
        url: "u3".into(), cover_url: None, is_airing: false, followed: false, next_episode_at: None, site_episode_count: None,
    };
    let sid_d = db.upsert_series(src, &discarded).unwrap();
    db.set_backlog_status(sid_d, Some("discarded")).unwrap();
    db.insert_series_genres(sid_d, &["Horror".to_string()]).unwrap();

    let scores = db.get_genre_affinity(src).unwrap();
    assert_eq!(scores.get("Seinen"), Some(&2.0));
    assert_eq!(scores.get("Romance"), Some(&1.0));
    assert_eq!(scores.get("Horror"), Some(&-1.5));
    assert_eq!(scores.get("Shared"), Some(&3.0)); // 2.0 (followed) + 1.0 (want)
    assert_eq!(scores.get("Unseen"), None);
}

#[test]
fn canonical_followed_prefers_a_displayable_cover_over_a_scraped_thumbnail() {
    let db = Db::open(":memory:").unwrap();
    let a = db.upsert_source("A", "https://a", "animeytx").unwrap();
    let b = db.upsert_source("B", "https://b", "tioanime").unwrap();
    // Same unlinked show on two sites: first a blocked scraped URL, then a cached file.
    let mut s1 = mk_airing("x", "Show X", None);
    s1.cover_url = Some("https://wwv.animeytx.net/x.jpg".into());
    s1.followed = true;
    let id1 = db.upsert_series(a, &s1).unwrap();
    db.set_followed(id1, true).unwrap();
    let mut s2 = mk_airing("x2", "Show X", None);
    s2.cover_url = Some("file:C:/covers/x.jpg".into());
    let id2 = db.upsert_series(b, &s2).unwrap();
    db.set_followed(id2, true).unwrap();
    // A linked show: the catalog (AniList) cover beats the scraped one.
    db.upsert_catalog_anime(&crate::db::test_support::catalog_anime(9, "Linked Y", &[]), 0).unwrap();
    let mut s3 = mk_airing("y", "Linked Y", None);
    s3.cover_url = Some("https://wwv.animeytx.net/y.jpg".into());
    let id3 = db.upsert_series(a, &s3).unwrap();
    db.set_anilist_id(id3, 9).unwrap();
    db.set_followed(id3, true).unwrap();

    let groups = db.group_followed_canonically().unwrap();
    let cover = |t: &str| groups.iter().find(|g| g.display_title == t).unwrap().cover_url.clone();
    assert_eq!(cover("Show X").as_deref(), Some("file:C:/covers/x.jpg"));
    assert_eq!(cover("Linked Y").as_deref(), Some("https://cdn/9.jpg"));
}

#[test]
fn get_stats_graph_data_returns_genres_kind_and_cover_for_followed_series() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();

    let full = crate::models::Series {
        id: 0, slug: "full".into(), title: "Full".into(),
        url: "u1".into(), cover_url: Some("data:image/png;base64,x".into()),
        is_airing: false, followed: true, next_episode_at: None, site_episode_count: None,
    };
    let sid_full = db.upsert_series(src, &full).unwrap();
    db.set_followed(sid_full, true).unwrap();
    db.insert_series_genres(sid_full, &["Seinen".to_string(), "Drama".to_string()]).unwrap();
    db.set_kind(sid_full, "TV").unwrap();

    let bare = crate::models::Series {
        id: 0, slug: "bare".into(), title: "Bare".into(),
        url: "u2".into(), cover_url: None, is_airing: false, followed: true, next_episode_at: None, site_episode_count: None,
    };
    let sid_bare = db.upsert_series(src, &bare).unwrap();
    db.set_followed(sid_bare, true).unwrap();

    // not followed => excluded
    let other = crate::models::Series {
        id: 0, slug: "other".into(), title: "Other".into(),
        url: "u3".into(), cover_url: None, is_airing: false, followed: false, next_episode_at: None, site_episode_count: None,
    };
    db.upsert_series(src, &other).unwrap();

    let rows = db.get_stats_graph_data().unwrap();
    assert_eq!(rows.len(), 2);

    let full_row = rows.iter().find(|r| r.id == sid_full).unwrap();
    assert_eq!(full_row.genres, vec!["Drama".to_string()], "site-only \"Seinen\" is not a genre");
    assert_eq!(full_row.kind.as_deref(), Some("TV"));
    assert_eq!(full_row.cover_url.as_deref(), Some("data:image/png;base64,x"));

    let bare_row = rows.iter().find(|r| r.id == sid_bare).unwrap();
    assert!(bare_row.genres.is_empty());
    assert_eq!(bare_row.kind, None);
    assert_eq!(bare_row.cover_url, None);
}

#[test]
fn get_genre_affinity_folds_spanish_and_english_genre_into_one_canonical_key() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();

    // A followed series tagged with the site's Spanish genre "Acción".
    let followed = crate::models::Series {
        id: 0, slug: "f".into(), title: "F".into(),
        url: "u1".into(), cover_url: None, is_airing: false, followed: true, next_episode_at: None, site_episode_count: None,
    };
    let sid_f = db.upsert_series(src, &followed).unwrap();
    db.set_followed(sid_f, true).unwrap();
    db.insert_series_genres(sid_f, &["Acción".to_string(), "Isekai".to_string()]).unwrap();

    // A 'want' series tagged with the catalog's English genre "Action".
    let want = crate::models::Series {
        id: 0, slug: "w".into(), title: "W".into(),
        url: "u2".into(), cover_url: None, is_airing: false, followed: false, next_episode_at: None, site_episode_count: None,
    };
    let sid_w = db.upsert_series(src, &want).unwrap();
    db.set_backlog_status(sid_w, Some("want")).unwrap();
    db.insert_series_genres(sid_w, &["Action".to_string()]).unwrap();

    let scores = db.get_genre_affinity(src).unwrap();
    // 2.0 (followed, "Acción" normalized to "Action") + 1.0 (want, "Action") = 3.0
    assert_eq!(scores.get("Action"), Some(&3.0));
    assert_eq!(scores.get("Acción"), None);
    // Site-only tag with no AniList equivalent keeps its raw name.
    assert_eq!(scores.get("Isekai"), Some(&2.0));
}

// ---- Cross-site aggregation (2026-08-13 stats-scoping fix) ----
//
// Every test above uses a single `upsert_source`, so it never exercised
// the bug: every stats query used to filter by `source_id`, silently
// hiding everything followed/watched/wanted on the app's other sites.
// These reproduce the real shape — the same show followed on two sites,
// plus a site-only show — and assert the canonical (deduped) totals.

#[test]
fn get_watch_summary_aggregates_and_dedups_a_show_followed_on_two_sites() {
    let db = Db::open(":memory:").unwrap();
    let a = db.upsert_source("AnimeYT", "a", "animeytx").unwrap();
    let b = db.upsert_source("TioAnime", "b", "tioanime").unwrap();

    // Same show, no anilist_id on either — same normalized title is what
    // must collapse them into one canonical entry.
    let shared_a = db.upsert_series(a, &mk_airing("shared", "Shared Show", None)).unwrap();
    db.set_followed(shared_a, true).unwrap();
    insert_eps_seen_up_to(&db, shared_a, 3, 3);
    let shared_b = db.upsert_series(b, &mk_airing("shared", "Shared Show", None)).unwrap();
    db.set_followed(shared_b, true).unwrap();
    insert_eps_seen_up_to(&db, shared_b, 2, 2);

    // Site-B-only show, want-listed — must still surface even though the
    // active site in real use is usually A.
    let want_only = db.upsert_series(b, &mk_airing("wantonly", "Only On B", None)).unwrap();
    db.set_backlog_status(want_only, Some("want")).unwrap();

    let summary = db.get_watch_summary().unwrap();
    assert_eq!(summary.followed_series, 1, "the shared show counts once, not twice");
    assert_eq!(
        summary.episodes_watched, 3,
        "one show's progress, not the 3+2 sum of two sites mirroring the same episodes — \
         the better site's 3 marks win, exactly as franchise_rollups' real_seen does"
    );
    assert_eq!(summary.backlog_want, 1, "site-B-only want show is not hidden by site A's scope");
    assert_eq!(summary.distinct_anime, 1);

    // The invariant the old raw `COUNT(*) FROM episodes WHERE seen=1`
    // broke: the episode figure and the minutes figure are two views of
    // the same collapsed roll-up, so they can never disagree about how
    // much was watched. 3 episodes at the default 24 min/ep estimate.
    let insights = db.get_watch_insights().unwrap();
    assert_eq!(
        insights.estimated_minutes_tracked,
        summary.episodes_watched * 24,
        "minutes and episodes both come off franchise_rollups"
    );
}

#[test]
fn get_watch_insights_funnel_dedups_a_show_followed_and_airing_on_two_sites() {
    let db = Db::open(":memory:").unwrap();
    let a = db.upsert_source("AnimeYT", "a", "animeytx").unwrap();
    let b = db.upsert_source("TioAnime", "b", "tioanime").unwrap();

    let mut airing_a = mk_airing("dup", "Dup Show", None);
    airing_a.is_airing = true;
    let sid_a = db.upsert_series(a, &airing_a).unwrap();
    db.set_followed(sid_a, true).unwrap();
    let mut airing_b = mk_airing("dup", "Dup Show", None);
    airing_b.is_airing = true;
    let sid_b = db.upsert_series(b, &airing_b).unwrap();
    db.set_followed(sid_b, true).unwrap();

    let discarded_b = db.upsert_series(b, &mk_airing("disc", "Discarded On B", None)).unwrap();
    db.set_backlog_status(discarded_b, Some("discarded")).unwrap();

    let insights = db.get_watch_insights().unwrap();
    assert_eq!(insights.followed_airing, 1, "followed+airing on both sites is still one show");
    assert_eq!(insights.discarded, 1, "a site-B-only discard is not lost");
}

#[test]
fn followed_airing_and_finished_partition_followed_series_when_sites_disagree() {
    // `series.is_airing` is per-site and only refreshed by that site's own
    // scan, so one cross-site show routinely has a row saying "airing" and
    // a stale row saying "finished". Counting the two buckets with
    // independent queries put it in BOTH, and the funnel's own
    // "followed" bar (airing + finished) then exceeded the canonical
    // follow count printed right next to it.
    let db = Db::open(":memory:").unwrap();
    let a = db.upsert_source("AnimeYT", "a", "animeytx").unwrap();
    let b = db.upsert_source("TioAnime", "b", "tioanime").unwrap();

    let mut on_a = mk_airing("dup", "Dup Show", None);
    on_a.is_airing = true;
    let sid_a = db.upsert_series(a, &on_a).unwrap();
    db.set_followed(sid_a, true).unwrap();
    let mut on_b = mk_airing("dup", "Dup Show", None);
    on_b.is_airing = false;
    let sid_b = db.upsert_series(b, &on_b).unwrap();
    db.set_followed(sid_b, true).unwrap();

    // A genuinely finished follow, so the finished bucket is exercised too
    // rather than merely being zero.
    let mut done = mk_airing("done", "Done Show", None);
    done.is_airing = false;
    let sid_done = db.upsert_series(b, &done).unwrap();
    db.set_followed(sid_done, true).unwrap();

    let summary = db.get_watch_summary().unwrap();
    let insights = db.get_watch_insights().unwrap();
    assert_eq!(summary.followed_series, 2, "the cross-site show plus the finished one");
    assert_eq!(
        insights.followed_airing, 1,
        "any site still calling it airing decides the verdict"
    );
    assert_eq!(
        insights.followed_finished, 1,
        "and it is therefore NOT also counted as finished — only Done Show is"
    );
    assert_eq!(
        insights.followed_airing + insights.followed_finished,
        summary.followed_series,
        "the two buckets partition the canonical follow count exactly, never overshoot it"
    );
    assert_eq!(
        summary.airing_followed, insights.followed_airing,
        "both screens reach the same verdict from the same pass"
    );
}

#[test]
fn external_titles_ratio_counts_both_sides_after_the_parent_merge() {
    // The "X de Y con datos" ratio compared two different groupings: the
    // numerator counted merged franchises, the denominator counted raw
    // `franchise_key`s from before the merge. Two arcs of one franchise,
    // both "Ya lo vi" and both with catalog data, read as "1 de 2".
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("A", "a", "animeytx").unwrap();

    // The plain base row is what lets the arcs merge at all — see
    // `franchise_parent_key`. It carries a real seen episode so it is a
    // franchise in its own right.
    let base = db.upsert_series(src, &mk_airing("op", "One Piece", None)).unwrap();
    db.set_followed(base, true).unwrap();
    insert_eps_seen_up_to(&db, base, 1, 1);

    for (id, slug, title, episodes) in [
        (900_i64, "op-wano", "One Piece: Wano", 100_i64),
        (901, "op-elbaph", "One Piece: Arco de Elbaph", 50),
    ] {
        db.upsert_catalog_anime(
            &crate::anilist::CatalogAnime {
                id, title: title.into(), title_romaji: None, title_english: None, synonyms: Vec::new(), next_airing_at: None, next_episode: None, tags: Vec::new(), relations: Vec::new(),
                cover_url: None, banner_url: None, format: Some("TV".into()), genres: vec![],
                episodes: Some(episodes), average_score: None, popularity: None,
                url: format!("https://anilist.co/anime/{id}"), status: None, duration: Some(24),
                studio: None, start_date: None,
            },
            0,
        ).unwrap();
        let sid = db.upsert_series(src, &mk_airing(slug, title, None)).unwrap();
        db.set_watched_externally(sid, true).unwrap();
        db.set_anilist_id(sid, id).unwrap();
    }

    let insights = db.get_watch_insights().unwrap();
    assert_eq!(
        insights.external_titles_estimated, 1,
        "both arcs merged into the One Piece franchise, which does have catalog data"
    );
    assert_eq!(
        insights.external_titles_total, 1,
        "the denominator counts the same merged franchise, not the two pre-merge arc keys"
    );
}

#[test]
fn external_titles_total_still_counts_a_franchise_with_no_catalog_data() {
    // `has_external` must track the "Ya lo vi" flag itself, not the
    // presence of catalog data — otherwise the ratio's denominator would
    // collapse onto its numerator and always read "N of N".
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("A", "a", "animeytx").unwrap();

    db.upsert_catalog_anime(
        &crate::anilist::CatalogAnime {
            id: 910, title: "Linked Show".into(), title_romaji: None, title_english: None, synonyms: Vec::new(), next_airing_at: None, next_episode: None, tags: Vec::new(), relations: Vec::new(),
            cover_url: None, banner_url: None, format: Some("TV".into()), genres: vec![],
            episodes: Some(12), average_score: None, popularity: None,
            url: "https://anilist.co/anime/910".into(), status: None, duration: Some(24),
            studio: None, start_date: None,
        },
        0,
    ).unwrap();
    let linked = db.upsert_series(src, &mk_airing("linked", "Linked Show", None)).unwrap();
    db.set_watched_externally(linked, true).unwrap();
    db.set_anilist_id(linked, 910).unwrap();

    let unlinked = db.upsert_series(src, &mk_airing("unlinked", "Unlinked Show", None)).unwrap();
    db.set_watched_externally(unlinked, true).unwrap();

    let insights = db.get_watch_insights().unwrap();
    assert_eq!(insights.external_titles_estimated, 1, "only the linked one has data");
    assert_eq!(insights.external_titles_total, 2, "but both were swiped 'Ya lo vi'");
}

#[test]
fn get_genre_cards_credits_the_best_site_not_the_sum_across_sites() {
    // Genre cards grouped canonically across sites but then summed every
    // member row's seen count, so a show followed with matching progress
    // on two sites doubled its own episode total — inflating the card and
    // its rank against every other genre.
    let db = Db::open(":memory:").unwrap();
    let a = db.upsert_source("AnimeYT", "a", "animeytx").unwrap();
    let b = db.upsert_source("TioAnime", "b", "tioanime").unwrap();

    let sid_a = db.upsert_series(a, &mk_airing("dup", "Dup Show", None)).unwrap();
    db.set_followed(sid_a, true).unwrap();
    db.insert_series_genres(sid_a, &["Action".into()]).unwrap();
    insert_eps_seen_up_to(&db, sid_a, 5, 5);
    let sid_b = db.upsert_series(b, &mk_airing("dup", "Dup Show", None)).unwrap();
    db.set_followed(sid_b, true).unwrap();
    db.insert_series_genres(sid_b, &["Action".into()]).unwrap();
    insert_eps_seen_up_to(&db, sid_b, 5, 5);

    // A single-site show with 8 real episodes must therefore outrank it;
    // with the double-count Action read 10 and won.
    let solo = db.upsert_series(a, &mk_airing("solo", "Solo Show", None)).unwrap();
    db.set_followed(solo, true).unwrap();
    db.insert_series_genres(solo, &["Comedy".into()]).unwrap();
    insert_eps_seen_up_to(&db, solo, 8, 8);

    let cards = db.get_genre_cards().unwrap();
    let action = cards.iter().find(|c| c.genre == "Action").expect("Action card");
    assert_eq!(action.count, 5, "the cross-site show counts 5 episodes, not 5+5");
    assert_eq!(action.top_series.len(), 1, "and appears as one show, not two");
    assert_eq!(cards[0].genre, "Comedy", "8 real episodes outrank the deduped 5");

    // And it agrees with the headline figure built on the same rule.
    let summary = db.get_watch_summary().unwrap();
    assert_eq!(
        summary.episodes_watched, 13,
        "5 (deduped cross-site) + 8, the same total the genre cards add up to"
    );
}

#[test]
fn get_watch_insights_top_series_credits_best_site_not_sum_across_sites() {
    // The real bug this guards: a user following One Piece on two sites
    // had ~1170 real marks on each site's own full-series row. Franchise
    // rollup used to sum real_seen across every member row regardless of
    // site, reporting ~2600 "episodes" for a ~1170-episode show. Summing
    // is still correct *within* one site (that site's own arc-split
    // rows are genuinely different episodes) — only cross-site should
    // take the best single signal.
    let db = Db::open(":memory:").unwrap();
    let a = db.upsert_source("AnimeYT", "a", "animeytx").unwrap();
    let b = db.upsert_source("TioAnime", "b", "tioanime").unwrap();

    let op_a = db.upsert_series(a, &mk_airing("op", "One Piece", None)).unwrap();
    db.set_followed(op_a, true).unwrap();
    insert_eps_seen_up_to(&db, op_a, 50, 50);
    let op_b = db.upsert_series(b, &mk_airing("op", "One Piece", None)).unwrap();
    db.set_followed(op_b, true).unwrap();
    insert_eps_seen_up_to(&db, op_b, 40, 40);

    let insights = db.get_watch_insights().unwrap();
    let op = insights.top_series.iter().find(|t| t.title == "One Piece").unwrap();
    assert_eq!(op.count, 50, "credits the better site's real progress, not the 90-episode sum of both sites");
}

#[test]
fn get_watch_insights_top_series_sums_arcs_within_one_site() {
    // Same site splitting a long-runner into arc rows is genuinely
    // additive — those are different episodes, not mirrors of the same
    // ones, so this must keep summing.
    let db = Db::open(":memory:").unwrap();
    let a = db.upsert_source("AnimeYT", "a", "animeytx").unwrap();

    // A colon-suffixed arc only merges into its base when that base is
    // independently present in the user's own data — same rule
    // `arc_rows_only_merge_when_the_base_show_is_in_the_users_own_data`
    // covers — so a bare "One Piece" row must exist alongside the arc.
    let base = db.upsert_series(a, &mk_airing("op0", "One Piece", None)).unwrap();
    db.set_followed(base, true).unwrap();
    insert_eps_seen_up_to(&db, base, 20, 20);
    let arc = db.upsert_series(a, &mk_airing("op1", "One Piece: Wano", None)).unwrap();
    db.set_followed(arc, true).unwrap();
    insert_eps_seen_up_to(&db, arc, 15, 15);

    let insights = db.get_watch_insights().unwrap();
    let op = insights.top_series.iter().find(|t| t.title == "One Piece").unwrap();
    assert_eq!(op.count, 35, "same-site arc rows still sum — they're different episodes");
}

#[test]
fn get_genre_and_type_stats_dedup_a_cross_site_show_and_union_its_genres() {
    let db = Db::open(":memory:").unwrap();
    let a = db.upsert_source("AnimeYT", "a", "animeytx").unwrap();
    let b = db.upsert_source("TioAnime", "b", "tioanime").unwrap();

    // Same show followed on both sites, each site's scrape tagged it with
    // a different genre subset and only site A recorded a `kind`.
    let sid_a = db.upsert_series(a, &mk_airing("dup", "Dup Show", None)).unwrap();
    db.set_followed(sid_a, true).unwrap();
    db.set_kind(sid_a, "TV").unwrap();
    db.insert_series_genres(sid_a, &["Comedia".to_string()]).unwrap();
    let sid_b = db.upsert_series(b, &mk_airing("dup", "Dup Show", None)).unwrap();
    db.set_followed(sid_b, true).unwrap();
    db.insert_series_genres(sid_b, &["Drama".to_string()]).unwrap();

    let genre_stats = db.get_genre_stats().unwrap();
    let genres: HashSet<&str> = genre_stats.iter().map(|g| g.genre.as_str()).collect();
    assert_eq!(genres, HashSet::from(["Comedy", "Drama"]), "genres from both sites' rows are unioned");
    assert!(
        genre_stats.iter().all(|g| g.count == 1),
        "one canonical show contributes 1 to each genre it carries, not 2: {genre_stats:?}"
    );

    let type_stats = db.get_type_stats().unwrap();
    assert_eq!(type_stats.len(), 1);
    assert_eq!(type_stats[0].kind, "TV");
    assert_eq!(type_stats[0].count, 1, "the one canonical show, not one per site row");
}

#[test]
fn signature_changes_when_episode_marked_seen() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
    let series = db.upsert_series(src, &mk_airing("x", "X", None)).unwrap();
    db.insert_episode(&crate::models::Episode {
        id: 0, series_id: series, number: "1".into(), title: None,
        url: "https://site/x-1".into(), released_at: None, seen: false, download_path: None, download_status: None,
    }).unwrap();
    let before = db.signature_counts().unwrap();
    db.set_seen_cascade(series, "1", true).unwrap();
    let after = db.signature_counts().unwrap();
    assert_ne!(before, after);
}

/// Siblings of the test above, for the mutations the old episode-only
/// fingerprint was blind to. Each of these is something a user can spend a
/// whole session doing without a single episode row or `seen_at` moving —
/// and while the signature didn't move, `is_auto_backup_due` answered "no"
/// and the backup never ran.
#[test]
fn signature_changes_when_a_series_is_followed_or_unfollowed() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
    let series = db.upsert_series(src, &mk_airing("x", "X", None)).unwrap();

    let before = db.signature_counts().unwrap();
    db.set_followed(series, true).unwrap();
    let followed = db.signature_counts().unwrap();
    assert_ne!(before, followed);
    db.set_followed(series, false).unwrap();
    assert_ne!(followed, db.signature_counts().unwrap());
}

#[test]
fn signature_changes_when_watched_externally_flips() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
    let series = db.upsert_series(src, &mk_airing("x", "X", None)).unwrap();

    let before = db.signature_counts().unwrap();
    db.set_watched_externally(series, true).unwrap();
    assert_ne!(before, db.signature_counts().unwrap());
}

#[test]
fn signature_changes_when_backlog_status_moves_between_lists() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
    let series = db.upsert_series(src, &mk_airing("x", "X", None)).unwrap();

    let before = db.signature_counts().unwrap();
    db.set_backlog_status(series, Some("want")).unwrap();
    let wanted = db.signature_counts().unwrap();
    assert_ne!(before, wanted);
    // want -> discarded keeps the row count identical; only per-status
    // counts can tell these apart.
    db.set_backlog_status(series, Some("discarded")).unwrap();
    assert_ne!(wanted, db.signature_counts().unwrap());
}

#[test]
fn signature_changes_when_a_series_is_linked_to_the_catalog() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
    let series = db.upsert_series(src, &mk_airing("x", "X", None)).unwrap();

    let before = db.signature_counts().unwrap();
    db.set_anilist_id(series, 12345).unwrap();
    assert_ne!(before, db.signature_counts().unwrap());
}

#[test]
fn signature_changes_when_deck_bans_change_but_not_when_a_backup_records_itself() {
    let db = Db::open(":memory:").unwrap();
    let before = db.signature_counts().unwrap();

    // A settings *value* change under an existing key — invisible to any
    // count over the table.
    db.set_banned_genres(&["Ecchi".to_string()]).unwrap();
    let banned = db.signature_counts().unwrap();
    assert_ne!(before, banned);
    db.set_banned_genres(&["Ecchi".to_string(), "Hentai".to_string()]).unwrap();
    assert_ne!(banned, db.signature_counts().unwrap());

    // Backup bookkeeping must NOT move it, or "has anything changed since
    // the last backup?" is true immediately after every backup.
    let after_bans = db.signature_counts().unwrap();
    db.set_setting("backup_last_at_unix", "1234567890").unwrap();
    db.set_setting("backup_signature", "whatever").unwrap();
    db.set_setting("gdrive_file_id", "abc").unwrap();
    assert_eq!(after_bans, db.signature_counts().unwrap());
}

#[test]
fn get_dusty_watchlist_shows_followed_series_older_than_90_days() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("AnimeYT", "a", "animeytx").unwrap();

    // Series with an episode seen >90 days ago — should appear
    let dusty = db.upsert_series(src, &mk_airing("dusty", "Dusty Show", None)).unwrap();
    db.set_followed(dusty, true).unwrap();
    db.insert_episode(&crate::models::Episode {
        id: 0, series_id: dusty, number: "1".into(), title: None,
        url: "https://site/dusty-1".into(), released_at: None, seen: true, download_path: None, download_status: None,
    }).unwrap();
    // Manually backdate seen_at to >90 days ago
    db.conn.execute(
        "UPDATE episodes SET seen_at = datetime('now', 'start of day', '-100 days', '+12 hours') WHERE series_id=?1",
        [dusty],
    ).unwrap();
    // ...with an episode still waiting: a fully seen show is finished,
    // not dusty (see `get_dusty_watchlist_skips_series_with_nothing_left_to_watch`).
    db.insert_episode(&crate::models::Episode {
        id: 0, series_id: dusty, number: "2".into(), title: None,
        url: "https://site/dusty-2".into(), released_at: None, seen: false, download_path: None, download_status: None,
    }).unwrap();

    // Series with a recent seen_at — should NOT appear
    let fresh = db.upsert_series(src, &mk_airing("fresh", "Fresh Show", None)).unwrap();
    db.set_followed(fresh, true).unwrap();
    db.insert_episode(&crate::models::Episode {
        id: 0, series_id: fresh, number: "1".into(), title: None,
        url: "https://site/fresh-1".into(), released_at: None, seen: true, download_path: None, download_status: None,
    }).unwrap();
    db.set_seen_cascade(fresh, "1", true).unwrap(); // stamps "now"

    // Followed series with NO seen episodes at all — should NOT appear (never started)
    let never = db.upsert_series(src, &mk_airing("never", "Never Started", None)).unwrap();
    db.set_followed(never, true).unwrap();
    db.insert_episode(&crate::models::Episode {
        id: 0, series_id: never, number: "1".into(), title: None,
        url: "https://site/never-1".into(), released_at: None, seen: false, download_path: None, download_status: None,
    }).unwrap();

    let dusty_list = db.get_dusty_watchlist().unwrap();
    assert_eq!(dusty_list.len(), 1);
    assert_eq!(dusty_list[0].title, "Dusty Show");
    // last_seen_at should be an ISO date string (YYYY-MM-DD)
    assert!(dusty_list[0].last_seen_at.contains("-"));
}

#[test]
fn get_dusty_watchlist_dedups_cross_site_same_show() {
    let db = Db::open(":memory:").unwrap();
    let a = db.upsert_source("AnimeYT", "a", "animeytx").unwrap();
    let b = db.upsert_source("TioAnime", "b", "tioanime").unwrap();

    // Same show followed on both sites, different seen_at dates
    // Site A: -100 days (more recent)
    // Site B: -120 days (older)
    // The CORRECT behavior: keep the MOST RECENT (MAX) date across sites = -100 days
    // because "dusty" means no engagement on ANY site recently.
    let dusty_a = db.upsert_series(a, &mk_airing("dusty", "Dusty Show", None)).unwrap();
    db.set_followed(dusty_a, true).unwrap();
    db.insert_episode(&crate::models::Episode {
        id: 0, series_id: dusty_a, number: "1".into(), title: None,
        url: "https://site/dusty-1".into(), released_at: None, seen: true, download_path: None, download_status: None,
    }).unwrap();
    db.conn.execute(
        "UPDATE episodes SET seen_at = datetime('now', 'start of day', '-100 days', '+12 hours') WHERE series_id=?1",
        [dusty_a],
    ).unwrap();
    // An episode still waiting, so the show is unfinished (dusty), not done.
    db.insert_episode(&crate::models::Episode {
        id: 0, series_id: dusty_a, number: "2".into(), title: None,
        url: "https://site/dusty-2".into(), released_at: None, seen: false, download_path: None, download_status: None,
    }).unwrap();

    let dusty_b = db.upsert_series(b, &mk_airing("dusty", "Dusty Show", None)).unwrap();
    db.set_followed(dusty_b, true).unwrap();
    db.insert_episode(&crate::models::Episode {
        id: 0, series_id: dusty_b, number: "1".into(), title: None,
        url: "https://site/dusty-1".into(), released_at: None, seen: true, download_path: None, download_status: None,
    }).unwrap();
    db.conn.execute(
        "UPDATE episodes SET seen_at = datetime('now', 'start of day', '-120 days', '+12 hours') WHERE series_id=?1",
        [dusty_b],
    ).unwrap();

    let dusty_list = db.get_dusty_watchlist().unwrap();
    // Should appear only once (canonical dedup)
    assert_eq!(dusty_list.len(), 1);
    assert_eq!(dusty_list[0].title, "Dusty Show");

    // The most recent date across sites (-100 days) should win
    // Expected date computed directly from SQLite
    let expected_str: String = db
        .conn
        .query_row(
            "SELECT DATE(datetime('now', 'start of day', '-100 days', '+12 hours'), 'localtime')",
            [],
            |r| r.get::<_, String>(0),
        )
        .unwrap();

    assert_eq!(dusty_list[0].last_seen_at, expected_str);
}

#[test]
fn get_binge_record_returns_day_with_highest_count() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
    let s1 = db.upsert_series(src, &mk_airing("s1", "Show 1", None)).unwrap();
    let s2 = db.upsert_series(src, &mk_airing("s2", "Show 2", None)).unwrap();

    // Day A (today): 3 episodes from s1 + 2 from s2 = 5 total
    for i in 1..=3 {
        db.insert_episode(&crate::models::Episode {
            id: 0, series_id: s1, number: format!("{i}"), title: None,
            url: format!("https://site/s1-{i}"), released_at: None, seen: false, download_path: None, download_status: None,
        }).unwrap();
    }
    for i in 1..=2 {
        db.insert_episode(&crate::models::Episode {
            id: 0, series_id: s2, number: format!("{i}"), title: None,
            url: format!("https://site/s2-{i}"), released_at: None, seen: false, download_path: None, download_status: None,
        }).unwrap();
    }
    db.set_seen_cascade(s1, "3", true).unwrap(); // marks 1,2,3 seen
    db.set_seen_cascade(s2, "2", true).unwrap(); // marks 1,2 seen

    // Day B (yesterday): 4 episodes from a third series
    let s3 = db.upsert_series(src, &mk_airing("s3", "Show 3", None)).unwrap();
    for i in 1..=4 {
        db.insert_episode(&crate::models::Episode {
            id: 0, series_id: s3, number: format!("{i}"), title: None,
            url: format!("https://site/s3-{i}"), released_at: None, seen: false, download_path: None, download_status: None,
        }).unwrap();
    }
    db.set_seen_cascade(s3, "4", true).unwrap(); // marks 1,2,3,4 seen

    // Backdate s3's episodes to yesterday so they fall on a different calendar day
    db.conn.execute(
        "UPDATE episodes SET seen_at = datetime('now', '-1 day') WHERE series_id = ?1",
        [s3],
    ).unwrap();

    // Day A (today) has 5, Day B (yesterday) has 4 -> Day A should win
    let record = db.get_binge_record().unwrap();
    let today = chrono::Local::now().date_naive().format("%Y-%m-%d").to_string();
    assert_eq!(record.count, 5, "Day A has 5 episodes, should be the binge record");
    assert_eq!(record.day, Some(today), "binge record day should be today (local)");

    // ---- Reverse case: make Day B have more episodes ----
    // Add 3 more episodes to s3 and mark them seen (still on Day B via the backdated seen_at)
    for i in 5..=7 {
        db.insert_episode(&crate::models::Episode {
            id: 0, series_id: s3, number: format!("{i}"), title: None,
            url: format!("https://site/s3-{i}"), released_at: None, seen: false, download_path: None, download_status: None,
        }).unwrap();
    }
    db.set_seen_cascade(s3, "7", true).unwrap(); // marks 5,6,7 seen (cascade from 4)

    // set_seen_cascade re-stamps seen_at to "now" for every episode in the
    // cascaded range, including ones already seen — so the second cascade
    // call above just clobbered episodes 1-4's backdated "yesterday"
    // timestamp back to today. Re-apply the backdate to all of s3's
    // episodes so the whole series still lands on "yesterday".
    db.conn.execute(
        "UPDATE episodes SET seen_at = datetime('now', '-1 day') WHERE series_id = ?1",
        [s3],
    ).unwrap();

    // Now Day B (yesterday) has 7, Day A (today) has 5 -> Day B should win
    let record = db.get_binge_record().unwrap();
    let yesterday = chrono::Local::now().date_naive().checked_sub_days(chrono::Days::new(1)).unwrap().format("%Y-%m-%d").to_string();
    assert_eq!(record.count, 7, "Day B now has 7 episodes, should be the binge record");
    assert_eq!(record.day, Some(yesterday), "binge record day should be yesterday (local)");
}

#[test]
fn get_binge_record_empty_db_returns_zero_and_none() {
    let db = Db::open(":memory:").unwrap();
    let record = db.get_binge_record().unwrap();
    assert_eq!(record.count, 0);
    assert_eq!(record.day, None);
}

#[test]
fn get_hourly_distribution_returns_24_zero_filled_entries() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("AnimeYT", "a", "animeytx").unwrap();
    let sid = db.upsert_series(src, &mk_airing("s1", "S1", None)).unwrap();
    db.set_kind(sid, "TV").unwrap();

    // Insert episodes relative to "now" (SQLite's 'now' is UTC) at fixed
    // hour offsets, then compute the EXPECTED local hour bucket for each
    // offset in Rust using the same wall-clock "now" the test machine
    // actually has — this only assumes SQLite's 'localtime' modifier and
    // chrono::Local agree on the system timezone (they must, both read
    // it from the OS), not that the machine happens to be UTC. Hardcoding
    // both the input timestamps *and* the expected hours to UTC values
    // is wrong on any machine not itself running in UTC (this one runs
    // CEST, UTC+2, at the time this test was written — exactly the kind
    // of environment-dependent assumption that broke the original,
    // non-deterministic version of this test).
    use chrono::Timelike;
    let now_local = chrono::Local::now();
    let hour_for_offset = |offset_hours: i64| -> i64 {
        (((now_local.hour() as i64 - offset_hours) % 24) + 24) % 24
    };
    db.conn
        .execute(
            "INSERT INTO episodes (series_id, number, url, seen, seen_at) VALUES
             (?1, '1', 'u1', 1, datetime('now', '-10 hours')),
             (?1, '2', 'u2', 1, datetime('now', '-7 hours')),
             (?1, '3', 'u3', 1, datetime('now', '-7 hours')),
             (?1, '4', 'u4', 1, datetime('now', '2 hours'))",
            [sid],
        )
        .unwrap();
    let hour_a = hour_for_offset(10); // 1 episode
    let hour_b = hour_for_offset(7); // 2 episodes
    let hour_c = hour_for_offset(-2); // 1 episode

    let dist = db.get_hourly_distribution().unwrap();
    assert_eq!(dist.len(), 24, "must return exactly 24 entries");
    // Check hours are in order 0..23
    for (i, hc) in dist.iter().enumerate() {
        assert_eq!(hc.hour, i as i64, "hours must be sequential 0..23");
    }
    // Exact per-hour counts against the dynamically-computed expected
    // hours, not just "some non-zero hour exists" — this is what
    // actually proves the hour extraction (and its timezone handling)
    // is correct rather than merely present. If two of the three
    // computed hours collide (possible with only three offsets on an
    // unlucky wall-clock minute), skip the exact-count assertions for
    // this run rather than asserting something that can't be true —
    // the total/shape assertions below still cover it.
    let hours_distinct = {
        let mut hs = [hour_a, hour_b, hour_c];
        hs.sort_unstable();
        hs.windows(2).all(|w| w[0] != w[1])
    };
    if hours_distinct {
        assert_eq!(dist[hour_a as usize].count, 1, "hour {hour_a} should have 1 episode");
        assert_eq!(dist[hour_b as usize].count, 2, "hour {hour_b} should have 2 episodes");
        assert_eq!(dist[hour_c as usize].count, 1, "hour {hour_c} should have 1 episode");
        let expected_hours = [hour_a, hour_b, hour_c];
        assert!(
            dist.iter().enumerate().all(|(i, h)| expected_hours.contains(&(i as i64)) || h.count == 0),
            "every hour outside {{{hour_a}, {hour_b}, {hour_c}}} must be zero-filled, not just present"
        );
    }
    let total: i64 = dist.iter().map(|h| h.count).sum();
    assert_eq!(total, 4, "total episodes marked seen should be 4");
}

#[test]
fn get_hourly_distribution_empty_db_returns_all_zeros() {
    let db = Db::open(":memory:").unwrap();
    let dist = db.get_hourly_distribution().unwrap();
    assert_eq!(dist.len(), 24);
    assert!(dist.iter().all(|h| h.count == 0), "all hours should be 0 in empty DB");
}

/// Seed helper: insert a finished series with episodes spanning a known
/// number of days by backdating `seen_at` via raw SQL (normal insert uses
/// `datetime('now')` which we can't control in tests). Backdates to noon
/// UTC so the local calendar date and spans stay deterministic across clock
/// times, timezone offsets within +/-11 h, and DST changes.
fn seed_finished_series(
    db: &Db,
    src: i64,
    title: &str,
    first_seen_days_ago: i64,
    last_seen_days_ago: i64,
) -> i64 {
    let id = db.upsert_series(src, &mk_airing(title, title, None)).unwrap();
    db.set_followed(id, true).unwrap();
    db.set_kind(id, "TV").unwrap();
    // Not airing = finished
    db.conn
        .execute("UPDATE series SET is_airing=0 WHERE id=?1", [id])
        .unwrap();
    // Insert 2 episodes and mark both seen, then backdate seen_at
    for n in 1..=2 {
        db.insert_episode(&crate::models::Episode {
            id: 0, series_id: id, number: n.to_string(), title: None,
            url: format!("https://site/{}-capitulo-{}/", title, n),
            released_at: None, seen: true, download_path: None, download_status: None,
        }).unwrap();
    }
    // Backdate first episode
    db.conn
        .execute(
            "UPDATE episodes SET seen_at=datetime('now', 'start of day', ?1, '+12 hours') WHERE series_id=?2 AND number='1'",
            [format!("-{} days", first_seen_days_ago), id.to_string()],
        )
        .unwrap();
    // Backdate last episode
    db.conn
        .execute(
            "UPDATE episodes SET seen_at=datetime('now', 'start of day', ?1, '+12 hours') WHERE series_id=?2 AND number='2'",
            [format!("-{} days", last_seen_days_ago), id.to_string()],
        )
        .unwrap();
    id
}

#[test]
fn get_avg_completion_days_empty_db_returns_none() {
    let db = Db::open(":memory:").unwrap();
    let avg = db.get_avg_completion_days().unwrap();
    assert_eq!(avg, None, "empty DB -> no qualifying franchises -> None");
}

#[test]
fn get_avg_completion_days_all_airing_returns_none() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("A", "a", "animeytx").unwrap();

    // Finished series with no seen episodes = not qualified
    let mut s = mk_airing("s1", "Show 1", None);
    s.is_airing = false;
    let id = db.upsert_series(src, &s).unwrap();
    db.set_followed(id, true).unwrap();
    db.insert_episode(&crate::models::Episode {
        id: 0, series_id: id, number: "1".into(), title: None,
        url: "https://site/s1-1/".into(), released_at: None, seen: false, download_path: None, download_status: None,
    }).unwrap();

    let avg = db.get_avg_completion_days().unwrap();
    assert_eq!(avg, None, "no real seen_at data -> None");
}

#[test]
fn get_avg_completion_days_single_franchise_known_span() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("A", "a", "animeytx").unwrap();

    // Finished franchise, episodes seen 10 days ago and 2 days ago -> span = 8 days
    seed_finished_series(&db, src, "Show A", 10, 2);

    let avg = db.get_avg_completion_days().unwrap();
    assert_eq!(avg, Some(8.0), "single franchise with 8-day span -> 8.0");
}

#[test]
fn get_avg_completion_days_two_franchises_average_of_spans() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("A", "a", "animeytx").unwrap();

    // Franchise 1: 10-day span
    seed_finished_series(&db, src, "Show A", 10, 0);
    // Franchise 2: 4-day span
    seed_finished_series(&db, src, "Show B", 4, 0);

    let avg = db.get_avg_completion_days().unwrap();
    // Average of 10 and 4 = 7
    assert!((avg.unwrap() - 7.0).abs() < 0.01, "average of 10 and 4 = 7.0, got {}", avg.unwrap());
}

#[test]
fn get_avg_completion_days_airing_franchise_excluded() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("A", "a", "animeytx").unwrap();

    // Finished franchise with 8-day span
    seed_finished_series(&db, src, "Show A", 10, 2);
    // Airing franchise with 8-day span -> should be excluded
    let airing_id = db.upsert_series(src, &mk_airing("Show B", "Show B", None)).unwrap();
    db.set_followed(airing_id, true).unwrap();
    db.set_kind(airing_id, "TV").unwrap();
    // is_airing defaults to true in mk_airing
    for n in 1..=2 {
        db.insert_episode(&crate::models::Episode {
            id: 0, series_id: airing_id, number: n.to_string(), title: None,
            url: format!("https://site/showb-{}/", n), released_at: None, seen: true, download_path: None, download_status: None,
        }).unwrap();
    }
    db.conn
        .execute(
            "UPDATE episodes SET seen_at=datetime('now', 'start of day', '-10 days', '+12 hours') WHERE series_id=?1 AND number='1'",
            [airing_id.to_string()],
        )
        .unwrap();
    db.conn
        .execute(
            "UPDATE episodes SET seen_at=datetime('now', 'start of day', '-2 days', '+12 hours') WHERE series_id=?1 AND number='2'",
            [airing_id.to_string()],
        )
        .unwrap();

    let avg = db.get_avg_completion_days().unwrap();
    // Only the finished franchise counts -> 8.0
    assert_eq!(avg, Some(8.0), "airing franchise excluded, only finished counted");
}

#[test]
fn get_avg_completion_days_cross_site_franchise_merged() {
    let db = Db::open(":memory:").unwrap();
    let a = db.upsert_source("AnimeYT", "a", "animeytx").unwrap();
    let b = db.upsert_source("TioAnime", "b", "tioanime").unwrap();

    // Same franchise on two sites, both finished
    // Site A: episodes seen 10 days ago and 2 days ago -> span = 8
    let mut s1 = mk_airing("shared", "Shared Show", None);
    s1.is_airing = false;
    let id_a = db.upsert_series(a, &s1).unwrap();
    db.set_followed(id_a, true).unwrap();
    db.set_kind(id_a, "TV").unwrap();
    for n in 1..=2 {
        db.insert_episode(&crate::models::Episode {
            id: 0, series_id: id_a, number: n.to_string(), title: None,
            url: format!("https://site/a-{}/", n), released_at: None, seen: true, download_path: None, download_status: None,
        }).unwrap();
    }
    db.conn
        .execute(
            "UPDATE episodes SET seen_at=datetime('now', 'start of day', ?1, '+12 hours') WHERE series_id=?2 AND number='1'",
            ("-10 days", id_a),
        )
        .unwrap();
    db.conn
        .execute(
            "UPDATE episodes SET seen_at=datetime('now', 'start of day', ?1, '+12 hours') WHERE series_id=?2 AND number='2'",
            ("-2 days", id_a),
        )
        .unwrap();

    // Site B: episodes seen 8 days ago and 1 day ago -> span = 7
    // But since they merge into one franchise, the overall span is
    // min(10,8)=10 days ago to max(2,1)=1 day ago = 9 days
    let mut s2 = mk_airing("shared", "Shared Show", None);
    s2.is_airing = false;
    let id_b = db.upsert_series(b, &s2).unwrap();
    db.set_followed(id_b, true).unwrap();
    db.set_kind(id_b, "TV").unwrap();
    for n in 1..=2 {
        db.insert_episode(&crate::models::Episode {
            id: 0, series_id: id_b, number: n.to_string(), title: None,
            url: format!("https://site/b-{}/", n), released_at: None, seen: true, download_path: None, download_status: None,
        }).unwrap();
    }
    db.conn
        .execute(
            "UPDATE episodes SET seen_at=datetime('now', 'start of day', ?1, '+12 hours') WHERE series_id=?2 AND number='1'",
            ("-8 days", id_b),
        )
        .unwrap();
    db.conn
        .execute(
            "UPDATE episodes SET seen_at=datetime('now', 'start of day', ?1, '+12 hours') WHERE series_id=?2 AND number='2'",
            ("-1 days", id_b),
        )
        .unwrap();

    let avg = db.get_avg_completion_days().unwrap();
    // Merged franchise: first seen 10 days ago, last seen 1 day ago -> span = 9
    assert_eq!(avg, Some(9.0), "cross-site franchise merged, overall span 9 days");
}

#[test]
fn get_avg_completion_days_ignores_a_finished_show_the_user_barely_started() {
    // The metric used to filter only on `!any_airing` — "the show stopped
    // broadcasting" — instead of on the user's own progress. A finished
    // 100-episode show with one episode marked seen has a single `seen_at`,
    // so first == last and it contributed a 0-day "completion", dragging
    // the average toward zero.
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("A", "a", "animeytx").unwrap();

    let mut sampled = mk_airing("sampled", "Sampled Show", None);
    sampled.is_airing = false;
    let sid = db.upsert_series(src, &sampled).unwrap();
    db.set_followed(sid, true).unwrap();
    db.set_kind(sid, "TV").unwrap();
    for n in 1..=100 {
        db.insert_episode(&crate::models::Episode {
            id: 0, series_id: sid, number: n.to_string(), title: None,
            url: format!("https://site/sampled-capitulo-{n}/"),
            released_at: None, seen: false, download_path: None, download_status: None,
        }).unwrap();
    }
    db.set_seen_cascade(sid, "1", true).unwrap();

    assert_eq!(
        db.get_avg_completion_days().unwrap(), None,
        "1 of 100 episodes is not a completed franchise, however finished the show is"
    );

    // And with a genuinely completed franchise present it must not pull
    // that franchise's average down toward its own 0-day span.
    seed_finished_series(&db, src, "Show A", 10, 2);
    assert_eq!(
        db.get_avg_completion_days().unwrap(), Some(8.0),
        "only the franchise the user actually finished contributes its 8-day span"
    );
}

#[test]
fn get_avg_completion_days_merges_a_colon_arc_into_its_parent_franchise() {
    // This function used to group by bare `franchise_key`, unlike every
    // other franchise stat in this file: "One Piece: Wano" was its own
    // franchise with its own span. Grouped that way these are two 4-day
    // franchises averaging 4; merged into the one franchise they really
    // are, the span runs from 10 days ago to today = 10.
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("A", "a", "animeytx").unwrap();

    seed_finished_series(&db, src, "One Piece", 10, 6);
    seed_finished_series(&db, src, "One Piece: Wano", 4, 0);

    assert_eq!(
        db.get_avg_completion_days().unwrap(), Some(10.0),
        "the arc folds into One Piece, giving one franchise with one merged span"
    );
}

#[test]
fn get_avg_completion_days_uses_the_catalog_total_as_the_completion_bar() {
    // The site can list far fewer episodes than the show really has (the
    // classic long-runner shape). When a linked AniList row declares the
    // real total, that is the bar the user's progress has to clear.
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("A", "a", "animeytx").unwrap();

    db.upsert_catalog_anime(
        &crate::anilist::CatalogAnime {
            id: 920, title: "Long Runner".into(), title_romaji: None, title_english: None, synonyms: Vec::new(), next_airing_at: None, next_episode: None, tags: Vec::new(), relations: Vec::new(),
            cover_url: None, banner_url: None, format: Some("TV".into()), genres: vec![],
            episodes: Some(500), average_score: None, popularity: None,
            url: "https://anilist.co/anime/920".into(), status: None, duration: Some(24),
            studio: None, start_date: None,
        },
        0,
    ).unwrap();
    let sid = seed_finished_series(&db, src, "Long Runner", 10, 2);
    db.set_anilist_id(sid, 920).unwrap();

    assert_eq!(
        db.get_avg_completion_days().unwrap(), None,
        "2 seen episodes of a 500-episode show is not a completion"
    );
}

// ---- Popularity Bias (2026-08-24) ----

#[test]
fn get_popularity_bias_returns_none_when_fewer_than_3_linked_followed_series() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("A", "a", "animeytx").unwrap();

    // Insert 2 followed series linked to catalog with popularity
    for (slug, title, pop) in [
        ("s1", "Show One", 50000),
        ("s2", "Show Two", 100000),
    ] {
        db.upsert_catalog_anime(
            &crate::anilist::CatalogAnime {
                id: pop, title: title.into(), title_romaji: None, title_english: None, synonyms: Vec::new(), next_airing_at: None, next_episode: None, tags: Vec::new(), relations: Vec::new(),
                cover_url: None, banner_url: None, format: Some("TV".into()), genres: vec![],
                episodes: Some(12), average_score: None, popularity: Some(pop),
                url: format!("https://anilist.co/anime/{pop}"), status: None, duration: Some(24),
                studio: None, start_date: None,
            },
            0,
        ).unwrap();
        let id = db.upsert_series(src, &mk_airing(slug, title, None)).unwrap();
        db.set_followed(id, true).unwrap();
        db.set_anilist_id(id, pop).unwrap();
    }

    let bias = db.get_popularity_bias().unwrap();
    assert_eq!(bias.average_popularity, None);
    assert_eq!(bias.normalized_score, None);
    assert_eq!(bias.percentile, None);
}

#[test]
fn get_popularity_bias_returns_none_when_no_catalog_link() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("A", "a", "animeytx").unwrap();

    // 3 followed series but none linked to catalog
    for (slug, title) in [
        ("s1", "Show One"),
        ("s2", "Show Two"),
        ("s3", "Show Three"),
    ] {
        let id = db.upsert_series(src, &mk_airing(slug, title, None)).unwrap();
        db.set_followed(id, true).unwrap();
    }

    let bias = db.get_popularity_bias().unwrap();
    assert_eq!(bias.average_popularity, None);
    assert_eq!(bias.normalized_score, None);
    assert_eq!(bias.percentile, None);
}

#[test]
fn get_popularity_bias_dedups_cross_site_same_show() {
    let db = Db::open(":memory:").unwrap();
    let a = db.upsert_source("AnimeYT", "a", "animeytx").unwrap();
    let b = db.upsert_source("TioAnime", "b", "tioanime").unwrap();

    // Same show on both sites, linked to same AniList ID
    db.upsert_catalog_anime(
        &crate::anilist::CatalogAnime {
            id: 100, title: "Shared Show".into(), title_romaji: None, title_english: None, synonyms: Vec::new(), next_airing_at: None, next_episode: None, tags: Vec::new(), relations: Vec::new(),
            cover_url: None, banner_url: None, format: Some("TV".into()), genres: vec![],
            episodes: Some(12), average_score: None, popularity: Some(210000),
            url: "https://anilist.co/anime/100".into(), status: None, duration: Some(24),
            studio: None, start_date: None,
        },
        0,
    ).unwrap();

    let id_a = db.upsert_series(a, &mk_airing("shared", "Shared Show", None)).unwrap();
    db.set_followed(id_a, true).unwrap();
    db.set_anilist_id(id_a, 100).unwrap();

    let id_b = db.upsert_series(b, &mk_airing("shared", "Shared Show", None)).unwrap();
    db.set_followed(id_b, true).unwrap();
    db.set_anilist_id(id_b, 100).unwrap();

    // Two other distinct shows on site A to reach the 3 minimum
    for (slug, title, pop) in [
        ("s2", "Show Two", 50000),
        ("s3", "Show Three", 150000),
    ] {
        db.upsert_catalog_anime(
            &crate::anilist::CatalogAnime {
                id: pop, title: title.into(), title_romaji: None, title_english: None, synonyms: Vec::new(), next_airing_at: None, next_episode: None, tags: Vec::new(), relations: Vec::new(),
                cover_url: None, banner_url: None, format: Some("TV".into()), genres: vec![],
                episodes: Some(12), average_score: None, popularity: Some(pop),
                url: format!("https://anilist.co/anime/{pop}"), status: None, duration: Some(24),
                studio: None, start_date: None,
            },
            0,
        ).unwrap();
        let id = db.upsert_series(a, &mk_airing(slug, title, None)).unwrap();
        db.set_followed(id, true).unwrap();
        db.set_anilist_id(id, pop).unwrap();
    }

    let bias = db.get_popularity_bias().unwrap();
    assert!(bias.average_popularity.is_some());
    // Average = (210000 + 50000 + 150000) / 3 = 410000 / 3 ≈ 136666.67
    let avg = bias.average_popularity.unwrap();
    assert!((avg - 136666.67).abs() < 1.0, "average: {avg}");
    let norm = bias.normalized_score.unwrap();
    assert!((norm - 1.95238).abs() < 0.01, "normalized: {norm}"); // 136666.67 / 70000
}

#[test]
fn get_popularity_bias_normalizes_correctly() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("A", "a", "animeytx").unwrap();

    // 3 shows with popularity summing to 700,000 -> avg = 233,333.33 -> norm ≈ 3.33
    for (slug, title, pop) in [
        ("s1", "Low Pop", 10000),
        ("s2", "Mid Pop", 200000),
        ("s3", "High Pop", 490000),
    ] {
        db.upsert_catalog_anime(
            &crate::anilist::CatalogAnime {
                id: pop, title: title.into(), title_romaji: None, title_english: None, synonyms: Vec::new(), next_airing_at: None, next_episode: None, tags: Vec::new(), relations: Vec::new(),
                cover_url: None, banner_url: None, format: Some("TV".into()), genres: vec![],
                episodes: Some(12), average_score: None, popularity: Some(pop),
                url: format!("https://anilist.co/anime/{pop}"), status: None, duration: Some(24),
                studio: None, start_date: None,
            },
            0,
        ).unwrap();
        let id = db.upsert_series(src, &mk_airing(slug, title, None)).unwrap();
        db.set_followed(id, true).unwrap();
        db.set_anilist_id(id, pop).unwrap();
    }

    let bias = db.get_popularity_bias().unwrap();
    let avg = bias.average_popularity.unwrap();
    assert!((avg - 233333.33).abs() < 1.0, "average: {avg}");
    let norm = bias.normalized_score.unwrap();
    assert!((norm - 3.33333).abs() < 0.01, "normalized: {norm}"); // 233333.33 / 70000
}

#[test]
fn get_popularity_bias_clamps_score_at_ten() {
    // A followed list skewed toward AniList's most popular shows can
    // individually clear 700,000 popularity — well above the 70,000
    // divisor's own scale. The normalized score must clamp to 10, not
    // report an out-of-range value that would break the 0-10 slider.
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("A", "a", "animeytx").unwrap();
    for (slug, title, pop) in [
        ("s1", "Mega Hit 1", 900_000),
        ("s2", "Mega Hit 2", 850_000),
        ("s3", "Mega Hit 3", 950_000),
    ] {
        db.upsert_catalog_anime(
            &crate::anilist::CatalogAnime {
                id: pop, title: title.into(), title_romaji: None, title_english: None, synonyms: Vec::new(), next_airing_at: None, next_episode: None, tags: Vec::new(), relations: Vec::new(),
                cover_url: None, banner_url: None, format: Some("TV".into()), genres: vec![],
                episodes: Some(12), average_score: None, popularity: Some(pop),
                url: format!("https://anilist.co/anime/{pop}"), status: None, duration: Some(24),
                studio: None, start_date: None,
            },
            0,
        ).unwrap();
        let id = db.upsert_series(src, &mk_airing(slug, title, None)).unwrap();
        db.set_followed(id, true).unwrap();
        db.set_anilist_id(id, pop).unwrap();
    }

    let bias = db.get_popularity_bias().unwrap();
    let norm = bias.normalized_score.unwrap();
    assert_eq!(norm, 10.0, "score must clamp at 10, not exceed it: {norm}");
}

#[test]
fn get_genre_cards_counts_episodes_not_shows_and_sorts_by_them() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("A", "a", "animeytx").unwrap();

    // Action: 2 shows, 3 + 2 seen episodes = 5 episodes total.
    let a1 = db.upsert_series(src, &mk_airing("a1", "Action Show 1", None)).unwrap();
    db.set_followed(a1, true).unwrap();
    db.insert_series_genres(a1, &["Action".into()]).unwrap();
    insert_eps_seen_up_to(&db, a1, 3, 3);

    let a2 = db.upsert_series(src, &mk_airing("a2", "Action Show 2", None)).unwrap();
    db.set_followed(a2, true).unwrap();
    db.insert_series_genres(a2, &["Action".into()]).unwrap();
    insert_eps_seen_up_to(&db, a2, 2, 2);

    // Comedy: 1 show, 10 seen episodes — fewer shows but more episodes
    // than Action, so a shows-count sort would rank it below Action
    // while an episode-count sort ranks it above.
    let c1 = db.upsert_series(src, &mk_airing("c1", "Comedy Show", None)).unwrap();
    db.set_followed(c1, true).unwrap();
    db.insert_series_genres(c1, &["Comedy".into()]).unwrap();
    insert_eps_seen_up_to(&db, c1, 10, 10);

    let cards = db.get_genre_cards().unwrap();
    assert_eq!(cards.len(), 2);
    assert_eq!(cards[0].genre, "Comedy", "10 episodes beats Action's 5");
    assert_eq!(cards[0].count, 10);
    assert_eq!(cards[0].top_series.len(), 1);
    assert_eq!(cards[1].genre, "Action");
    assert_eq!(cards[1].count, 5, "sum of both Action shows' episodes, not the show count (2)");
    assert_eq!(cards[1].top_series.len(), 2);
}

#[test]
fn get_genre_cards_caps_thumbnails_but_counts_all_episodes() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("A", "a", "animeytx").unwrap();

    for (slug, title, eps) in [
        ("d1", "Drama 1", 1),
        ("d2", "Drama 2", 1),
        ("d3", "Drama 3", 1),
        ("d4", "Drama 4", 1),
        ("d5", "Drama 5", 1),
    ] {
        let id = db.upsert_series(src, &mk_airing(slug, title, None)).unwrap();
        db.set_followed(id, true).unwrap();
        db.insert_series_genres(id, &["Drama".into()]).unwrap();
        insert_eps_seen_up_to(&db, id, eps, eps);
    }

    let cards = db.get_genre_cards().unwrap();
    assert_eq!(cards.len(), 1);
    assert_eq!(cards[0].top_series.len(), CARD_COVERS, "thumbnails capped");
    assert_eq!(cards[0].count, 5, "but the episode count reflects all 5 shows");
}

// ---- Yearly activity heatmap (2026-08-24) ----

#[test]
fn get_yearly_activity_returns_full_year_zero_filled() {
    // Use 2025 (non-leap) to avoid the Feb 29 edge case.
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("A", "a", "animeytx").unwrap();
    let series = db.upsert_series(src, &mk_airing("x", "X", None)).unwrap();

    // Seed three marks on specific dates within 2025, and one in 2024 to
    // prove year-filtering works. Use mid-day times (12:00:00) so the
    // UTC→localtime conversion in the query never crosses a day boundary
    // regardless of the test machine's timezone.
    let eps = [
        ("1", "2025-01-15 12:00:00"),
        ("2", "2025-01-15 12:00:00"),
        ("3", "2025-06-20 12:00:00"),
        ("4", "2024-12-31 12:00:00"), // different year, must not appear
    ];
    for (num, seen_at) in eps {
        let (ep, _) = db.insert_episode(&crate::models::Episode {
            id: 0, series_id: series, number: num.into(), title: None,
            url: format!("https://site/x-{num}/"), released_at: None, seen: true, download_path: None, download_status: None,
        }).unwrap();
        // Overwrite seen_at to the exact timestamp we want (local time stored
        // as UTC in the DB; we rely on the query's 'localtime' conversion).
        db.conn
            .execute("UPDATE episodes SET seen_at = ?1 WHERE id = ?2", [seen_at, ep.to_string().as_str()])
            .unwrap();
    }

    let activity = db.get_yearly_activity(2025).unwrap();
    assert_eq!(activity.year, 2025);
    assert_eq!(activity.days.len(), 365, "non-leap year has 365 days");

    // Jan 15 should have 2 marks.
    let jan15 = activity.days.iter().find(|d| d.day == "2025-01-15").unwrap();
    assert_eq!(jan15.count, 2);

    // Jun 20 should have 1 mark.
    let jun20 = activity.days.iter().find(|d| d.day == "2025-06-20").unwrap();
    assert_eq!(jun20.count, 1);

    // Dec 31 2024 must not be counted (year filter).
    let dec31_2024 = activity.days.iter().find(|d| d.day == "2024-12-31");
    assert!(dec31_2024.is_none() || dec31_2024.unwrap().count == 0);

    // All other days must be zero.
    let nonzero = activity.days.iter().filter(|d| d.count > 0).count();
    assert_eq!(nonzero, 2, "only Jan 15 and Jun 20 have marks");
}

#[test]
fn get_activity_years_includes_current_year_even_when_empty() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("A", "a", "animeytx").unwrap();

    // No episodes at all — should still get the current year.
    let years = db.get_activity_years().unwrap();
    let this_year = chrono::Local::now().date_naive().year();
    assert!(years.contains(&this_year), "current year present even with no data: {years:?}");
    assert!(years.iter().all(|&y| y <= this_year), "no future years: {years:?}");
    assert!(years.windows(2).all(|w| w[0] > w[1]), "descending order: {years:?}");

    // Add a mark in a past year — both should appear.
    let series = db.upsert_series(src, &mk_airing("x", "X", None)).unwrap();
    let (ep, _) = db.insert_episode(&crate::models::Episode {
        id: 0, series_id: series, number: "1".into(), title: None,
        url: "https://site/x-1/".into(), released_at: None, seen: true, download_path: None, download_status: None,
    }).unwrap();
    let past_year = this_year - 1;
    db.conn
        .execute(
            "UPDATE episodes SET seen_at = ?1 WHERE id = ?2",
            [format!("{past_year}-07-15 12:00:00"), ep.to_string()],
        )
        .unwrap();

    let years = db.get_activity_years().unwrap();
    assert!(years.contains(&this_year), "current year still present: {years:?}");
    assert!(years.contains(&past_year), "past year with data present: {years:?}");
    assert!(years[0] == this_year, "current year first (descending): {years:?}");
}

#[test]
fn watch_history_stats_all_require_seen_and_seen_at_together() {
    // `seen` and `seen_at` are written together by `set_seen`/
    // `set_seen_cascade`, so today they always agree — but they are two
    // columns, and this file used to mix predicates: the hourly and dusty
    // queries required `seen=1 AND seen_at IS NOT NULL` while the day,
    // year and binge queries trusted `seen_at IS NOT NULL` alone. One
    // write that clears the flag without clearing the timestamp and the
    // same screen contradicts itself. Every one of them now asserts both,
    // which is what this row proves.
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("A", "a", "animeytx").unwrap();
    let sid = db.upsert_series(src, &mk_airing("x", "X", None)).unwrap();
    db.conn
        .execute(
            "INSERT INTO episodes (series_id, number, url, seen, seen_at)
             VALUES (?1, '1', 'u1', 0, datetime('now'))",
            [sid],
        )
        .unwrap();

    assert_eq!(db.get_binge_record().unwrap().count, 0, "binge record ignores an unseen row");
    let insights = db.get_watch_insights().unwrap();
    assert!(
        insights.marks_by_day.iter().all(|d| d.count == 0),
        "the 30-day spine ignores it too: {:?}",
        insights.marks_by_day
    );
    assert_eq!(insights.marks_tracked_since, None, "and it starts no tracking history");

    let this_year = chrono::Local::now().date_naive().year();
    assert!(
        db.get_yearly_activity(this_year).unwrap().days.iter().all(|d| d.count == 0),
        "the yearly heatmap ignores it"
    );
    assert_eq!(
        db.get_activity_years().unwrap(),
        vec![this_year],
        "only the always-present current year — no year invented by a stale seen_at"
    );
    assert!(
        db.get_hourly_distribution().unwrap().iter().all(|h| h.count == 0),
        "the hourly distribution already required both, and still does"
    );
}

#[test]
fn extra_external_minutes_stay_positive_when_an_arc_is_movie_length() {
    // The two sides of the "Ya lo vi" estimate used to be computed on
    // different bases: the minute remainder subtracted `real_minutes`
    // (each member row priced at its OWN per-episode rate — a "Pelicula"
    // arc at 100 min/ep) from the estimate's own minutes (24 min/ep), so a
    // franchise with a movie-format arc reported positive extra *episodes*
    // and zero extra *minutes* on the same page.
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("A", "a", "animeytx").unwrap();

    db.upsert_catalog_anime(
        &crate::anilist::CatalogAnime {
            id: 777, title: "Long Show".into(), title_romaji: None, title_english: None, synonyms: Vec::new(), next_airing_at: None, next_episode: None, tags: Vec::new(),
            cover_url: None, banner_url: None, format: Some("TV".into()), genres: vec![],
            episodes: Some(100), average_score: None, popularity: None,
            url: "https://anilist.co/anime/777".into(), status: None, duration: Some(24),
            studio: None, start_date: None, relations: Vec::new(),
        },
        0,
    ).unwrap();

    // 25 movie-length episodes really marked: 25 * 100 = 2500 minutes,
    // more than the 100-episode estimate's own 100 * 24 = 2400.
    let arc = db.upsert_series(src, &mk_airing("ls-movies", "Long Show: Peliculas", None)).unwrap();
    db.set_kind(arc, "Pelicula").unwrap();
    insert_eps_seen_up_to(&db, arc, 25, 25);

    let whole = db.upsert_series(src, &mk_airing("long-show", "Long Show", None)).unwrap();
    db.set_watched_externally(whole, true).unwrap();
    db.set_anilist_id(whole, 777).unwrap();

    let summary = db.get_watch_summary().unwrap();
    assert_eq!(summary.episodes_watched, 25);
    assert_eq!(summary.episodes_watched_external, 75, "100 - 25, the remainder only");

    let insights = db.get_watch_insights().unwrap();
    assert_eq!(insights.estimated_minutes_tracked, 25 * 100, "arc rows keep their own rate");
    assert_eq!(
        insights.estimated_minutes_external, 75 * 24,
        "the remainder priced at the estimate's own rate — never 0 while extra episodes are positive"
    );
    assert!(
        summary.episodes_watched_external > 0 && insights.estimated_minutes_external > 0,
        "episodes and minutes agree about whether there is anything extra at all"
    );
}

#[test]
fn franchise_grouping_folds_two_titles_that_share_an_anilist_id() {
    // Same show, two sites, genuinely different display titles. Grouping
    // by normalized title alone made them two franchises: two entries in
    // top_series, distinct_anime 2, and — because the per-site "best single
    // site" rule buckets by that key — both sites' marks credited in full
    // instead of the larger one winning.
    let db = Db::open(":memory:").unwrap();
    let a = db.upsert_source("AnimeYT", "a", "animeytx").unwrap();
    let b = db.upsert_source("TioAnime", "b", "tioanime").unwrap();

    db.upsert_catalog_anime(
        &crate::anilist::CatalogAnime {
            id: 16498, title: "Shingeki no Kyojin".into(), title_romaji: None,
            title_english: Some("Attack on Titan".into()), synonyms: Vec::new(), next_airing_at: None, next_episode: None, tags: Vec::new(), cover_url: None, banner_url: None,
            format: Some("TV".into()), genres: vec![], episodes: Some(25),
            average_score: None, popularity: None,
            url: "https://anilist.co/anime/16498".into(), status: None, duration: Some(24),
            studio: None, start_date: None, relations: Vec::new(),
        },
        0,
    ).unwrap();

    assert_ne!(
        franchise_key("Shingeki no Kyojin"), franchise_key("Attack on Titan"),
        "the two titles share no text — only the AniList link ties them together"
    );

    let jp = db.upsert_series(a, &mk_airing("snk", "Shingeki no Kyojin", None)).unwrap();
    db.set_followed(jp, true).unwrap();
    db.set_anilist_id(jp, 16498).unwrap();
    insert_eps_seen_up_to(&db, jp, 25, 10);

    let en = db.upsert_series(b, &mk_airing("aot", "Attack on Titan", None)).unwrap();
    db.set_followed(en, true).unwrap();
    db.set_anilist_id(en, 16498).unwrap();
    insert_eps_seen_up_to(&db, en, 25, 8);

    let insights = db.get_watch_insights().unwrap();
    assert_eq!(
        insights.top_series.len(), 1,
        "one franchise, not one per spelling: {:?}", insights.top_series
    );
    assert_eq!(
        insights.top_series[0].count, 10,
        "and the better site's 10 marks win, rather than 10 and 8 standing as two shows"
    );

    let summary = db.get_watch_summary().unwrap();
    assert_eq!(summary.distinct_anime, 1, "one anime watched, under either of its names");
    assert_eq!(summary.episodes_watched, 10);
}

#[test]
fn parent_franchise_tie_break_does_not_depend_on_row_order() {
    // Two rows share one `franchise_key` while proposing *different*
    // parents ("Alpha Beta" vs "Alpha"): their colon-heads differ but the
    // whole titles normalize identically. `parent_of` used to keep
    // whichever row SQLite returned first, so the same data could roll up
    // two different ways between runs.
    //
    // The two titles must still be *distinct* rows: `upsert_series` folds
    // a source's rows together by normalized title, so the season marker
    // is what keeps them two rows that nonetheless share one franchise key.
    assert_eq!(
        franchise_key("Alpha Beta: Gamma"), franchise_key("Alpha: Beta Gamma Temporada 2"),
        "the ambiguous case this test is about"
    );
    assert_ne!(
        crate::matching::normalize_title("Alpha Beta: Gamma"),
        crate::matching::normalize_title("Alpha: Beta Gamma Temporada 2"),
        "…and they survive `upsert_series`' own title-based dedup as two rows"
    );
    assert_eq!(franchise_parent_key("Alpha Beta: Gamma").as_deref(), Some("alpha beta"));
    assert_eq!(franchise_parent_key("Alpha: Beta Gamma Temporada 2").as_deref(), Some("alpha"));

    let seed = |titles: &[&str]| -> Vec<(String, i64)> {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("A", "a", "animeytx").unwrap();
        for (i, title) in titles.iter().enumerate() {
            let id = db.upsert_series(src, &mk_airing(&format!("s{i}"), title, None)).unwrap();
            insert_eps_seen_up_to(&db, id, 1, 1);
        }
        db.get_watch_insights()
            .unwrap()
            .top_series
            .into_iter()
            .map(|t| (t.title, t.count))
            .collect()
    };

    let forward = seed(&["Alpha", "Alpha Beta", "Alpha Beta: Gamma", "Alpha: Beta Gamma Temporada 2"]);
    let reversed = seed(&["Alpha: Beta Gamma Temporada 2", "Alpha Beta: Gamma", "Alpha Beta", "Alpha"]);
    assert_eq!(forward, reversed, "same data, same roll-up, whatever order the rows arrive in");
    assert_eq!(
        forward,
        vec![("Alpha".to_string(), 3), ("Alpha Beta".to_string(), 1)],
        "and the winner is the one `prefer_display_title` picks — the shorter parent key"
    );
}

#[test]
fn the_30_day_spine_counts_exactly_the_rows_it_can_plot() {
    // A future-dated `seen_at` (clock skew, or a backup restored from a
    // machine running ahead) passed the query's lower bound and then had no
    // bucket to land in — counted by the SQL, dropped by the spine, so the
    // chart's total disagreed with a raw count of its own source rows.
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("A", "a", "animeytx").unwrap();
    let sid = db.upsert_series(src, &mk_airing("x", "X", None)).unwrap();
    insert_eps_seen_up_to(&db, sid, 3, 3);
    db.conn
        .execute(
            "UPDATE episodes SET seen_at = datetime('now', '+3 days') WHERE number = '3'",
            [],
        )
        .unwrap();

    let insights = db.get_watch_insights().unwrap();
    let plotted: i64 = insights.marks_by_day.iter().map(|d| d.count).sum();
    let in_window: i64 = db
        .conn
        .query_row(
            "SELECT COUNT(*) FROM episodes
             WHERE seen = 1 AND seen_at IS NOT NULL
               AND DATE(seen_at, 'localtime') >= DATE('now', 'localtime', '-29 days')
               AND DATE(seen_at, 'localtime') <= DATE('now', 'localtime')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(insights.marks_by_day.len(), 30);
    assert_eq!(in_window, 2, "two of the three marks are dated today");
    assert_eq!(
        plotted, in_window,
        "the chart's total is exactly the rows its 30 buckets can hold: {:?}",
        insights.marks_by_day
    );
}

#[test]
fn favorite_genres_see_the_whole_library_not_just_the_active_site() {
    // "Tus géneros favoritos" reused the swipe deck's per-site
    // `get_genre_affinity`, so a genre whose only evidence sat on a
    // non-active site was invisible — and a show followed on two sites
    // counted its taste signal twice.
    let db = Db::open(":memory:").unwrap();
    let a = db.upsert_source("AnimeYT", "a", "animeytx").unwrap();
    let b = db.upsert_source("TioAnime", "b", "tioanime").unwrap();

    // Only on the non-active site.
    let only_b = db.upsert_series(b, &mk_airing("mecha", "Mecha Show", None)).unwrap();
    db.set_followed(only_b, true).unwrap();
    db.insert_series_genres(only_b, &["Mecha".to_string()]).unwrap();

    // Followed on both sites: one show, one +2, not two.
    for src in [a, b] {
        let dup = db.upsert_series(src, &mk_airing("dup", "Dup Show", None)).unwrap();
        db.set_followed(dup, true).unwrap();
        db.insert_series_genres(dup, &["Drama".to_string()]).unwrap();
    }

    let favorites = db.get_favorite_genres(5).unwrap();
    let mecha = favorites.iter().find(|g| g.genre == "Mecha");
    assert!(
        mecha.is_some(),
        "a genre followed only on the non-active site still counts: {favorites:?}"
    );
    assert_eq!(mecha.unwrap().score, 2.0);
    let drama = favorites.iter().find(|g| g.genre == "Drama").expect("Drama");
    assert_eq!(drama.score, 2.0, "one canonical show contributes +2 once, not once per site");

    // The deck's own per-site query is untouched and still site-scoped —
    // which is exactly why it was the wrong source for this display.
    let deck_scores = db.get_genre_affinity(a).unwrap();
    assert!(
        !deck_scores.contains_key("Mecha"),
        "the per-site affinity the swipe deck uses still sees only site A"
    );
}

#[test]
fn taste_signals_across_sites_weights_tags_and_studios() {
    let db = Db::open(":memory:").unwrap();

    // 0. Empty DB -> both maps empty (cold start)
    let empty = db.get_taste_signals_across_sites().unwrap();
    assert!(empty.tag_affinity.is_empty(), "empty DB -> tag_affinity empty");
    assert!(empty.studio_affinity.is_empty(), "empty DB -> studio_affinity empty");

    let src_a = db.upsert_source("SiteA", "https://a.example", "site_a").unwrap();
    let src_b = db.upsert_source("SiteB", "https://b.example", "site_b").unwrap();

    // Catalog entries:
    // Anime 1: tag "Isekai" (rank 80), tag "LowRank" (rank 40), studio "Bones"
    let anime1 = crate::anilist::CatalogAnime {
        id: 1, title: "Isekai Bones".into(), title_romaji: None, title_english: None,
        cover_url: None, banner_url: None, format: None, episodes: None, average_score: None, popularity: None,
        url: "x".into(), genres: vec![], synonyms: vec![],
        tags: vec![
            crate::anilist::CatalogTag { name: "Isekai".into(), rank: 80 },
            crate::anilist::CatalogTag { name: "LowRank".into(), rank: 40 },
        ],
        relations: vec![], status: None, duration: None,
        studio: Some("Bones".into()),
        start_date: None, next_airing_at: None, next_episode: None,
    };
    db.upsert_catalog_anime(&anime1, 0).unwrap();

    // Anime 2: tag "Isekai" (rank 75), studio "Mappa"
    let anime2 = crate::anilist::CatalogAnime {
        id: 2, title: "Isekai Mappa".into(), title_romaji: None, title_english: None,
        cover_url: None, banner_url: None, format: None, episodes: None, average_score: None, popularity: None,
        url: "x".into(), genres: vec![], synonyms: vec![],
        tags: vec![
            crate::anilist::CatalogTag { name: "Isekai".into(), rank: 75 },
        ],
        relations: vec![], status: None, duration: None,
        studio: Some("Mappa".into()),
        start_date: None, next_airing_at: None, next_episode: None,
    };
    db.upsert_catalog_anime(&anime2, 1).unwrap();

    // Anime 3: tag "Gore" (rank 90), studio "Wit"
    let anime3 = crate::anilist::CatalogAnime {
        id: 3, title: "Gore Wit".into(), title_romaji: None, title_english: None,
        cover_url: None, banner_url: None, format: None, episodes: None, average_score: None, popularity: None,
        url: "x".into(), genres: vec![], synonyms: vec![],
        tags: vec![
            crate::anilist::CatalogTag { name: "Gore".into(), rank: 90 },
        ],
        relations: vec![], status: None, duration: None,
        studio: Some("Wit".into()),
        start_date: None, next_airing_at: None, next_episode: None,
    };
    db.upsert_catalog_anime(&anime3, 2).unwrap();

    // Followed linked show (Anime 1) on Site A:
    let s1_a = db.upsert_series(src_a, &mk_airing("s1-a", "Isekai Bones", None)).unwrap();
    db.set_followed(s1_a, true).unwrap();
    db.set_anilist_id(s1_a, 1).unwrap();

    // Duplicate of Anime 1 on Site B (followed): same show on two sites must count once (+2 once)
    let s1_b = db.upsert_series(src_b, &mk_airing("s1-b", "Isekai Bones", None)).unwrap();
    db.set_followed(s1_b, true).unwrap();
    db.set_anilist_id(s1_b, 1).unwrap();

    // 'want' show (Anime 2) on Site A (+1):
    let s2 = db.upsert_series(src_a, &mk_airing("s2", "Isekai Mappa", None)).unwrap();
    db.conn.execute("UPDATE series SET backlog_status = 'want' WHERE id = ?1", [s2]).unwrap();
    db.set_anilist_id(s2, 2).unwrap();

    // Discarded show (Anime 3) on Site A (-1.5):
    let s3 = db.upsert_series(src_a, &mk_airing("s3", "Gore Wit", None)).unwrap();
    db.conn.execute("UPDATE series SET backlog_status = 'discarded' WHERE id = ?1", [s3]).unwrap();
    db.set_anilist_id(s3, 3).unwrap();

    // Unlinked series on Site A (followed=1, but anilist_id is NULL) -> must contribute nothing
    let unlinked = db.upsert_series(src_a, &mk_airing("unlinked", "Unlinked Show", None)).unwrap();
    db.set_followed(unlinked, true).unwrap();

    let signals = db.get_taste_signals_across_sites().unwrap();

    // Isekai had raw 2.0 (from Anime 1, counted once) + 1.0 (from Anime 2) = 3.0. Max raw = 3.0 -> normalized to 1.0.
    assert_eq!(signals.tag_affinity.get("Isekai"), Some(&1.0));

    // Gore had raw -1.5 (from Anime 3 discarded) -> negative, dropped
    assert!(!signals.tag_affinity.contains_key("Gore"));

    // LowRank had rank 40 < 50 -> ignored
    assert!(!signals.tag_affinity.contains_key("LowRank"));

    // Bones had raw 2.0 (Anime 1 counted once despite two sites).
    // Mappa had raw 1.0 (Anime 2 want).
    // Wit had raw -1.5 (Anime 3 discarded) -> dropped.
    // Max positive studio = 2.0 (Bones). Bones = 2.0/2.0 = 1.0. Mappa = 1.0/2.0 = 0.5.
    assert_eq!(signals.studio_affinity.get("Bones"), Some(&1.0));
    assert_eq!(signals.studio_affinity.get("Mappa"), Some(&0.5));
    assert!(!signals.studio_affinity.contains_key("Wit"));
}

#[test]
fn effective_genres_prefer_catalog_when_linked() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("TestSite", "https://site.example", "testsite").unwrap();

    let mut anime = crate::anilist::CatalogAnime {
        id: 100, title: "Test".into(), title_romaji: None, title_english: None,
        cover_url: None, banner_url: None, format: None, episodes: None, average_score: None, popularity: None,
        url: "x".into(), genres: vec!["Action".into(), "Drama".into()], synonyms: vec![], tags: vec![], relations: vec![],
        status: None, duration: None, studio: None, start_date: None,
        next_airing_at: None, next_episode: None,
    };
    db.upsert_catalog_anime(&anime, 0).unwrap();

    let sid1 = db.upsert_series(src, &mk_airing("linked-with-genres", "Linked", None)).unwrap();
    db.set_followed(sid1, true).unwrap();
    db.insert_series_genres(sid1, &["Seinen".into()]).unwrap();
    db.set_anilist_id(sid1, 100).unwrap();

    let sid2 = db.upsert_series(src, &mk_airing("unlinked", "Unlinked", None)).unwrap();
    db.set_followed(sid2, true).unwrap();
    db.insert_series_genres(sid2, &["Seinen".into()]).unwrap();

    anime.id = 101;
    anime.genres = vec![];
    db.upsert_catalog_anime(&anime, 1).unwrap();

    let sid3 = db.upsert_series(src, &mk_airing("linked-no-genres", "LinkedNoGenres", None)).unwrap();
    db.set_followed(sid3, true).unwrap();
    db.insert_series_genres(sid3, &["Comedy".into()]).unwrap();
    db.set_anilist_id(sid3, 101).unwrap();

    let stats = db.get_genre_stats().unwrap();

    let has_action = stats.iter().any(|g| g.genre == "Action");
    let has_drama = stats.iter().any(|g| g.genre == "Drama");
    let seinen_count = stats.iter().find(|g| g.genre == "Seinen").map(|g| g.count).unwrap_or(0);
    let has_comedy = stats.iter().any(|g| g.genre == "Comedy");

    assert!(has_action, "reports Action from catalog");
    assert!(has_drama, "reports Drama from catalog");
    assert_eq!(seinen_count, 0, "site-only Seinen is not a real genre");
    assert!(has_comedy, "reports Comedy from site because catalog had NO genres");
}

#[test]
fn type_stats_uses_catalog_format_when_linked() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("TestSite", "https://site.example", "testsite").unwrap();

    let anime = crate::anilist::CatalogAnime {
        id: 100, title: "Test".into(), title_romaji: None, title_english: None,
        cover_url: None, banner_url: None, format: Some("TV_SHORT".into()), episodes: None, average_score: None, popularity: None,
        url: "x".into(), genres: vec![], synonyms: vec![], tags: vec![], relations: vec![],
        status: None, duration: None, studio: None, start_date: None,
        next_airing_at: None, next_episode: None,
    };
    db.upsert_catalog_anime(&anime, 0).unwrap();

    let sid1 = db.upsert_series(src, &mk_airing("linked", "Linked", None)).unwrap();
    db.conn.execute("UPDATE series SET kind='Película' WHERE id=?1", [sid1]).unwrap();
    db.set_followed(sid1, true).unwrap();
    db.set_anilist_id(sid1, 100).unwrap();

    let sid2 = db.upsert_series(src, &mk_airing("unlinked", "Unlinked", None)).unwrap();
    db.conn.execute("UPDATE series SET kind='OVA' WHERE id=?1", [sid2]).unwrap();
    db.set_followed(sid2, true).unwrap();

    let stats = db.get_type_stats().unwrap();
    let has_tv = stats.iter().any(|s| s.kind == "TV");
    let has_ova = stats.iter().any(|s| s.kind == "OVA");
    let has_pelicula = stats.iter().any(|s| s.kind == "Película" || s.kind == "MOVIE");

    assert!(has_tv, "catalog TV_SHORT maps to TV");
    assert!(has_ova, "site OVA kept when unlinked");
    assert!(!has_pelicula, "site Película ignored when linked");
}

#[test]
fn effective_kind_puts_both_sources_in_the_library_vocabulary() {
    assert_eq!(effective_kind(Some("MOVIE"), Some("Pelicula")).as_deref(), Some("MOVIE"));
    assert_eq!(effective_kind(None, Some("Pelicula")).as_deref(), Some("MOVIE"));
    assert_eq!(effective_kind(None, Some("TV Show")).as_deref(), Some("TV"));
    assert_eq!(effective_kind(None, Some("Anime")).as_deref(), Some("TV"));
    assert_eq!(effective_kind(Some("TV_SHORT"), None).as_deref(), Some("TV"));
    assert_eq!(effective_kind(None, Some("Sin Censura")).as_deref(), Some("Sin Censura"));
    assert_eq!(effective_kind(None, None), None);
}

#[test]
fn linked_and_unlinked_series_share_one_genre_bucket_across_vocabularies() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("TestSite", "https://site.example", "testsite").unwrap();
    let mut anime = crate::db::test_support::catalog_anime(100, "Test", &["Action"]);
    anime.id = 100;
    db.upsert_catalog_anime(&anime, 0).unwrap();
    let linked = db.upsert_series(src, &mk_airing("a", "Linked", None)).unwrap();
    db.set_followed(linked, true).unwrap();
    db.set_anilist_id(linked, 100).unwrap();
    let unlinked = db.upsert_series(src, &mk_airing("b", "Unlinked", None)).unwrap();
    db.set_followed(unlinked, true).unwrap();
    db.insert_series_genres(unlinked, &["Acción".into()]).unwrap();

    let stats = db.get_genre_stats().unwrap();
    let action = stats.iter().find(|g| g.genre == "Action").map(|g| g.count);
    assert_eq!(action, Some(2), "Acción (site) and Action (AniList) are one genre: {stats:?}");
    assert!(!stats.iter().any(|g| g.genre == "Acción"));
}

#[test]
fn series_effective_genres_is_fast_at_real_size() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("TestSite", "https://site.example", "testsite").unwrap();

    let tx = db.conn.unchecked_transaction().unwrap();
    for i in 1..=22000 {
        tx.execute(
            "INSERT INTO anilist_catalog(id, title, url, sort_order, metadata_version) VALUES(?1, 'Cat', 'x', ?1, 1)",
            [i],
        ).unwrap();
        for j in 1..=3 {
            tx.execute(
                "INSERT INTO anilist_catalog_genres(anilist_id, genre) VALUES(?1, ?2)",
                (i, format!("Genre{}", j)),
            ).unwrap();
        }
    }
    for i in 1..=6000 {
        let anilist_id = if i % 2 == 0 { Some(i) } else { None };
        tx.execute(
            "INSERT INTO series(id, source_id, slug, title, url, anilist_id) VALUES(?1, ?2, ?3, 'Series', 'x', ?4)",
            (i, src, format!("slug{i}"), anilist_id),
        ).unwrap();
        for j in 1..=2 {
            tx.execute(
                "INSERT INTO series_genres(series_id, genre) VALUES(?1, ?2)",
                (i, format!("SiteGenre{}", j)),
            ).unwrap();
        }
    }
    tx.commit().unwrap();

    let started = std::time::Instant::now();
    let count: i64 = db.conn.query_row("SELECT COUNT(*) FROM series_effective_genres", [], |r| r.get(0)).unwrap();
    let took = started.elapsed();

    assert!(count > 0);
    assert!(took < std::time::Duration::from_secs(1), "series_effective_genres full scan took {took:?}");
}

#[test]
fn episodes_total_is_never_below_episodes_watched() {
    // Two arcs on two sites, both watched, plus an unseen followed row of
    // the parent show: the denominator's grouping merges the arcs while the
    // numerator's does not. The ratio must still be <= 100 %.
    let db = Db::open(":memory:").unwrap();
    let a = db.upsert_source("A", "a", "animeytx").unwrap();
    let b = db.upsert_source("B", "b", "tioanime").unwrap();
    let arc_a = db.upsert_series(a, &mk_airing("a1", "Big Show: Arc One", None)).unwrap();
    db.set_followed(arc_a, true).unwrap();
    insert_eps_seen_up_to(&db, arc_a, 10, 10);
    let arc_b = db.upsert_series(b, &mk_airing("b1", "Big Show: Arc Two", None)).unwrap();
    db.set_followed(arc_b, true).unwrap();
    insert_eps_seen_up_to(&db, arc_b, 10, 10);
    let parent = db.upsert_series(a, &mk_airing("p", "Big Show", None)).unwrap();
    db.set_followed(parent, true).unwrap();

    let summary = db.get_watch_summary().unwrap();
    assert!(
        summary.episodes_total >= summary.episodes_watched,
        "total {} must not be below watched {}",
        summary.episodes_total,
        summary.episodes_watched
    );
}

#[test]
fn canonical_totals_are_fast_at_real_size() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("A", "a", "animeytx").unwrap();
    let tx = db.conn.unchecked_transaction().unwrap();
    for i in 1..=6000 {
        tx.execute(
            "INSERT INTO series(id, source_id, slug, title, url, followed) VALUES(?1, ?2, ?3, ?4, 'x', ?5)",
            (i, src, format!("slug{i}"), format!("Show Number {i}"), i % 2),
        ).unwrap();
        for n in 1..=20 {
            tx.execute(
                "INSERT INTO episodes(series_id, number, url, seen) VALUES(?1, ?2, ?3, ?4)",
                (i, n.to_string(), format!("https://site/{i}/{n}"), n % 5 == 0),
            ).unwrap();
        }
    }
    tx.commit().unwrap();

    let started = std::time::Instant::now();
    let totals = db.canonical_franchise_episode_totals(false).unwrap();
    let took = started.elapsed();
    assert!(!totals.is_empty());
    assert!(took < std::time::Duration::from_secs(1), "canonical totals took {took:?} for 6k series / 120k episodes");
}

#[test]
fn t4_a_same_show_two_sites_episodes_total() {
    let db = Db::open(":memory:").unwrap();
    let src1 = db.upsert_source("A", "a", "animeytx").unwrap();
    let src2 = db.upsert_source("B", "b", "tioanime").unwrap();

    let s1 = db.upsert_series(src1, &mk_airing("s1", "Same Show", None)).unwrap();
    db.set_followed(s1, true).unwrap();
    insert_eps_seen_up_to(&db, s1, 12, 6);

    let s2 = db.upsert_series(src2, &mk_airing("s2", "Same Show", None)).unwrap();
    db.set_followed(s2, true).unwrap();
    insert_eps_seen_up_to(&db, s2, 12, 6);

    let summary = db.get_watch_summary().unwrap();
    assert_eq!(summary.episodes_total, 12, "episodes_total must not double count");
    assert_eq!(summary.episodes_watched, 6, "episodes_watched must not double count");
}

#[test]
fn t4_b_linked_show_catalog_exceeds_scraped() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("A", "a", "animeytx").unwrap();

    db.upsert_catalog_anime(
        &crate::anilist::CatalogAnime {
            id: 100, title: "Linked Show".into(), title_romaji: None, title_english: None, synonyms: Vec::new(), next_airing_at: None, next_episode: None, tags: Vec::new(),
            cover_url: None, banner_url: None, format: Some("TV".into()), genres: vec![],
            episodes: Some(24), average_score: None, popularity: None,
            url: "https://anilist.co/anime/100".into(), status: None, duration: Some(24),
            studio: None, start_date: None, relations: Vec::new(),
        },
        0,
    ).unwrap();

    let s1 = db.upsert_series(src, &mk_airing("s1", "Linked Show", None)).unwrap();
    db.set_followed(s1, true).unwrap();
    db.set_anilist_id(s1, 100).unwrap();
    insert_eps_seen_up_to(&db, s1, 13, 0);

    let summary = db.get_watch_summary().unwrap();
    assert_eq!(summary.episodes_total, 24, "catalog episodes win when larger");
}

#[test]
fn t4_c_long_runner_catalog_null_uses_best_site_row_count() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("A", "a", "animeytx").unwrap();
    let src2 = db.upsert_source("B", "b", "tioanime").unwrap();

    db.upsert_catalog_anime(
        &crate::anilist::CatalogAnime {
            id: 100, title: "Long Runner".into(), title_romaji: None, title_english: None, synonyms: Vec::new(), next_airing_at: None, next_episode: None, tags: Vec::new(),
            cover_url: None, banner_url: None, format: Some("TV".into()), genres: vec![],
            episodes: None, average_score: None, popularity: None,
            url: "https://anilist.co/anime/100".into(), status: None, duration: Some(24),
            studio: None, start_date: None, relations: Vec::new(),
        },
        0,
    ).unwrap();

    let s1 = db.upsert_series(src, &mk_airing("s1", "Long Runner", None)).unwrap();
    db.set_followed(s1, true).unwrap();
    db.set_anilist_id(s1, 100).unwrap();
    insert_eps_seen_up_to(&db, s1, 50, 0);

    let s2 = db.upsert_series(src2, &mk_airing("s2", "Long Runner", None)).unwrap();
    db.set_followed(s2, true).unwrap();
    db.set_anilist_id(s2, 100).unwrap();
    insert_eps_seen_up_to(&db, s2, 75, 0);

    let summary = db.get_watch_summary().unwrap();
    assert_eq!(summary.episodes_total, 75, "best site row count wins when catalog is null");
}

#[test]
fn t4_d_followed_series_zero_seen_counts_in_total() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("A", "a", "animeytx").unwrap();

    let s1 = db.upsert_series(src, &mk_airing("s1", "Zero Seen", None)).unwrap();
    db.set_followed(s1, true).unwrap();
    insert_eps_seen_up_to(&db, s1, 12, 0);

    let summary = db.get_watch_summary().unwrap();
    assert_eq!(summary.episodes_total, 12, "followed but not seen still counts in episodes_total");
}

#[test]
fn t4_e_avg_episodes_two_sites_one_show() {
    let db = Db::open(":memory:").unwrap();
    let src1 = db.upsert_source("A", "a", "animeytx").unwrap();
    let src2 = db.upsert_source("B", "b", "tioanime").unwrap();

    let s1 = db.upsert_series(src1, &mk_airing("s1", "Same Show", None)).unwrap();
    db.set_followed(s1, true).unwrap();
    insert_eps_seen_up_to(&db, s1, 12, 0);

    let s2 = db.upsert_series(src2, &mk_airing("s2", "Same Show", None)).unwrap();
    db.set_followed(s2, true).unwrap();
    insert_eps_seen_up_to(&db, s2, 12, 0);

    let insights = db.get_watch_insights().unwrap();
    assert_eq!(insights.avg_episodes_per_series, 12.0, "average over one franchise, not two 12-episode shows");
}

#[test]
fn t4_f_completion_days_ignores_0_day_franchise() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("A", "a", "animeytx").unwrap();

    let s1 = mk_airing("s1", "Zero Day Show", None);
    let id1 = db.upsert_series(src, &s1).unwrap();
    db.set_followed(id1, true).unwrap();
    db.conn.execute("UPDATE series SET is_airing=0 WHERE id=?1", [id1]).unwrap();
    for n in 1..=2 {
        db.insert_episode(&crate::models::Episode {
            id: 0, series_id: id1, number: n.to_string(), title: None,
            url: format!("https://site/{n}/"), released_at: None, seen: true,
            download_path: None, download_status: None,
        }).unwrap();
        // Same day for both -> 0-day span
        db.conn.execute(
            "UPDATE episodes SET seen_at=datetime('now', 'start of day', '-5 days', '+12 hours') WHERE series_id=?1 AND number=?2",
            [id1.to_string(), n.to_string()],
        ).unwrap();
    }

    let avg = db.get_avg_completion_days().unwrap();
    assert_eq!(avg, None, "only 0-day franchises exist, must return None");

    seed_finished_series(&db, src, "Normal Show", 10, 2); // 8-day span
    let avg2 = db.get_avg_completion_days().unwrap();
    assert_eq!(avg2, Some(8.0), "0-day show ignored, only 8-day show counts");
}

#[test]
fn t6_backlog_two_sites_of_one_show_count_once() {
    let db = Db::open(":memory:").unwrap();
    let src1 = db.upsert_source("A", "a", "animeytx").unwrap();
    let src2 = db.upsert_source("B", "b", "tioanime").unwrap();

    let s1 = db.upsert_series(src1, &mk_airing("s1", "Same Show", None)).unwrap();
    db.set_followed(s1, true).unwrap();
    insert_eps_seen_up_to(&db, s1, 10, 4); // available 10, seen 4. remaining 6

    let s2 = db.upsert_series(src2, &mk_airing("s2", "Same Show", None)).unwrap();
    db.set_followed(s2, true).unwrap();
    insert_eps_seen_up_to(&db, s2, 12, 4); // available 12, seen 4. remaining 8

    let insights = db.get_watch_insights().unwrap();
    // most available (12) minus most marked on any site (4)
    assert_eq!(insights.backlog_episodes, 8);
    assert_eq!(insights.backlog_minutes, 8 * 24); // default for TV (assumed)
}

#[test]
fn t6_backlog_airing_show_counts_only_available_rows() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("A", "a", "animeytx").unwrap();

    let s = db.upsert_series(src, &mk_airing("s", "Airing", None)).unwrap();
    db.set_followed(s, true).unwrap();
    db.set_anilist_id(s, 100).unwrap();
    db.upsert_catalog_anime(&crate::anilist::CatalogAnime {
        id: 100, title: "Airing".into(), title_romaji: None, title_english: None, synonyms: Vec::new(), next_airing_at: None, next_episode: None, tags: Vec::new(),
        cover_url: None, banner_url: None, format: Some("TV".into()), genres: vec![],
        episodes: Some(24), average_score: None, popularity: None,
        url: "x".into(), status: Some("RELEASING".into()), duration: Some(25),
        studio: None, start_date: None, relations: Vec::new(),
    }, 0).unwrap();
    insert_eps_seen_up_to(&db, s, 3, 0); // available 3

    let insights = db.get_watch_insights().unwrap();
    assert_eq!(insights.backlog_episodes, 3, "counts available rows, not catalog episodes");
    assert_eq!(insights.backlog_minutes, 3 * 25, "linked duration beats estimate");
}

#[test]
fn t6_backlog_watched_franchise_gives_zero() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("A", "a", "animeytx").unwrap();

    let s = db.upsert_series(src, &mk_airing("s", "Watched", None)).unwrap();
    db.set_followed(s, true).unwrap();
    db.conn.execute("UPDATE series SET watched_externally=1 WHERE id=?1", [s]).unwrap();

    let insights = db.get_watch_insights().unwrap();
    assert_eq!(insights.backlog_episodes, 0, "watched_externally with no rows gives 0");
}

#[test]
fn t6_studios_and_era() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("A", "a", "animeytx").unwrap();

    let s1 = db.upsert_series(src, &mk_airing("s1", "Show 1", None)).unwrap();
    db.set_followed(s1, true).unwrap();
    db.set_anilist_id(s1, 1).unwrap();
    db.upsert_catalog_anime(&crate::anilist::CatalogAnime {
        id: 1, title: "S1".into(), title_romaji: None, title_english: None, synonyms: Vec::new(), next_airing_at: None, next_episode: None, tags: Vec::new(),
        cover_url: None, banner_url: None, format: Some("TV".into()), genres: vec![],
        episodes: Some(12), average_score: None, popularity: None,
        url: "x".into(), status: None, duration: Some(24),
        studio: Some("Bones".into()), start_date: Some(946684800), relations: Vec::new(),
    }, 0).unwrap();

    let s2 = db.upsert_series(src, &mk_airing("s2", "Show 2", None)).unwrap();
    db.set_followed(s2, true).unwrap();
    db.set_anilist_id(s2, 2).unwrap();
    db.upsert_catalog_anime(&crate::anilist::CatalogAnime {
        id: 2, title: "S2".into(), title_romaji: None, title_english: None, synonyms: Vec::new(), next_airing_at: None, next_episode: None, tags: Vec::new(),
        cover_url: None, banner_url: None, format: Some("TV".into()), genres: vec![],
        episodes: Some(12), average_score: None, popularity: None,
        url: "x".into(), status: None, duration: Some(24),
        studio: Some("Bones".into()), start_date: Some(1262304000), relations: Vec::new(),
    }, 0).unwrap();

    let s3 = db.upsert_series(src, &mk_airing("s3", "Show 1: Arc 2", None)).unwrap();
    db.set_followed(s3, true).unwrap();
    db.set_anilist_id(s3, 3).unwrap();
    db.upsert_catalog_anime(&crate::anilist::CatalogAnime {
        id: 3, title: "S3".into(), title_romaji: None, title_english: None, synonyms: Vec::new(), next_airing_at: None, next_episode: None, tags: Vec::new(),
        cover_url: None, banner_url: None, format: Some("TV".into()), genres: vec![],
        episodes: Some(12), average_score: None, popularity: None,
        url: "x".into(), status: None, duration: Some(24),
        studio: Some("Bones".into()), start_date: Some(978307200), relations: Vec::new(),
    }, 0).unwrap();

    let s4 = db.upsert_series(src, &mk_airing("s4", "Show 4", None)).unwrap();
    db.set_followed(s4, true).unwrap();
    db.set_anilist_id(s4, 4).unwrap();
    db.upsert_catalog_anime(&crate::anilist::CatalogAnime {
        id: 4, title: "S4".into(), title_romaji: None, title_english: None, synonyms: Vec::new(), next_airing_at: None, next_episode: None, tags: Vec::new(),
        cover_url: None, banner_url: None, format: Some("TV".into()), genres: vec![],
        episodes: Some(12), average_score: None, popularity: None,
        url: "x".into(), status: None, duration: Some(24),
        studio: Some("Mappa".into()), start_date: Some(1262304000), relations: Vec::new(),
    }, 0).unwrap();

    let insights = db.get_watch_insights().unwrap();
    assert_eq!(insights.top_studios.len(), 2);
    assert_eq!(insights.top_studios[0].title, "Bones");
    assert_eq!(insights.top_studios[0].count, 2);
    assert_eq!(insights.top_studios[1].title, "Mappa");
    assert_eq!(insights.top_studios[1].count, 1);

    let eras = insights.era_distribution;
    assert_eq!(eras.len(), 2);
    assert_eq!(eras[0].decade, 2000);
    assert_eq!(eras[0].count, 1);
    assert_eq!(eras[1].decade, 2010);
    assert_eq!(eras[1].count, 2);
}

#[test]
fn t6_binge_top_series_and_empty() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("A", "a", "animeytx").unwrap();

    let binge_empty = db.get_binge_record().unwrap();
    assert_eq!(binge_empty.day, None);
    assert_eq!(binge_empty.count, 0);
    assert!(binge_empty.top_series.is_empty());

    let s1 = db.upsert_series(src, &mk_airing("s1", "A Show", None)).unwrap();
    let s2 = db.upsert_series(src, &mk_airing("s2", "B Show", None)).unwrap();

    for n in 1..=3 {
        db.insert_episode(&crate::models::Episode {
            id: 0, series_id: s1, number: n.to_string(), title: None,
            url: format!("https://site/a/{n}/"), released_at: None, seen: true,
            download_path: None, download_status: None,
        }).unwrap();
        db.conn.execute("UPDATE episodes SET seen_at=datetime('now') WHERE series_id=?1 AND number=?2", [s1, n]).unwrap();
    }

    for n in 1..=2 {
        db.insert_episode(&crate::models::Episode {
            id: 0, series_id: s2, number: n.to_string(), title: None,
            url: format!("https://site/b/{n}/"), released_at: None, seen: true,
            download_path: None, download_status: None,
        }).unwrap();
        db.conn.execute("UPDATE episodes SET seen_at=datetime('now') WHERE series_id=?1 AND number=?2", [s2, n]).unwrap();
    }

    let binge = db.get_binge_record().unwrap();
    assert_eq!(binge.count, 5);
    assert_eq!(binge.top_series.len(), 2);
    assert_eq!(binge.top_series[0], "A Show");
    assert_eq!(binge.top_series[1], "B Show");
}

#[test]
fn backlog_uses_the_site_the_user_actually_watched_on() {
    // Site A lists 12 episodes (none marked), site B lists 10 and the user marked all 10:
    // only 2 are left, not 12.
    let db = Db::open(":memory:").unwrap();
    let a = db.upsert_source("A", "a", "animeytx").unwrap();
    let b = db.upsert_source("B", "b", "tioanime").unwrap();
    let sa = db.upsert_series(a, &mk_airing("a", "Cross Show", None)).unwrap();
    db.set_followed(sa, true).unwrap();
    insert_eps_seen_up_to(&db, sa, 12, 0);
    let sb = db.upsert_series(b, &mk_airing("b", "Cross Show", None)).unwrap();
    db.set_followed(sb, true).unwrap();
    insert_eps_seen_up_to(&db, sb, 10, 10);

    let insights = db.get_watch_insights().unwrap();
    assert_eq!(insights.backlog_episodes, 2);
}

#[test]
fn backlog_ignores_a_ya_lo_vi_franchise_even_with_unseen_rows() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("A", "a", "animeytx").unwrap();
    let s = db.upsert_series(src, &mk_airing("s", "Claimed Watched", None)).unwrap();
    db.set_followed(s, true).unwrap();
    insert_eps_seen_up_to(&db, s, 12, 0);
    db.conn.execute("UPDATE series SET watched_externally=1 WHERE id=?1", [s]).unwrap();

    let insights = db.get_watch_insights().unwrap();
    assert_eq!(insights.backlog_episodes, 0);
    assert_eq!(insights.backlog_minutes, 0);
}

#[test]
fn t6_timing_get_watch_insights() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("A", "a", "animeytx").unwrap();
    let tx = db.conn.unchecked_transaction().unwrap();
    for i in 1..=22000 {
        tx.execute(
            "INSERT INTO anilist_catalog(id, title, url, sort_order, format, episodes, duration, start_date, metadata_version) VALUES(?1, ?2, 'x', ?1, 'TV', 12, 24, 946684800, 3)",
            (i, format!("Show {i}")),
        ).unwrap();
    }
    for i in 1..=6000 {
        tx.execute(
            "INSERT INTO series(id, source_id, slug, title, url, followed, anilist_id) VALUES(?1, ?2, ?3, ?4, 'x', 1, ?5)",
            (i, src, format!("slug{i}"), format!("Show {i}"), i),
        ).unwrap();
        for n in 1..=20 {
            tx.execute(
                "INSERT INTO episodes(series_id, number, url, seen) VALUES(?1, ?2, ?3, ?4)",
                (i, n.to_string(), format!("https://site/{i}/{n}"), n % 5 == 0),
            ).unwrap();
        }
    }
    tx.commit().unwrap();

    let started = std::time::Instant::now();
    let _insights = db.get_watch_insights().unwrap();
    let took = started.elapsed();
    assert!(took < std::time::Duration::from_millis(3000), "get_watch_insights took {took:?} (limit 3000ms)");
}

#[test]
fn twin_site_rows_deduplicate_seen_activity_across_all_four_metrics() {
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

    // Two site rows for the same show (same title)
    let row_a = db.upsert_series(src_a, &mk("frieren-a", "Sousou no Frieren")).unwrap();
    let row_b = db.upsert_series(src_b, &mk("frieren-b", "Sousou no Frieren")).unwrap();

    let now = chrono::Local::now();
    let current_year = now.year();
    let current_day = now.date_naive().format("%Y-%m-%d").to_string();

    // Episode 1 present and seen on both sites, with slightly different timestamps within today
    let (ep_a1, _) = db.insert_episode(&crate::models::Episode {
        id: 0, series_id: row_a, number: "1".into(), title: None,
        url: "https://a/1".into(), released_at: None, seen: true,
        download_path: None, download_status: None,
    }).unwrap();
    let (ep_b1, _) = db.insert_episode(&crate::models::Episode {
        id: 0, series_id: row_b, number: "1".into(), title: None,
        url: "https://b/1".into(), released_at: None, seen: true,
        download_path: None, download_status: None,
    }).unwrap();

    db.conn.execute("UPDATE episodes SET seen_at = datetime('now') WHERE id = ?1", [ep_a1]).unwrap();
    db.conn.execute("UPDATE episodes SET seen_at = datetime('now', '+1 minute') WHERE id = ?1", [ep_b1]).unwrap();

    // 1. marks_by_day in get_watch_insights: episode 1 on two sites counts ONCE
    let insights = db.get_watch_insights().unwrap();
    let today_count = insights.marks_by_day.iter().find(|d| d.day == current_day).map(|d| d.count).unwrap_or(0);
    assert_eq!(today_count, 1, "marks_by_day counts episode 1 only once despite twin site rows");

    // 2. get_binge_record: count is 1
    let binge = db.get_binge_record().unwrap();
    assert_eq!(binge.day.as_deref(), Some(current_day.as_str()));
    assert_eq!(binge.count, 1, "get_binge_record counts episode 1 only once");

    // 3. get_hourly_distribution: total count is 1
    let hourly = db.get_hourly_distribution().unwrap();
    let total_hour_marks: i64 = hourly.iter().map(|h| h.count).sum();
    assert_eq!(total_hour_marks, 1, "get_hourly_distribution counts episode 1 only once");

    // 4. get_yearly_activity: total count is 1
    let yearly = db.get_yearly_activity(current_year).unwrap();
    let total_year_marks: i64 = yearly.days.iter().map(|d| d.count).sum();
    assert_eq!(total_year_marks, 1, "get_yearly_activity counts episode 1 only once");

    // Now add episode 2 on Site A: two different episodes still count twice
    let (ep_a2, _) = db.insert_episode(&crate::models::Episode {
        id: 0, series_id: row_a, number: "2".into(), title: None,
        url: "https://a/2".into(), released_at: None, seen: true,
        download_path: None, download_status: None,
    }).unwrap();
    db.conn.execute("UPDATE episodes SET seen_at = datetime('now') WHERE id = ?1", [ep_a2]).unwrap();

    let insights2 = db.get_watch_insights().unwrap();
    let today_count2 = insights2.marks_by_day.iter().find(|d| d.day == current_day).map(|d| d.count).unwrap_or(0);
    assert_eq!(today_count2, 2, "marks_by_day counts 2 different episodes as 2");

    let binge2 = db.get_binge_record().unwrap();
    assert_eq!(binge2.count, 2, "get_binge_record counts 2 different episodes as 2");

    let hourly2 = db.get_hourly_distribution().unwrap();
    let total_hour_marks2: i64 = hourly2.iter().map(|h| h.count).sum();
    assert_eq!(total_hour_marks2, 2, "get_hourly_distribution counts 2 different episodes as 2");

    let yearly2 = db.get_yearly_activity(current_year).unwrap();
    let total_year_marks2: i64 = yearly2.days.iter().map(|d| d.count).sum();
    assert_eq!(total_year_marks2, 2, "get_yearly_activity counts 2 different episodes as 2");
}

// ---- Stats consistency: "Ya lo vi" on both sides (2026-10-21) ----

#[test]
fn completion_numerator_and_denominator_share_the_ya_lo_vi_base() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("A", "a", "animeytx").unwrap();

    // Followed, unlinked: 4 of 10 really seen.
    let naruto = db.upsert_series(src, &mk_airing("naruto", "Naruto", None)).unwrap();
    db.set_followed(naruto, true).unwrap();
    insert_eps_seen_up_to(&db, naruto, 10, 4);

    // Followed AND "Ya lo vi", linked to a 293-episode catalog entry:
    // 29 real marks, the catalog total wins (max, never sum).
    let mut boruto_cat = catalog_anime(1, "Boruto", &[]);
    boruto_cat.episodes = Some(293);
    db.upsert_catalog_anime(&boruto_cat, 0).unwrap();
    let boruto = db.upsert_series(src, &mk_airing("boruto", "Boruto", None)).unwrap();
    db.set_followed(boruto, true).unwrap();
    db.set_anilist_id(boruto, 1).unwrap();
    db.set_watched_externally(boruto, true).unwrap();
    insert_eps_seen_up_to(&db, boruto, 30, 29);

    // "Ya lo vi" only, linked to a 28-episode entry, no site episodes.
    let mut frieren_cat = catalog_anime(2, "Frieren", &[]);
    frieren_cat.episodes = Some(28);
    db.upsert_catalog_anime(&frieren_cat, 0).unwrap();
    let frieren = db.upsert_series(src, &mk_airing("frieren", "Frieren", None)).unwrap();
    db.set_anilist_id(frieren, 2).unwrap();
    db.set_watched_externally(frieren, true).unwrap();

    // "Ya lo vi" only, unlinked (no catalog count), 12 unseen site rows:
    // contributes nothing to either side and is reported as missing data.
    let mystery = db.upsert_series(src, &mk_airing("mystery", "Mystery Show", None)).unwrap();
    db.set_watched_externally(mystery, true).unwrap();
    insert_eps_seen_up_to(&db, mystery, 12, 0);

    // "Ya lo vi" only, unlinked, but with real marks: credited with those
    // marks, so it is NOT missing data.
    let gintama = db.upsert_series(src, &mk_airing("gintama", "Gintama", None)).unwrap();
    db.set_watched_externally(gintama, true).unwrap();
    insert_eps_seen_up_to(&db, gintama, 5, 3);

    let summary = db.get_watch_summary().unwrap();
    let watched = summary.episodes_watched + summary.episodes_watched_external;
    assert_eq!(watched, 4 + 293 + 28 + 3, "rollup episodes (real max estimate)");
    assert_eq!(
        summary.episodes_total,
        10 + 293 + 28 + 5,
        "denominator counts the same franchises on the same base: {summary:?}"
    );
    assert!(watched <= summary.episodes_total);
    assert_eq!(summary.titles_without_episode_data, 1, "only Mystery Show has no data");
}

#[test]
fn titles_without_episode_data_is_zero_when_every_claim_has_a_count() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("A", "a", "animeytx").unwrap();
    db.upsert_catalog_anime(&catalog_anime(1, "Frieren", &[]), 0).unwrap();
    let frieren = db.upsert_series(src, &mk_airing("frieren", "Frieren", None)).unwrap();
    db.set_anilist_id(frieren, 1).unwrap();
    db.set_watched_externally(frieren, true).unwrap();
    let summary = db.get_watch_summary().unwrap();
    assert_eq!(summary.titles_without_episode_data, 0);
    assert_eq!(summary.episodes_total, 12);
    assert_eq!(summary.episodes_watched_external, 12);
}

#[test]
fn completion_agrees_for_a_franchise_with_several_ya_lo_vi_seasons() {
    // Three AniList entries of one franchise, all "Ya lo vi", unfollowed,
    // unseen. The numerator credits the largest claim (25), so the
    // denominator must not sum the three (59) or a fully watched show
    // reads 42 %.
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("A", "a", "animeytx").unwrap();
    for (id, slug, title, eps) in [
        (11, "mob1", "Mob Psycho 100", 25),
        (12, "mob2", "Mob Psycho 100 Temporada 2", 12),
        (13, "mob3", "Mob Psycho 100 Temporada 3", 22),
    ] {
        let mut cat = catalog_anime(id, title, &[]);
        cat.episodes = Some(eps);
        db.upsert_catalog_anime(&cat, 0).unwrap();
        let sid = db.upsert_series(src, &mk_airing(slug, title, None)).unwrap();
        db.set_anilist_id(sid, id).unwrap();
        db.set_watched_externally(sid, true).unwrap();
    }
    let summary = db.get_watch_summary().unwrap();
    let watched = summary.episodes_watched + summary.episodes_watched_external;
    assert_eq!(summary.distinct_anime, 1, "one franchise");
    assert_eq!(watched, 25);
    assert_eq!(summary.episodes_total, watched, "100 %, not 25/59: {summary:?}");
}

/// Seeds: A followed (Action, pop 100k); B "Ya lo vi" only (Drama, 24
/// eps, pop 500k); C followed on site 1 and "Ya lo vi" on site 2, same
/// AniList id (Comedy, 12 eps, pop 300k).
fn seed_taste_mix(db: &Db) {
    let s1 = db.upsert_source("A", "a", "animeytx").unwrap();
    let s2 = db.upsert_source("B", "b", "tioanime").unwrap();
    db.upsert_catalog_anime(&catalog_anime_with_popularity(10, "Show A", &["Action"], Some(100_000)), 0).unwrap();
    let mut b = catalog_anime_with_popularity(20, "Show B", &["Drama"], Some(500_000));
    b.episodes = Some(24);
    db.upsert_catalog_anime(&b, 0).unwrap();
    db.upsert_catalog_anime(&catalog_anime_with_popularity(30, "Show C", &["Comedy"], Some(300_000)), 0).unwrap();

    let a = db.upsert_series(s1, &mk_airing("show-a", "Show A", None)).unwrap();
    db.set_followed(a, true).unwrap();
    db.set_anilist_id(a, 10).unwrap();
    insert_eps_seen_up_to(db, a, 6, 6);

    let b = db.upsert_series(s1, &mk_airing("show-b", "Show B", None)).unwrap();
    db.set_anilist_id(b, 20).unwrap();
    db.set_watched_externally(b, true).unwrap();

    let c1 = db.upsert_series(s1, &mk_airing("show-c", "Show C", None)).unwrap();
    db.set_followed(c1, true).unwrap();
    db.set_anilist_id(c1, 30).unwrap();
    insert_eps_seen_up_to(db, c1, 5, 5);
    let c2 = db.upsert_series(s2, &mk_airing("show-c", "Show C", None)).unwrap();
    db.set_anilist_id(c2, 30).unwrap();
    db.set_watched_externally(c2, true).unwrap();
}

#[test]
fn genre_and_type_stats_include_ya_lo_vi_without_double_counting() {
    let db = Db::open(":memory:").unwrap();
    seed_taste_mix(&db);

    let genres = db.get_genre_stats().unwrap();
    let count = |g: &str| genres.iter().find(|x| x.genre == g).map(|x| x.count);
    assert_eq!(count("Action"), Some(1));
    assert_eq!(count("Drama"), Some(1), "a \"Ya lo vi\"-only show counts: {genres:?}");
    assert_eq!(count("Comedy"), Some(1), "followed + \"Ya lo vi\" is one show");

    let types = db.get_type_stats().unwrap();
    assert_eq!(types.iter().find(|t| t.kind == "TV").map(|t| t.count), Some(3), "{types:?}");
}

#[test]
fn genre_cards_weight_ya_lo_vi_by_the_rollup_episode_estimate() {
    let db = Db::open(":memory:").unwrap();
    seed_taste_mix(&db);
    let cards = db.get_genre_cards().unwrap();
    let count = |g: &str| cards.iter().find(|x| x.genre == g).map(|x| x.count);
    assert_eq!(count("Drama"), Some(24), "estimated episodes of a \"Ya lo vi\"-only show");
    assert_eq!(count("Comedy"), Some(12), "max(5 real, 12 estimated), never 17: {cards:?}");
    assert_eq!(count("Action"), Some(6));
}

#[test]
fn favorites_and_popularity_bias_include_ya_lo_vi_once() {
    let db = Db::open(":memory:").unwrap();
    seed_taste_mix(&db);
    let favorites = db.get_favorite_genres(10).unwrap();
    let score = |g: &str| favorites.iter().find(|x| x.genre == g).map(|x| x.score);
    assert_eq!(score("Drama"), Some(2.0), "{favorites:?}");
    assert_eq!(score("Comedy"), Some(2.0), "followed + \"Ya lo vi\" is +2 once");

    let bias = db.get_popularity_bias().unwrap();
    assert_eq!(bias.average_popularity, Some(300_000.0), "A, B and C, C once");
}

// ---- Genres vs tags (2026-10-21) ----

/// One linked, followed show carrying AniList genres plus tags above and
/// below the rank-50 floor, and one unlinked show whose scraped labels
/// mix a mappable genre ("Acción") with a site-only label ("Shounen").
fn seed_genres_and_tags(db: &Db) -> (i64, i64) {
    let src = db.upsert_source("A", "a", "animeytx").unwrap();
    let mut anime = catalog_anime(500, "Tagged Show", &["Action", "Drama"]);
    anime.tags = vec![
        crate::anilist::CatalogTag { name: "Male Protagonist".into(), rank: 80 },
        crate::anilist::CatalogTag { name: "Kuudere".into(), rank: 30 },
    ];
    db.upsert_catalog_anime(&anime, 0).unwrap();
    let linked = db.upsert_series(src, &mk_airing("tagged", "Tagged Show", None)).unwrap();
    db.set_followed(linked, true).unwrap();
    db.set_anilist_id(linked, 500).unwrap();
    insert_eps_seen_up_to(db, linked, 4, 4);

    let unlinked = db.upsert_series(src, &mk_airing("site", "Site Show", None)).unwrap();
    db.set_followed(unlinked, true).unwrap();
    db.insert_series_genres(unlinked, &["Acción".into(), "Shounen".into()]).unwrap();
    insert_eps_seen_up_to(db, unlinked, 2, 2);
    (linked, unlinked)
}

#[test]
fn stats_graph_nodes_carry_only_real_genres_never_tags() {
    let db = Db::open(":memory:").unwrap();
    let (linked, unlinked) = seed_genres_and_tags(&db);
    let nodes = db.get_stats_graph_data().unwrap();
    let genres = |id: i64| nodes.iter().find(|n| n.id == id).unwrap().genres.clone();
    assert_eq!(genres(linked), vec!["Action".to_string(), "Drama".to_string()]);
    assert_eq!(
        genres(unlinked),
        vec!["Action".to_string()],
        "scraped labels map onto AniList's genres; unmapped ones are dropped"
    );
}

#[test]
fn genre_cards_and_genre_stats_exclude_tags_and_unmapped_site_labels() {
    let db = Db::open(":memory:").unwrap();
    seed_genres_and_tags(&db);
    let cards = db.get_genre_cards().unwrap();
    let names: HashSet<&str> = cards.iter().map(|c| c.genre.as_str()).collect();
    assert_eq!(names, HashSet::from(["Action", "Drama"]), "{cards:?}");
    let action = cards.iter().find(|c| c.genre == "Action").unwrap();
    assert_eq!(action.count, 6);
    assert_eq!(action.show_count, 2);
    assert!((action.share - 1.0).abs() < 1e-9, "every watched episode is in an Action show");

    let stats = db.get_genre_stats().unwrap();
    let names: HashSet<&str> = stats.iter().map(|g| g.genre.as_str()).collect();
    assert_eq!(names, HashSet::from(["Action", "Drama"]), "{stats:?}");
}

#[test]
fn unlinked_show_with_spanish_sobrenatural_lands_in_supernatural() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("A", "a", "animeytx").unwrap();
    let sid = db.upsert_series(src, &mk_airing("sob", "Site Sobrenatural", None)).unwrap();
    db.set_followed(sid, true).unwrap();
    db.insert_series_genres(sid, &["Sobrenatural".into(), "Suspenso".into()]).unwrap();
    insert_eps_seen_up_to(&db, sid, 2, 2);
    let names: HashSet<String> = db.get_genre_cards().unwrap().into_iter().map(|c| c.genre).collect();
    assert_eq!(names, HashSet::from(["Supernatural".to_string(), "Thriller".to_string()]));
    let node = db.get_stats_graph_data().unwrap();
    assert_eq!(node[0].genres, vec!["Supernatural".to_string(), "Thriller".to_string()]);
}

/// 22k catalog rows (3 genres + 15 tags each), 6000 series, 600 of them in
/// the taste corpus (half linked, all with site genres and 12 seen
/// episodes) — the owner's real data is ~170 followed shows.
fn seed_real_size_taste(db: &Db) {
    let src = db.upsert_source("TestSite", "https://site.example", "testsite").unwrap();
    let tx = db.conn.unchecked_transaction().unwrap();
    for i in 1..=22000i64 {
        tx.execute(
            "INSERT INTO anilist_catalog(id, title, url, sort_order, metadata_version) VALUES(?1, 'Cat', 'x', ?1, 1)",
            [i],
        ).unwrap();
        for j in 1..=3 {
            tx.execute(
                "INSERT INTO anilist_catalog_genres(anilist_id, genre) VALUES(?1, ?2)",
                (i, ["Action", "Drama", "Comedy", "Fantasy", "Romance"][((i + j) % 5) as usize]),
            ).ok();
        }
        for j in 1..=15 {
            tx.execute(
                "INSERT INTO anilist_catalog_tags(anilist_id, tag, rank) VALUES(?1, ?2, ?3)",
                (i, format!("Tag{}", (i + j) % 300), 40 + j * 4),
            ).ok();
        }
    }
    for i in 1..=6000i64 {
        let anilist_id = if i % 2 == 0 { Some(i) } else { None };
        let taste = i <= 600;
        tx.execute(
            "INSERT INTO series(id, source_id, slug, title, url, anilist_id, followed, watched_externally)
             VALUES(?1, ?2, ?3, ?4, 'x', ?5, ?6, 0)",
            (i, src, format!("slug{i}"), format!("Series {i}"), anilist_id, taste as i64),
        ).unwrap();
        for g in ["Acción", "Sobrenatural"] {
            tx.execute("INSERT INTO series_genres(series_id, genre) VALUES(?1, ?2)", (i, g)).unwrap();
        }
        if taste {
            for e in 1..=12 {
                tx.execute(
                    "INSERT INTO episodes(series_id, number, url, seen, seen_at) VALUES(?1, ?2, ?3, 1, datetime('now'))",
                    (i, e.to_string(), format!("u{i}-{e}")),
                ).unwrap();
            }
        }
    }
    tx.commit().unwrap();
}

#[test]
fn genre_and_tag_cards_are_fast_at_real_size() {
    let db = Db::open(":memory:").unwrap();
    seed_real_size_taste(&db);

    let started = std::time::Instant::now();
    let genres = db.get_genre_cards().unwrap();
    let took_genres = started.elapsed();
    let started = std::time::Instant::now();
    let tags = db.get_tag_cards(24).unwrap();
    let took_tags = started.elapsed();
    let started = std::time::Instant::now();
    let graph = db.get_stats_graph_data().unwrap();
    let took_graph = started.elapsed();

    assert!(!genres.is_empty() && graph.len() == 600);
    assert_eq!(tags.len(), 24);
    let limit = std::time::Duration::from_secs(1);
    assert!(took_genres < limit, "get_genre_cards took {took_genres:?}");
    assert!(took_tags < limit, "get_tag_cards took {took_tags:?}");
    assert!(took_graph < limit, "get_stats_graph_data took {took_graph:?}");
}

#[test]
fn tag_cards_list_ranked_tags_weighted_like_genre_cards() {
    let db = Db::open(":memory:").unwrap();
    seed_genres_and_tags(&db);
    let tags = db.get_tag_cards(24).unwrap();
    let names: Vec<&str> = tags.iter().map(|c| c.genre.as_str()).collect();
    assert_eq!(names, vec!["Male Protagonist"], "rank < 50 tags stay out: {tags:?}");
    assert_eq!(tags[0].count, 4, "weighted by the show's watched episodes");
    assert!((tags[0].share - 4.0 / 6.0).abs() < 1e-9);
    assert_eq!(tags[0].top_series[0].title, "Tagged Show");
}

#[test]
fn tag_cards_honour_the_limit() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("A", "a", "animeytx").unwrap();
    let mut anime = catalog_anime(600, "Many Tags", &["Action"]);
    anime.tags = (0..5)
        .map(|i| crate::anilist::CatalogTag { name: format!("Tag {i}"), rank: 90 })
        .collect();
    db.upsert_catalog_anime(&anime, 0).unwrap();
    let sid = db.upsert_series(src, &mk_airing("many", "Many Tags", None)).unwrap();
    db.set_followed(sid, true).unwrap();
    db.set_anilist_id(sid, 600).unwrap();
    assert_eq!(db.get_tag_cards(3).unwrap().len(), 3);
}

#[test]
fn genre_cards_do_not_lead_with_a_show_that_already_leads_a_higher_card() {
    // Before: the biggest show topped every card it belonged to, so every
    // card opened with the same One Piece / Pokémon covers.
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("A", "a", "animeytx").unwrap();
    let big = db.upsert_series(src, &mk_airing("big", "Big Show", None)).unwrap();
    db.set_followed(big, true).unwrap();
    db.insert_series_genres(big, &["Adventure".into(), "Action".into()]).unwrap();
    insert_eps_seen_up_to(&db, big, 100, 100);
    let adv = db.upsert_series(src, &mk_airing("adv", "Adventure Show", None)).unwrap();
    db.set_followed(adv, true).unwrap();
    db.insert_series_genres(adv, &["Adventure".into()]).unwrap();
    insert_eps_seen_up_to(&db, adv, 10, 10);
    let act = db.upsert_series(src, &mk_airing("act", "Action Show", None)).unwrap();
    db.set_followed(act, true).unwrap();
    db.insert_series_genres(act, &["Action".into()]).unwrap();
    insert_eps_seen_up_to(&db, act, 5, 5);

    let cards = db.get_genre_cards().unwrap();
    assert_eq!(cards[0].genre, "Adventure", "110 episodes beat Action's 105");
    assert_eq!(cards[0].top_series[0].title, "Big Show");
    assert_eq!(cards[1].genre, "Action");
    assert_eq!(cards[1].count, 105, "the count still includes the big show");
    let action_titles: Vec<&str> = cards[1].top_series.iter().map(|s| s.title.as_str()).collect();
    assert_eq!(action_titles, vec!["Action Show"], "Big Show already leads Adventure");
}

#[test]
fn genre_card_falls_back_to_its_top_show_when_every_show_leads_elsewhere() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("A", "a", "animeytx").unwrap();
    let only = db.upsert_series(src, &mk_airing("only", "Only Show", None)).unwrap();
    db.set_followed(only, true).unwrap();
    db.insert_series_genres(only, &["Action".into(), "Drama".into()]).unwrap();
    insert_eps_seen_up_to(&db, only, 3, 3);
    let cards = db.get_genre_cards().unwrap();
    assert_eq!(cards.len(), 2);
    assert!(cards.iter().all(|c| c.top_series.len() == 1), "never an empty card: {cards:?}");
}

// ---- AniList precision: episode counts and duration (2026-10-26) ----

#[test]
fn ya_lo_vi_releasing_credits_next_episode_minus_one() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("A", "a", "animeytx").unwrap();

    db.upsert_catalog_anime(
        &crate::anilist::CatalogAnime {
            id: 100,
            title: "Airing Show".into(),
            title_romaji: None,
            title_english: None,
            synonyms: Vec::new(),
            cover_url: None,
            banner_url: None,
            format: Some("TV".into()),
            genres: vec![],
            episodes: None,
            average_score: None,
            popularity: None,
            url: "https://anilist.co/anime/100".into(),
            status: Some("RELEASING".into()),
            duration: Some(24),
            studio: None,
            start_date: None,
            next_airing_at: None,
            next_episode: Some(8),
            tags: Vec::new(),
            relations: Vec::new(),
        },
        0,
    )
    .unwrap();

    let sid = db.upsert_series(src, &mk_airing("airing-show", "Airing Show", None)).unwrap();
    db.set_watched_externally(sid, true).unwrap();
    db.set_anilist_id(sid, 100).unwrap();

    let summary = db.get_watch_summary().unwrap();
    assert_eq!(summary.episodes_watched, 0);
    assert_eq!(summary.episodes_watched_external, 7, "credits 8 - 1 = 7 episodes");
    assert_eq!(summary.titles_without_episode_data, 0, "not counted in titles_without_episode_data");

    let insights = db.get_watch_insights().unwrap();
    assert_eq!(insights.estimated_minutes_external, 7 * 24, "credits 7 * 24 = 168 minutes");
    assert_eq!(insights.external_titles_estimated, 1);
    assert_eq!(insights.external_titles_total, 1);
}

#[test]
fn ya_lo_vi_releasing_with_invalid_or_finished_data_credits_zero() {
    // Sub-case 1: RELEASING with next_episode = 1
    {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("A", "a", "animeytx").unwrap();
        db.upsert_catalog_anime(
            &crate::anilist::CatalogAnime {
                id: 101,
                title: "Ep One".into(),
                title_romaji: None,
                title_english: None,
                synonyms: Vec::new(),
                cover_url: None,
                banner_url: None,
                format: Some("TV".into()),
                genres: vec![],
                episodes: None,
                average_score: None,
                popularity: None,
                url: "https://anilist.co/anime/101".into(),
                status: Some("RELEASING".into()),
                duration: Some(24),
                studio: None,
                start_date: None,
                next_airing_at: None,
                next_episode: Some(1),
                tags: Vec::new(),
                relations: Vec::new(),
            },
            0,
        )
        .unwrap();
        let sid = db.upsert_series(src, &mk_airing("ep-one", "Ep One", None)).unwrap();
        db.set_watched_externally(sid, true).unwrap();
        db.set_anilist_id(sid, 101).unwrap();

        let summary = db.get_watch_summary().unwrap();
        assert_eq!(summary.episodes_watched_external, 0);
        assert_eq!(summary.titles_without_episode_data, 1);
    }

    // Sub-case 2: RELEASING with next_episode = NULL
    {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("A", "a", "animeytx").unwrap();
        db.upsert_catalog_anime(
            &crate::anilist::CatalogAnime {
                id: 102,
                title: "No Next".into(),
                title_romaji: None,
                title_english: None,
                synonyms: Vec::new(),
                cover_url: None,
                banner_url: None,
                format: Some("TV".into()),
                genres: vec![],
                episodes: None,
                average_score: None,
                popularity: None,
                url: "https://anilist.co/anime/102".into(),
                status: Some("RELEASING".into()),
                duration: Some(24),
                studio: None,
                start_date: None,
                next_airing_at: None,
                next_episode: None,
                tags: Vec::new(),
                relations: Vec::new(),
            },
            0,
        )
        .unwrap();
        let sid = db.upsert_series(src, &mk_airing("no-next", "No Next", None)).unwrap();
        db.set_watched_externally(sid, true).unwrap();
        db.set_anilist_id(sid, 102).unwrap();

        let summary = db.get_watch_summary().unwrap();
        assert_eq!(summary.episodes_watched_external, 0);
        assert_eq!(summary.titles_without_episode_data, 1);
    }

    // Sub-case 3: FINISHED with episodes NULL
    {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("A", "a", "animeytx").unwrap();
        db.upsert_catalog_anime(
            &crate::anilist::CatalogAnime {
                id: 103,
                title: "Finished Null".into(),
                title_romaji: None,
                title_english: None,
                synonyms: Vec::new(),
                cover_url: None,
                banner_url: None,
                format: Some("TV".into()),
                genres: vec![],
                episodes: None,
                average_score: None,
                popularity: None,
                url: "https://anilist.co/anime/103".into(),
                status: Some("FINISHED".into()),
                duration: Some(24),
                studio: None,
                start_date: None,
                next_airing_at: None,
                next_episode: Some(8),
                tags: Vec::new(),
                relations: Vec::new(),
            },
            0,
        )
        .unwrap();
        let sid = db.upsert_series(src, &mk_airing("fin-null", "Finished Null", None)).unwrap();
        db.set_watched_externally(sid, true).unwrap();
        db.set_anilist_id(sid, 103).unwrap();

        let summary = db.get_watch_summary().unwrap();
        assert_eq!(summary.episodes_watched_external, 0);
        assert_eq!(summary.titles_without_episode_data, 1);
    }
}

#[test]
fn completion_denominator_releasing_uses_next_episode() {
    // Case A: 10 scraped rows, next_episode = 13 (n - 1 = 12) -> contributes 12 (max(10, 12))
    {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("A", "a", "animeytx").unwrap();
        db.upsert_catalog_anime(
            &crate::anilist::CatalogAnime {
                id: 201,
                title: "Releasing Show".into(),
                title_romaji: None,
                title_english: None,
                synonyms: Vec::new(),
                cover_url: None,
                banner_url: None,
                format: Some("TV".into()),
                genres: vec![],
                episodes: None,
                average_score: None,
                popularity: None,
                url: "https://anilist.co/anime/201".into(),
                status: Some("RELEASING".into()),
                duration: Some(24),
                studio: None,
                start_date: None,
                next_airing_at: None,
                next_episode: Some(13),
                tags: Vec::new(),
                relations: Vec::new(),
            },
            0,
        )
        .unwrap();
        let sid = db.upsert_series(src, &mk_airing("rel1", "Releasing Show", None)).unwrap();
        db.set_followed(sid, true).unwrap();
        db.set_anilist_id(sid, 201).unwrap();
        insert_eps_seen_up_to(&db, sid, 10, 0);

        let totals = db.canonical_franchise_episode_totals(false).unwrap();
        assert_eq!(totals, vec![12], "10 scraped rows vs next_episode 13 -> max(10, 12) = 12");
    }

    // Case B: 14 scraped rows, next_episode = 13 (n - 1 = 12) -> contributes 14 (max(14, 12))
    {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("A", "a", "animeytx").unwrap();
        db.upsert_catalog_anime(
            &crate::anilist::CatalogAnime {
                id: 202,
                title: "Releasing Show 2".into(),
                title_romaji: None,
                title_english: None,
                synonyms: Vec::new(),
                cover_url: None,
                banner_url: None,
                format: Some("TV".into()),
                genres: vec![],
                episodes: None,
                average_score: None,
                popularity: None,
                url: "https://anilist.co/anime/202".into(),
                status: Some("RELEASING".into()),
                duration: Some(24),
                studio: None,
                start_date: None,
                next_airing_at: None,
                next_episode: Some(13),
                tags: Vec::new(),
                relations: Vec::new(),
            },
            0,
        )
        .unwrap();
        let sid = db.upsert_series(src, &mk_airing("rel2", "Releasing Show 2", None)).unwrap();
        db.set_followed(sid, true).unwrap();
        db.set_anilist_id(sid, 202).unwrap();
        insert_eps_seen_up_to(&db, sid, 14, 0);

        let totals = db.canonical_franchise_episode_totals(false).unwrap();
        assert_eq!(totals, vec![14], "14 scraped rows vs next_episode 13 -> max(14, 12) = 14");
    }

    // Case C: episodes = 12 set -> unchanged (both with next_episode and without)
    {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("A", "a", "animeytx").unwrap();
        db.upsert_catalog_anime(
            &crate::anilist::CatalogAnime {
                id: 203,
                title: "Known Eps Show".into(),
                title_romaji: None,
                title_english: None,
                synonyms: Vec::new(),
                cover_url: None,
                banner_url: None,
                format: Some("TV".into()),
                genres: vec![],
                episodes: Some(12),
                average_score: None,
                popularity: None,
                url: "https://anilist.co/anime/203".into(),
                status: Some("RELEASING".into()),
                duration: Some(24),
                studio: None,
                start_date: None,
                next_airing_at: None,
                next_episode: Some(13),
                tags: Vec::new(),
                relations: Vec::new(),
            },
            0,
        )
        .unwrap();
        let sid = db.upsert_series(src, &mk_airing("known1", "Known Eps Show", None)).unwrap();
        db.set_followed(sid, true).unwrap();
        db.set_anilist_id(sid, 203).unwrap();
        insert_eps_seen_up_to(&db, sid, 10, 0);

        let totals = db.canonical_franchise_episode_totals(false).unwrap();
        assert_eq!(totals, vec![12], "episodes = 12 set -> unchanged, contributes 12");
    }
}

#[test]
fn backlog_prices_remaining_episodes_at_non_movie_duration() {
    // Case A: TV member (duration 24, 6 unseen episodes) and MOVIE member (duration 100)
    // -> backlog minutes = 6 * 24 = 144, not 6 * 100 = 600.
    {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("A", "a", "animeytx").unwrap();

        db.upsert_catalog_anime(
            &crate::anilist::CatalogAnime {
                id: 301,
                title: "Franchise Show".into(),
                title_romaji: None,
                title_english: None,
                synonyms: Vec::new(),
                cover_url: None,
                banner_url: None,
                format: Some("TV".into()),
                genres: vec![],
                episodes: Some(12),
                average_score: None,
                popularity: None,
                url: "https://anilist.co/anime/301".into(),
                status: None,
                duration: Some(24),
                studio: None,
                start_date: None,
                next_airing_at: None,
                next_episode: None,
                tags: Vec::new(),
                relations: Vec::new(),
            },
            0,
        )
        .unwrap();

        db.upsert_catalog_anime(
            &crate::anilist::CatalogAnime {
                id: 302,
                title: "Franchise Show: Movie".into(),
                title_romaji: None,
                title_english: None,
                synonyms: Vec::new(),
                cover_url: None,
                banner_url: None,
                format: Some("MOVIE".into()),
                genres: vec![],
                episodes: Some(1),
                average_score: None,
                popularity: None,
                url: "https://anilist.co/anime/302".into(),
                status: None,
                duration: Some(100),
                studio: None,
                start_date: None,
                next_airing_at: None,
                next_episode: None,
                tags: Vec::new(),
                relations: Vec::new(),
            },
            0,
        )
        .unwrap();

        let tv = db.upsert_series(src, &mk_airing("f-tv", "Franchise Show", None)).unwrap();
        db.set_followed(tv, true).unwrap();
        db.set_anilist_id(tv, 301).unwrap();
        insert_eps_seen_up_to(&db, tv, 6, 0);

        let movie = db.upsert_series(src, &mk_airing("f-movie", "Franchise Show: Movie", None)).unwrap();
        db.set_followed(movie, true).unwrap();
        db.set_anilist_id(movie, 302).unwrap();
        insert_eps_seen_up_to(&db, movie, 1, 1);

        let insights = db.get_watch_insights().unwrap();
        assert_eq!(insights.backlog_episodes, 6, "6 unseen TV episodes");
        assert_eq!(
            insights.backlog_minutes,
            6 * 24,
            "priced at TV duration 24, not movie duration 100"
        );
    }

    // Case B: A franchise with only a movie keeps its own duration
    {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("A", "a", "animeytx").unwrap();

        db.upsert_catalog_anime(
            &crate::anilist::CatalogAnime {
                id: 303,
                title: "Solo Movie".into(),
                title_romaji: None,
                title_english: None,
                synonyms: Vec::new(),
                cover_url: None,
                banner_url: None,
                format: Some("MOVIE".into()),
                genres: vec![],
                episodes: Some(1),
                average_score: None,
                popularity: None,
                url: "https://anilist.co/anime/303".into(),
                status: None,
                duration: Some(100),
                studio: None,
                start_date: None,
                next_airing_at: None,
                next_episode: None,
                tags: Vec::new(),
                relations: Vec::new(),
            },
            0,
        )
        .unwrap();

        let movie = db.upsert_series(src, &mk_airing("solo-movie", "Solo Movie", None)).unwrap();
        db.set_followed(movie, true).unwrap();
        db.set_anilist_id(movie, 303).unwrap();
        insert_eps_seen_up_to(&db, movie, 1, 0);

        let insights = db.get_watch_insights().unwrap();
        assert_eq!(insights.backlog_episodes, 1);
        assert_eq!(
            insights.backlog_minutes,
            100,
            "franchise with only a movie keeps its own duration"
        );
    }

    // Case C: the TV member has no stored duration yet (not backfilled) next to a
    // 100-minute movie -> TV estimate 6 * 24, never the movie runtime.
    {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("A", "a", "animeytx").unwrap();
        let anime = |id: i64, title: &str, format: &str, duration: Option<i64>| crate::anilist::CatalogAnime {
            id,
            title: title.into(),
            title_romaji: None,
            title_english: None,
            synonyms: Vec::new(),
            cover_url: None,
            banner_url: None,
            format: Some(format.into()),
            genres: vec![],
            episodes: Some(12),
            average_score: None,
            popularity: None,
            url: format!("https://anilist.co/anime/{id}"),
            status: None,
            duration,
            studio: None,
            start_date: None,
            next_airing_at: None,
            next_episode: None,
            tags: Vec::new(),
            relations: Vec::new(),
        };
        db.upsert_catalog_anime(&anime(304, "Gap Show", "TV", None), 0).unwrap();
        db.upsert_catalog_anime(&anime(305, "Gap Show: Movie", "MOVIE", Some(100)), 0).unwrap();

        let tv = db.upsert_series(src, &mk_airing("g-tv", "Gap Show", None)).unwrap();
        db.set_followed(tv, true).unwrap();
        db.set_anilist_id(tv, 304).unwrap();
        insert_eps_seen_up_to(&db, tv, 6, 0);

        let movie = db.upsert_series(src, &mk_airing("g-movie", "Gap Show: Movie", None)).unwrap();
        db.set_followed(movie, true).unwrap();
        db.set_anilist_id(movie, 305).unwrap();
        insert_eps_seen_up_to(&db, movie, 1, 1);

        let insights = db.get_watch_insights().unwrap();
        assert_eq!(insights.backlog_episodes, 6);
        assert_eq!(insights.backlog_minutes, 6 * 24, "TV estimate, not the movie runtime");
    }
}

// ---- Data-accuracy fixes (2026-10-30 plan, section A) ----

/// Backdates every seen mark of `series_id` to `days_ago` days before now.
fn backdate_seen(db: &Db, series_id: i64, days_ago: i64) {
    db.conn
        .execute(
            "UPDATE episodes SET seen_at = datetime('now', 'start of day', ?1, '+12 hours')
             WHERE series_id=?2 AND seen=1",
            rusqlite::params![format!("-{days_ago} days"), series_id],
        )
        .unwrap();
}

#[test]
fn get_watch_summary_pending_to_watch_skips_series_marked_watched_externally() {
    // Pendientes (`list_pending`) never lists a "Ya lo vi" row, so the
    // Stats counter must not either: it read "1 pendiente" over an empty
    // Pendientes screen.
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("A", "a", "animeytx").unwrap();
    let id = db.upsert_series(src, &mk_airing("ext", "Ext Show", None)).unwrap();
    db.set_followed(id, true).unwrap();
    db.set_watched_externally(id, true).unwrap();
    insert_eps_seen_up_to(&db, id, 3, 0);

    assert_eq!(db.get_watch_summary().unwrap().pending_to_watch, 0);
}

#[test]
fn get_dusty_watchlist_skips_series_with_nothing_left_to_watch() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("A", "a", "animeytx").unwrap();

    // Finished long ago: nothing is waiting, so it isn't "gathering dust".
    let done = db.upsert_series(src, &mk_airing("done", "Done Show", None)).unwrap();
    db.set_followed(done, true).unwrap();
    insert_eps_seen_up_to(&db, done, 3, 3);
    backdate_seen(&db, done, 120);

    // Abandoned halfway: still has unseen episodes.
    let stuck = db.upsert_series(src, &mk_airing("stuck", "Stuck Show", None)).unwrap();
    db.set_followed(stuck, true).unwrap();
    insert_eps_seen_up_to(&db, stuck, 3, 1);
    backdate_seen(&db, stuck, 120);

    let titles: Vec<String> = db.get_dusty_watchlist().unwrap().into_iter().map(|d| d.title).collect();
    assert_eq!(titles, vec!["Stuck Show".to_string()]);
}

#[test]
fn get_dusty_watchlist_groups_seasons_of_one_franchise() {
    // S1 last watched 120 days ago, S2 of the same franchise yesterday:
    // the user is actively watching the franchise, so neither is dusty.
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("A", "a", "animeytx").unwrap();

    let s1 = db.upsert_series(src, &mk_airing("ky1", "Kimetsu no Yaiba", None)).unwrap();
    db.set_followed(s1, true).unwrap();
    insert_eps_seen_up_to(&db, s1, 12, 6);
    backdate_seen(&db, s1, 120);

    let s2 = db.upsert_series(src, &mk_airing("ky2", "Kimetsu no Yaiba Temporada 2", None)).unwrap();
    db.set_followed(s2, true).unwrap();
    insert_eps_seen_up_to(&db, s2, 12, 3);
    backdate_seen(&db, s2, 1);

    assert!(db.get_dusty_watchlist().unwrap().is_empty());

}

#[test]
fn real_progress_adds_up_different_seasons_watched_on_different_sites() {
    // S1 marked on site A, S2 on site B: 24 distinct episodes — the
    // heatmap already counts 24, the best-single-site rule said 12.
    let db = Db::open(":memory:").unwrap();
    let a = db.upsert_source("AnimeYT", "a", "animeytx").unwrap();
    let b = db.upsert_source("TioAnime", "b", "tioanime").unwrap();

    let s1 = db.upsert_series(a, &mk_airing("ky1", "Kimetsu no Yaiba", None)).unwrap();
    db.set_followed(s1, true).unwrap();
    insert_eps_seen_up_to(&db, s1, 12, 12);
    let s2 = db.upsert_series(b, &mk_airing("ky2", "Kimetsu no Yaiba Temporada 2", None)).unwrap();
    db.set_followed(s2, true).unwrap();
    insert_eps_seen_up_to(&db, s2, 12, 12);

    let summary = db.get_watch_summary().unwrap();
    assert_eq!(summary.episodes_watched, 24);
    assert_eq!(summary.distinct_anime, 1, "still one franchise");
    let insights = db.get_watch_insights().unwrap();
    assert_eq!(insights.estimated_minutes_tracked, 24 * 24);
}

#[test]
fn real_progress_counts_partial_mirrors_of_one_show_by_distinct_episode() {
    // Same show on two sites, episodes 1-6 marked on A and 1-9 on B:
    // 9 distinct episodes, never the 15 sum.
    let db = Db::open(":memory:").unwrap();
    let a = db.upsert_source("AnimeYT", "a", "animeytx").unwrap();
    let b = db.upsert_source("TioAnime", "b", "tioanime").unwrap();

    let ra = db.upsert_series(a, &mk_airing("m", "Mirror Show", None)).unwrap();
    db.set_followed(ra, true).unwrap();
    insert_eps_seen_up_to(&db, ra, 12, 6);
    let rb = db.upsert_series(b, &mk_airing("m", "Mirror Show", None)).unwrap();
    db.set_followed(rb, true).unwrap();
    insert_eps_seen_up_to(&db, rb, 12, 9);

    assert_eq!(db.get_watch_summary().unwrap().episodes_watched, 9);
    assert_eq!(db.get_watch_insights().unwrap().estimated_minutes_tracked, 9 * 24);
}

#[test]
fn real_progress_still_counts_seen_marks_without_a_seen_at() {
    // A seen mark with no date is still a real seen episode: the date
    // only decides which day bucket it lands in, never whether it counts.
    let db = Db::open(":memory:").unwrap();
    let a = db.upsert_source("AnimeYT", "a", "animeytx").unwrap();
    let b = db.upsert_source("TioAnime", "b", "tioanime").unwrap();

    let s1 = db.upsert_series(a, &mk_airing("ky1", "Kimetsu no Yaiba", None)).unwrap();
    db.set_followed(s1, true).unwrap();
    insert_eps_seen_up_to(&db, s1, 12, 12);
    db.conn.execute("UPDATE episodes SET seen_at = NULL WHERE series_id=?1", [s1]).unwrap();
    let s2 = db.upsert_series(b, &mk_airing("ky2", "Kimetsu no Yaiba Temporada 2", None)).unwrap();
    db.set_followed(s2, true).unwrap();
    insert_eps_seen_up_to(&db, s2, 12, 12);

    assert_eq!(db.get_watch_summary().unwrap().episodes_watched, 24);
}

#[test]
fn zero_catalog_duration_is_treated_as_unknown() {
    // AniList reports `duration: 0` for some entries; read literally it
    // priced every episode at 0 minutes.
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("A", "a", "animeytx").unwrap();
    let mut zero = catalog_anime_full(601, "Zero Show", &[], "TV", Some(12), None);
    zero.duration = Some(0);
    db.upsert_catalog_anime(&zero, 0).unwrap();
    let id = db.upsert_series(src, &mk_airing("zero", "Zero Show", None)).unwrap();
    db.set_followed(id, true).unwrap();
    db.set_anilist_id(id, 601).unwrap();
    insert_eps_seen_up_to(&db, id, 12, 6);

    let insights = db.get_watch_insights().unwrap();
    assert_eq!(insights.estimated_minutes_tracked, 6 * 24, "TV estimate, not 0 min/ep");
    assert_eq!(insights.backlog_episodes, 6);
    assert_eq!(insights.backlog_minutes, 6 * 24, "TV estimate, not 0 min/ep");
}

#[test]
fn get_activity_years_stays_descending_with_a_future_year() {
    // A future-dated mark (clock skew, restored backup) must not leave the
    // current year inserted ahead of it.
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("A", "a", "animeytx").unwrap();
    let id = db.upsert_series(src, &mk_airing("x", "X", None)).unwrap();
    insert_eps_seen_up_to(&db, id, 1, 1);
    let this_year = chrono::Local::now().date_naive().year();
    db.conn
        .execute(
            "UPDATE episodes SET seen_at = ?1 WHERE series_id = ?2",
            rusqlite::params![format!("{}-07-15 12:00:00", this_year + 1), id],
        )
        .unwrap();

    assert_eq!(db.get_activity_years().unwrap(), vec![this_year + 1, this_year]);
}

/// A followed row on `src` titled `title`, linked to `anilist_id`, with
/// `total` episodes of which the first `seen` are marked seen.
fn linked_row(db: &Db, src: i64, slug: &str, title: &str, anilist_id: i64, total: i64, seen: i64) -> i64 {
    let id = db.upsert_series(src, &mk_airing(slug, title, None)).unwrap();
    db.set_followed(id, true).unwrap();
    db.set_anilist_id(id, anilist_id).unwrap();
    insert_eps_seen_up_to(db, id, total, seen);
    id
}

#[test]
fn real_progress_caps_a_cumulative_row_at_its_seasons_catalog_length() {
    // High Card shape from the reference database: one site numbers the
    // whole show 1-25 on a row linked to the 12-episode S1 entry, another
    // splits it into S1 (1-12) and S2 (1-12). Labels 13-25 of the first
    // row are S2 episodes, not more S1 ones: 25, never 25 + 12.
    let db = Db::open(":memory:").unwrap();
    let a = db.upsert_source("AnimeYT", "a", "animeytx").unwrap();
    let b = db.upsert_source("TioAnime", "b", "tioanime").unwrap();
    db.upsert_catalog_anime(&catalog_anime_full(135778, "HIGH CARD", &[], "TV", Some(12), None), 0).unwrap();
    db.upsert_catalog_anime(&catalog_anime_full(163151, "HIGH CARD Season 2", &[], "TV", Some(12), None), 0).unwrap();

    linked_row(&db, b, "hc", "HIGH CARD", 135778, 25, 25);
    linked_row(&db, a, "hc", "HIGH CARD", 135778, 12, 12);
    linked_row(&db, a, "hc2", "High Card Temporada 2", 163151, 12, 12);

    let summary = db.get_watch_summary().unwrap();
    assert_eq!(summary.distinct_anime, 1);
    assert_eq!(summary.episodes_watched, 25);
    assert_eq!(db.get_watch_insights().unwrap().estimated_minutes_tracked, 25 * 24);
}

#[test]
fn real_progress_caps_a_cumulative_row_on_the_site_without_the_sequel_row() {
    // Mushoku Tensei shape: the cumulative row (1-23 on the 11-episode S1
    // entry) is on the site that has no S2 row; the other site has S1
    // (1-11) and S2 (1-12). 23 episodes, not 23 + 12.
    let db = Db::open(":memory:").unwrap();
    let a = db.upsert_source("AnimeYT", "a", "animeytx").unwrap();
    let b = db.upsert_source("TioAnime", "b", "tioanime").unwrap();
    db.upsert_catalog_anime(&catalog_anime_full(108465, "Mushoku Tensei", &[], "TV", Some(11), None), 0).unwrap();
    db.upsert_catalog_anime(&catalog_anime_full(127720, "Mushoku Tensei Part 2", &[], "TV", Some(12), None), 0).unwrap();

    linked_row(&db, a, "mt", "Mushoku Tensei", 108465, 23, 23);
    linked_row(&db, b, "mt", "Mushoku Tensei", 108465, 11, 11);
    linked_row(&db, b, "mt2", "Mushoku Tensei Temporada 2", 127720, 12, 12);

    assert_eq!(db.get_watch_summary().unwrap().episodes_watched, 23);
}

#[test]
fn real_progress_adds_up_linked_seasons_watched_on_different_sites() {
    // Otome Game shape: S1 (12 eps) on one site, S2 (12 eps) on another,
    // both linked — the catalog caps don't bite, 24 stands.
    let db = Db::open(":memory:").unwrap();
    let a = db.upsert_source("AnimeYT", "a", "animeytx").unwrap();
    let b = db.upsert_source("TioAnime", "b", "tioanime").unwrap();
    db.upsert_catalog_anime(&catalog_anime_full(142074, "Otome Game", &[], "TV", Some(12), None), 0).unwrap();
    db.upsert_catalog_anime(&catalog_anime_full(159309, "Otome Game Season 2", &[], "TV", Some(12), None), 0).unwrap();

    linked_row(&db, a, "og", "Otome Game", 142074, 12, 12);
    linked_row(&db, b, "og2", "Otome Game Temporada 2", 159309, 12, 12);

    assert_eq!(db.get_watch_summary().unwrap().episodes_watched, 24);
}

#[test]
fn estimated_minutes_share_mixed_matches_spec_example() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("A", "a", "animeytx").unwrap();

    // Series A: 2 seen episodes, linked to AniList catalog row with duration = 23 -> 46 mins (0 estimated)
    db.upsert_catalog_anime(
        &crate::anilist::CatalogAnime {
            id: 101, title: "Show A".into(), title_romaji: None, title_english: None,
            synonyms: Vec::new(), cover_url: None, banner_url: None, format: Some("TV".into()),
            genres: vec![], episodes: Some(12), average_score: None, popularity: None,
            url: "https://anilist.co/anime/101".into(), status: None, duration: Some(23),
            studio: None, start_date: None, next_airing_at: None, next_episode: None,
            tags: Vec::new(), relations: Vec::new(),
        },
        0,
    ).unwrap();
    let sa = db.upsert_series(src, &mk_airing("sa", "Show A", None)).unwrap();
    db.set_followed(sa, true).unwrap();
    db.set_anilist_id(sa, 101).unwrap();
    insert_eps_seen_up_to(&db, sa, 12, 2);

    // Series B: 1 seen episode, unlinked (default TV = 24) -> 24 mins (24 estimated)
    let sb = db.upsert_series(src, &mk_airing("sb", "Show B", None)).unwrap();
    db.set_followed(sb, true).unwrap();
    insert_eps_seen_up_to(&db, sb, 12, 1);

    let insights = db.get_watch_insights().unwrap();
    assert_eq!(insights.estimated_minutes_tracked, 70);
    let expected_share = 24.0 / 70.0;
    assert!(
        (insights.estimated_minutes_share - expected_share).abs() < 1e-9,
        "expected share {expected_share}, got {}",
        insights.estimated_minutes_share
    );
}

#[test]
fn estimated_minutes_share_all_known_gives_zero() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("A", "a", "animeytx").unwrap();

    db.upsert_catalog_anime(
        &crate::anilist::CatalogAnime {
            id: 201, title: "Known Show".into(), title_romaji: None, title_english: None,
            synonyms: Vec::new(), cover_url: None, banner_url: None, format: Some("TV".into()),
            genres: vec![], episodes: Some(12), average_score: None, popularity: None,
            url: "https://anilist.co/anime/201".into(), status: None, duration: Some(25),
            studio: None, start_date: None, next_airing_at: None, next_episode: None,
            tags: Vec::new(), relations: Vec::new(),
        },
        0,
    ).unwrap();
    let s = db.upsert_series(src, &mk_airing("s", "Known Show", None)).unwrap();
    db.set_followed(s, true).unwrap();
    db.set_anilist_id(s, 201).unwrap();
    insert_eps_seen_up_to(&db, s, 12, 5);

    let insights = db.get_watch_insights().unwrap();
    assert_eq!(insights.estimated_minutes_tracked, 5 * 25);
    assert_eq!(insights.estimated_minutes_share, 0.0);
}

#[test]
fn estimated_minutes_share_all_unlinked_gives_one() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("A", "a", "animeytx").unwrap();

    let s = db.upsert_series(src, &mk_airing("s", "Unlinked Show", None)).unwrap();
    db.set_followed(s, true).unwrap();
    insert_eps_seen_up_to(&db, s, 12, 4);

    let insights = db.get_watch_insights().unwrap();
    assert_eq!(insights.estimated_minutes_tracked, 4 * 24);
    assert_eq!(insights.estimated_minutes_share, 1.0);
}

#[test]
fn get_popularity_bias_percentile_matches_spec_example() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("A", "a", "animeytx").unwrap();

    // Catalog popularities [10, 20, 30, 40]
    for (id, pop) in [(1, 10), (2, 20), (3, 30), (4, 40)] {
        db.upsert_catalog_anime(
            &crate::anilist::CatalogAnime {
                id, title: format!("Pop Show {id}"), title_romaji: None, title_english: None,
                synonyms: Vec::new(), cover_url: None, banner_url: None, format: Some("TV".into()),
                genres: vec![], episodes: Some(12), average_score: None, popularity: Some(pop),
                url: format!("https://anilist.co/anime/{id}"), status: None, duration: Some(24),
                studio: None, start_date: None, next_airing_at: None, next_episode: None,
                tags: Vec::new(), relations: Vec::new(),
            },
            0,
        ).unwrap();
        let sid = db.upsert_series(src, &mk_airing(&format!("s{id}"), &format!("Pop Show {id}"), None)).unwrap();
        db.set_followed(sid, true).unwrap();
        db.set_anilist_id(sid, id).unwrap();
    }

    // User watched 4 series with popularities [10, 20, 30, 40]
    // User average = (10 + 20 + 30 + 40) / 4 = 25.0
    // Catalog popularities strictly below 25.0 are 10, 20 (2 out of 4) -> percentile 50.0
    let bias = db.get_popularity_bias().unwrap();
    assert_eq!(bias.average_popularity, Some(25.0));
    assert_eq!(bias.percentile, Some(50.0));
}

#[test]
fn get_dusty_watchlist_skips_a_finished_season_beside_a_never_started_one() {
    // S1 finished 120 days ago, S2 followed but never started: that's a
    // backlog entry, not an abandoned show — the only unseen episodes
    // belong to a member nobody has started.
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("A", "a", "animeytx").unwrap();

    let s1 = db.upsert_series(src, &mk_airing("ky1", "Kimetsu no Yaiba", None)).unwrap();
    db.set_followed(s1, true).unwrap();
    insert_eps_seen_up_to(&db, s1, 12, 12);
    backdate_seen(&db, s1, 120);

    let s2 = db.upsert_series(src, &mk_airing("ky2", "Kimetsu no Yaiba Temporada 2", None)).unwrap();
    db.set_followed(s2, true).unwrap();
    insert_eps_seen_up_to(&db, s2, 12, 0);

    assert!(db.get_dusty_watchlist().unwrap().is_empty());

}
