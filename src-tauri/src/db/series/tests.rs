use super::*;

#[test]
fn upsert_series_writes_and_updates_scan_owned_airing_metadata() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
    let t1 = chrono::Utc::now().timestamp() - 7 * 86400;
    let t2 = t1 + 7 * 86400;
    let mut s = mk_airing("x", "X", Some(t1));
    s.site_episode_count = Some(2);
    let sid = db.upsert_series(src, &s).unwrap();
    let got = db.list_airing(src).unwrap().into_iter().find(|r| r.id == sid).unwrap();
    assert_eq!(got.next_episode_at, Some(t1));
    assert_eq!(got.site_episode_count, Some(2));

    // A re-scan with fresh values overwrites both (scan-owned fields).
    let mut s2 = mk_airing("x", "X", Some(t2));
    s2.site_episode_count = Some(3);
    db.upsert_series(src, &s2).unwrap();
    let got = db.list_airing(src).unwrap().into_iter().find(|r| r.id == sid).unwrap();
    assert_eq!(got.next_episode_at, Some(t2));
    assert_eq!(got.site_episode_count, Some(3));
}

/// A fetched cover (`data:` URI) must survive the next airing-listing
/// upsert. `refresh()` upserts every listing card before it reads
/// `list_followed`, so clobbering the stored `data:` URI with the
/// listing's remote thumbnail put every followed airing series back into
/// the "needs a cover fetch" state on every cycle — re-opening a WebView2
/// window per series, per refresh, forever.
#[test]
fn upsert_series_never_clobbers_an_already_fetched_data_uri_cover() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
    let mut s = mk_airing("x", "X", None);
    s.cover_url = Some("https://site/wp-content/x.jpg".into());
    let sid = db.upsert_series(src, &s).unwrap();

    // refresh()'s cover step downloads it and stores the pixels inline.
    const FETCHED: &str = "data:image/png;base64,AAAA";
    db.update_series_cover(sid, FETCHED).unwrap();

    // Next scan re-sends the original remote thumbnail on the card.
    let mut again = mk_airing("x", "X", None);
    again.cover_url = Some("https://site/wp-content/x.jpg".into());
    assert_eq!(db.upsert_series(src, &again).unwrap(), sid, "same row");
    assert_eq!(
        stored_cover(&db, sid).as_deref(),
        Some(FETCHED),
        "the fetched data: URI must survive the re-scan"
    );

    // A card that carries no cover at all must not wipe it either.
    let none_cover = mk_airing("x", "X", None);
    assert_eq!(none_cover.cover_url, None, "sanity: this card has no cover");
    db.upsert_series(src, &none_cover).unwrap();
    assert_eq!(stored_cover(&db, sid).as_deref(), Some(FETCHED));
}

/// The raw `series.cover_url` column — deliberately not `list_airing`,
/// which COALESCEs a linked catalog poster over the site's own value.
fn stored_cover(db: &Db, series_id: i64) -> Option<String> {
    db.conn
        .query_row("SELECT cover_url FROM series WHERE id=?1", [series_id], |r| r.get(0))
        .unwrap()
}

/// The guard is narrow on purpose: it only protects a stored `data:` URI
/// from a *remote* value. Everything else about `cover_url` still behaves
/// like the scan-owned field it is.
#[test]
fn upsert_series_still_refreshes_a_remote_cover_and_accepts_a_new_data_uri() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
    let mut s = mk_airing("x", "X", None);
    s.cover_url = Some("https://old.site/x.jpg".into());
    let sid = db.upsert_series(src, &s).unwrap();

    // remote -> remote: the site moved the image, take the new URL.
    let mut moved = mk_airing("x", "X", None);
    moved.cover_url = Some("https://new.site/x.jpg".into());
    db.upsert_series(src, &moved).unwrap();
    assert_eq!(stored_cover(&db, sid).as_deref(), Some("https://new.site/x.jpg"));

    // remote -> data:: a freshly fetched image is strictly newer, take it.
    let mut fetched = mk_airing("x", "X", None);
    fetched.cover_url = Some("data:image/png;base64,BBBB".into());
    db.upsert_series(src, &fetched).unwrap();
    assert_eq!(stored_cover(&db, sid).as_deref(), Some("data:image/png;base64,BBBB"));

    // data: -> data:: also taken (only a non-data: value is held off).
    let mut newer = mk_airing("x", "X", None);
    newer.cover_url = Some("data:image/png;base64,CCCC".into());
    db.upsert_series(src, &newer).unwrap();
    assert_eq!(stored_cover(&db, sid).as_deref(), Some("data:image/png;base64,CCCC"));

    // remote -> NULL on a row whose cover is NOT a data: URI still clears
    // it, exactly as before — the guard is only about protecting a real
    // downloaded image.
    db.update_series_cover(sid, "https://plain.site/x.jpg").unwrap();
    db.upsert_series(src, &mk_airing("x", "X", None)).unwrap();
    assert_eq!(stored_cover(&db, sid), None);
}

#[test]
fn a_failed_airing_cover_backs_off_until_its_url_changes_or_time_passes() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
    let mut dead = mk_airing("dead", "Dead Cover", None);
    dead.cover_url = Some("https://dead.example/a.jpg".into());
    let sid = db.upsert_series(src, &dead).unwrap();
    assert_eq!(db.airing_series_needing_cover_fetch(20, None).unwrap().len(), 1);

    db.record_cover_failure(sid, "https://dead.example/a.jpg").unwrap();
    assert!(db.airing_series_needing_cover_fetch(20, None).unwrap().is_empty(), "just failed -> skipped");

    // First backoff is 6 h.
    db.conn.execute("UPDATE series SET cover_failed_at=datetime('now','-7 hours') WHERE id=?1", [sid]).unwrap();
    assert_eq!(db.airing_series_needing_cover_fetch(20, None).unwrap().len(), 1, "6 h passed -> retried");

    // Second failure of the same URL doubles it to 12 h.
    db.record_cover_failure(sid, "https://dead.example/a.jpg").unwrap();
    db.conn.execute("UPDATE series SET cover_failed_at=datetime('now','-7 hours') WHERE id=?1", [sid]).unwrap();
    assert!(db.airing_series_needing_cover_fetch(20, None).unwrap().is_empty(), "12 h backoff after 2 failures");

    // Capped at 7 days however many failures.
    for _ in 0..20 {
        db.record_cover_failure(sid, "https://dead.example/a.jpg").unwrap();
    }
    db.conn.execute("UPDATE series SET cover_failed_at=datetime('now','-8 days') WHERE id=?1", [sid]).unwrap();
    assert_eq!(db.airing_series_needing_cover_fetch(20, None).unwrap().len(), 1, "cap is 7 days");

    // A new URL from the site is tried at once.
    db.record_cover_failure(sid, "https://dead.example/a.jpg").unwrap();
    db.conn.execute("UPDATE series SET cover_url='https://cdn.example/new.jpg' WHERE id=?1", [sid]).unwrap();
    assert_eq!(db.airing_series_needing_cover_fetch(20, None).unwrap().len(), 1, "different url -> no backoff");
}

#[test]
fn airing_series_needing_cover_fetch_finds_unfollowed_unlinked_airing_rows_only() {
    // The bug: covers only ever got fetched for followed series (the
    // main fetch loop and its backlog are both scoped to
    // list_followed), so an airing series nobody follows kept its raw
    // remote thumbnail forever — CSP silently blocks it, so it just
    // never showed a photo. Confirmed live: 279 of 308 currently-airing
    // series across one real library.
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();

    // Needs a fetch: airing, unfollowed, unlinked, remote cover.
    let mut needs_fetch = mk_airing("needs-fetch", "Needs Fetch", None);
    needs_fetch.cover_url = Some("https://site/needs-fetch.jpg".into());
    let sid_needs = db.upsert_series(src, &needs_fetch).unwrap();

    // Already fetched: must not show up again.
    let mut already = mk_airing("already", "Already Fetched", None);
    already.cover_url = Some("https://site/already.jpg".into());
    let sid_already = db.upsert_series(src, &already).unwrap();
    db.update_series_cover(sid_already, "data:image/png;base64,AAAA").unwrap();

    // AniList-linked: list_airing shows the catalog's own cover for
    // these instead (see db/airing.rs), so fetching this column would
    // be wasted work, even if it has no cover in the airing listing.
    let mut linked = mk_airing("linked", "Linked Show", None);
    linked.cover_url = None;
    let sid_linked = db.upsert_series(src, &linked).unwrap();
    db.set_anilist_id(sid_linked, 999).unwrap();

    // Unlinked with no cover: select it so its series page can fill the cover.
    let sid_no_cover = db.upsert_series(src, &mk_airing("no-cover", "No Cover", None)).unwrap();

    // Not airing: out of scope for this backlog (finished shows are the
    // followed-only backlog's job, not this one's).
    let mut finished = mk_airing("finished", "Finished Show", None);
    finished.cover_url = Some("https://site/finished.jpg".into());
    finished.is_airing = false;
    db.upsert_series(src, &finished).unwrap();

    let rows = db.airing_series_needing_cover_fetch(20, None).unwrap();
    assert_eq!(rows.len(), 1, "a NULL cover is only picked for the site that can fill it");
    let rows = db.airing_series_needing_cover_fetch(20, Some(src)).unwrap();
    assert_eq!(rows.len(), 2, "remote-cover and NULL-cover unlinked airing rows qualify");
    assert!(rows.iter().any(|row| row.0 == sid_needs));
    assert!(rows.iter().any(|row| row.0 == sid_no_cover));
    assert!(!rows.iter().any(|row| row.0 == sid_linked));
    let needs = rows.iter().find(|row| row.0 == sid_needs).unwrap();
    assert_eq!(needs.2.as_deref(), Some("https://site/needs-fetch.jpg"));

    // The cap is real, not just a LIMIT that happens not to bite here.
    for i in 0..5 {
        let mut s = mk_airing(&format!("more-{i}"), &format!("More {i}"), None);
        s.cover_url = Some(format!("https://site/more-{i}.jpg"));
        db.upsert_series(src, &s).unwrap();
    }
    assert_eq!(db.airing_series_needing_cover_fetch(3, None).unwrap().len(), 3);
}

#[test]
fn upsert_series_slug_change_updates_existing_row_and_keeps_seen_history() {
    // Reproduces the "Youjo Senki II" bug: the site restructures a
    // series' slug between scans. A slug-keyed upsert would treat that
    // as a brand new series, orphaning the seen episodes under the old
    // id. Matching on normalized title within the same source instead
    // should update the existing row in place.
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
    let sid = db.upsert_series(src, &mk_airing("youjo-senki-2", "Youjo Senki II", None)).unwrap();
    insert_eps_seen_up_to(&db, sid, 5, 5);

    // Site moves the slug (e.g. "-2" -> "-ii") on a later scan; title
    // (once normalized) is unchanged.
    let sid2 = db.upsert_series(src, &mk_airing("youjo-senki-ii", "Youjo Senki II", None)).unwrap();

    assert_eq!(sid, sid2, "slug drift on the same source/title must reuse the existing series id");
    let eps = db.list_series_episodes(sid).unwrap();
    assert_eq!(eps.len(), 5, "seen episodes must still be reachable under the same series id");
    assert!(eps.iter().all(|e| e.seen), "seen state must survive the slug change");
}

/// The title-fallback over-merge: `normalize_title` strips noise suffixes
/// ("latino", "sub espanol", "hd", "online"), so two GENUINELY separate
/// cards on the same listing that differ only by such a suffix normalized
/// to the same string. The fallback then folded the second card onto the
/// first row and the UPDATE overwrote its slug/url/cover_url — two shows
/// collapsed into one, with the survivor pointing at the wrong page.
#[test]
fn upsert_series_keeps_two_cards_that_differ_only_by_a_noise_suffix_separate() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();

    let mut plain = mk_airing("naruto", "Naruto", None);
    plain.url = "https://site/anime/naruto/".into();
    plain.cover_url = Some("https://site/covers/naruto.jpg".into());
    let plain_id = db.upsert_series(src, &plain).unwrap();

    let mut latino = mk_airing("naruto-latino", "Naruto Latino", None);
    latino.url = "https://site/anime/naruto-latino/".into();
    latino.cover_url = Some("https://site/covers/naruto-latino.jpg".into());
    let latino_id = db.upsert_series(src, &latino).unwrap();

    assert_ne!(plain_id, latino_id, "two distinct listing cards must stay two rows");
    let rows: i64 = db
        .conn
        .query_row("SELECT COUNT(*) FROM series WHERE source_id=?1", [src], |r| r.get(0))
        .unwrap();
    assert_eq!(rows, 2);
    // The first card kept its own slug/url/cover — nothing was overwritten.
    let (slug, url, cover): (String, String, Option<String>) = db
        .conn
        .query_row("SELECT slug, url, cover_url FROM series WHERE id=?1", [plain_id], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })
        .unwrap();
    assert_eq!(slug, "naruto");
    assert_eq!(url, "https://site/anime/naruto/");
    assert_eq!(cover.as_deref(), Some("https://site/covers/naruto.jpg"));

    // Re-scanning either card still resolves to its own row (idempotent).
    assert_eq!(db.upsert_series(src, &plain).unwrap(), plain_id);
    assert_eq!(db.upsert_series(src, &latino).unwrap(), latino_id);
}

/// The guard must not cost the fallback its actual job: a card whose title
/// is unchanged but whose slug the site restructured still merges, even
/// when the two slugs share no prefix at all.
#[test]
fn upsert_series_title_fallback_still_merges_a_drifted_slug() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
    let a = db.upsert_series(src, &mk_airing("youjo-senki-2", "Youjo Senki II", None)).unwrap();
    // Same title, a wholly different slug shape.
    let b = db.upsert_series(src, &mk_airing("anime-youjo-senki-ii", "Youjo Senki II", None)).unwrap();
    assert_eq!(a, b, "identical titles are pure slug drift and must merge");
}

/// …and a card whose *title* gained/lost punctuation or case still merges
/// (the strict normalization only keeps the noise tokens, nothing else).
#[test]
fn upsert_series_title_fallback_merges_across_punctuation_and_case() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
    let a = db.upsert_series(src, &mk_airing("bocchi", "Bocchi the Rock!", None)).unwrap();
    let b = db.upsert_series(src, &mk_airing("bocchi-the-rock", "BOCCHI THE ROCK", None)).unwrap();
    assert_eq!(a, b);
}

/// A related slug rescues the merge when only the title picked up noise:
/// the URL path says it is the same card, so the tail is real evidence.
#[test]
fn upsert_series_title_fallback_merges_when_the_slug_tail_is_not_noise() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
    let a = db.upsert_series(src, &mk_airing("naruto", "Naruto", None)).unwrap();
    // Title gained a noise word, but the slug tail ("tv") is a real token.
    let b = db.upsert_series(src, &mk_airing("naruto-tv", "Naruto HD", None)).unwrap();
    assert_eq!(a, b);
}

#[test]
fn slugs_are_related_rejects_a_pure_noise_tail() {
    assert!(slugs_are_related("naruto", "naruto"));
    assert!(slugs_are_related("naruto", "naruto-tv"));
    assert!(slugs_are_related("youjo-senki-2", "youjo_senki_2"));
    // Purely a noise tail — the exact coincidence the guard exists for.
    assert!(!slugs_are_related("naruto", "naruto-latino"));
    assert!(!slugs_are_related("bleach", "bleach-sub-espanol"));
    assert!(!slugs_are_related("one-piece", "one-piece-hd"));
    // Mid-word, not a token boundary.
    assert!(!slugs_are_related("naruto", "narutox"));
    // Unrelated slugs.
    assert!(!slugs_are_related("naruto", "bleach"));
    assert!(!slugs_are_related("", "naruto"));
}

/// A failed cross-site import must not leave an unreachable `anilist-N`
/// placeholder behind (its url points at anilist.co, not the site) — see
/// `delete_orphan_synthetic_series`. The guards make it safe to call
/// blindly on the error path.
#[test]
fn delete_orphan_synthetic_series_only_removes_untouched_placeholders() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("TioAnime", "b", "tioanime").unwrap();

    let synthetic = |slug: &str| crate::models::Series {
        id: 0, slug: slug.into(), title: "Placeholder".into(),
        url: "https://anilist.co/anime/21".into(), cover_url: None,
        is_airing: false, followed: false, next_episode_at: None, site_episode_count: None,
    };

    // An untouched placeholder from a failed import: deleted.
    let orphan = db.upsert_series(src, &synthetic("anilist-21")).unwrap();
    db.set_anilist_id(orphan, 21).unwrap();
    assert!(db.delete_orphan_synthetic_series(orphan).unwrap());
    assert_eq!(db.get_series_url(orphan).unwrap(), None);

    // A real (linked) row is never touched, even with an anilist_id set.
    let real = db.upsert_series(src, &mk_airing("one-piece", "One Piece", None)).unwrap();
    db.set_anilist_id(real, 21).unwrap();
    assert!(!db.delete_orphan_synthetic_series(real).unwrap());
    assert!(db.get_series_url(real).unwrap().is_some());

    // A placeholder the user has classified is real state — kept.
    let wanted = db.upsert_series(src, &synthetic("anilist-99")).unwrap();
    db.set_anilist_id(wanted, 99).unwrap();
    db.set_backlog_status(wanted, Some("want")).unwrap();
    assert!(!db.delete_orphan_synthetic_series(wanted).unwrap());
    assert!(db.get_series_url(wanted).unwrap().is_some());

    // …as is one that somehow already has episodes.
    let with_eps = db.upsert_series(src, &synthetic("anilist-100")).unwrap();
    db.set_anilist_id(with_eps, 100).unwrap();
    db.insert_episode(&crate::models::Episode {
        id: 0, series_id: with_eps, number: "1".into(), title: None,
        url: "https://site/x-1".into(), released_at: None, seen: false, download_path: None, download_status: None,
    }).unwrap();
    assert!(!db.delete_orphan_synthetic_series(with_eps).unwrap());
    assert!(db.get_series_url(with_eps).unwrap().is_some());
}

#[test]
fn upsert_series_different_title_creates_new_row() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
    let sid = db.upsert_series(src, &mk_airing("one-piece", "One Piece", None)).unwrap();
    let sid2 = db.upsert_series(src, &mk_airing("naruto", "Naruto", None)).unwrap();
    assert_ne!(sid, sid2, "genuinely different series must not be merged");
}

#[test]
fn carry_follow_only_touches_unfollowed_rows() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("A", "a", "animeytx").unwrap();

    let sid = db.upsert_series(src, &mk_airing("s", "S", None)).unwrap();
    assert!(db.carry_follow(sid, 4).unwrap(), "unfollowed row carries");
    assert_eq!(db.take_carried_seen_number(sid).unwrap(), Some(4));
    // Applied once: a second take yields None.
    assert_eq!(db.take_carried_seen_number(sid).unwrap(), None);

    // Already-followed row: carry is a no-op and leaves no watermark.
    let sid2 = db.upsert_series(src, &mk_airing("t", "T", None)).unwrap();
    db.set_followed(sid2, true).unwrap();
    assert!(!db.carry_follow(sid2, 9).unwrap(), "followed row is left untouched");
    assert_eq!(db.take_carried_seen_number(sid2).unwrap(), None);
}

#[test]
fn carry_follow_never_re_follows_a_row_the_user_explicitly_unfollowed() {
    // The reported bug: unfollow a show, and a fuzzy title match against
    // a still-followed sibling on another site (score()'s season-blind
    // same_franchise floor treating "Temporada 2" and a same-base
    // sibling as interchangeable) silently re-followed it — and
    // re-cascaded its old watermark seen — on the very next scan.
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("A", "a", "animeytx").unwrap();
    let sid = db.upsert_series(src, &mk_airing("s", "S", None)).unwrap();
    let is_followed = |db: &Db, id: i64| -> bool {
        db.conn
            .query_row("SELECT followed FROM series WHERE id=?1", [id], |r| r.get::<_, i64>(0))
            .unwrap()
            != 0
    };

    db.set_followed(sid, true).unwrap();
    db.set_followed_canonical(sid, false).unwrap();
    assert!(!is_followed(&db, sid));

    assert!(
        !db.carry_follow(sid, 7).unwrap(),
        "a declined row must not be re-followed by carry-over"
    );
    assert!(!is_followed(&db, sid));
    assert_eq!(db.take_carried_seen_number(sid).unwrap(), None, "no watermark leaks through either");

    // An explicit re-follow clears the decline, so carry-over is welcome
    // to touch the row normally again afterwards.
    db.set_followed_canonical(sid, true).unwrap();
    db.set_followed(sid, false).unwrap(); // back to unfollowed, but no longer declined
    assert!(db.carry_follow(sid, 7).unwrap(), "decline was cleared by the explicit re-follow");
}

#[test]
fn carried_watermark_marks_episodes_seen_via_cascade() {
    // Simulates refresh()'s apply-once step: carry a follow, later fetch
    // the new site's episodes, then apply the stored watermark.
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("B", "b", "tioanime").unwrap();
    let sid = db.upsert_series(src, &mk_airing("x", "X", None)).unwrap();

    db.carry_follow(sid, 3).unwrap();
    // Episodes arrive on the new site (all unseen initially).
    insert_eps_seen_up_to(&db, sid, 6, 0);

    // refresh() step: read+clear the watermark, cascade-mark up to it.
    let n = db.take_carried_seen_number(sid).unwrap().expect("watermark present");
    db.set_seen_cascade(sid, &n.to_string(), true).unwrap();

    let eps = db.list_series_episodes(sid).unwrap();
    let seen: Vec<&str> = eps.iter().filter(|e| e.seen).map(|e| e.number.as_str()).collect();
    assert_eq!(seen, vec!["1", "2", "3"], "episodes 1..=3 seen, 4..=6 unseen");
    // Applied once — column cleared.
    assert_eq!(db.take_carried_seen_number(sid).unwrap(), None);
}

#[test]
fn unfollowed_watched_externally_series_with_real_seen_episodes_credits_the_remainder() {
    // followed=0, watched_externally=1, 3 real seen episodes, ALSO linked
    // to a catalog row with 12 episodes. "Ya lo vi" means the whole show
    // was watched, so all 12 are credited — but the 3 real marks are not
    // counted twice: they stay in episodes_watched and only the 9-episode
    // remainder lands in episodes_watched_external.
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("A", "a", "animeytx").unwrap();

    db.upsert_catalog_anime(
        &crate::anilist::CatalogAnime {
            id: 300, title: "Ext".into(), title_romaji: None, title_english: None, synonyms: Vec::new(), next_airing_at: None, next_episode: None, tags: Vec::new(), relations: Vec::new(),
            cover_url: None, banner_url: None, format: Some("TV".into()), genres: vec![],
            episodes: Some(12), average_score: None, popularity: None,
            url: "https://anilist.co/anime/300".into(), status: None, duration: None,
            studio: None, start_date: None,
        },
        0,
    ).unwrap();
    let sid = db.upsert_series(src, &mk_airing("ext1", "Ext", None)).unwrap();
    db.set_watched_externally(sid, true).unwrap();
    db.set_anilist_id(sid, 300).unwrap();
    db.set_kind(sid, "TV").unwrap();
    for n in 1..=3 {
        db.insert_episode(&crate::models::Episode {
            id: 0, series_id: sid, number: n.to_string(), title: None,
            url: format!("https://site/anime/ext1-capitulo-{n}/"),
            released_at: None, seen: true, download_path: None, download_status: None,
        }).unwrap();
    }

    let summary = db.get_watch_summary().unwrap();
    assert_eq!(summary.episodes_watched, 3, "the 3 real seen episodes count, even though the series isn't followed");
    assert_eq!(summary.episodes_watched_external, 9, "12 catalog episodes minus the 3 already marked");
    assert_eq!(
        summary.episodes_watched + summary.episodes_watched_external, 12,
        "the two figures add up to the catalog total, never past it"
    );

    let insights = db.get_watch_insights().unwrap();
    assert_eq!(insights.estimated_minutes_tracked, 72, "3 seen eps * 24 min (TV), tracked minutes aren't followed-only anymore");
    assert_eq!(insights.estimated_minutes_external, 9 * 24, "remainder minutes only");
}

#[test]
fn watched_externally_series_without_episodes_counts_via_catalog_estimate() {
    // watched_externally=1, no scraped episodes at all: falls back to the
    // catalog's episode count for both the episode count and the minutes.
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("A", "a", "animeytx").unwrap();

    db.upsert_catalog_anime(
        &crate::anilist::CatalogAnime {
            id: 301, title: "Ext2".into(), title_romaji: None, title_english: None, synonyms: Vec::new(), next_airing_at: None, next_episode: None, tags: Vec::new(), relations: Vec::new(),
            cover_url: None, banner_url: None, format: Some("TV".into()), genres: vec![],
            episodes: Some(12), average_score: None, popularity: None,
            url: "https://anilist.co/anime/301".into(), status: None, duration: None,
            studio: None, start_date: None,
        },
        0,
    ).unwrap();
    let sid = db.upsert_series(src, &mk_airing("ext2", "Ext2", None)).unwrap();
    db.set_watched_externally(sid, true).unwrap();
    db.set_anilist_id(sid, 301).unwrap();

    let summary = db.get_watch_summary().unwrap();
    assert_eq!(summary.episodes_watched, 0, "no real episodes for this series");
    assert_eq!(summary.episodes_watched_external, 12, "falls back to the catalog's episode count");

    let insights = db.get_watch_insights().unwrap();
    assert_eq!(insights.estimated_minutes_external, 12 * 24, "12 catalog eps * minutes_per_episode(TV)");
}

#[test]
fn followed_series_with_no_seen_episodes_excluded_from_distinct_anime() {
    // followed=1 with zero seen episodes must NOT count toward
    // distinct_anime — it has no watch evidence. A watched-externally
    // series (with seen evidence) still counts.
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("A", "a", "animeytx").unwrap();

    // Followed, zero episodes seen — must be excluded.
    let unseen_id = db.upsert_series(src, &mk_airing("unseen1", "Unseen Show", None)).unwrap();
    db.set_followed(unseen_id, true).unwrap();
    db.insert_episode(&crate::models::Episode {
        id: 0, series_id: unseen_id, number: "1".into(), title: None,
        url: "https://site/anime/unseen1-capitulo-1/".into(),
        released_at: None, seen: false, download_path: None, download_status: None,
    }).unwrap();

    // Watched-externally, no scraped episodes — has watch evidence via
    // the flag itself, must count.
    let ext_id = db.upsert_series(src, &mk_airing("ext3", "Ext3", None)).unwrap();
    db.set_watched_externally(ext_id, true).unwrap();

    let summary = db.get_watch_summary().unwrap();
    assert_eq!(summary.distinct_anime, 1, "only the watched-externally show counts; the unwatched followed show is excluded");
}

#[test]
fn followed_and_watched_externally_series_with_seen_episodes_counts_each_episode_once() {
    // followed=1 AND watched_externally=1, linked to a catalog row, with
    // real seen episodes: every episode contributes exactly once. The 3
    // real ones go to the tracked side and only the remaining 9 of the
    // catalog's 12 to the external side — never 12 on top of 3.
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("A", "a", "animeytx").unwrap();

    db.upsert_catalog_anime(
        &crate::anilist::CatalogAnime {
            id: 302, title: "Both".into(), title_romaji: None, title_english: None, synonyms: Vec::new(), next_airing_at: None, next_episode: None, tags: Vec::new(), relations: Vec::new(),
            cover_url: None, banner_url: None, format: Some("TV".into()), genres: vec![],
            episodes: Some(12), average_score: None, popularity: None,
            url: "https://anilist.co/anime/302".into(), status: None, duration: None,
            studio: None, start_date: None,
        },
        0,
    ).unwrap();
    let sid = db.upsert_series(src, &mk_airing("both1", "Both", None)).unwrap();
    db.set_followed(sid, true).unwrap();
    db.set_watched_externally(sid, true).unwrap();
    db.set_anilist_id(sid, 302).unwrap();
    db.set_kind(sid, "TV").unwrap();
    for n in 1..=3 {
        db.insert_episode(&crate::models::Episode {
            id: 0, series_id: sid, number: n.to_string(), title: None,
            url: format!("https://site/anime/both1-capitulo-{n}/"),
            released_at: None, seen: true, download_path: None, download_status: None,
        }).unwrap();
    }

    let insights = db.get_watch_insights().unwrap();
    assert_eq!(insights.estimated_minutes_tracked, 72, "3 seen eps * 24 min (TV)");
    assert_eq!(insights.estimated_minutes_external, 9 * 24, "the other 9 catalog eps, counted once");

    // Criterion 4: episodes and hours cover the exact same universe —
    // episodes_watched + episodes_watched_external matches what the
    // minutes were computed from (3 real eps + 9 remainder eps).
    let summary = db.get_watch_summary().unwrap();
    assert_eq!(summary.episodes_watched, 3);
    assert_eq!(summary.episodes_watched_external, 9);
    assert_eq!(
        (summary.episodes_watched + summary.episodes_watched_external) * 24,
        insights.estimated_minutes_tracked + insights.estimated_minutes_external
    );
}

#[test]
fn series_genres_insert_is_idempotent_and_lists_sorted() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
    let s = crate::models::Series {
        id: 0, slug: "x".into(), title: "X".into(),
        url: "u".into(), cover_url: None, is_airing: false, followed: false, next_episode_at: None, site_episode_count: None,
    };
    let sid = db.upsert_series(src, &s).unwrap();

    db.insert_series_genres(sid, &["Seinen".to_string(), "Drama".to_string()]).unwrap();
    // inserting the same genres again must not error or duplicate
    db.insert_series_genres(sid, &["Drama".to_string()]).unwrap();

    let genres = db.list_series_genres(sid).unwrap();
    assert_eq!(genres, vec!["Drama".to_string(), "Seinen".to_string()]);
}

#[test]
fn backlog_status_and_kind_round_trip() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
    let s = crate::models::Series {
        id: 0, slug: "x".into(), title: "X".into(),
        url: "u".into(), cover_url: None, is_airing: false, followed: false, next_episode_at: None, site_episode_count: None,
    };
    let sid = db.upsert_series(src, &s).unwrap();

    assert_eq!(db.get_backlog_status(sid).unwrap(), None);
    db.set_backlog_status(sid, Some("want")).unwrap();
    assert_eq!(db.get_backlog_status(sid).unwrap(), Some("want".to_string()));
    db.set_kind(sid, "TV").unwrap();

    // start_watching's transition: 'want' -> followed, backlog_status cleared
    db.set_followed(sid, true).unwrap();
    db.set_backlog_status(sid, None).unwrap();
    assert_eq!(db.get_backlog_status(sid).unwrap(), None);
    assert!(db.list_followed(src).unwrap().iter().any(|f| f.id == sid));
}

#[test]
fn list_backlog_filters_by_status() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
    let want = crate::models::Series {
        id: 0, slug: "want".into(), title: "Want".into(),
        url: "u1".into(), cover_url: None, is_airing: false, followed: false, next_episode_at: None, site_episode_count: None,
    };
    let sid_want = db.upsert_series(src, &want).unwrap();
    db.set_backlog_status(sid_want, Some("want")).unwrap();

    let discarded = crate::models::Series {
        id: 0, slug: "disc".into(), title: "Disc".into(),
        url: "u2".into(), cover_url: None, is_airing: false, followed: false, next_episode_at: None, site_episode_count: None,
    };
    let sid_disc = db.upsert_series(src, &discarded).unwrap();
    db.set_backlog_status(sid_disc, Some("discarded")).unwrap();

    let wants = db.list_backlog(src, "want").unwrap();
    assert_eq!(wants.len(), 1);
    assert_eq!(wants[0].id, sid_want);

    let discards = db.list_backlog(src, "discarded").unwrap();
    assert_eq!(discards.len(), 1);
    assert_eq!(discards[0].id, sid_disc);
}

#[test]
fn list_watched_externally_filters_by_flag() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();

    let watched = crate::models::Series {
        id: 0, slug: "watched".into(), title: "Watched".into(),
        url: "u1".into(), cover_url: None, is_airing: false, followed: false, next_episode_at: None, site_episode_count: None,
    };
    let sid_watched = db.upsert_series(src, &watched).unwrap();
    db.set_watched_externally(sid_watched, true).unwrap();

    let other = crate::models::Series {
        id: 0, slug: "other".into(), title: "Other".into(),
        url: "u2".into(), cover_url: None, is_airing: false, followed: false, next_episode_at: None, site_episode_count: None,
    };
    let sid_other = db.upsert_series(src, &other).unwrap();
    db.set_backlog_status(sid_other, Some("want")).unwrap();

    let watched_rows = db.list_watched_externally(src).unwrap();
    assert_eq!(watched_rows.len(), 1);
    assert_eq!(watched_rows[0].id, sid_watched);
}

/// The Listas view must show a Want/Descartar decision taken under
/// another site: the Descubrir deck excludes it globally regardless of
/// the active site, so scoping the listing per-site left the user with no
/// UI path at all to review or reverse it after a site switch.
#[test]
fn list_backlog_still_lists_a_decision_made_under_another_site() {
    let db = Db::open(":memory:").unwrap();
    let a = db.upsert_source("AnimeYT", "a", "animeytx").unwrap();
    let b = db.upsert_source("TioAnime", "b", "tioanime").unwrap();

    let want = crate::models::Series {
        id: 0, slug: "anilist-1".into(), title: "Decided On A".into(),
        url: "https://anilist.co/anime/1".into(), cover_url: None, is_airing: false,
        followed: false, next_episode_at: None, site_episode_count: None,
    };
    let sid_want = db.upsert_series(a, &want).unwrap();
    db.set_backlog_status(sid_want, Some("want")).unwrap();

    let discarded = crate::models::Series {
        id: 0, slug: "anilist-2".into(), title: "Discarded On A".into(),
        url: "https://anilist.co/anime/2".into(), cover_url: None, is_airing: false,
        followed: false, next_episode_at: None, site_episode_count: None,
    };
    let sid_disc = db.upsert_series(a, &discarded).unwrap();
    db.set_backlog_status(sid_disc, Some("discarded")).unwrap();

    // Site B is now the active one — both decisions must still be listed.
    let wants = db.list_backlog(b, "want").unwrap();
    assert_eq!(wants.len(), 1, "a 'want' decided on site A vanished once site B became active");
    assert_eq!(wants[0].id, sid_want);

    let discards = db.list_backlog(b, "discarded").unwrap();
    assert_eq!(discards.len(), 1);
    assert_eq!(discards[0].id, sid_disc);
}

/// Same for the "Ya vistas" sub-list.
#[test]
fn list_watched_externally_still_lists_a_decision_made_under_another_site() {
    let db = Db::open(":memory:").unwrap();
    let a = db.upsert_source("AnimeYT", "a", "animeytx").unwrap();
    let b = db.upsert_source("TioAnime", "b", "tioanime").unwrap();

    let watched = crate::models::Series {
        id: 0, slug: "anilist-9".into(), title: "Seen On A".into(),
        url: "https://anilist.co/anime/9".into(), cover_url: None, is_airing: false,
        followed: false, next_episode_at: None, site_episode_count: None,
    };
    let sid = db.upsert_series(a, &watched).unwrap();
    db.set_watched_externally(sid, true).unwrap();

    let rows = db.list_watched_externally(b).unwrap();
    assert_eq!(rows.len(), 1, "a 'ya lo vi' decided on site A vanished once site B became active");
    assert_eq!(rows[0].id, sid);
}

/// One show present on two sites is one Listas entry, not two — and the
/// active site's row is the representative, so the row the UI acts on
/// ("Empezar a ver") belongs to the site currently being scraped.
#[test]
fn list_backlog_collapses_cross_site_duplicates_preferring_the_active_site() {
    let db = Db::open(":memory:").unwrap();
    let a = db.upsert_source("AnimeYT", "a", "animeytx").unwrap();
    let b = db.upsert_source("TioAnime", "b", "tioanime").unwrap();

    let make = |slug: &str, url: &str| crate::models::Series {
        id: 0, slug: slug.into(), title: "Overlord IV".into(), url: url.into(),
        cover_url: None, is_airing: false, followed: false,
        next_episode_at: None, site_episode_count: None,
    };
    let sid_a = db.upsert_series(a, &make("overlord-iv", "https://a.example/overlord-iv")).unwrap();
    db.set_anilist_id(sid_a, 4242).unwrap();
    db.set_backlog_status(sid_a, Some("want")).unwrap();
    let sid_b = db.upsert_series(b, &make("overlord-4", "https://b.example/overlord-4")).unwrap();
    db.set_anilist_id(sid_b, 4242).unwrap();
    db.set_backlog_status(sid_b, Some("want")).unwrap();

    let wants = db.list_backlog(b, "want").unwrap();
    assert_eq!(wants.len(), 1, "the same show on two sites must be one Listas entry");
    assert_eq!(wants[0].id, sid_b, "the active site's row must be the representative");
}

#[test]
fn series_exists_is_true_until_the_row_is_deleted() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
    let sid = db
        .upsert_series(
            src,
            &crate::models::Series {
                id: 0, slug: "s".into(), title: "S".into(), url: "u".into(),
                cover_url: None, is_airing: false, followed: false,
                next_episode_at: None, site_episode_count: None,
            },
        )
        .unwrap();
    assert!(db.series_exists(sid).unwrap());
    db.delete_series(sid).unwrap();
    assert!(!db.series_exists(sid).unwrap());
    assert!(!db.series_exists(sid + 9_999).unwrap());
}

#[test]
fn series_still_is_detects_rowid_reuse_after_delete_and_reinsert() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();

    // 1. AniList-linked stub: id reused, anilist_id differs.
    let sid_a = db
        .upsert_series(
            src,
            &crate::models::Series {
                id: 0, slug: "anilist-1".into(), title: "A".into(), url: "https://anilist.co/anime/1".into(),
                cover_url: None, is_airing: false, followed: false,
                next_episode_at: None, site_episode_count: None,
            },
        )
        .unwrap();
    db.set_anilist_id(sid_a, 1).unwrap();
    assert!(db.series_still_is(sid_a, Some(1), "anilist-1").unwrap());

    // Row deleted (e.g. undo swipe).
    db.delete_series(sid_a).unwrap();
    assert!(!db.series_still_is(sid_a, Some(1), "anilist-1").unwrap());

    // New row inserted (e.g. next swipe): reuses sid_a rowid.
    let sid_b = db
        .upsert_series(
            src,
            &crate::models::Series {
                id: 0, slug: "anilist-2".into(), title: "B".into(), url: "https://anilist.co/anime/2".into(),
                cover_url: None, is_airing: false, followed: false,
                next_episode_at: None, site_episode_count: None,
            },
        )
        .unwrap();
    db.set_anilist_id(sid_b, 2).unwrap();
    assert_eq!(sid_b, sid_a, "SQLite must reuse the rowid of the recently deleted row");
    // Stale scrape for A must NOT see row B as the same series!
    assert!(!db.series_still_is(sid_b, Some(1), "anilist-1").unwrap());
    assert!(db.series_still_is(sid_b, Some(2), "anilist-2").unwrap());

    // 2. NULL-anilist rows: id reused, slug compared.
    db.delete_series(sid_b).unwrap();
    let sid_c = db
        .upsert_series(
            src,
            &crate::models::Series {
                id: 0, slug: "show-c".into(), title: "C".into(), url: "https://site/show-c".into(),
                cover_url: None, is_airing: false, followed: false,
                next_episode_at: None, site_episode_count: None,
            },
        )
        .unwrap();
    assert_eq!(sid_c, sid_a, "SQLite must reuse the rowid");
    assert!(db.series_still_is(sid_c, None, "show-c").unwrap());
    assert!(!db.series_still_is(sid_c, None, "show-d").unwrap());
    assert!(!db.series_still_is(sid_c, Some(2), "show-c").unwrap());
    // Linked to AniList while the scrape ran: still the same row.
    db.set_anilist_id(sid_c, 7).unwrap();
    assert!(db.series_still_is(sid_c, None, "show-c").unwrap());
    assert!(!db.series_still_is(sid_c, None, "show-d").unwrap());

    db.delete_series(sid_c).unwrap();
    let sid_d = db
        .upsert_series(
            src,
            &crate::models::Series {
                id: 0, slug: "show-d".into(), title: "D".into(), url: "https://site/show-d".into(),
                cover_url: None, is_airing: false, followed: false,
                next_episode_at: None, site_episode_count: None,
            },
        )
        .unwrap();
    assert_eq!(sid_d, sid_a);
    assert!(!db.series_still_is(sid_a, None, "show-c").unwrap());
    assert!(db.series_still_is(sid_a, None, "show-d").unwrap());
}

#[test]
fn list_library_includes_watched_externally_rows_with_no_episodes() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();

    // Followed series with episodes — the existing, already-working path.
    let followed = crate::models::Series {
        id: 0, slug: "followed".into(), title: "Followed".into(),
        url: "u1".into(), cover_url: None, is_airing: true, followed: false, next_episode_at: None, site_episode_count: None,
    };
    let sid_followed = db.upsert_series(src, &followed).unwrap();
    db.set_followed(sid_followed, true).unwrap();
    db.insert_episode(&crate::models::Episode {
        id: 0, series_id: sid_followed, number: "1".into(), title: None,
        url: "ep1".into(), released_at: None, seen: true, download_path: None, download_status: None,
    }).unwrap();

    // "Ya lo vi" catalog row: watched_externally=1, followed=0, no episodes.
    let watched = crate::models::Series {
        id: 0, slug: "watched".into(), title: "Watched".into(),
        url: "u2".into(), cover_url: None, is_airing: false, followed: false, next_episode_at: None, site_episode_count: None,
    };
    let sid_watched = db.upsert_series(src, &watched).unwrap();
    db.set_watched_externally(sid_watched, true).unwrap();

    let items = db.list_library(src).unwrap();
    assert_eq!(items.len(), 2, "both the followed and the watched-externally-only rows must be returned");

    let watched_item = items.iter().find(|it| it.series.id == sid_watched)
        .expect("watched_externally=1, followed=0 row must be present in list_library");
    assert!(watched_item.watched_externally);
    assert_eq!(watched_item.total_episodes, 0);

    let followed_item = items.iter().find(|it| it.series.id == sid_followed).unwrap();
    assert!(!followed_item.watched_externally);
    assert_eq!(followed_item.total_episodes, 1);
}

#[test]
fn list_library_returns_kind_and_genres_via_one_bulk_query() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();

    let s1 = crate::models::Series {
        id: 0, slug: "s1".into(), title: "S1".into(),
        url: "u1".into(), cover_url: None, is_airing: false, followed: false, next_episode_at: None, site_episode_count: None,
    };
    let sid1 = db.upsert_series(src, &s1).unwrap();
    db.set_followed(sid1, true).unwrap();
    db.set_kind(sid1, "TV").unwrap();
    db.insert_series_genres(sid1, &["Accion".to_string(), "Comedia".to_string()]).unwrap();

    // A second series with no kind/genres set at all — must come back
    // with kind=None and an empty genres vec, not an error or a missing row.
    let s2 = crate::models::Series {
        id: 0, slug: "s2".into(), title: "S2".into(),
        url: "u2".into(), cover_url: None, is_airing: false, followed: false, next_episode_at: None, site_episode_count: None,
    };
    let sid2 = db.upsert_series(src, &s2).unwrap();
    db.set_followed(sid2, true).unwrap();

    let items = db.list_library(src).unwrap();
    let item1 = items.iter().find(|it| it.series.id == sid1).unwrap();
    assert_eq!(item1.kind.as_deref(), Some("TV"));
    // Site labels come back mapped onto AniList's genre names.
    assert_eq!(item1.genres, vec!["Action".to_string(), "Comedy".to_string()]);

    let item2 = items.iter().find(|it| it.series.id == sid2).unwrap();
    assert_eq!(item2.kind, None);
    assert!(item2.genres.is_empty());
}

/// The library genre filter offers real genres only: AniList genres for a
/// linked row (never its tags), and for an unlinked row the site labels
/// that map onto a real genre ("Acción" -> "Action"); unmapped site labels
/// ("Shounen") are dropped. Same mapping as the stats genre cards.
#[test]
fn list_library_genres_are_real_genres_not_tags_or_raw_site_labels() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();

    let mut anime = catalog_anime_with_popularity(700, "Linked", &["Action", "Drama"], Some(1000));
    anime.tags = vec![
        crate::anilist::CatalogTag { name: "Age Gap".into(), rank: 90 },
        crate::anilist::CatalogTag { name: "Achromatic".into(), rank: 60 },
    ];
    db.upsert_catalog_anime(&anime, 0).unwrap();
    let linked = crate::models::Series {
        id: 0, slug: "linked".into(), title: "Linked".into(),
        url: "u1".into(), cover_url: None, is_airing: false, followed: false, next_episode_at: None, site_episode_count: None,
    };
    let sid_linked = db.upsert_series(src, &linked).unwrap();
    db.set_followed(sid_linked, true).unwrap();
    db.set_anilist_id(sid_linked, 700).unwrap();

    let unlinked = crate::models::Series {
        id: 0, slug: "unlinked".into(), title: "Unlinked".into(),
        url: "u2".into(), cover_url: None, is_airing: false, followed: false, next_episode_at: None, site_episode_count: None,
    };
    let sid_unlinked = db.upsert_series(src, &unlinked).unwrap();
    db.set_followed(sid_unlinked, true).unwrap();
    db.insert_series_genres(
        sid_unlinked,
        &["Acción".to_string(), "Action".to_string(), "Shounen".to_string(), "Fantasía".to_string()],
    )
    .unwrap();

    let items = db.list_library(src).unwrap();
    let linked_item = items.iter().find(|it| it.series.id == sid_linked).unwrap();
    assert_eq!(linked_item.genres, vec!["Action".to_string(), "Drama".to_string()]);
    let unlinked_item = items.iter().find(|it| it.series.id == sid_unlinked).unwrap();
    assert_eq!(unlinked_item.genres, vec!["Action".to_string(), "Fantasy".to_string()]);
}

#[test]
fn list_library_returns_studio_only_for_linked_series() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();

    // A catalog row carrying a studio, linked to a followed series via
    // anilist_id.
    let mut linked_anime = catalog_anime_with_popularity(500, "Linked", &["Drama"], Some(1000));
    linked_anime.studio = Some("Studio Ghibli".into());
    db.upsert_catalog_anime(&linked_anime, 0).unwrap();
    let linked = crate::models::Series {
        id: 0, slug: "linked".into(), title: "Linked".into(),
        url: "u1".into(), cover_url: None, is_airing: false, followed: false, next_episode_at: None, site_episode_count: None,
    };
    let sid_linked = db.upsert_series(src, &linked).unwrap();
    db.set_followed(sid_linked, true).unwrap();
    db.set_anilist_id(sid_linked, 500).unwrap();

    // An unlinked followed series — must come back with studio: None,
    // not an error or a stray value from another row.
    let unlinked = crate::models::Series {
        id: 0, slug: "unlinked".into(), title: "Unlinked".into(),
        url: "u2".into(), cover_url: None, is_airing: false, followed: false, next_episode_at: None, site_episode_count: None,
    };
    let sid_unlinked = db.upsert_series(src, &unlinked).unwrap();
    db.set_followed(sid_unlinked, true).unwrap();

    let items = db.list_library(src).unwrap();
    let linked_item = items.iter().find(|it| it.series.id == sid_linked).unwrap();
    assert_eq!(linked_item.studio.as_deref(), Some("Studio Ghibli"));

    let unlinked_item = items.iter().find(|it| it.series.id == sid_unlinked).unwrap();
    assert_eq!(unlinked_item.studio, None);
}

#[test]
fn list_library_returns_catalog_episodes_and_status_for_linked_series() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();

    let mut linked_anime = catalog_anime_with_popularity(600, "Linked Show", &["Action"], Some(100));
    linked_anime.episodes = Some(24);
    linked_anime.status = Some("FINISHED".into());
    db.upsert_catalog_anime(&linked_anime, 0).unwrap();

    let linked = crate::models::Series {
        id: 0, slug: "linked-show".into(), title: "Linked Show".into(),
        url: "u1".into(), cover_url: None, is_airing: false, followed: false, next_episode_at: None, site_episode_count: None,
    };
    let sid_linked = db.upsert_series(src, &linked).unwrap();
    db.set_followed(sid_linked, true).unwrap();
    db.set_anilist_id(sid_linked, 600).unwrap();

    let unlinked = crate::models::Series {
        id: 0, slug: "unlinked-show".into(), title: "Unlinked Show".into(),
        url: "u2".into(), cover_url: None, is_airing: false, followed: false, next_episode_at: None, site_episode_count: None,
    };
    let sid_unlinked = db.upsert_series(src, &unlinked).unwrap();
    db.set_followed(sid_unlinked, true).unwrap();

    let items = db.list_library(src).unwrap();
    let linked_item = items.iter().find(|it| it.series.id == sid_linked).unwrap();
    assert_eq!(linked_item.catalog_episodes, Some(24));
    assert_eq!(linked_item.catalog_status.as_deref(), Some("FINISHED"));

    let unlinked_item = items.iter().find(|it| it.series.id == sid_unlinked).unwrap();
    assert_eq!(unlinked_item.catalog_episodes, None);
    assert_eq!(unlinked_item.catalog_status, None);
}

#[test]
fn followed_titles_with_watermark_returns_anilist_id_and_anilist_ids_for_series() {
    let db = Db::open(":memory:").unwrap();
    let src_a = db.upsert_source("A", "a", "animeytx").unwrap();
    let src_b = db.upsert_source("B", "b", "tioanime").unwrap();

    let s1 = db.upsert_series(src_a, &mk_airing("s1", "Show One", None)).unwrap();
    db.set_followed(s1, true).unwrap();
    db.set_anilist_id(s1, 100).unwrap();
    insert_eps_seen_up_to(&db, s1, 3, 2);

    let s2 = db.upsert_series(src_a, &mk_airing("s2", "Show Two", None)).unwrap();
    db.set_followed(s2, true).unwrap();

    let mut got = db.followed_titles_with_watermark(src_b).unwrap();
    got.sort();
    assert_eq!(
        got,
        vec![
            ("Show One".to_string(), 2, Some(100)),
            ("Show Two".to_string(), 0, None),
        ]
    );

    let ids = db.anilist_ids_for_series(&[s1, s2]).unwrap();
    assert_eq!(ids, vec![Some(100), None]);
}

#[test]
fn delete_series_cascades_episodes_and_genres() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
    let s = crate::models::Series {
        id: 0, slug: "x".into(), title: "X".into(),
        url: "u".into(), cover_url: None, is_airing: false, followed: true, next_episode_at: None, site_episode_count: None,
    };
    let sid = db.upsert_series(src, &s).unwrap();
    db.insert_series_genres(sid, &["Seinen".to_string()]).unwrap();
    db.insert_episode(&crate::models::Episode {
        id: 0, series_id: sid, number: "1".into(), title: None,
        url: "e1".into(), released_at: None, seen: false, download_path: None, download_status: None,
    }).unwrap();

    db.delete_series(sid).unwrap();

    assert!(db.list_followed(src).unwrap().iter().all(|f| f.id != sid));
    assert!(db.list_series_genres(sid).unwrap().is_empty());
    assert!(db.list_series_episodes(sid).unwrap().is_empty());
}

#[test]
fn get_series_for_link_reads_link_relevant_fields() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
    let s = crate::models::Series {
        id: 0, slug: "anilist-42".into(), title: "X".into(),
        url: "https://anilist.co/anime/42".into(), cover_url: None, is_airing: false, followed: false, next_episode_at: None, site_episode_count: None,
    };
    let sid = db.upsert_series(src, &s).unwrap();
    db.set_anilist_id(sid, 42).unwrap();
    db.set_backlog_status(sid, Some("want")).unwrap();

    let info = db.get_series_for_link(sid).unwrap().unwrap();
    assert_eq!(info.id, sid);
    assert_eq!(info.source_id, src);
    assert_eq!(info.slug, "anilist-42");
    assert_eq!(info.anilist_id, Some(42));
    assert_eq!(info.backlog_status.as_deref(), Some("want"));
    assert!(!info.followed);
    assert!(!info.watched_externally);
}

#[test]
fn get_series_for_link_none_for_missing_row() {
    let db = Db::open(":memory:").unwrap();
    assert!(db.get_series_for_link(999).unwrap().is_none());
}

#[test]
fn already_linked_to_site_covers_both_null_anilist_id_and_relinked_slug() {
    let plain_site_row = SeriesForLink {
        id: 1, source_id: 1, slug: "baki-dou".into(), anilist_id: None,
        followed: true, backlog_status: None, watched_externally: false,
    };
    assert!(plain_site_row.already_linked_to_site());

    let unlinked_catalog_row = SeriesForLink {
        id: 2, source_id: 1, slug: "anilist-42".into(), anilist_id: Some(42),
        followed: false, backlog_status: Some("want".into()), watched_externally: false,
    };
    assert!(!unlinked_catalog_row.already_linked_to_site());

    // The critical case: a catalog row that was already linked in a
    // previous call keeps its `anilist_id` (relink_series never clears
    // it) but its slug is now the real site slug — must still count as
    // already-linked, or a second call (e.g. a race between the Seen
    // swipe's fire-and-forget link and opening SeriesDetail) would
    // re-search and re-scrape.
    let already_relinked_row = SeriesForLink {
        id: 3, source_id: 1, slug: "baki-dou".into(), anilist_id: Some(42),
        followed: false, backlog_status: None, watched_externally: true,
    };
    assert!(already_relinked_row.already_linked_to_site());
}

#[test]
fn find_series_id_by_slug_excludes_given_id_and_scopes_by_source() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
    let real = crate::models::Series {
        id: 0, slug: "baki-dou".into(), title: "Baki-dou".into(),
        url: "u1".into(), cover_url: None, is_airing: true, followed: false, next_episode_at: None, site_episode_count: None,
    };
    let real_id = db.upsert_series(src, &real).unwrap();
    let synthetic = crate::models::Series {
        id: 0, slug: "anilist-7".into(), title: "Baki-dou".into(),
        url: "https://anilist.co/anime/7".into(), cover_url: None, is_airing: false, followed: false, next_episode_at: None, site_episode_count: None,
    };
    let synthetic_id = db.upsert_series(src, &synthetic).unwrap();

    // Excluding the synthetic row's own id, "baki-dou" resolves to the real row.
    assert_eq!(
        db.find_series_id_by_slug(src, "baki-dou", synthetic_id).unwrap(),
        Some(real_id)
    );
    // No collision for a slug nothing owns.
    assert_eq!(db.find_series_id_by_slug(src, "no-such-slug", synthetic_id).unwrap(), None);
}

#[test]
fn merge_series_into_transfers_flags_and_deletes_synthetic() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
    let existing = crate::models::Series {
        id: 0, slug: "baki-dou".into(), title: "Baki-dou".into(),
        url: "https://site/tv/baki-dou/".into(), cover_url: None, is_airing: true, followed: false, next_episode_at: None, site_episode_count: None,
    };
    let existing_id = db.upsert_series(src, &existing).unwrap();

    let synthetic = crate::models::Series {
        id: 0, slug: "anilist-7".into(), title: "Baki-dou".into(),
        url: "https://anilist.co/anime/7".into(), cover_url: None, is_airing: false, followed: false, next_episode_at: None, site_episode_count: None,
    };
    let synthetic_id = db.upsert_series(src, &synthetic).unwrap();
    db.set_anilist_id(synthetic_id, 7).unwrap();
    db.set_followed(synthetic_id, true).unwrap();
    db.set_backlog_status(synthetic_id, Some("want")).unwrap();

    db.merge_series_into(existing_id, synthetic_id).unwrap();

    // Synthetic row is gone.
    assert!(db.get_series_for_link(synthetic_id).unwrap().is_none());
    // Existing row picked up the synthetic row's flags.
    let merged = db.get_series_for_link(existing_id).unwrap().unwrap();
    assert!(merged.followed);
    assert_eq!(merged.backlog_status.as_deref(), Some("want"));
    // Existing row's own slug/url survive untouched (it stays canonical).
    assert_eq!(merged.slug, "baki-dou");
}

#[test]
fn merge_series_into_keeps_existing_backlog_status_when_it_already_has_one() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
    let existing = crate::models::Series {
        id: 0, slug: "baki-dou".into(), title: "Baki-dou".into(),
        url: "https://site/tv/baki-dou/".into(), cover_url: None, is_airing: true, followed: false, next_episode_at: None, site_episode_count: None,
    };
    let existing_id = db.upsert_series(src, &existing).unwrap();
    db.set_backlog_status(existing_id, Some("discarded")).unwrap();

    let synthetic = crate::models::Series {
        id: 0, slug: "anilist-7".into(), title: "Baki-dou".into(),
        url: "https://anilist.co/anime/7".into(), cover_url: None, is_airing: false, followed: false, next_episode_at: None, site_episode_count: None,
    };
    let synthetic_id = db.upsert_series(src, &synthetic).unwrap();
    db.set_backlog_status(synthetic_id, Some("want")).unwrap();

    db.merge_series_into(existing_id, synthetic_id).unwrap();

    // Existing row's own (more specific/pre-existing) status wins.
    let merged = db.get_series_for_link(existing_id).unwrap().unwrap();
    assert_eq!(merged.backlog_status.as_deref(), Some("discarded"));
}

/// The bug: decide "Ya lo vi" on a catalog card -> the background site
/// link finds the matched slug is already a tracked series and merges the
/// synthetic row into it (deleting the synthetic row) -> undo pops the
/// synthetic id, finds no row, deletes nothing, and reports success. The
/// pre-existing series was left flagged `watched_externally` forever.
#[test]
fn undo_of_a_seen_decision_that_got_merged_reverts_the_survivors_flag() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
    let existing = crate::models::Series {
        id: 0, slug: "baki-dou".into(), title: "Baki-dou".into(),
        url: "https://site/tv/baki-dou/".into(), cover_url: None, is_airing: true,
        followed: true, next_episode_at: None, site_episode_count: None,
    };
    let existing_id = db.upsert_series(src, &existing).unwrap();
    // A real series the user already follows and has NOT marked watched.
    db.set_followed(existing_id, true).unwrap();
    let before = db.get_series_for_link(existing_id).unwrap().unwrap();
    assert!(!before.watched_externally);

    // What decide_catalog_card's `Seen` branch writes.
    let synthetic = crate::models::Series {
        id: 0, slug: "anilist-7".into(), title: "Baki-dou".into(),
        url: "https://anilist.co/anime/7".into(), cover_url: None, is_airing: false,
        followed: false, next_episode_at: None, site_episode_count: None,
    };
    let synthetic_id = db.upsert_series(src, &synthetic).unwrap();
    db.set_anilist_id(synthetic_id, 7).unwrap();
    db.set_watched_externally(synthetic_id, true).unwrap();

    // ...then the background link's slug-collision branch.
    db.merge_series_into(existing_id, synthetic_id).unwrap();
    assert!(db.get_series_for_link(existing_id).unwrap().unwrap().watched_externally);

    // Undo, by the id that's in swipe_history — the now-deleted one.
    assert_eq!(db.undo_swipe_decision(synthetic_id).unwrap().as_deref(), Some("Baki-dou"));

    let after = db.get_series_for_link(existing_id).unwrap().unwrap();
    assert!(!after.watched_externally, "the merge's watched_externally must be reversed");
    assert!(after.followed, "the survivor's own pre-existing follow must survive the undo");
    // The trail is one-shot: a second undo of the same id changes nothing.
    assert_eq!(db.undo_swipe_decision(synthetic_id).unwrap(), None);
}

#[test]
fn undo_of_a_merged_decision_never_clears_a_flag_the_survivor_already_had() {
    // The survivor was ALREADY marked watched-externally by the user
    // before any swipe happened. The merge's `OR` therefore changed
    // nothing, so undoing the swipe must not clear it either.
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
    let existing = crate::models::Series {
        id: 0, slug: "baki-dou".into(), title: "Baki-dou".into(),
        url: "https://site/tv/baki-dou/".into(), cover_url: None, is_airing: true,
        followed: false, next_episode_at: None, site_episode_count: None,
    };
    let existing_id = db.upsert_series(src, &existing).unwrap();
    db.set_watched_externally(existing_id, true).unwrap();
    db.set_backlog_status(existing_id, Some("discarded")).unwrap();

    let synthetic = crate::models::Series {
        id: 0, slug: "anilist-7".into(), title: "Baki-dou".into(),
        url: "https://anilist.co/anime/7".into(), cover_url: None, is_airing: false,
        followed: false, next_episode_at: None, site_episode_count: None,
    };
    let synthetic_id = db.upsert_series(src, &synthetic).unwrap();
    db.set_watched_externally(synthetic_id, true).unwrap();
    db.set_backlog_status(synthetic_id, Some("want")).unwrap();

    db.merge_series_into(existing_id, synthetic_id).unwrap();
    db.undo_swipe_decision(synthetic_id).unwrap();

    let after = db.get_series_for_link(existing_id).unwrap().unwrap();
    assert!(after.watched_externally, "the user's own earlier decision is not the merge's to undo");
    assert_eq!(after.backlog_status.as_deref(), Some("discarded"));
}

#[test]
fn undo_swipe_decision_deletes_a_still_live_row_and_reports_nothing_for_an_unknown_id() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
    let s = crate::models::Series {
        id: 0, slug: "anilist-9".into(), title: "Solo Card".into(),
        url: "https://anilist.co/anime/9".into(), cover_url: None, is_airing: false,
        followed: false, next_episode_at: None, site_episode_count: None,
    };
    let sid = db.upsert_series(src, &s).unwrap();
    db.set_watched_externally(sid, true).unwrap();

    assert_eq!(db.undo_swipe_decision(sid).unwrap().as_deref(), Some("Solo Card"));
    assert!(db.get_series_for_link(sid).unwrap().is_none());
    // Nothing left to undo: a no-op, not an error, and nothing to report.
    assert_eq!(db.undo_swipe_decision(sid).unwrap(), None);
    assert_eq!(db.undo_swipe_decision(99_999).unwrap(), None);
}

#[test]
fn relink_series_updates_slug_url_cover_and_kind_in_place() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
    let synthetic = crate::models::Series {
        id: 0, slug: "anilist-7".into(), title: "Baki-dou".into(),
        url: "https://anilist.co/anime/7".into(), cover_url: Some("https://anilist-cdn/7.jpg".into()),
        is_airing: false, followed: true, next_episode_at: None, site_episode_count: None,
    };
    let sid = db.upsert_series(src, &synthetic).unwrap();
    db.set_anilist_id(sid, 7).unwrap();
    // upsert_series deliberately never writes `followed` (see its own
    // doc comment) — set it the same way the rest of the codebase does.
    db.set_followed(sid, true).unwrap();

    db.relink_series(sid, "baki-dou", "https://site/tv/baki-dou/", Some("https://site/cover.jpg"), "TV").unwrap();

    let url = db.get_series_url(sid).unwrap().unwrap();
    assert_eq!(url, "https://site/tv/baki-dou/");
    // followed/anilist_id survive — relink_series only touches slug/url/cover/kind.
    let info = db.get_series_for_link(sid).unwrap().unwrap();
    assert_eq!(info.slug, "baki-dou");
    assert!(info.followed);
    assert_eq!(info.anilist_id, Some(7));
}

#[test]
fn replace_series_genres_drops_old_genres_instead_of_accumulating() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
    let s = crate::models::Series {
        id: 0, slug: "x".into(), title: "X".into(),
        url: "u".into(), cover_url: None, is_airing: false, followed: false, next_episode_at: None, site_episode_count: None,
    };
    let sid = db.upsert_series(src, &s).unwrap();
    db.insert_series_genres(sid, &["Action".to_string(), "Isekai".to_string()]).unwrap();

    db.replace_series_genres(sid, &["Drama".to_string()]).unwrap();

    assert_eq!(db.list_series_genres(sid).unwrap(), vec!["Drama".to_string()]);
}

#[test]
fn a_linked_series_does_not_need_site_genre_backfill() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
    db.upsert_catalog_anime(&crate::db::test_support::catalog_anime(100, "X", &["Action"]), 0).unwrap();
    let s = crate::models::Series {
        id: 0, slug: "x".into(), title: "X".into(),
        url: "u".into(), cover_url: None, is_airing: false, followed: true, next_episode_at: None, site_episode_count: None,
    };
    let sid = db.upsert_series(src, &s).unwrap();
    db.set_anilist_id(sid, 100).unwrap();
    // Linked series: AniList is the source of truth, so backfill is false even with zero site rows.
    assert!(!db.series_needs_genre_backfill(sid).unwrap(), "linked series must not need site genre backfill");
    db.insert_series_genres(sid, &["Seinen".into()]).unwrap();
    assert!(!db.series_needs_genre_backfill(sid).unwrap());
}

#[test]
fn series_needs_genre_backfill_reflects_series_genres_rows() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
    let s = crate::models::Series {
        id: 0, slug: "x".into(), title: "X".into(),
        url: "u".into(), cover_url: None, is_airing: false, followed: true, next_episode_at: None, site_episode_count: None,
    };
    let sid = db.upsert_series(src, &s).unwrap();

    // Unlinked series: site rows decide (true with zero rows, false once populated).
    assert!(db.series_needs_genre_backfill(sid).unwrap());
    db.insert_series_genres(sid, &["Seinen".to_string()]).unwrap();
    assert!(!db.series_needs_genre_backfill(sid).unwrap());
}

#[test]
fn engaged_series_titles_covers_followed_want_discarded_and_watched_externally() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();

    let make = |slug: &str, title: &str| crate::models::Series {
        id: 0,
        slug: slug.into(),
        title: title.into(),
        url: format!("https://example.com/tv/{slug}"),
        cover_url: None,
        is_airing: false,
        followed: false,
        next_episode_at: None,
        site_episode_count: None,
    };

    let sid_followed = db.upsert_series(src, &make("followed-show", "Followed Show")).unwrap();
    db.set_followed(sid_followed, true).unwrap();

    let sid_want = db.upsert_series(src, &make("want-show", "Want Show")).unwrap();
    db.set_backlog_status(sid_want, Some("want")).unwrap();

    let sid_discarded = db.upsert_series(src, &make("discarded-show", "Discarded Show")).unwrap();
    db.set_backlog_status(sid_discarded, Some("discarded")).unwrap();

    let sid_watched = db.upsert_series(src, &make("watched-show", "Watched Show")).unwrap();
    db.set_watched_externally(sid_watched, true).unwrap();

    // Untouched row must NOT be engaged.
    db.upsert_series(src, &make("untouched-show", "Untouched Show")).unwrap();

    let titles: std::collections::HashSet<String> = db.engaged_series_titles().unwrap().into_iter().collect();
    assert!(titles.contains("Followed Show"));
    assert!(titles.contains("Want Show"));
    assert!(titles.contains("Discarded Show"));
    assert!(titles.contains("Watched Show"));
    assert!(!titles.contains("Untouched Show"));
}

#[test]
fn engaged_series_titles_covers_engagement_on_any_site_not_just_one() {
    let db = Db::open(":memory:").unwrap();
    let a = db.upsert_source("AnimeYT", "a", "animeytx").unwrap();
    let b = db.upsert_source("TioAnime", "b", "tioanime").unwrap();

    let make = |slug: &str, title: &str| crate::models::Series {
        id: 0, slug: slug.into(), title: title.into(),
        url: format!("https://example.com/tv/{slug}"),
        cover_url: None, is_airing: false, followed: false,
        next_episode_at: None, site_episode_count: None,
    };

    // Followed on site B only — must still show up globally, so the
    // Descubrir deck (which reads this without a site filter) never
    // re-offers it while site A happens to be active.
    let sid = db.upsert_series(b, &make("overlord-iv", "Overlord IV")).unwrap();
    db.set_followed(sid, true).unwrap();
    // Untouched on site A.
    db.upsert_series(a, &make("other", "Other Show")).unwrap();

    let titles: std::collections::HashSet<String> = db.engaged_series_titles().unwrap().into_iter().collect();
    assert!(titles.contains("Overlord IV"), "engagement on a non-active site must still be visible");
    assert!(!titles.contains("Other Show"));
}

#[test]
fn followed_series_without_episodes_finds_only_never_fetched_followed_rows() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("AnimeYT", "a", "animeytx").unwrap();
    let make = |slug: &str, title: &str| crate::models::Series {
        id: 0, slug: slug.into(), title: title.into(),
        url: format!("https://example.com/tv/{slug}"),
        cover_url: None, is_airing: true, followed: false,
        next_episode_at: None, site_episode_count: None,
    };

    // Followed, never fetched — the carry-over-onto-a-new-site shape.
    let empty = db.upsert_series(src, &make("empty", "Empty Show")).unwrap();
    db.set_followed(empty, true).unwrap();

    // Followed, already has episodes — must NOT show up again.
    let has_eps = db.upsert_series(src, &make("has-eps", "Has Episodes")).unwrap();
    db.set_followed(has_eps, true).unwrap();
    db.insert_episode(&crate::models::Episode {
        id: 0, series_id: has_eps, number: "1".into(), title: None,
        url: "https://example.com/has-eps-1".into(), released_at: None, seen: false, download_path: None, download_status: None,
    }).unwrap();

    // Never fetched but NOT followed — must not show up either.
    db.upsert_series(src, &make("not-followed", "Not Followed")).unwrap();

    let needing = db.followed_series_without_episodes(src).unwrap();
    let titles: Vec<&str> = needing.iter().map(|s| s.title.as_str()).collect();
    assert_eq!(titles, vec!["Empty Show"]);
}

#[test]
fn get_series_for_history_returns_none_for_a_deleted_row() {
    let db = Db::open(":memory:").unwrap();
    assert!(db.get_series_for_history(999).unwrap().is_none());
}

#[test]
fn get_series_for_history_includes_the_rows_url() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
    let s = crate::models::Series {
        id: 0, slug: "anilist-7".into(), title: "T".into(),
        url: "https://anilist.co/anime/7/t".into(), cover_url: None,
        is_airing: false, followed: false, next_episode_at: None, site_episode_count: None,
    };
    let sid = db.upsert_series(src, &s).unwrap();
    let row = db.get_series_for_history(sid).unwrap().unwrap();
    assert_eq!(row.url, "https://anilist.co/anime/7/t");
}

#[test]
fn anilist_id_and_watched_externally_round_trip() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
    let s = crate::models::Series {
        id: 0, slug: "anilist-42".into(), title: "X".into(),
        url: "u".into(), cover_url: None, is_airing: false, followed: false, next_episode_at: None, site_episode_count: None,
    };
    let sid = db.upsert_series(src, &s).unwrap();

    db.set_anilist_id(sid, 42).unwrap();
    db.set_watched_externally(sid, true).unwrap();

    let (anilist_id, watched): (Option<i64>, i64) = db.conn.query_row(
        "SELECT anilist_id, watched_externally FROM series WHERE id=?1",
        [sid],
        |r| Ok((r.get(0)?, r.get(1)?)),
    ).unwrap();
    assert_eq!(anilist_id, Some(42));
    assert_eq!(watched, 1);
}

#[test]
fn upsert_series_never_clobbers_a_cached_file_cover_with_a_remote_url() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
    let mk = |cover: &str| crate::models::Series {
        id: 0, slug: "s".into(), title: "S".into(), url: "https://x/s".into(),
        cover_url: Some(cover.into()), is_airing: true, followed: false,
        next_episode_at: None, site_episode_count: None,
    };
    let id = db.upsert_series(src, &mk("https://cdn/x.jpg")).unwrap();
    db.update_series_cover(id, "file:C:/covers/abc.jpg").unwrap();
    // The next scan re-upserts the card with the remote thumbnail.
    db.upsert_series(src, &mk("https://cdn/x.jpg")).unwrap();
    let cover: Option<String> = db.conn
        .query_row("SELECT cover_url FROM series WHERE id=?1", [id], |r| r.get(0)).unwrap();
    assert_eq!(cover.as_deref(), Some("file:C:/covers/abc.jpg"));
}

#[test]
fn heal_missing_cached_covers_clears_only_dead_file_paths() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
    let mk = |slug: &str, cover: &str| crate::models::Series {
        id: 0, slug: slug.into(), title: slug.into(), url: format!("https://x/{slug}"),
        cover_url: Some(cover.into()), is_airing: true, followed: false,
        next_episode_at: None, site_episode_count: None,
    };
    let live_file = std::env::temp_dir().join("aot-heal-test-cover.jpg");
    std::fs::write(&live_file, b"x").unwrap();
    let live_url = format!("file:{}", live_file.display().to_string().replace('\\', "/"));
    let dead = db.upsert_series(src, &mk("dead", "https://cdn/d.jpg")).unwrap();
    db.update_series_cover(dead, "file:C:/definitely/not/here/abc.jpg").unwrap();
    let live = db.upsert_series(src, &mk("live", "https://cdn/l.jpg")).unwrap();
    db.update_series_cover(live, &live_url).unwrap();
    let data = db.upsert_series(src, &mk("data", "https://cdn/x.jpg")).unwrap();
    db.update_series_cover(data, "data:image/png;base64,AAAA").unwrap();

    // `unlisted` has a dead file: cover too, but is not in the listing
    // (finished / scan failed): it must be left alone.
    let unlisted = db.upsert_series(src, &mk("unlisted", "https://cdn/u.jpg")).unwrap();
    db.update_series_cover(unlisted, "file:C:/definitely/not/here/u.jpg").unwrap();

    let listed: std::collections::HashSet<&str> = ["dead", "live", "data"].into_iter().collect();
    assert_eq!(db.heal_missing_cached_covers(src, &listed).unwrap(), 1);
    let get = |id: i64| -> Option<String> {
        db.conn.query_row("SELECT cover_url FROM series WHERE id=?1", [id], |r| r.get(0)).unwrap()
    };
    assert_eq!(get(dead), None);
    assert_eq!(get(live).as_deref(), Some(live_url.as_str()));
    assert_eq!(get(data).as_deref(), Some("data:image/png;base64,AAAA"));
    assert_eq!(get(unlisted).as_deref(), Some("file:C:/definitely/not/here/u.jpg"));
    let _ = std::fs::remove_file(live_file);
}

#[test]
fn airing_series_needing_cover_fetch_filters_cached_and_data_covers() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
    let make = |slug: &str, cover: Option<&str>| crate::models::Series {
        id: 0,
        slug: slug.into(),
        title: slug.into(),
        url: format!("https://example.com/{slug}"),
        cover_url: cover.map(|c| c.to_string()),
        is_airing: true,
        followed: false,
        next_episode_at: None,
        site_episode_count: None,
    };

    // Needs fetch: remote un-cached cover
    db.upsert_series(src, &make("remote", Some("https://example.com/cover.jpg"))).unwrap();
    // Already cached as file:
    db.upsert_series(src, &make("cached-file", Some("file:covers/abc.jpg"))).unwrap();
    // Already cached as data:
    db.upsert_series(src, &make("cached-data", Some("data:image/jpeg;base64,123"))).unwrap();
    // Null cover
    db.upsert_series(src, &make("null-cover", None)).unwrap();

    let needing = db.airing_series_needing_cover_fetch(60, Some(src)).unwrap();
    assert_eq!(needing.len(), 2);
    let remote = needing.iter().find(|row| row.1 == "remote").unwrap();
    let null = needing.iter().find(|row| row.1 == "null-cover").unwrap();
    assert_eq!(remote.3, src);
    assert_eq!(remote.2.as_deref(), Some("https://example.com/cover.jpg"));
    assert_eq!(null.2, None);
    assert_eq!(null.4, "https://example.com/null-cover");
}

#[test]
fn airing_series_needing_cover_fetch_spans_all_sites() {
    // Two sources (different site_ids). Rows with http covers on both
    // must appear; each tuple must carry the right source_id.
    // A linked row (anilist_id set), a data: cover, a file: cover, and
    // a not-airing row must NOT be returned.
    let db = Db::open(":memory:").unwrap();
    let src_a = db.upsert_source("AnimeYT", "https://animeytx.net", "animeytx").unwrap();
    let src_b = db.upsert_source("TioAnime", "https://tioanime.com", "tioanime").unwrap();

    let make = |slug: &str, cover: &str, is_airing: bool| crate::models::Series {
        id: 0,
        slug: slug.into(),
        title: slug.into(),
        url: format!("https://example.com/{slug}"),
        cover_url: Some(cover.to_string()),
        is_airing,
        followed: false,
        next_episode_at: None,
        site_episode_count: None,
    };

    // Both of these should be returned — one per site.
    let id_a = db.upsert_series(src_a, &make("show-a", "https://cdn.a/a.jpg", true)).unwrap();
    let id_b = db.upsert_series(src_b, &make("show-b", "https://cdn.b/b.jpg", true)).unwrap();

    // Linked row — anilist_id IS NOT NULL, must be excluded.
    let id_linked = db.upsert_series(src_a, &make("show-linked", "https://cdn.a/l.jpg", true)).unwrap();
    db.set_anilist_id(id_linked, 999).unwrap();

    // Already-fetched covers — must be excluded.
    db.upsert_series(src_a, &make("show-data", "data:image/png;base64,AAAA", true)).unwrap();
    db.upsert_series(src_b, &make("show-file", "file:C:/covers/x.jpg", true)).unwrap();

    // Not airing — must be excluded.
    let mut not_air = make("show-not-airing", "https://cdn.a/na.jpg", false);
    not_air.slug = "show-not-airing".into();
    db.upsert_series(src_a, &not_air).unwrap();

    let needing = db.airing_series_needing_cover_fetch(60, None).unwrap();

    let ids: std::collections::HashSet<i64> = needing.iter().map(|(id, _, _, _, _)| *id).collect();
    assert!(ids.contains(&id_a), "show-a (site animeytx) must be selected");
    assert!(ids.contains(&id_b), "show-b (site tioanime) must be selected");
    assert!(!ids.contains(&id_linked), "linked row must not be selected");
    assert_eq!(needing.len(), 2, "only the two unlinked airing remote-cover rows");

    // Each tuple carries its own source_id.
    let src_for = |id: i64| needing.iter().find(|(i, _, _, _, _)| *i == id).unwrap().3;
    assert_eq!(src_for(id_a), src_a);
    assert_eq!(src_for(id_b), src_b);
}

#[test]
fn clear_placeholder_covers_clears_edge_and_leaves_real_and_null() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
    let mk = |slug: &str, cover: Option<&str>| crate::models::Series {
        id: 0, slug: slug.into(), title: slug.into(), url: format!("https://x/{slug}"),
        cover_url: cover.map(|c| c.to_string()), is_airing: true, followed: false,
        next_episode_at: None, site_episode_count: None,
    };

    // Edge placeholder
    let edge_bytes = include_bytes!("../../../tests/fixtures/edge-logo-48.jpg");
    use base64::Engine;
    let edge_b64 = base64::prelude::BASE64_STANDARD.encode(edge_bytes);
    let edge_uri = format!("data:image/jpeg;base64,{}", edge_b64);
    let id_edge = db.upsert_series(src, &mk("edge", Some(&edge_uri))).unwrap();

    // Real size cover (200x300 PNG)
    let mut real_bytes = vec![];
    real_bytes.extend_from_slice(b"\x89PNG\r\n\x1a\n");
    real_bytes.extend_from_slice(&[0; 8]);
    real_bytes.extend_from_slice(&200u32.to_be_bytes()); // 16..20
    real_bytes.extend_from_slice(&300u32.to_be_bytes()); // 20..24
    // Realistic size (0.3 bytes per pixel), so it is not mistaken for a flat placeholder
    real_bytes.extend(vec![0; 200 * 300 * 3 / 10]);
    let real_b64 = base64::prelude::BASE64_STANDARD.encode(&real_bytes);
    let real_uri = format!("data:image/png;base64,{}", real_b64);
    let id_real = db.upsert_series(src, &mk("real", Some(&real_uri))).unwrap();

    // Null cover
    let id_null = db.upsert_series(src, &mk("null", None)).unwrap();

    assert_eq!(db.clear_placeholder_covers().unwrap(), 1);

    let get = |id: i64| -> Option<String> {
        db.conn.query_row("SELECT cover_url FROM series WHERE id=?1", [id], |r| r.get(0)).unwrap()
    };

    assert_eq!(get(id_edge), None);
    assert_eq!(get(id_real).as_deref(), Some(real_uri.as_str()));
    assert_eq!(get(id_null), None);
}

fn series_in(db: &Db, src: i64, slug: &str, title: &str, cover: Option<&str>) -> i64 {
    db.upsert_series(src, &crate::models::Series {
        id: 0, slug: slug.into(), title: title.into(), url: format!("https://x/{slug}"),
        cover_url: cover.map(str::to_string), is_airing: true, followed: false,
        next_episode_at: None, site_episode_count: None,
    }).unwrap()
}

fn airing_flag(db: &Db, id: i64) -> bool {
    db.conn.query_row("SELECT is_airing FROM series WHERE id=?1", [id], |r| r.get::<_, i64>(0)).unwrap() != 0
}

#[test]
fn a_show_the_site_stopped_listing_stops_airing_unless_linked() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("A", "https://a", "animeytx").unwrap();
    let mut keep: Vec<String> = Vec::new();
    for i in 0..12 {
        let slug = format!("listed-{i}");
        series_in(&db, src, &slug, &format!("Listed {i}"), None);
        keep.push(slug);
    }
    let dropped = series_in(&db, src, "dropped", "Dropped Show", None);
    let linked = series_in(&db, src, "linked", "Linked Show", None);
    db.set_anilist_id(linked, 1).unwrap();

    let listed: std::collections::HashSet<&str> = keep.iter().map(String::as_str).collect();
    assert_eq!(db.mark_unlisted_not_airing(src, &listed).unwrap(), 1);
    assert!(!airing_flag(&db, dropped));
    assert!(airing_flag(&db, linked), "AniList decides for a linked show");
    assert!(airing_flag(&db, db.conn.query_row("SELECT id FROM series WHERE slug='listed-0'", [], |r| r.get(0)).unwrap()));
}

#[test]
fn a_tiny_listing_is_treated_as_broken_and_changes_nothing() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("A", "https://a", "animeytx").unwrap();
    let other = series_in(&db, src, "other", "Other", None);
    let listed: std::collections::HashSet<&str> = ["x"].into_iter().collect();
    assert_eq!(db.mark_unlisted_not_airing(src, &listed).unwrap(), 0);
    assert!(airing_flag(&db, other));
}

#[test]
fn a_cover_shared_by_three_unrelated_series_is_a_placeholder() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("A", "https://a", "gogoanime").unwrap();
    let no_image = "data:image/jpeg;base64,AAAA"; // content is irrelevant, only that it repeats
    let a = series_in(&db, src, "a", "A", None);
    let b = series_in(&db, src, "b", "B", None);
    let c = series_in(&db, src, "c", "C", None);
    let real = series_in(&db, src, "d", "D", None);
    for id in [a, b, c] {
        db.conn.execute("UPDATE series SET cover_url=?1 WHERE id=?2", (no_image, id)).unwrap();
    }
    db.conn.execute("UPDATE series SET cover_url=?1 WHERE id=?2", ("data:image/jpeg;base64,BBBB", real)).unwrap();

    assert_eq!(db.clear_placeholder_covers().unwrap(), 3);
    let cover = |id: i64| -> Option<String> {
        db.conn.query_row("SELECT cover_url FROM series WHERE id=?1", [id], |r| r.get(0)).unwrap()
    };
    assert_eq!(cover(a), None);
    assert!(cover(real).is_some());
}

#[test]
fn storing_a_cover_that_two_others_already_carry_leaves_it_empty() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("A", "https://a", "gogoanime").unwrap();
    let no_image = "data:image/jpeg;base64,CCCC";
    for slug in ["a", "b"] {
        let id = series_in(&db, src, slug, slug, None);
        db.conn.execute("UPDATE series SET cover_url=?1 WHERE id=?2", (no_image, id)).unwrap();
    }
    let third = series_in(&db, src, "c", "C", Some("https://remote/c.jpg"));
    db.update_series_cover(third, no_image).unwrap();
    let cover: Option<String> = db.conn.query_row("SELECT cover_url FROM series WHERE id=?1", [third], |r| r.get(0)).unwrap();
    assert_eq!(cover, None, "a placeholder is not stored, and nothing is left to refetch");

    let fresh = series_in(&db, src, "d", "D", None);
    db.update_series_cover(fresh, "data:image/jpeg;base64,DDDD").unwrap();
    let cover: Option<String> = db.conn.query_row("SELECT cover_url FROM series WHERE id=?1", [fresh], |r| r.get(0)).unwrap();
    assert_eq!(cover.as_deref(), Some("data:image/jpeg;base64,DDDD"));
}

#[test]
fn unlisting_spares_followed_rows_other_sources_and_guards_against_a_mass_drop() {
    let db = Db::open(":memory:").unwrap();
    let a = db.upsert_source("A", "https://a", "animeytx").unwrap();
    let b = db.upsert_source("B", "https://b", "tioanime").unwrap();
    let mut listed_slugs: Vec<String> = Vec::new();
    for i in 0..12 {
        let slug = format!("l{i}");
        series_in(&db, a, &slug, &format!("Listed {i}"), None);
        listed_slugs.push(slug);
    }
    let followed_gone = series_in(&db, a, "fg", "Followed Gone", None);
    db.set_followed(followed_gone, true).unwrap();
    let plain_gone = series_in(&db, a, "pg", "Plain Gone", None);
    let other_site = series_in(&db, b, "ob", "Other Site Show", None);
    let listed: std::collections::HashSet<&str> = listed_slugs.iter().map(String::as_str).collect();

    assert_eq!(db.mark_unlisted_not_airing(a, &listed).unwrap(), 1);
    assert!(airing_flag(&db, followed_gone), "a followed show is never unaired by an unlisting");
    assert!(!airing_flag(&db, plain_gone));
    assert!(airing_flag(&db, other_site), "another source is untouched");

    // a listing far smaller than what the source has airing is a broken parse
    let db2 = Db::open(":memory:").unwrap();
    let s2 = db2.upsert_source("A", "https://a", "animeytx").unwrap();
    for i in 0..40 {
        series_in(&db2, s2, &format!("x{i}"), &format!("Show {i}"), None);
    }
    let tiny: std::collections::HashSet<&str> = (0..12).map(|_| "x0").collect();
    let mut big: Vec<String> = (0..12).map(|i| format!("x{i}")).collect();
    big.truncate(12);
    let twelve: std::collections::HashSet<&str> = big.iter().map(String::as_str).collect();
    assert_eq!(db2.mark_unlisted_not_airing(s2, &tiny).unwrap(), 0);
    assert_eq!(db2.mark_unlisted_not_airing(s2, &twelve).unwrap(), 0, "12 of 40 is not a credible listing");
}

#[test]
fn arcs_of_one_franchise_sharing_a_cover_keep_it() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("A", "https://a", "gogoanime").unwrap();
    let shared = "data:image/jpeg;base64,EEEE";
    let ids: Vec<i64> = ["Big Show: Arc One", "Big Show: Arc Two", "Big Show: Arc Three"]
        .iter()
        .enumerate()
        .map(|(i, t)| series_in(&db, src, &format!("s{i}"), t, None))
        .collect();
    for id in &ids {
        db.conn.execute("UPDATE series SET cover_url=?1 WHERE id=?2", (shared, id)).unwrap();
    }
    assert_eq!(db.clear_placeholder_covers().unwrap(), 0, "one franchise is not three unrelated shows");
    let fourth = series_in(&db, src, "s9", "Big Show: Arc Four", None);
    db.update_series_cover(fourth, shared).unwrap();
    let cover: Option<String> = db.conn.query_row("SELECT cover_url FROM series WHERE id=?1", [fourth], |r| r.get(0)).unwrap();
    assert_eq!(cover.as_deref(), Some(shared));
}

#[test]
fn list_series_genres_batch_returns_mapped_genres_and_empty_for_missing() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("A", "https://a", "animeytx").unwrap();
    let s1 = series_in(&db, src, "s1", "Show 1", None);
    let s2 = series_in(&db, src, "s2", "Show 2", None);
    let s3 = series_in(&db, src, "s3", "Show 3", None);

    db.conn.execute("INSERT INTO series_genres (series_id, genre) VALUES (?1, ?2)", (s1, "Action")).unwrap();
    db.conn.execute("INSERT INTO series_genres (series_id, genre) VALUES (?1, ?2)", (s1, "Adventure")).unwrap();
    db.conn.execute("INSERT INTO series_genres (series_id, genre) VALUES (?1, ?2)", (s2, "Comedy")).unwrap();

    let batch = db.list_series_genres_batch(&[s1, s2, s3]).unwrap();
    assert_eq!(batch.get(&s1).map(|v| v.as_slice()), Some(&["Action".to_string(), "Adventure".to_string()][..]));
    assert_eq!(batch.get(&s2).map(|v| v.as_slice()), Some(&["Comedy".to_string()][..]));
    assert_eq!(batch.get(&s3).map(|v| v.as_slice()), Some(&[][..]));

    let empty = db.list_series_genres_batch(&[]).unwrap();
    assert!(empty.is_empty());
}

#[test]
fn list_entry_meta_must_pass_contract() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("A", "https://a", "animeytx").unwrap();

    // 1. Empty slice returns empty vec without querying
    assert_eq!(db.list_entry_meta(&[]).unwrap(), Vec::new());

    // 2. Unlinked row returns None fields and series_genres genres
    let unlinked = series_in(&db, src, "unlinked", "Unlinked Show", None);
    db.insert_series_genres(unlinked, &["Action".into(), "Sci-Fi".into()]).unwrap();

    // 3. Linked want-row with catalog entry
    db.upsert_catalog_anime(
        &crate::anilist::CatalogAnime {
            id: 555,
            title: "Linked Show".into(),
            title_romaji: Some("Rinkudo Sho".into()),
            title_english: Some("Linked Show".into()),
            synonyms: Vec::new(),
            next_airing_at: None,
            next_episode: None,
            tags: Vec::new(),
            relations: Vec::new(),
            cover_url: Some("https://catalog/cover.jpg".into()),
            banner_url: None,
            format: Some("TV".into()),
            genres: vec!["Adventure".into(), "Comedy".into()],
            episodes: Some(24),
            average_score: Some(88),
            popularity: Some(12345),
            url: "https://anilist.co/anime/555".into(),
            status: Some("RELEASING".into()),
            duration: Some(24),
            studio: Some("Bones".into()),
            start_date: Some(1_700_000_000),
        },
        0,
    ).unwrap();

    let linked = series_in(&db, src, "linked", "Linked Show", None);
    db.set_anilist_id(linked, 555).unwrap();
    db.set_backlog_status(linked, Some("want")).unwrap();

    // Query with an unknown id (999999) interspersed
    let meta = db.list_entry_meta(&[unlinked, 999999, linked]).unwrap();
    assert_eq!(meta.len(), 2, "unknown id 999999 must be skipped");

    let m_unlinked = &meta[0];
    assert_eq!(m_unlinked.series_id, unlinked);
    assert_eq!(m_unlinked.anilist_id, None);
    assert_eq!(m_unlinked.score, None);
    assert_eq!(m_unlinked.format, None);
    assert_eq!(m_unlinked.status, None);
    assert_eq!(m_unlinked.episodes, None);
    assert_eq!(m_unlinked.start_date, None);
    assert_eq!(m_unlinked.studio, None);
    assert_eq!(m_unlinked.popularity, None);
    assert_eq!(m_unlinked.cover_url, None);
    assert_eq!(m_unlinked.genres, vec!["Action".to_string(), "Sci-Fi".to_string()]);

    let m_linked = &meta[1];
    assert_eq!(m_linked.series_id, linked);
    assert_eq!(m_linked.anilist_id, Some(555));
    assert_eq!(m_linked.score, Some(88));
    assert_eq!(m_linked.format, Some("TV".into()));
    assert_eq!(m_linked.status, Some("RELEASING".into()));
    assert_eq!(m_linked.episodes, Some(24));
    assert_eq!(m_linked.start_date, Some(1_700_000_000));
    assert_eq!(m_linked.studio, Some("Bones".into()));
    assert_eq!(m_linked.popularity, Some(12345));
    assert_eq!(m_linked.cover_url, Some("https://catalog/cover.jpg".into()));
    assert_eq!(m_linked.genres, vec!["Adventure".to_string(), "Comedy".to_string()]);

    // 4. More than 500 ids work (chunking)
    let mut bulk_ids = Vec::with_capacity(550);
    for i in 0..550 {
        let id = series_in(&db, src, &format!("bulk-{i}"), &format!("Bulk {i}"), None);
        bulk_ids.push(id);
    }
    let bulk_meta = db.list_entry_meta(&bulk_ids).unwrap();
    assert_eq!(bulk_meta.len(), 550, "chunking over 500 must return all 550 entries");
    assert_eq!(bulk_meta[0].series_id, bulk_ids[0]);
    assert_eq!(bulk_meta[549].series_id, bulk_ids[549]);
}

#[test]
fn list_library_prefers_catalog_title_and_cover_for_linked_series() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();

    db.upsert_catalog_anime(&crate::db::test_support::catalog_anime(2001, "As a Reincarnated Aristocrat", &[]), 0).unwrap();

    let s = crate::models::Series {
        id: 0,
        slug: "tensei-kizoku".into(),
        title: "Tensei Kizoku".into(),
        url: "https://site/tensei-kizoku".into(),
        cover_url: Some("file:C:/covers/tensei.jpg".into()),
        is_airing: false,
        followed: false,
        next_episode_at: None,
        site_episode_count: None,
    };
    let sid = db.upsert_series(src, &s).unwrap();
    db.set_followed(sid, true).unwrap();
    db.set_anilist_id(sid, 2001).unwrap();

    let items = db.list_library(src).unwrap();
    assert_eq!(items.len(), 1);
    let item = &items[0];
    assert_eq!(item.series.title, "As a Reincarnated Aristocrat");
    assert_eq!(item.series.cover_url.as_deref(), Some("https://cdn/2001.jpg"));
}

#[test]
fn list_library_keeps_site_title_and_cover_for_unlinked_series() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();

    let s = crate::models::Series {
        id: 0,
        slug: "unlinked-show".into(),
        title: "Unlinked Site Title".into(),
        url: "https://site/unlinked".into(),
        cover_url: Some("file:C:/covers/unlinked.jpg".into()),
        is_airing: false,
        followed: false,
        next_episode_at: None,
        site_episode_count: None,
    };
    let sid = db.upsert_series(src, &s).unwrap();
    db.set_followed(sid, true).unwrap();

    let items = db.list_library(src).unwrap();
    assert_eq!(items.len(), 1);
    let item = &items[0];
    assert_eq!(item.series.title, "Unlinked Site Title");
    assert_eq!(item.series.cover_url.as_deref(), Some("file:C:/covers/unlinked.jpg"));
}

#[test]
fn list_library_sorts_by_catalog_title_case_insensitively() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();

    // Series 1: site title "Zzz", catalog title "alpha show"
    db.upsert_catalog_anime(&crate::db::test_support::catalog_anime(3001, "alpha show", &[]), 0).unwrap();
    let s1 = crate::models::Series {
        id: 0,
        slug: "zzz".into(),
        title: "Zzz".into(),
        url: "https://site/zzz".into(),
        cover_url: None,
        is_airing: false,
        followed: false,
        next_episode_at: None,
        site_episode_count: None,
    };
    let sid1 = db.upsert_series(src, &s1).unwrap();
    db.set_followed(sid1, true).unwrap();
    db.set_anilist_id(sid1, 3001).unwrap();

    // Series 2: site title "Aaa", catalog title "Beta Show"
    db.upsert_catalog_anime(&crate::db::test_support::catalog_anime(3002, "Beta Show", &[]), 0).unwrap();
    let s2 = crate::models::Series {
        id: 0,
        slug: "aaa".into(),
        title: "Aaa".into(),
        url: "https://site/aaa".into(),
        cover_url: None,
        is_airing: false,
        followed: false,
        next_episode_at: None,
        site_episode_count: None,
    };
    let sid2 = db.upsert_series(src, &s2).unwrap();
    db.set_followed(sid2, true).unwrap();
    db.set_anilist_id(sid2, 3002).unwrap();

    let items = db.list_library(src).unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].series.title, "alpha show");
    assert_eq!(items[1].series.title, "Beta Show");
}

#[test]
fn followed_linked_series_ids_returns_only_followed_linked_series_of_source() {
    let db = Db::open(":memory:").unwrap();
    let src_a = db.upsert_source("AnimeYT", "https://a", "animeytx").unwrap();
    let src_b = db.upsert_source("TioAnime", "https://b", "tioanime").unwrap();

    let make = |slug: &str, title: &str| crate::models::Series {
        id: 0,
        slug: slug.into(),
        title: title.into(),
        url: format!("https://site/{slug}"),
        cover_url: None,
        is_airing: false,
        followed: false,
        next_episode_at: None,
        site_episode_count: None,
    };

    // On Source A:
    // s1: followed=true, linked=true -> in set
    let s1 = db.upsert_series(src_a, &make("s1", "Show 1")).unwrap();
    db.set_followed(s1, true).unwrap();
    db.set_anilist_id(s1, 101).unwrap();

    // s2: followed=false, linked=true -> NOT in set (unfollowed)
    let s2 = db.upsert_series(src_a, &make("s2", "Show 2")).unwrap();
    db.set_anilist_id(s2, 102).unwrap();

    // s3: followed=true, linked=false -> NOT in set (unlinked)
    let s3 = db.upsert_series(src_a, &make("s3", "Show 3")).unwrap();
    db.set_followed(s3, true).unwrap();

    // On Source B:
    // s4: followed=true, linked=true -> in set for B, NOT for A
    let s4 = db.upsert_series(src_b, &make("s4", "Show 4")).unwrap();
    db.set_followed(s4, true).unwrap();
    db.set_anilist_id(s4, 104).unwrap();

    // s5: followed + linked, but its catalog row has no cover -> NOT in set (the site
    // cover is the only one it could ever show)
    let s5 = db.upsert_series(src_a, &make("s5", "Show 5")).unwrap();
    db.set_followed(s5, true).unwrap();
    db.set_anilist_id(s5, 105).unwrap();
    let mut no_cover = crate::db::test_support::catalog_anime(105, "Show 5", &[]);
    no_cover.cover_url = None;
    db.upsert_catalog_anime(&no_cover, 4).unwrap();
    for (i, id) in [101, 102, 104].into_iter().enumerate() {
        db.upsert_catalog_anime(&crate::db::test_support::catalog_anime(id, &format!("Show {id}"), &[]), i as i64).unwrap();
    }

    let ids_a = db.followed_linked_series_ids(src_a).unwrap();
    assert_eq!(ids_a, [s1].into_iter().collect::<std::collections::HashSet<_>>());

    let ids_b = db.followed_linked_series_ids(src_b).unwrap();
    assert_eq!(ids_b, [s4].into_iter().collect::<std::collections::HashSet<_>>());
}

#[test]
fn list_library_linked_series_with_null_catalog_cover_falls_back_to_site_cover() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();

    let mut cat = crate::db::test_support::catalog_anime(4001, "Catalog Title", &[]);
    cat.cover_url = None;
    db.upsert_catalog_anime(&cat, 0).unwrap();

    let s = crate::models::Series {
        id: 0,
        slug: "null-cat-cover".into(),
        title: "Site Title".into(),
        url: "https://site/null-cat-cover".into(),
        cover_url: Some("file:C:/covers/site.jpg".into()),
        is_airing: false,
        followed: false,
        next_episode_at: None,
        site_episode_count: None,
    };
    let sid = db.upsert_series(src, &s).unwrap();
    db.set_followed(sid, true).unwrap();
    db.set_anilist_id(sid, 4001).unwrap();

    let items = db.list_library(src).unwrap();
    assert_eq!(items.len(), 1);
    let item = &items[0];
    assert_eq!(item.series.title, "Catalog Title");
    assert_eq!(item.series.cover_url.as_deref(), Some("file:C:/covers/site.jpg"));
}

#[test]
fn list_library_collapses_two_members_with_same_anilist_id_into_one_item() {
    let db = Db::open(":memory:").unwrap();
    let a = db.upsert_source("AnimeYT", "https://a", "animeytx").unwrap();
    let b = db.upsert_source("TioAnime", "https://b", "tioanime").unwrap();

    db.upsert_catalog_anime(&crate::db::test_support::catalog_anime(5001, "Canonical Title", &[]), 0).unwrap();

    let make = |slug: &str, title: &str| crate::models::Series {
        id: 0,
        slug: slug.into(),
        title: title.into(),
        url: format!("https://site/{slug}"),
        cover_url: None,
        is_airing: false,
        followed: false,
        next_episode_at: None,
        site_episode_count: None,
    };

    let sid_a = db.upsert_series(a, &make("arc-1", "Arc One")).unwrap();
    db.set_followed(sid_a, true).unwrap();
    db.set_anilist_id(sid_a, 5001).unwrap();

    let sid_b = db.upsert_series(b, &make("arc-2", "Arc Two")).unwrap();
    db.set_followed(sid_b, true).unwrap();
    db.set_anilist_id(sid_b, 5001).unwrap();

    let items = db.list_library(a).unwrap();
    assert_eq!(items.len(), 1, "two members sharing anilist_id must collapse into one library item");
    assert_eq!(items[0].series.title, "Canonical Title");
}

#[test]
fn set_series_source_moves_stub_and_preserves_fields_and_errors_on_collision() {
    let db = Db::open(":memory:").unwrap();
    let src_a = db.upsert_source("Site A", "https://site-a.com", "site_a").unwrap();
    let src_b = db.upsert_source("Site B", "https://site-b.com", "site_b").unwrap();

    let s = crate::models::Series {
        id: 0,
        slug: "stub-1".into(),
        title: "Stub Title".into(),
        url: "https://site-a.com/stub-1".into(),
        cover_url: Some("https://site-a.com/cover.jpg".into()),
        is_airing: false,
        followed: true,
        next_episode_at: None,
        site_episode_count: None,
    };
    let sid = db.upsert_series(src_a, &s).unwrap();
    db.set_anilist_id(sid, 999).unwrap();
    // upsert_series deliberately never writes `followed` (see its doc comment).
    db.set_followed(sid, true).unwrap();

    // Move stub from source A to source B
    db.set_series_source(sid, src_b, "real-slug", "https://site-b.com/real-slug").unwrap();

    let (src_id, slug, anilist_id, followed): (i64, String, Option<i64>, bool) = db
        .conn
        .query_row(
            "SELECT source_id, slug, anilist_id, followed FROM series WHERE id=?1",
            [sid],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get::<_, i64>(3)? != 0)),
        )
        .unwrap();
    assert_eq!(src_id, src_b, "source_id must be moved to src_b");
    assert_eq!(slug, "real-slug", "slug moves together with the source");
    assert_eq!(anilist_id, Some(999), "anilist_id must be left untouched");
    assert!(followed, "followed must be left untouched");

    // Another stub on source A
    let s2 = crate::models::Series {
        id: 0,
        slug: "collision-slug".into(),
        title: "Collision 1".into(),
        url: "https://site-a.com/collision-slug".into(),
        cover_url: None,
        is_airing: false,
        followed: false,
        next_episode_at: None,
        site_episode_count: None,
    };
    let sid2 = db.upsert_series(src_a, &s2).unwrap();

    // Existing row on source B with the same slug
    let s3 = crate::models::Series {
        id: 0,
        slug: "collision-slug".into(),
        title: "Collision 2".into(),
        url: "https://site-b.com/collision-slug".into(),
        cover_url: None,
        is_airing: false,
        followed: false,
        next_episode_at: None,
        site_episode_count: None,
    };
    let _sid3 = db.upsert_series(src_b, &s3).unwrap();

    // Moving sid2 to src_b collides on UNIQUE(source_id, slug) and returns Err, not panic
    let res = db.set_series_source(sid2, src_b, "collision-slug", "https://site-b.com/collision-slug");
    assert!(res.is_err(), "UNIQUE(source_id, slug) violation must return Err, not panic");

    // A stub the same title left on the target earlier must not block the move:
    // the real slug is written in the same statement.
    let mk = |slug: &str| crate::models::Series {
        id: 0, slug: slug.into(), title: slug.into(), url: format!("u-{slug}"),
        cover_url: None, is_airing: false, followed: false, next_episode_at: None, site_episode_count: None,
    };
    let moving = db.upsert_series(src_a, &mk("anilist-77")).unwrap();
    let _left_behind = db.upsert_series(src_b, &mk("anilist-77")).unwrap();
    db.set_series_source(moving, src_b, "movie-77", "https://site-b.com/movie-77").unwrap();
}

#[test]
fn followed_listings_used_for_scraping_skip_unlinked_catalog_rows() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("AnimeYT", "https://a.example", "animeytx").unwrap();
    let mk = |slug: &str| crate::models::Series {
        id: 0, slug: slug.into(), title: slug.into(), url: format!("https://a.example/{slug}"),
        cover_url: None, is_airing: true, followed: false, next_episode_at: None, site_episode_count: None,
    };
    let synthetic = db.upsert_series(src, &mk("anilist-77")).unwrap();
    let real = db.upsert_series(src, &mk("some-show")).unwrap();
    db.set_followed(synthetic, true).unwrap();
    db.set_followed(real, true).unwrap();

    let ids = |v: Vec<crate::models::Series>| v.into_iter().map(|s| s.id).collect::<Vec<_>>();
    assert_eq!(ids(db.list_followed(src).unwrap()), vec![real]);
    assert_eq!(ids(db.followed_series_without_episodes(src).unwrap()), vec![real]);
}

fn repair_fixture(db: &Db) -> (i64, i64) {
    let src = db.upsert_source("AnimeYT", "https://animeyt.cc", "animeytx").unwrap();
    let mk = |slug: &str, url: &str| crate::models::Series {
        id: 0, slug: slug.into(), title: slug.into(), url: url.into(),
        cover_url: None, is_airing: true, followed: false, next_episode_at: None, site_episode_count: None,
    };
    let synthetic = db.upsert_series(src, &mk("anilist-77", "https://anilist.co/anime/77")).unwrap();
    db.set_anilist_id(synthetic, 77).unwrap();
    db.set_followed(synthetic, true).unwrap();
    // What the scrape left behind: foreign episodes and a rewritten url.
    db.conn.execute("UPDATE series SET url='https://animeyt.cc/anime/77' WHERE id=?1", [synthetic]).unwrap();
    let real = db.upsert_series(src, &mk("some-show", "https://animeyt.cc/tv/some-show/")).unwrap();
    for (sid, n) in [(synthetic, "7"), (synthetic, "8"), (real, "1")] {
        db.insert_episode(&crate::models::Episode {
            id: 0, series_id: sid, number: n.into(), title: None, url: format!("u/{sid}/{n}"),
            released_at: None, seen: false, download_path: None, download_status: None,
        }).unwrap();
    }
    (synthetic, real)
}

#[test]
fn repair_removes_scraped_episodes_of_unlinked_rows_once() {
    let db = Db::open(":memory:").unwrap();
    let (synthetic, real) = repair_fixture(&db);
    db.delete_setting("repair:synthetic_rows_v1").unwrap();

    assert_eq!(db.repair_scraped_synthetic_rows(None).unwrap(), 2);
    let count = |sid: i64| -> i64 {
        db.conn.query_row("SELECT COUNT(*) FROM episodes WHERE series_id=?1", [sid], |r| r.get(0)).unwrap()
    };
    assert_eq!(count(synthetic), 0);
    assert_eq!(count(real), 1, "a real row is never touched");
    let url: String = db.conn.query_row("SELECT url FROM series WHERE id=?1", [synthetic], |r| r.get(0)).unwrap();
    assert_eq!(url, "https://anilist.co/anime/77");

    // Runs once: episodes scraped afterwards are left for the other fixes.
    db.insert_episode(&crate::models::Episode {
        id: 0, series_id: synthetic, number: "9".into(), title: None, url: "u/again".into(),
        released_at: None, seen: false, download_path: None, download_status: None,
    }).unwrap();
    assert_eq!(db.repair_scraped_synthetic_rows(None).unwrap(), 0);
    assert_eq!(count(synthetic), 1);
}

#[test]
fn repair_writes_a_backup_first_and_aborts_when_it_cannot() {
    let dir = std::env::temp_dir().join(format!("aot-repair-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let db_path = dir.join("db.sqlite");
    let db = Db::open(db_path.to_str().unwrap()).unwrap();
    let (synthetic, _) = repair_fixture(&db);
    db.delete_setting("repair:synthetic_rows_v1").unwrap();

    // A backup path in a directory that does not exist: nothing is deleted.
    let bad = dir.join("missing").join("b.sqlite");
    assert!(db.repair_scraped_synthetic_rows(Some(bad.to_str().unwrap())).is_err());
    let n: i64 = db.conn.query_row("SELECT COUNT(*) FROM episodes WHERE series_id=?1", [synthetic], |r| r.get(0)).unwrap();
    assert_eq!(n, 2);

    let good = dir.join("backup.sqlite");
    assert_eq!(db.repair_scraped_synthetic_rows(Some(good.to_str().unwrap())).unwrap(), 2);
    // Plain connection: opening the copy through `Db::open` would run the repair on it too.
    let backup = rusqlite::Connection::open(good.to_str().unwrap()).unwrap();
    let kept: i64 = backup.query_row("SELECT COUNT(*) FROM episodes WHERE series_id=?1", [synthetic], |r| r.get(0)).unwrap();
    assert_eq!(kept, 2, "the backup still has the rows the repair removed");
    drop(backup);
    drop(db);
    let _ = std::fs::remove_dir_all(&dir);
}

fn listing_row(slug: &str) -> crate::models::Series {
    crate::models::Series {
        id: 0, slug: slug.into(), title: slug.into(), url: format!("https://x/{slug}"),
        cover_url: None, is_airing: true, followed: false,
        next_episode_at: None, site_episode_count: None,
    }
}

#[test]
fn persist_listing_upserts_all_and_marks_unlisted_not_airing() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
    for i in 0..12 {
        db.upsert_series(src, &listing_row(&format!("old{i}"))).unwrap();
    }
    let listing: Vec<_> = (0..11).map(|i| listing_row(&format!("old{i}"))).collect();
    let out = db.persist_listing(src, &listing, true).unwrap();
    assert_eq!(out.upserted, 11);
    assert_eq!(out.unlisted, 1);
    let gone: i64 = db.conn.query_row("SELECT is_airing FROM series WHERE slug='old11'", [], |r| r.get(0)).unwrap();
    assert_eq!(gone, 0);
    let kept: i64 = db.conn.query_row("SELECT is_airing FROM series WHERE slug='old0'", [], |r| r.get(0)).unwrap();
    assert_eq!(kept, 1);
    // An incomplete listing never unmarks anything.
    let out = db.persist_listing(src, &listing[..1], false).unwrap();
    assert_eq!((out.upserted, out.unlisted), (1, 0));
}

#[test]
fn persist_listing_rolls_back_everything_when_one_row_fails() {
    let db = Db::open(":memory:").unwrap();
    let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
    db.upsert_series(src, &listing_row("existing")).unwrap();
    db.conn
        .execute_batch(
            "CREATE TRIGGER boom BEFORE INSERT ON series WHEN NEW.slug='boom'
             BEGIN SELECT RAISE(ABORT, 'forced failure'); END;",
        )
        .unwrap();
    let mut existing = listing_row("existing");
    existing.title = "changed".into();
    let listing = vec![listing_row("a"), existing, listing_row("boom"), listing_row("z")];
    assert!(db.persist_listing(src, &listing, true).is_err());
    let n: i64 = db.conn.query_row("SELECT count(*) FROM series", [], |r| r.get(0)).unwrap();
    assert_eq!(n, 1, "no partial rows");
    let title: String = db.conn.query_row("SELECT title FROM series WHERE slug='existing'", [], |r| r.get(0)).unwrap();
    assert_eq!(title, "existing", "earlier update rolled back");
    assert!(db.conn.is_autocommit(), "transaction closed");
}
