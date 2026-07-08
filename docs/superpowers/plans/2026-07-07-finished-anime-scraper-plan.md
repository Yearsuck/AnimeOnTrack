# Finished-Anime Scraper + Genre Data Model Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add genre-archive scraping, a per-series genre tag model, and a swipe-deck backend (`discover_swipe_card` / `decide_swipe` / `start_watching`) that let the user triage finished anime without a bulk pre-scrape of the catalog.

**Architecture:** Extend the existing `SiteAdapter` trait with genre-list/finished-page/series-detail parsing (same `scraper`-crate style as the current `.bsx`/`.eplister` selectors), add two nullable `series` columns plus a `series_genres` table via a guarded `ALTER TABLE`, and add three new Tauri commands that reuse the existing `scrape_via_mirrors` fetch path — no new fetch mechanism, no bulk scraping.

**Tech Stack:** Rust, `rusqlite`, `scraper` (HTML parsing), `anyhow`, Tauri v2 commands/state.

**Live markup confirmed 2026-07-07** (browser-verified against `wwv.animeytx.net`, same rigor as the existing selectors):
- `.status.Completed` (text "Finalizado") is present only on finished cards; absent (not a different class) on ongoing ones.
- `.typez`'s **text** is the real type badge and can disagree with its own second CSS class (e.g. `class="typez Music"` with text `"Donghua"` observed live) — must read `.text()`, never the class list.
- `.pagination a.page-numbers` matches page-number links **and** the "Siguiente »" next-link (same class) — the last page number must be the max of purely-numeric link text, not just any match.
- Homepage `a[href*="/genres/"]` returns many duplicate (slug, name) pairs (each series card links its own genres) — must dedupe by slug.
- `.genxed a` gives full genre list on a series detail page.
- `.spe` is a flat list of `<span><b>Label:</b> value</span>`; the type is the span whose `<b>` text is exactly `Tipo:`.
- Synopsis is `.entry-content[itemprop="description"]` (unique match, confirmed no collision with other selectors).

---

## File Structure

- Modify `src-tauri/src/models.rs` — add `FinishedCard` (+ `SwipeCard` type alias) and `SeriesDetail`.
- Modify `src-tauri/src/db.rs` — schema migration (`series.backlog_status`, `series.kind`, `series_genres` table) + new query methods.
- Modify `src-tauri/src/adapter/mod.rs` — extend `SiteAdapter` trait with 6 new methods.
- Modify `src-tauri/src/adapter/animeytx.rs` — implement the 6 new methods.
- Create `src-tauri/src/swipe.rs` — pure, DB/network-free filtering + pseudo-random helpers (mirrors the existing `diff.rs` pattern).
- Modify `src-tauri/src/commands.rs` — `AppState` gains 3 in-memory fields; add `discover_swipe_card`, `decide_swipe`, `start_watching`.
- Modify `src-tauri/src/lib.rs` — register the 3 new commands, init the 3 new `AppState` fields, add `mod swipe;`.
- Create fixtures: `src-tauri/tests/fixtures/genre_listing.html`, `homepage.html`, `series_detail.html` (already captured from live markup — see Task 5).

---

### Task 1: DB schema migration

**Files:**
- Modify: `src-tauri/src/db.rs`

- [ ] **Step 1: Write the failing tests**

Add to the `tests` module at the bottom of `src-tauri/src/db.rs` (after the existing `open_creates_schema` test):

```rust
    #[test]
    fn schema_includes_series_genres_table() {
        let db = Db::open(":memory:").unwrap();
        let count: i64 = db
            .conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table' AND name IN ('sources','series','episodes','settings','series_genres')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 5);
    }

    #[test]
    fn series_table_has_backlog_status_and_kind_columns() {
        let db = Db::open(":memory:").unwrap();
        let mut stmt = db.conn.prepare("PRAGMA table_info(series)").unwrap();
        let cols: Vec<String> = stmt
            .query_map([], |r| r.get::<_, String>(1))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert!(cols.contains(&"backlog_status".to_string()));
        assert!(cols.contains(&"kind".to_string()));
    }

    #[test]
    fn migration_is_idempotent_on_an_existing_on_disk_db() {
        let path = std::env::temp_dir().join(format!("aot_migration_test_{}.sqlite", std::process::id()));
        let path_str = path.to_str().unwrap();
        let _ = std::fs::remove_file(&path);
        {
            Db::open(path_str).unwrap();
        }
        // Second open must not error even though backlog_status/kind/series_genres
        // already exist from the first open (ALTER TABLE ADD COLUMN errors if run
        // twice unguarded).
        let db = Db::open(path_str).unwrap();
        let count: i64 = db
            .conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='series_genres'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
        let _ = std::fs::remove_file(&path);
    }
```

Also update the existing `open_creates_schema` test's expected count from 4 to 5 tables and add `'series_genres'` to its `IN (...)` list:

```rust
    #[test]
    fn open_creates_schema() {
        let db = Db::open(":memory:").unwrap();
        let count: i64 = db
            .conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table' AND name IN ('sources','series','episodes','settings','series_genres')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 5);
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --manifest-path src-tauri/Cargo.toml schema_includes_series_genres_table series_table_has_backlog_status_and_kind_columns migration_is_idempotent`
Expected: FAIL (table/columns don't exist yet)

- [ ] **Step 3: Implement the migration**

In `src-tauri/src/db.rs`, add a free function above `impl Db` (near `parse_ep_number`):

```rust
/// Add `column` to `table` if it isn't already there. SQLite has no
/// `ALTER TABLE ... ADD COLUMN IF NOT EXISTS`, so this checks `PRAGMA
/// table_info` first — needed because `init_schema` runs on every `Db::open`
/// and must be a no-op on a DB that already has the column (the codebase has
/// no separate migration framework; `series`/`episodes` already shipped
/// before this change, so evolving their schema in place is the existing idiom).
fn ensure_column(conn: &Connection, table: &str, column: &str, coltype: &str) -> Result<()> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let has_column = stmt
        .query_map([], |r| r.get::<_, String>(1))?
        .filter_map(|r| r.ok())
        .any(|name| name == column);
    if !has_column {
        conn.execute(&format!("ALTER TABLE {table} ADD COLUMN {column} {coltype}"), [])?;
    }
    Ok(())
}
```

Then update `init_schema` to call it and create the new table:

```rust
    fn init_schema(&self) -> Result<()> {
        self.conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS sources (
                id INTEGER PRIMARY KEY,
                name TEXT NOT NULL,
                base_url TEXT NOT NULL UNIQUE
            );
            CREATE TABLE IF NOT EXISTS series (
                id INTEGER PRIMARY KEY,
                source_id INTEGER NOT NULL REFERENCES sources(id),
                slug TEXT NOT NULL,
                title TEXT NOT NULL,
                url TEXT NOT NULL,
                cover_url TEXT,
                is_airing INTEGER NOT NULL DEFAULT 1,
                followed INTEGER NOT NULL DEFAULT 0,
                UNIQUE(source_id, slug)
            );
            CREATE TABLE IF NOT EXISTS episodes (
                id INTEGER PRIMARY KEY,
                series_id INTEGER NOT NULL REFERENCES series(id),
                number TEXT NOT NULL,
                title TEXT,
                url TEXT NOT NULL,
                released_at TEXT,
                seen INTEGER NOT NULL DEFAULT 0,
                added_at TEXT NOT NULL DEFAULT (datetime('now')),
                UNIQUE(series_id, url)
            );
            CREATE TABLE IF NOT EXISTS settings (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
            "#,
        )?;
        // series.backlog_status: NULL (normal row) | 'want' | 'discarded'.
        // series.kind: free-text type badge ("TV"/"OVA"/"Movie"/etc), same
        // vocabulary as the adapter's FinishedCard.kind — don't validate
        // against an enum, the live site's own vocabulary is inconsistent.
        ensure_column(&self.conn, "series", "backlog_status", "TEXT")?;
        ensure_column(&self.conn, "series", "kind", "TEXT")?;
        self.conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS series_genres (
                series_id INTEGER NOT NULL REFERENCES series(id),
                genre TEXT NOT NULL,
                PRIMARY KEY(series_id, genre)
            );
            "#,
        )?;
        Ok(())
    }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --manifest-path src-tauri/Cargo.toml db::`
Expected: PASS (all `db.rs` tests, including the pre-existing ones)

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/db.rs
git commit -m "feat: add series_genres table and backlog_status/kind columns"
```

---

### Task 2: Models — `FinishedCard`, `SwipeCard`, `SeriesDetail`

**Files:**
- Modify: `src-tauri/src/models.rs`

- [ ] **Step 1: Write the failing tests**

Add to the `tests` module in `src-tauri/src/models.rs`:

```rust
    #[test]
    fn finished_card_json_roundtrips() {
        let c = FinishedCard {
            title: "Liar Game".into(),
            url: "https://wwv.animeytx.net/tv/liar-game/".into(),
            poster_url: Some("https://x/img.jpg".into()),
            kind: "TV".into(),
        };
        let j = serde_json::to_string(&c).unwrap();
        let back: FinishedCard = serde_json::from_str(&j).unwrap();
        assert_eq!(c, back);
    }

    #[test]
    fn series_detail_json_roundtrips() {
        let d = SeriesDetail {
            genres: vec!["Drama".into(), "Seinen".into()],
            kind: Some("TV".into()),
            synopsis: Some("...".into()),
        };
        let j = serde_json::to_string(&d).unwrap();
        let back: SeriesDetail = serde_json::from_str(&j).unwrap();
        assert_eq!(d, back);
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --manifest-path src-tauri/Cargo.toml models::`
Expected: FAIL with "cannot find type `FinishedCard`" / "`SeriesDetail`"

- [ ] **Step 3: Implement the structs**

Add to `src-tauri/src/models.rs`, after the `Episode` struct:

```rust
/// A completed-anime card scraped off a genre-listing page (a `.bsx` card
/// carrying a `.status.Completed` div). Also doubles as the swipe-mode UI's
/// card payload as-is (see `SwipeCard`) — the swipe deck shows exactly what
/// the adapter parses off the listing page, no separate shape needed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FinishedCard {
    pub title: String,
    pub url: String,
    pub poster_url: Option<String>,
    pub kind: String,
}

pub type SwipeCard = FinishedCard;

/// Parsed from a series detail page (`/tv/{slug}/`) — the only place a
/// series' *complete* genre set and authoritative type ("Tipo:") are
/// available; listing cards only imply the one genre archive they were
/// found under.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SeriesDetail {
    pub genres: Vec<String>,
    pub kind: Option<String>,
    pub synopsis: Option<String>,
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --manifest-path src-tauri/Cargo.toml models::`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/models.rs
git commit -m "feat: add FinishedCard/SwipeCard/SeriesDetail models"
```

---

### Task 3: DB query methods for genres/backlog/kind

**Files:**
- Modify: `src-tauri/src/db.rs`

- [ ] **Step 1: Write the failing tests**

Add to the `tests` module in `src-tauri/src/db.rs`:

```rust
    #[test]
    fn series_genres_insert_is_idempotent_and_lists_sorted() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "b").unwrap();
        let s = crate::models::Series {
            id: 0, slug: "x".into(), title: "X".into(),
            url: "u".into(), cover_url: None, is_airing: false, followed: false,
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
        let src = db.upsert_source("AnimeYT", "b").unwrap();
        let s = crate::models::Series {
            id: 0, slug: "x".into(), title: "X".into(),
            url: "u".into(), cover_url: None, is_airing: false, followed: false,
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
    fn known_series_urls_reflects_any_decided_row() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "b").unwrap();
        let s = crate::models::Series {
            id: 0, slug: "x".into(), title: "X".into(),
            url: "https://site/tv/x/".into(), cover_url: None, is_airing: false, followed: false,
        };
        let sid = db.upsert_series(src, &s).unwrap();
        db.set_backlog_status(sid, Some("discarded")).unwrap();

        let known = db.known_series_urls(src).unwrap();
        assert!(known.contains("https://site/tv/x/"));
        assert_eq!(known.len(), 1);
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --manifest-path src-tauri/Cargo.toml series_genres_insert_is_idempotent backlog_status_and_kind_round_trip known_series_urls_reflects_any_decided_row`
Expected: FAIL with "no method named `insert_series_genres`" etc.

- [ ] **Step 3: Implement the methods**

Add to `impl Db` in `src-tauri/src/db.rs` (after `set_setting`):

```rust
    pub fn set_backlog_status(&self, series_id: i64, status: Option<&str>) -> Result<()> {
        self.conn
            .execute("UPDATE series SET backlog_status=?1 WHERE id=?2", (status, series_id))?;
        Ok(())
    }

    pub fn get_backlog_status(&self, series_id: i64) -> Result<Option<String>> {
        Ok(self.conn.query_row(
            "SELECT backlog_status FROM series WHERE id=?1",
            [series_id],
            |r| r.get::<_, Option<String>>(0),
        )?)
    }

    pub fn set_kind(&self, series_id: i64, kind: &str) -> Result<()> {
        self.conn
            .execute("UPDATE series SET kind=?1 WHERE id=?2", (kind, series_id))?;
        Ok(())
    }

    /// Insert genres for a series; already-present (series_id, genre) pairs
    /// are left alone (genres are static once fetched, no need to delete).
    pub fn insert_series_genres(&self, series_id: i64, genres: &[String]) -> Result<()> {
        for g in genres {
            self.conn.execute(
                "INSERT INTO series_genres(series_id, genre) VALUES(?1, ?2)
                 ON CONFLICT(series_id, genre) DO NOTHING",
                (series_id, g),
            )?;
        }
        Ok(())
    }

    pub fn list_series_genres(&self, series_id: i64) -> Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT genre FROM series_genres WHERE series_id=?1 ORDER BY genre")?;
        let rows = stmt
            .query_map([series_id], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Every URL that already has a `series` row for this source — any
    /// `backlog_status` or `followed` value counts as "already decided", so
    /// the swipe deck never re-offers a card the user has already acted on.
    pub fn known_series_urls(&self, source_id: i64) -> Result<std::collections::HashSet<String>> {
        let mut stmt = self.conn.prepare("SELECT url FROM series WHERE source_id=?1")?;
        let urls = stmt
            .query_map([source_id], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<std::collections::HashSet<_>>>()?;
        Ok(urls)
    }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --manifest-path src-tauri/Cargo.toml db::`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/db.rs
git commit -m "feat: add db methods for series_genres, backlog_status and kind"
```

---

### Task 4: Extend `SiteAdapter` trait

**Files:**
- Modify: `src-tauri/src/adapter/mod.rs`

- [ ] **Step 1: Update the trait**

Replace the full contents of `src-tauri/src/adapter/mod.rs` with:

```rust
use crate::models::{Episode, FinishedCard, Series, SeriesDetail};
use anyhow::Result;

pub mod animeytx;

/// A site-specific parser. HTML is fetched by the scraper engine; the adapter
/// only turns HTML into domain models.
pub trait SiteAdapter: Send + Sync {
    /// Absolute URL of the "currently airing" listing.
    fn airing_url(&self, base_url: &str) -> String;
    /// Parse the airing-list HTML into series (id/followed unset -> 0/false).
    fn parse_airing(&self, html: &str) -> Result<Vec<Series>>;
    /// Parse a series page HTML into its episodes (id/series_id/seen unset).
    fn parse_series(&self, html: &str) -> Result<Vec<Episode>>;

    /// Homepage URL — there is no dedicated "all genres" index page on this
    /// site, so genre discovery scrapes whatever links the homepage has.
    fn genre_list_url(&self, base_url: &str) -> String;
    /// `(slug, display_name)` pairs from `a[href*="/genres/"]`, deduped by
    /// slug (the homepage links to a genre many times over: once per series
    /// card plus its own nav).
    fn parse_genre_list(&self, html: &str) -> Result<Vec<(String, String)>>;
    /// `page=1` maps to the bare `/genres/{slug}/` (no `/page/1/` suffix) —
    /// confirmed that's how the site's own pagination links behave.
    fn genre_page_url(&self, base_url: &str, genre_slug: &str, page: u32) -> String;
    /// Cards from a genre-listing page, filtered to only those carrying a
    /// `.status.Completed` div (the finished/not-finished signal — ongoing
    /// cards don't have a `.status` div at all, rather than a different
    /// class value).
    fn parse_finished_page(&self, html: &str) -> Result<Vec<FinishedCard>>;
    /// Highest page number found in `.pagination`, or 1 if there's no
    /// pagination element at all (a genre with a single page of results).
    fn parse_pagination_last_page(&self, html: &str) -> u32;
    /// Full genre tag list, authoritative type, and synopsis from a series
    /// detail page (`/tv/{slug}/`) — this is the only place a series'
    /// *complete* genre set is available; listing cards only imply the
    /// genre archive they were found under.
    fn parse_series_detail(&self, html: &str) -> Result<SeriesDetail>;
}
```

- [ ] **Step 2: Confirm it fails to compile (expected — impl not updated yet)**

Run: `cargo build --manifest-path src-tauri/Cargo.toml`
Expected: FAIL with "not all trait items implemented" for `AnimeytxAdapter`

- [ ] **Step 3: Commit**

This step intentionally leaves the build red — Task 5 makes it green. Do not commit yet; proceed directly to Task 5 (same logical unit of work, but kept as a separate task here for reviewability).

---

### Task 5: Implement the new adapter methods + fixtures

**Files:**
- Modify: `src-tauri/src/adapter/animeytx.rs`
- Create: `src-tauri/tests/fixtures/genre_listing.html`
- Create: `src-tauri/tests/fixtures/homepage.html`
- Create: `src-tauri/tests/fixtures/series_detail.html`

- [ ] **Step 1: Create the fixtures (already captured from live markup)**

Create `src-tauri/tests/fixtures/genre_listing.html`:

```html
<!doctype html>
<html lang="es">
<head><meta charset="utf-8"><title>Anime de Genero: Aventura - AnimeYT</title></head>
<body>
<div class="listupd">
<div class="bs">
<div class="bsx">
<a href="https://wwv.animeytx.net/tv/omae-gotoki-ga-maou-ni-kateru-to-omouna-to-yuusha-party-wo-tsuihou-sareta-node-outo-de-kimama-ni-kurashitai/" itemprop="url" title="«Omae Gotoki ga Maou ni Kateru to Omouna» to Yuusha Party wo Tsuihou sareta node, Outo de Kimama ni Kurashitai" class="tip" rel="103336"><div class="limit"><div class="status Completed">Finalizado</div><div class="typez Yuri">Yuri</div><div class="ply"><i class="far fa-play-circle"></i></div><div class="bt">
<span class="epx">Completado</span></div>
<img src="https://wwv.animeytx.net/wp-content/uploads/2026/01/h9j45nmh98245hj45h.jpg" class="ts-post-image wp-post-image attachment-medium_large size-medium_large" loading="lazy" itemprop="image" title="«Omae Gotoki ga Maou ni Kateru to Omouna» to Yuusha Party wo Tsuihou sareta node, Outo de Kimama ni Kurashitai" width="680" height="1000"></div><div class="tt">
«Omae Gotoki ga Maou ni Kateru to Omouna» to Yuusha Party wo Tsuihou sareta node, Outo de Kimama ni Kurashitai<h2 itemprop="headline">«Omae Gotoki ga Maou ni Kateru to Omouna» to Yuusha Party wo Tsuihou sareta node, Outo de Kimama ni Kurashitai</h2></div>
</a></div>
<div class="bsx">
<a href="https://wwv.animeytx.net/tv/dekiru-neko-wa-kyou-mo-yuuutsu/" itemprop="url" title="Dekiru Neko wa Kyou mo Yuuutsu" class="tip" rel="103337"><div class="limit"><div class="status Completed">Finalizado</div><div class="typez Music">Donghua</div><div class="ply"><i class="far fa-play-circle"></i></div><div class="bt">
<span class="epx">Completado</span></div>
<img src="https://wwv.animeytx.net/wp-content/uploads/2026/02/aa11bb22cc33.jpg" class="ts-post-image wp-post-image attachment-medium_large size-medium_large" loading="lazy" itemprop="image" title="Dekiru Neko wa Kyou mo Yuuutsu" width="680" height="1000"></div><div class="tt">
Dekiru Neko wa Kyou mo Yuuutsu<h2 itemprop="headline">Dekiru Neko wa Kyou mo Yuuutsu</h2></div>
</a></div>
<div class="bsx">
<a href="https://wwv.animeytx.net/tv/another-journey-to-the-west/" itemprop="url" title="Another Journey to the West" class="tip" rel="71542"><div class="limit"><div class="typez Music">Donghua</div><div class="ply"><i class="far fa-play-circle"></i></div><div class="bt">
<span class="epx">Hiatus</span></div>
<img src="https://wwv.animeytx.net/wp-content/uploads/2024/10/n4589hn54h8045hh-768x1129.jpg.webp" class="ts-post-image wp-post-image attachment-medium_large size-medium_large" loading="lazy" itemprop="image" title="Another Journey to the West" width="768" height="1129"></div><div class="tt">
Another Journey to the West<h2 itemprop="headline">Another Journey to the West</h2></div>
</a></div>
</div>
</div>
<div class="pagination">
<span aria-current="page" class="page-numbers current">1</span>
<a class="page-numbers" href="https://wwv.animeytx.net/genres/aventura/page/2/">2</a>
<a class="page-numbers" href="https://wwv.animeytx.net/genres/aventura/page/3/">3</a>
<a class="page-numbers" href="https://wwv.animeytx.net/genres/aventura/page/24/">24</a>
<a class="next page-numbers" href="https://wwv.animeytx.net/genres/aventura/page/2/">Siguiente »</a>
</div>
</body>
</html>
```

Create `src-tauri/tests/fixtures/homepage.html`:

```html
<!doctype html>
<html lang="es">
<head><meta charset="utf-8"><title>AnimeYT - Ver Anime Online Sub Español Full HD y 4K</title></head>
<body>
<nav class="genre-menu">
<a href="https://wwv.animeytx.net/genres/aventura/">Aventura</a>
<a href="https://wwv.animeytx.net/genres/drama/">Drama</a>
<a href="https://wwv.animeytx.net/genres/ecchi/">Ecchi</a>
<a href="https://wwv.animeytx.net/genres/fantasia/">Fantasía</a>
<a href="https://wwv.animeytx.net/genres/isekai/">Isekai</a>
</nav>
<div class="listupd">
<div class="bsx">
<a href="https://wwv.animeytx.net/tv/world-is-dancing/" title="World Is Dancing"><div class="tt">World Is Dancing</div></a>
<div class="genxed"><a href="https://wwv.animeytx.net/genres/fantasia/">Fantasía</a> <a href="https://wwv.animeytx.net/genres/isekai/">Isekai</a></div>
</div>
<div class="bsx">
<a href="https://wwv.animeytx.net/tv/liar-game/" title="Liar Game"><div class="tt">Liar Game</div></a>
<div class="genxed"><a href="https://wwv.animeytx.net/genres/drama/">Drama</a> <a href="https://wwv.animeytx.net/genres/aventura/">Aventura</a></div>
</div>
</div>
<a href="https://wwv.animeytx.net/tv/world-is-dancing/">World Is Dancing (not a genre link, must be excluded)</a>
</body>
</html>
```

Create `src-tauri/tests/fixtures/series_detail.html`:

```html
<!doctype html>
<html lang="es">
<head><meta charset="utf-8"><title>Liar Game (2026) Sub Español - AnimeYT</title></head>
<body>
<article class="post-110000 hentry">
<div class="genxed"><a href="https://wwv.animeytx.net/genres/drama/" rel="tag">Drama</a> <a href="https://wwv.animeytx.net/genres/juegos/" rel="tag">Juegos</a> <a href="https://wwv.animeytx.net/genres/psicologico/" rel="tag">Psicológico</a> <a href="https://wwv.animeytx.net/genres/seinen/" rel="tag">Seinen</a> <a href="https://wwv.animeytx.net/genres/suspenso/" rel="tag">Suspenso</a></div>
<div class="spe">
<span><b>Estado:</b> En emisión</span>
<span><b>Estudio:</b> <a href="https://wwv.animeytx.net/studio/madhouse/" rel="tag">Madhouse</a></span>
<span class="split"><b>Estreno:</b> Abr 08, 2026</span>
<span><b>Duración:</b> 23 min.</span>
<span><b>Temporada:</b> <a href="https://wwv.animeytx.net/season/primavera-2026/" rel="tag">Primavera 2026</a></span>
<span><b>Tipo:</b> TV</span>
<span class="author vcard"><b>Autor:</b> <i class="fn">AnimeYT</i></span>
</div>
<div class="bixbox synp">
<div class="entry-content" itemprop="description">
<p><strong>Nao Kanzaki</strong> es una estudiante universitaria que ha vivido siempre apegada al significado de su nombre: «honesta hasta la tontería«. Un día recibe un paquete misterioso que la arrastra al Juego del Mentiroso, una competición clandestina donde ganar requiere traicionar la confianza de los demás.</p>
</div>
</div>
</article>
</body>
</html>
```

- [ ] **Step 2: Write the failing tests**

Add to the `tests` module in `src-tauri/src/adapter/animeytx.rs`:

```rust
    #[test]
    fn genre_list_url_is_homepage() {
        let a = AnimeytxAdapter;
        assert_eq!(a.genre_list_url("https://wwv.animeytx.net/"), "https://wwv.animeytx.net/");
    }

    #[test]
    fn genre_page_url_omits_page_suffix_for_page_one() {
        let a = AnimeytxAdapter;
        assert_eq!(
            a.genre_page_url("https://wwv.animeytx.net", "aventura", 1),
            "https://wwv.animeytx.net/genres/aventura/"
        );
        assert_eq!(
            a.genre_page_url("https://wwv.animeytx.net", "aventura", 3),
            "https://wwv.animeytx.net/genres/aventura/page/3/"
        );
    }

    #[test]
    fn parses_genre_list_fixture_deduped() {
        let html = include_str!("../../tests/fixtures/homepage.html");
        let out = AnimeytxAdapter.parse_genre_list(html).unwrap();
        // "aventura" appears in the nav AND inside a .genxed card => must be deduped to 1
        assert_eq!(out.iter().filter(|(slug, _)| slug == "aventura").count(), 1);
        assert!(out.iter().any(|(slug, name)| slug == "fantasia" && name == "Fantasía"));
        assert_eq!(out.len(), 5, "aventura, drama, ecchi, fantasia, isekai — no new slugs from .genxed dupes");
    }

    #[test]
    fn parses_finished_page_fixture_skips_non_completed() {
        let html = include_str!("../../tests/fixtures/genre_listing.html");
        let out = AnimeytxAdapter.parse_finished_page(html).unwrap();
        assert_eq!(out.len(), 2, "the Hiatus card (no .status div) must be excluded");
        assert_eq!(out[0].kind, "Yuri");
        // Regression: .typez's 2nd CSS class ("Music") must NOT leak into kind —
        // the live site's class list disagrees with the badge text it displays.
        assert_eq!(out[1].kind, "Donghua");
        for c in &out {
            assert!(!c.url.is_empty());
            assert!(c.poster_url.is_some());
        }
    }

    #[test]
    fn parses_pagination_last_page_ignoring_next_link() {
        let html = include_str!("../../tests/fixtures/genre_listing.html");
        assert_eq!(AnimeytxAdapter.parse_pagination_last_page(html), 24);
    }

    #[test]
    fn parses_series_detail_fixture() {
        let html = include_str!("../../tests/fixtures/series_detail.html");
        let d = AnimeytxAdapter.parse_series_detail(html).unwrap();
        assert_eq!(
            d.genres,
            vec!["Drama".to_string(), "Juegos".to_string(), "Psicológico".to_string(), "Seinen".to_string(), "Suspenso".to_string()]
        );
        assert_eq!(d.kind.as_deref(), Some("TV"));
        assert!(d.synopsis.unwrap().contains("Nao Kanzaki"));
    }
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib adapter::animeytx`
Expected: FAIL to compile — trait methods not implemented yet (this also resolves Task 4's red build)

- [ ] **Step 4: Implement the adapter methods**

Add to `src-tauri/src/adapter/animeytx.rs`, inside `impl SiteAdapter for AnimeytxAdapter` (after `parse_series`):

```rust
    fn genre_list_url(&self, base_url: &str) -> String {
        format!("{}/", base_url.trim_end_matches('/'))
    }

    fn parse_genre_list(&self, html: &str) -> Result<Vec<(String, String)>> {
        let doc = Html::parse_document(html);
        let a_sel = Selector::parse(r#"a[href*="/genres/"]"#).unwrap();
        let mut seen = std::collections::HashSet::new();
        let mut out = Vec::new();
        for a in doc.select(&a_sel) {
            let href = match a.value().attr("href") {
                Some(h) => h,
                None => continue,
            };
            let slug = slug_from_url(href);
            let name = a.text().collect::<String>().trim().to_string();
            if slug.is_empty() || name.is_empty() || !seen.insert(slug.clone()) {
                continue;
            }
            out.push((slug, name));
        }
        Ok(out)
    }

    fn genre_page_url(&self, base_url: &str, genre_slug: &str, page: u32) -> String {
        let base = base_url.trim_end_matches('/');
        if page <= 1 {
            format!("{base}/genres/{genre_slug}/")
        } else {
            format!("{base}/genres/{genre_slug}/page/{page}/")
        }
    }

    fn parse_finished_page(&self, html: &str) -> Result<Vec<FinishedCard>> {
        let doc = Html::parse_document(html);
        let card_sel = Selector::parse(AIRING_CARD).unwrap();
        let a_sel = Selector::parse("a").unwrap();
        let status_sel = Selector::parse(".status.Completed").unwrap();
        let typez_sel = Selector::parse(".typez").unwrap();
        let img_sel = Selector::parse("img").unwrap();

        let mut out = Vec::new();
        for card in doc.select(&card_sel) {
            if card.select(&status_sel).next().is_none() {
                continue; // no .status.Completed => not finished, skip
            }
            let anchor = match card.select(&a_sel).next() {
                Some(a) => a,
                None => continue,
            };
            let url = match anchor.value().attr("href") {
                Some(h) => h.to_string(),
                None => continue,
            };
            let title = anchor
                .value()
                .attr("title")
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| slug_from_url(&url));
            let poster_url = card.select(&img_sel).next().and_then(|i| {
                i.value()
                    .attr("data-src")
                    .or_else(|| i.value().attr("src"))
                    .map(|s| s.to_string())
            });
            // .typez's text is the actual type badge; its 2nd CSS class does
            // NOT reliably match (e.g. class="typez Music" with text
            // "Donghua" observed live), so this must read text, never class.
            let kind = text_of(card, &typez_sel).unwrap_or_default();
            out.push(FinishedCard { title, url, poster_url, kind });
        }
        Ok(out)
    }

    fn parse_pagination_last_page(&self, html: &str) -> u32 {
        let doc = Html::parse_document(html);
        let sel = Selector::parse(".pagination a.page-numbers").unwrap();
        doc.select(&sel)
            // the "Siguiente »" next-link also carries class `page-numbers`,
            // so only text that parses as a plain number counts as a page.
            .filter_map(|a| a.text().collect::<String>().trim().parse::<u32>().ok())
            .max()
            .unwrap_or(1)
    }

    fn parse_series_detail(&self, html: &str) -> Result<SeriesDetail> {
        let doc = Html::parse_document(html);
        let genxed_sel = Selector::parse(".genxed a").unwrap();
        let spe_sel = Selector::parse(".spe > span").unwrap();
        let b_sel = Selector::parse("b").unwrap();
        let synopsis_sel = Selector::parse(r#".entry-content[itemprop="description"]"#).unwrap();

        let genres = doc
            .select(&genxed_sel)
            .map(|a| a.text().collect::<String>().trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();

        // .spe is a flat list of <span><b>Label:</b> value</span>; find the
        // span whose <b> reads "Tipo:" and take the rest of its text.
        let mut kind = None;
        for span in doc.select(&spe_sel) {
            let label = text_of(span, &b_sel).unwrap_or_default();
            if label == "Tipo:" {
                let full = span.text().collect::<String>();
                kind = full.strip_prefix(&label).map(|s| s.trim().to_string());
                break;
            }
        }

        let synopsis = doc
            .select(&synopsis_sel)
            .next()
            .map(|el| el.text().collect::<String>().trim().to_string())
            .filter(|s| !s.is_empty());

        Ok(SeriesDetail { genres, kind, synopsis })
    }
```

Update the top of `src-tauri/src/adapter/animeytx.rs` to import the new model types:

```rust
use super::SiteAdapter;
use crate::models::{Episode, FinishedCard, Series, SeriesDetail};
use anyhow::Result;
use scraper::{Html, Selector};
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib adapter::`
Expected: PASS (all adapter tests, including the pre-existing `parses_airing_fixture` / `parses_series_fixture`)

- [ ] **Step 6: Commit**

```bash
git add src-tauri/src/adapter/mod.rs src-tauri/src/adapter/animeytx.rs src-tauri/tests/fixtures/genre_listing.html src-tauri/tests/fixtures/homepage.html src-tauri/tests/fixtures/series_detail.html
git commit -m "feat: add genre/finished-page/series-detail parsing to the adapter"
```

---

### Task 6: Pure swipe-deck filtering module

**Files:**
- Create: `src-tauri/src/swipe.rs`
- Modify: `src-tauri/src/lib.rs` (add `mod swipe;`)

- [ ] **Step 1: Write the failing tests**

Create `src-tauri/src/swipe.rs` with just the test module first:

```rust
use crate::models::FinishedCard;
use std::collections::HashSet;

#[cfg(test)]
mod tests {
    use super::*;

    fn card(url: &str) -> FinishedCard {
        FinishedCard { title: url.into(), url: url.into(), poster_url: None, kind: "TV".into() }
    }

    #[test]
    fn undecided_cards_excludes_known_urls() {
        let cards = vec![card("a"), card("b"), card("c")];
        let known: HashSet<String> = ["a".to_string()].into_iter().collect();
        let out = undecided_cards(cards, &known);
        assert_eq!(out.len(), 2);
        assert!(out.iter().all(|c| c.url != "a"));
    }

    #[test]
    fn undecided_cards_empty_when_all_known() {
        let cards = vec![card("a")];
        let known: HashSet<String> = ["a".to_string()].into_iter().collect();
        assert!(undecided_cards(cards, &known).is_empty());
    }

    #[test]
    fn undecided_cards_all_kept_when_nothing_known() {
        let cards = vec![card("a"), card("b")];
        let known: HashSet<String> = HashSet::new();
        assert_eq!(undecided_cards(cards, &known).len(), 2);
    }

    #[test]
    fn shuffle_preserves_all_elements() {
        let mut items = vec![1, 2, 3, 4, 5];
        shuffle(&mut items);
        items.sort();
        assert_eq!(items, vec![1, 2, 3, 4, 5]);
    }

    #[test]
    fn pick_index_is_in_bounds_and_none_for_empty() {
        for _ in 0..20 {
            let i = pick_index(7).unwrap();
            assert!(i < 7);
        }
        assert_eq!(pick_index(0), None);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --manifest-path src-tauri/Cargo.toml swipe::`
Expected: FAIL to compile — `undecided_cards`/`shuffle`/`pick_index` don't exist (this will also fail because `mod swipe;` isn't registered yet — add that now so the module compiles)

Add to `src-tauri/src/lib.rs`, in the `mod` list at the top:

```rust
mod adapter;
mod commands;
mod db;
mod diff;
mod models;
mod player;
mod scraper_engine;
mod swipe;
```

- [ ] **Step 3: Implement the pure functions**

Add above the `#[cfg(test)]` block in `src-tauri/src/swipe.rs`:

```rust
/// Cards whose url doesn't already have a `series` row (i.e. hasn't been
/// swiped/decided yet).
pub fn undecided_cards(cards: Vec<FinishedCard>, known_urls: &HashSet<String>) -> Vec<FinishedCard> {
    cards.into_iter().filter(|c| !known_urls.contains(&c.url)).collect()
}

/// Pseudo-random index in `0..len`, or `None` if `len` is 0. Uses the current
/// time as its source of randomness — same low-effort approach
/// `scraper_engine::uuid_like` already takes to avoid pulling in a `rand`
/// dependency for something this small; the swipe deck's shuffle order has
/// no correctness requirement beyond "not always the same order".
pub fn pick_index(len: usize) -> Option<usize> {
    if len == 0 {
        return None;
    }
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    Some((nanos as usize) % len)
}

/// Fisher-Yates shuffle driven by `pick_index`.
pub fn shuffle<T>(items: &mut Vec<T>) {
    for i in (1..items.len()).rev() {
        if let Some(j) = pick_index(i + 1) {
            items.swap(i, j);
        }
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --manifest-path src-tauri/Cargo.toml swipe::`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/swipe.rs src-tauri/src/lib.rs
git commit -m "feat: add pure swipe-deck filtering and shuffle helpers"
```

---

### Task 7: Commands — `discover_swipe_card`, `decide_swipe`, `start_watching`

**Files:**
- Modify: `src-tauri/src/commands.rs`

No new unit tests in this task: every existing command in `commands.rs` requires a live `AppHandle`/webview and has no unit tests today (only the pure/DB-only modules — `db.rs`, `models.rs`, `diff.rs`, `adapter/`, and now `swipe.rs` — are unit tested). This task's correctness rests on the tests already written in Tasks 1–6 (the `undecided_cards` empty-result test is exactly what guarantees `discover_swipe_card` returns `None` instead of a stale/duplicate card on a fully-decided page) plus the manual verification in Task 8.

- [ ] **Step 1: Add imports and `AppState` fields**

At the top of `src-tauri/src/commands.rs`, change:

```rust
use crate::adapter::{animeytx::AnimeytxAdapter, SiteAdapter};
use crate::db::Db;
use crate::diff::new_episodes;
use crate::models::{Episode, Series};
use crate::player::{BrowserPlayer, EpisodePlayer};
use crate::scraper_engine::{fetch_cover_image, fetch_html, ScrapeResult};
use serde::Serialize;
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, State};
```

to:

```rust
use crate::adapter::{animeytx::AnimeytxAdapter, SiteAdapter};
use crate::db::Db;
use crate::diff::new_episodes;
use crate::models::{Episode, FinishedCard, Series, SeriesDetail};
use crate::player::{BrowserPlayer, EpisodePlayer};
use crate::scraper_engine::{fetch_cover_image, fetch_html, ScrapeResult};
use crate::swipe::{pick_index, shuffle, undecided_cards};
use serde::Serialize;
use std::collections::HashMap;
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, State};
```

Change the `AppState` struct:

```rust
pub struct AppState {
    pub db: Mutex<Db>,
    pub source_id: Mutex<Option<i64>>,
    /// Cards from a (genre, page) fetch not yet shown this session — lets
    /// discover_swipe_card serve ~10 swipes off one HTTP fetch instead of one
    /// fetch per swipe.
    pub swipe_buffer: Mutex<HashMap<(String, u32), Vec<FinishedCard>>>,
    /// Highest page number seen so far for each genre slug this session.
    pub swipe_last_page: Mutex<HashMap<String, u32>>,
    /// Cards handed out by discover_swipe_card, keyed by url, so decide_swipe
    /// (which only receives a url) can look up the card data to persist.
    pub swipe_served: Mutex<HashMap<String, FinishedCard>>,
}
```

- [ ] **Step 2: Add the genre-list cache helpers**

Add after `save_mirrors` in `src-tauri/src/commands.rs`:

```rust
const GENRE_LIST_KEY: &str = "genre_list";

fn load_genre_list(db: &Db) -> Result<Vec<(String, String)>, String> {
    let raw = db.get_setting(GENRE_LIST_KEY).map_err(|e| e.to_string())?;
    match raw {
        Some(s) => serde_json::from_str(&s).map_err(|e| e.to_string()),
        None => Ok(Vec::new()),
    }
}

fn save_genre_list(db: &Db, list: &[(String, String)]) -> Result<(), String> {
    let raw = serde_json::to_string(list).map_err(|e| e.to_string())?;
    db.set_setting(GENRE_LIST_KEY, &raw).map_err(|e| e.to_string())
}

/// Cached genre (slug, name) list, scraped once per install and reused after
/// (mirrors the `mirror_urls` settings-cache pattern already used elsewhere).
async fn ensure_genre_list(
    app: &AppHandle,
    db_mirrors: &[String],
    state: &State<'_, AppState>,
) -> Result<Vec<(String, String)>, String> {
    let cached = {
        let db = state.db.lock().unwrap();
        load_genre_list(&db)?
    };
    if !cached.is_empty() {
        return Ok(cached);
    }
    let a = adapter();
    let (_scraped, pairs, _mirror) =
        scrape_via_mirrors(app, db_mirrors, &a.genre_list_url(""), |html| a.parse_genre_list(html)).await?;
    let db = state.db.lock().unwrap();
    save_genre_list(&db, &pairs)?;
    Ok(pairs)
}

fn path_of(url: &str) -> Result<String, String> {
    let u = url::Url::parse(url).map_err(|_| format!("url inválida: {url}"))?;
    Ok(format!("{}{}", u.path(), u.query().map(|q| format!("?{q}")).unwrap_or_default()))
}

/// Fetch and parse a series detail page, falling through mirrors the same
/// way every other scrape does. An empty genre list is treated the same as
/// "page loaded but didn't parse" (see `scrape_via_mirrors`'s doc comment) —
/// it means this mirror's detail-page markup didn't match, not that the
/// series genuinely has zero genres.
async fn fetch_series_detail(
    app: &AppHandle,
    mirrors: &[String],
    series_url: &str,
) -> Result<SeriesDetail, String> {
    let a = adapter();
    let path = path_of(series_url)?;
    let (_scraped, details, _mirror) = scrape_via_mirrors(app, mirrors, &path, |html| {
        let d = a.parse_series_detail(html)?;
        if d.genres.is_empty() {
            Err(anyhow::anyhow!("no genres parsed (likely wrong/incompatible mirror)"))
        } else {
            Ok(vec![d])
        }
    })
    .await?;
    details.into_iter().next().ok_or_else(|| "empty series detail page".to_string())
}

async fn fetch_episode_list_for(
    app: &AppHandle,
    mirrors: &[String],
    series_url: &str,
) -> Result<Vec<Episode>, String> {
    let a = adapter();
    let path = path_of(series_url)?;
    let (_scraped, eps, _mirror) =
        scrape_via_mirrors(app, mirrors, &path, |html| a.parse_series(html)).await?;
    Ok(eps)
}

fn slug_from_url(url: &str) -> String {
    url.trim_end_matches('/').rsplit('/').next().unwrap_or("").to_string()
}
```

- [ ] **Step 3: Add `SwipeDecision` and `discover_swipe_card`**

Add near the bottom of `src-tauri/src/commands.rs`:

```rust
#[derive(serde::Deserialize)]
pub enum SwipeDecision {
    Seen,
    Want,
    Discard,
}

/// Pick a random cached-or-fresh (genre, page) pair, scrape it if not
/// already buffered this session, filter out anything already decided,
/// shuffle, and pop one card. `Ok(None)` means the freshly-scraped page (or
/// buffer) had nothing left after filtering — a normal "everything on this
/// page was already decided" case, not an error; the frontend just calls
/// again.
#[tauri::command]
pub async fn discover_swipe_card(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Option<FinishedCard>, String> {
    let src = get_source_id(&state)?;
    let mirrors = {
        let db = state.db.lock().unwrap();
        load_mirrors(&db)?
    };
    let genres = ensure_genre_list(&app, &mirrors, &state).await?;
    if genres.is_empty() {
        return Err("no se encontraron géneros; reintenta el escaneo".into());
    }
    let (slug, _name) = &genres[pick_index(genres.len()).unwrap()];

    let last_page = state.swipe_last_page.lock().unwrap().get(slug).copied().unwrap_or(1);
    let page = pick_index(last_page as usize).map(|i| i as u32 + 1).unwrap_or(1);

    let buffered = state.swipe_buffer.lock().unwrap().remove(&(slug.clone(), page));
    let mut cards = match buffered {
        Some(cards) => cards,
        None => {
            let a = adapter();
            let path = a.genre_page_url("", slug, page);
            let (scraped, raw_cards, _mirror) =
                scrape_via_mirrors(&app, &mirrors, &path, |html| a.parse_finished_page(html)).await?;
            state
                .swipe_last_page
                .lock()
                .unwrap()
                .insert(slug.clone(), a.parse_pagination_last_page(&scraped.html));
            let known = {
                let db = state.db.lock().unwrap();
                db.known_series_urls(src).map_err(|e| e.to_string())?
            };
            let mut fresh = undecided_cards(raw_cards, &known);
            shuffle(&mut fresh);
            fresh
        }
    };

    let Some(card) = cards.pop() else {
        return Ok(None);
    };
    if !cards.is_empty() {
        state.swipe_buffer.lock().unwrap().insert((slug.clone(), page), cards);
    }
    state.swipe_served.lock().unwrap().insert(card.url.clone(), card.clone());
    Ok(Some(card))
}
```

- [ ] **Step 4: Add `decide_swipe`**

```rust
#[tauri::command]
pub async fn decide_swipe(
    app: AppHandle,
    state: State<'_, AppState>,
    series_url: String,
    decision: SwipeDecision,
) -> Result<(), String> {
    let src = get_source_id(&state)?;
    let card = state
        .swipe_served
        .lock()
        .unwrap()
        .remove(&series_url)
        .ok_or_else(|| "unknown swipe card; call discover_swipe_card first".to_string())?;

    let series = Series {
        id: 0,
        slug: slug_from_url(&card.url),
        title: card.title.clone(),
        url: card.url.clone(),
        cover_url: card.poster_url.clone(),
        is_airing: false,
        followed: false,
    };

    match decision {
        SwipeDecision::Discard => {
            let db = state.db.lock().unwrap();
            let sid = db.upsert_series(src, &series).map_err(|e| e.to_string())?;
            db.set_kind(sid, &card.kind).map_err(|e| e.to_string())?;
            db.set_backlog_status(sid, Some("discarded")).map_err(|e| e.to_string())?;
        }
        SwipeDecision::Want => {
            let mirrors = {
                let db = state.db.lock().unwrap();
                load_mirrors(&db)?
            };
            let detail = fetch_series_detail(&app, &mirrors, &card.url).await?;
            let db = state.db.lock().unwrap();
            let sid = db.upsert_series(src, &series).map_err(|e| e.to_string())?;
            db.set_kind(sid, detail.kind.as_deref().unwrap_or(&card.kind)).map_err(|e| e.to_string())?;
            db.insert_series_genres(sid, &detail.genres).map_err(|e| e.to_string())?;
            db.set_backlog_status(sid, Some("want")).map_err(|e| e.to_string())?;
        }
        SwipeDecision::Seen => {
            let mirrors = {
                let db = state.db.lock().unwrap();
                load_mirrors(&db)?
            };
            let detail = fetch_series_detail(&app, &mirrors, &card.url).await?;
            let eps = fetch_episode_list_for(&app, &mirrors, &card.url).await?;
            let db = state.db.lock().unwrap();
            let sid = db.upsert_series(src, &series).map_err(|e| e.to_string())?;
            db.set_followed(sid, true).map_err(|e| e.to_string())?;
            db.set_kind(sid, detail.kind.as_deref().unwrap_or(&card.kind)).map_err(|e| e.to_string())?;
            db.insert_series_genres(sid, &detail.genres).map_err(|e| e.to_string())?;
            for mut e in eps {
                e.series_id = sid;
                e.seen = true;
                db.insert_episode(&e).map_err(|e| e.to_string())?;
            }
        }
    }
    Ok(())
}
```

- [ ] **Step 5: Add `start_watching`**

```rust
/// Promote a `backlog_status='want'` row to an ordinary followed series:
/// fetch its episode list (all unseen), start following it, clear the
/// backlog status. `refresh()` already scans all followed rows regardless
/// of `is_airing`, so no changes are needed there.
#[tauri::command]
pub async fn start_watching(
    app: AppHandle,
    state: State<'_, AppState>,
    series_id: i64,
) -> Result<(), String> {
    let (series_url, mirrors) = {
        let db = state.db.lock().unwrap();
        let status = db.get_backlog_status(series_id).map_err(|e| e.to_string())?;
        if status.as_deref() != Some("want") {
            return Err("series is not in the 'want' backlog".into());
        }
        let url = db
            .conn
            .query_row("SELECT url FROM series WHERE id=?1", [series_id], |r| r.get::<_, String>(0))
            .map_err(|e| e.to_string())?;
        (url, load_mirrors(&db)?)
    };
    let eps = fetch_episode_list_for(&app, &mirrors, &series_url).await?;
    let db = state.db.lock().unwrap();
    for mut e in eps {
        e.series_id = series_id;
        e.seen = false;
        db.insert_episode(&e).map_err(|e| e.to_string())?;
    }
    db.set_followed(series_id, true).map_err(|e| e.to_string())?;
    db.set_backlog_status(series_id, None).map_err(|e| e.to_string())?;
    Ok(())
}
```

- [ ] **Step 6: Build to confirm it compiles**

Run: `cargo build --manifest-path src-tauri/Cargo.toml`
Expected: builds clean (warnings OK, no errors)

- [ ] **Step 7: Commit**

```bash
git add src-tauri/src/commands.rs
git commit -m "feat: add discover_swipe_card, decide_swipe and start_watching commands"
```

---

### Task 8: Register commands and state in `lib.rs`

**Files:**
- Modify: `src-tauri/src/lib.rs`

- [ ] **Step 1: Initialize the new `AppState` fields**

In `src-tauri/src/lib.rs`, change:

```rust
            app.manage(AppState {
                db: Mutex::new(db),
                source_id: Mutex::new(source_id),
            });
```

to:

```rust
            app.manage(AppState {
                db: Mutex::new(db),
                source_id: Mutex::new(source_id),
                swipe_buffer: Mutex::new(std::collections::HashMap::new()),
                swipe_last_page: Mutex::new(std::collections::HashMap::new()),
                swipe_served: Mutex::new(std::collections::HashMap::new()),
            });
```

- [ ] **Step 2: Register the 3 new commands**

Change the `invoke_handler` list to add the 3 new commands at the end:

```rust
        .invoke_handler(tauri::generate_handler![
            commands::scan_airing,
            commands::list_airing,
            commands::list_library,
            commands::set_followed,
            commands::refresh,
            commands::list_pending,
            commands::pending_count,
            commands::open_episode,
            commands::set_seen,
            commands::set_seen_cascade,
            commands::list_episodes,
            commands::rescan_airing,
            commands::get_mirrors,
            commands::set_mirrors,
            commands::discover_swipe_card,
            commands::decide_swipe,
            commands::start_watching,
        ])
```

- [ ] **Step 3: Run the full test suite**

Run: `cargo test --manifest-path src-tauri/Cargo.toml`
Expected: PASS — all tests across `models`, `db`, `diff`, `adapter::animeytx`, `swipe`

- [ ] **Step 4: Build the full app**

Run: `cargo build --manifest-path src-tauri/Cargo.toml`
Expected: builds clean

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/lib.rs
git commit -m "feat: register swipe-deck commands and state"
```

---

### Task 9: Manual verification

**Files:** none (verification only)

- [ ] **Step 1: Run the full backend test suite one more time**

Run: `cargo test --manifest-path src-tauri/Cargo.toml`
Expected: PASS

- [ ] **Step 2: Manually exercise the new commands via the Tauri dev console**

Run: `npm run tauri dev` (kill any previous `aot-scaffold.exe` first, per `CLAUDE.md`)

In the webview devtools console (or a temporary debug button, since no UI exists yet — this piece is backend-only per the spec's "no UI of its own"), call:

```js
await window.__TAURI__.core.invoke('discover_swipe_card')
```

Expected: after `scan_airing`/`rescan_airing` has run at least once (so a `source_id` exists), this returns `{ title, url, poster_url, kind }` for a real finished series, or `null` if that page's cards were all already decided. Repeat 10+ times and confirm no duplicate urls are returned within the session.

Then:

```js
await window.__TAURI__.core.invoke('decide_swipe', { seriesUrl: '<url from above>', decision: 'Discard' })
```

Expected: resolves with no error. Inspect the sqlite DB directly (`%APPDATA%\com.ernes.aot-scaffold\animeontrack.sqlite`) per `CLAUDE.md`'s guidance:

```sql
SELECT slug, title, backlog_status, kind FROM series WHERE backlog_status IS NOT NULL;
```

Expected: one row with `backlog_status='discarded'`.

Repeat with `decision: 'Want'` on a different card and confirm `series_genres` gets populated:

```sql
SELECT s.title, g.genre FROM series s JOIN series_genres g ON g.series_id = s.id WHERE s.backlog_status='want';
```

Then call `start_watching` with that series' `id` and confirm `followed=1`, `backlog_status IS NULL`, and `episodes` has rows with `seen=0`.

- [ ] **Step 3: No commit for this task** (verification only — if a bug surfaces, fix it in the relevant task's file and amend that task's commit conversation, not this one)

---

## Explicitly out of scope (per the spec)

- Swipe UI, decision buttons, backlog list screen — separate piece (`2026-07-07-tinder-swipe-design.md`).
- Genre stats aggregation/charts — separate piece (`2026-07-07-genre-stats-design.md`).
- No content filtering (e.g. excluding the "Hentai" genre) — left unfiltered, per spec.
