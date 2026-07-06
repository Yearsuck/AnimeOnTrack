# AnimeOnTrack Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A Windows desktop app that tracks airing anime on animeytx, detects new episodes for followed series, and stacks them into a pending-to-watch list that opens episodes in the browser.

**Architecture:** Tauri app. A Rust core owns SQLite, the episode-diff logic, and a pluggable `SiteAdapter` that parses HTML with the `scraper` crate. Scraping is done by a hidden WebView2 window that loads the page (passing Cloudflare), then returns rendered `outerHTML` to Rust for parsing. A React + TypeScript UI calls the core over Tauri IPC.

**Tech Stack:** Rust, Tauri v2, rusqlite (bundled SQLite), scraper (HTML parsing), serde, React, Vite, TypeScript.

---

## File structure

Rust core (`src-tauri/src/`):
- `models.rs` — `Series`, `Episode` domain types (serde).
- `db.rs` — SQLite connection, schema/migrations, all CRUD.
- `diff.rs` — pure new-episode detection.
- `adapter/mod.rs` — `SiteAdapter` trait.
- `adapter/animeytx.rs` — `AnimeytxAdapter` (DooPlay selectors).
- `scraper_engine.rs` — hidden WebView2 orchestration (URL → rendered HTML).
- `player.rs` — `EpisodePlayer` trait + `BrowserPlayer`.
- `commands.rs` — Tauri command handlers (the IPC surface).
- `lib.rs` — app state wiring, `run()`.
- `tests/fixtures/` — saved HTML snapshots.

Frontend (`src/`):
- `api.ts` — typed wrappers over `invoke`.
- `types.ts` — TS mirrors of Rust models.
- `App.tsx` — shell + simple view routing.
- `views/Onboarding.tsx`, `views/AiringGrid.tsx`, `views/Pending.tsx`, `views/SeriesDetail.tsx`, `views/Settings.tsx`.

---

## Task 1: Scaffold the Tauri + React-TS project

**Files:**
- Create: whole project skeleton via CLI.

- [ ] **Step 1: Create the app**

Run from the repo root (`C:\Users\ernes\Documents\GitHub\AnimeOnTrack`):

```bash
npm create tauri-app@latest . -- --template react-ts --manager npm
```

If the CLI refuses because the directory is non-empty, scaffold in a temp dir and copy `src/`, `src-tauri/`, `index.html`, `package.json`, `vite.config.ts`, `tsconfig*.json` into the repo, keeping the existing `docs/` and `.git/`.

- [ ] **Step 2: Install JS deps**

Run: `npm install`
Expected: `node_modules/` created, no errors.

- [ ] **Step 3: Add Rust crates**

Edit `src-tauri/Cargo.toml`, add under `[dependencies]`:

```toml
rusqlite = { version = "0.32", features = ["bundled"] }
scraper = "0.20"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
anyhow = "1"
url = "2"
```

Ensure `tauri` has the needed features. In `[dependencies]` the `tauri` line should include:

```toml
tauri = { version = "2", features = [] }
```

Add the shell plugin for opening URLs:

Run: `cd src-tauri && cargo add tauri-plugin-shell && cd ..`

- [ ] **Step 4: Verify it builds and runs**

Run: `npm run tauri dev`
Expected: the default Tauri window opens. Close it. If it compiles and launches, the scaffold works.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "chore: scaffold Tauri + React-TS app"
```

---

## Task 2: Domain models

**Files:**
- Create: `src-tauri/src/models.rs`
- Modify: `src-tauri/src/lib.rs` (add `mod models;`)
- Test: inline `#[cfg(test)]` in `models.rs`

- [ ] **Step 1: Write the failing test**

Create `src-tauri/src/models.rs`:

```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Series {
    pub id: i64,
    pub slug: String,
    pub title: String,
    pub url: String,
    pub cover_url: Option<String>,
    pub is_airing: bool,
    pub followed: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Episode {
    pub id: i64,
    pub series_id: i64,
    pub number: String,
    pub title: Option<String>,
    pub url: String,
    pub released_at: Option<String>,
    pub seen: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn series_json_roundtrips() {
        let s = Series {
            id: 1,
            slug: "baki-dou".into(),
            title: "Baki-dou".into(),
            url: "https://wwv.animeytx.net/tv/baki-dou/".into(),
            cover_url: Some("https://x/img.jpg".into()),
            is_airing: true,
            followed: false,
        };
        let j = serde_json::to_string(&s).unwrap();
        let back: Series = serde_json::from_str(&j).unwrap();
        assert_eq!(s, back);
    }

    #[test]
    fn episode_json_roundtrips() {
        let e = Episode {
            id: 5,
            series_id: 1,
            number: "1x05".into(),
            title: Some("Ep 5".into()),
            url: "https://wwv.animeytx.net/episodio/baki-dou-5/".into(),
            released_at: None,
            seen: false,
        };
        let j = serde_json::to_string(&e).unwrap();
        let back: Episode = serde_json::from_str(&j).unwrap();
        assert_eq!(e, back);
    }
}
```

Add to `src-tauri/src/lib.rs` near the top:

```rust
mod models;
```

- [ ] **Step 2: Run test to verify it passes (compiles)**

Run: `cd src-tauri && cargo test models:: && cd ..`
Expected: 2 passed. (These are trivial roundtrips; they exist to lock the shape and catch serde breakage.)

- [ ] **Step 3: Commit**

```bash
git add src-tauri/src/models.rs src-tauri/src/lib.rs
git commit -m "feat: add Series and Episode models"
```

---

## Task 3: Database schema and connection

**Files:**
- Create: `src-tauri/src/db.rs`
- Modify: `src-tauri/src/lib.rs` (add `mod db;`)
- Test: inline `#[cfg(test)]` in `db.rs`

- [ ] **Step 1: Write the failing test**

Create `src-tauri/src/db.rs`:

```rust
use anyhow::Result;
use rusqlite::Connection;

pub struct Db {
    pub conn: Connection,
}

impl Db {
    /// Open a DB at `path` (":memory:" for tests) and ensure schema exists.
    pub fn open(path: &str) -> Result<Self> {
        let conn = Connection::open(path)?;
        let db = Db { conn };
        db.init_schema()?;
        Ok(db)
    }

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
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_creates_schema() {
        let db = Db::open(":memory:").unwrap();
        let count: i64 = db
            .conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table' AND name IN ('sources','series','episodes','settings')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 4);
    }
}
```

Add to `src-tauri/src/lib.rs`:

```rust
mod db;
```

- [ ] **Step 2: Run test to verify it passes**

Run: `cd src-tauri && cargo test db::tests::open_creates_schema && cd ..`
Expected: 1 passed.

- [ ] **Step 3: Commit**

```bash
git add src-tauri/src/db.rs src-tauri/src/lib.rs
git commit -m "feat: add SQLite schema and connection"
```

---

## Task 4: Database CRUD

**Files:**
- Modify: `src-tauri/src/db.rs`
- Test: inline `#[cfg(test)]` in `db.rs`

- [ ] **Step 1: Write the failing test**

Append these tests inside the existing `mod tests` block in `src-tauri/src/db.rs`:

```rust
    #[test]
    fn upsert_source_and_series_then_follow() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "https://wwv.animeytx.net").unwrap();

        let s = crate::models::Series {
            id: 0,
            slug: "baki-dou".into(),
            title: "Baki-dou".into(),
            url: "https://wwv.animeytx.net/tv/baki-dou/".into(),
            cover_url: None,
            is_airing: true,
            followed: false,
        };
        let sid = db.upsert_series(src, &s).unwrap();
        // upsert again with new title => same row updated, not duplicated
        let s2 = crate::models::Series { title: "Baki-dou 2".into(), ..s.clone() };
        let sid2 = db.upsert_series(src, &s2).unwrap();
        assert_eq!(sid, sid2);

        db.set_followed(sid, true).unwrap();
        let airing = db.list_airing(src).unwrap();
        assert_eq!(airing.len(), 1);
        assert!(airing[0].followed);
        assert_eq!(airing[0].title, "Baki-dou 2");
    }

    #[test]
    fn insert_episode_dedups_and_marks_seen() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "https://wwv.animeytx.net").unwrap();
        let s = crate::models::Series {
            id: 0, slug: "x".into(), title: "X".into(),
            url: "u".into(), cover_url: None, is_airing: true, followed: true,
        };
        let sid = db.upsert_series(src, &s).unwrap();

        let ep = crate::models::Episode {
            id: 0, series_id: sid, number: "1".into(), title: None,
            url: "https://site/ep1".into(), released_at: None, seen: false,
        };
        let eid = db.insert_episode(&ep).unwrap();
        // same url again => no new row
        let eid_dup = db.insert_episode(&ep).unwrap();
        assert_eq!(eid, eid_dup);

        assert_eq!(db.pending_count().unwrap(), 1);
        db.mark_seen(eid).unwrap();
        assert_eq!(db.pending_count().unwrap(), 0);
    }

    #[test]
    fn existing_episode_urls_returns_known_urls() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "b").unwrap();
        let s = crate::models::Series {
            id: 0, slug: "x".into(), title: "X".into(),
            url: "u".into(), cover_url: None, is_airing: true, followed: true,
        };
        let sid = db.upsert_series(src, &s).unwrap();
        db.insert_episode(&crate::models::Episode {
            id: 0, series_id: sid, number: "1".into(), title: None,
            url: "https://site/ep1".into(), released_at: None, seen: false,
        }).unwrap();
        let urls = db.existing_episode_urls(sid).unwrap();
        assert!(urls.contains("https://site/ep1"));
        assert_eq!(urls.len(), 1);
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd src-tauri && cargo test db::tests:: && cd ..`
Expected: FAIL — methods `upsert_source`, `upsert_series`, etc. not found.

- [ ] **Step 3: Write the implementation**

Add these methods inside `impl Db` in `src-tauri/src/db.rs` (above the `#[cfg(test)]`):

```rust
    pub fn upsert_source(&self, name: &str, base_url: &str) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO sources(name, base_url) VALUES(?1, ?2)
             ON CONFLICT(base_url) DO UPDATE SET name=excluded.name",
            (name, base_url),
        )?;
        let id: i64 = self.conn.query_row(
            "SELECT id FROM sources WHERE base_url=?1",
            [base_url],
            |r| r.get(0),
        )?;
        Ok(id)
    }

    pub fn upsert_series(&self, source_id: i64, s: &crate::models::Series) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO series(source_id, slug, title, url, cover_url, is_airing)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(source_id, slug) DO UPDATE SET
                title=excluded.title, url=excluded.url,
                cover_url=excluded.cover_url, is_airing=excluded.is_airing",
            (
                source_id, &s.slug, &s.title, &s.url,
                &s.cover_url, s.is_airing as i64,
            ),
        )?;
        let id: i64 = self.conn.query_row(
            "SELECT id FROM series WHERE source_id=?1 AND slug=?2",
            (source_id, &s.slug),
            |r| r.get(0),
        )?;
        Ok(id)
    }

    pub fn set_followed(&self, series_id: i64, followed: bool) -> Result<()> {
        self.conn.execute(
            "UPDATE series SET followed=?1 WHERE id=?2",
            (followed as i64, series_id),
        )?;
        Ok(())
    }

    fn row_to_series(r: &rusqlite::Row) -> rusqlite::Result<crate::models::Series> {
        Ok(crate::models::Series {
            id: r.get("id")?,
            slug: r.get("slug")?,
            title: r.get("title")?,
            url: r.get("url")?,
            cover_url: r.get("cover_url")?,
            is_airing: r.get::<_, i64>("is_airing")? != 0,
            followed: r.get::<_, i64>("followed")? != 0,
        })
    }

    pub fn list_airing(&self, source_id: i64) -> Result<Vec<crate::models::Series>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, slug, title, url, cover_url, is_airing, followed
             FROM series WHERE source_id=?1 AND is_airing=1 ORDER BY title",
        )?;
        let rows = stmt
            .query_map([source_id], Self::row_to_series)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    pub fn list_followed(&self, source_id: i64) -> Result<Vec<crate::models::Series>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, slug, title, url, cover_url, is_airing, followed
             FROM series WHERE source_id=?1 AND followed=1 ORDER BY title",
        )?;
        let rows = stmt
            .query_map([source_id], Self::row_to_series)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    pub fn existing_episode_urls(&self, series_id: i64) -> Result<std::collections::HashSet<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT url FROM episodes WHERE series_id=?1")?;
        let urls = stmt
            .query_map([series_id], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<std::collections::HashSet<_>>>()?;
        Ok(urls)
    }

    /// Insert episode if its (series_id, url) is new. Returns the row id either way.
    pub fn insert_episode(&self, e: &crate::models::Episode) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO episodes(series_id, number, title, url, released_at, seen)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(series_id, url) DO NOTHING",
            (
                e.series_id, &e.number, &e.title, &e.url,
                &e.released_at, e.seen as i64,
            ),
        )?;
        let id: i64 = self.conn.query_row(
            "SELECT id FROM episodes WHERE series_id=?1 AND url=?2",
            (e.series_id, &e.url),
            |r| r.get(0),
        )?;
        Ok(id)
    }

    pub fn mark_seen(&self, episode_id: i64) -> Result<()> {
        self.conn
            .execute("UPDATE episodes SET seen=1 WHERE id=?1", [episode_id])?;
        Ok(())
    }

    pub fn pending_count(&self) -> Result<i64> {
        let n: i64 = self.conn.query_row(
            "SELECT count(*) FROM episodes WHERE seen=0",
            [],
            |r| r.get(0),
        )?;
        Ok(n)
    }

    /// Unseen episodes joined with their series, newest first.
    pub fn list_pending(&self) -> Result<Vec<(crate::models::Series, crate::models::Episode)>> {
        let mut stmt = self.conn.prepare(
            "SELECT s.id, s.slug, s.title, s.url, s.cover_url, s.is_airing, s.followed,
                    e.id, e.series_id, e.number, e.title, e.url, e.released_at, e.seen
             FROM episodes e JOIN series s ON s.id = e.series_id
             WHERE e.seen=0
             ORDER BY s.title, e.added_at DESC",
        )?;
        let rows = stmt
            .query_map([], |r| {
                let series = crate::models::Series {
                    id: r.get(0)?,
                    slug: r.get(1)?,
                    title: r.get(2)?,
                    url: r.get(3)?,
                    cover_url: r.get(4)?,
                    is_airing: r.get::<_, i64>(5)? != 0,
                    followed: r.get::<_, i64>(6)? != 0,
                };
                let ep = crate::models::Episode {
                    id: r.get(7)?,
                    series_id: r.get(8)?,
                    number: r.get(9)?,
                    title: r.get(10)?,
                    url: r.get(11)?,
                    released_at: r.get(12)?,
                    seen: r.get::<_, i64>(13)? != 0,
                };
                Ok((series, ep))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cd src-tauri && cargo test db::tests:: && cd ..`
Expected: all db tests pass.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/db.rs
git commit -m "feat: add DB CRUD for series, episodes, follows, pending"
```

---

## Task 5: New-episode diff logic

**Files:**
- Create: `src-tauri/src/diff.rs`
- Modify: `src-tauri/src/lib.rs` (add `mod diff;`)
- Test: inline `#[cfg(test)]` in `diff.rs`

- [ ] **Step 1: Write the failing test**

Create `src-tauri/src/diff.rs`:

```rust
use crate::models::Episode;
use std::collections::HashSet;

/// Return the subset of `scraped` whose url is not already known.
pub fn new_episodes(scraped: &[Episode], known_urls: &HashSet<String>) -> Vec<Episode> {
    scraped
        .iter()
        .filter(|e| !known_urls.contains(&e.url))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ep(url: &str) -> Episode {
        Episode {
            id: 0, series_id: 1, number: "1".into(), title: None,
            url: url.into(), released_at: None, seen: false,
        }
    }

    #[test]
    fn returns_only_unknown() {
        let scraped = vec![ep("a"), ep("b"), ep("c")];
        let known: HashSet<String> = ["a".to_string(), "b".to_string()].into_iter().collect();
        let out = new_episodes(&scraped, &known);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].url, "c");
    }

    #[test]
    fn empty_when_all_known() {
        let scraped = vec![ep("a")];
        let known: HashSet<String> = ["a".to_string()].into_iter().collect();
        assert!(new_episodes(&scraped, &known).is_empty());
    }

    #[test]
    fn all_new_when_nothing_known() {
        let scraped = vec![ep("a"), ep("b")];
        let known: HashSet<String> = HashSet::new();
        assert_eq!(new_episodes(&scraped, &known).len(), 2);
    }
}
```

Add to `src-tauri/src/lib.rs`:

```rust
mod diff;
```

- [ ] **Step 2: Run tests to verify they pass**

Run: `cd src-tauri && cargo test diff:: && cd ..`
Expected: 3 passed.

- [ ] **Step 3: Commit**

```bash
git add src-tauri/src/diff.rs src-tauri/src/lib.rs
git commit -m "feat: add new-episode diff logic"
```

---

## Task 6: Capture real HTML fixtures

The adapter selectors must match the real site. Cloudflare blocks headless fetches, but the site loads normally in your logged-in browser. Capture two fixtures manually.

**Files:**
- Create: `src-tauri/tests/fixtures/airing.html`
- Create: `src-tauri/tests/fixtures/series.html`

- [ ] **Step 1: Save the airing page**

In your normal Chrome, open `https://wwv.animeytx.net/anime-en-emision/`. Wait until the grid of shows renders. Open DevTools Console and run:

```js
copy(document.documentElement.outerHTML)
```

Paste the clipboard into `src-tauri/tests/fixtures/airing.html`.

- [ ] **Step 2: Save a series page**

Open a currently-airing series, e.g. `https://wwv.animeytx.net/tv/baki-dou/`. Wait until the episode list renders. In the Console run `copy(document.documentElement.outerHTML)` again and paste into `src-tauri/tests/fixtures/series.html`.

- [ ] **Step 3: Inspect the markup and note the selectors**

Open both fixtures and confirm the DooPlay-style structure. Record, in a scratch note, the actual values for:
- Airing: the repeating series card element, its title text node, its `<a href>` to the series page, and the poster `<img>` src attribute (may be `data-src` for lazy images).
- Series: the repeating episode `<li>`/row, its `<a href>` to the episode, its number text, its title text, and any date text.

You will plug these into Task 7. The plan seeds DooPlay defaults; adjust them to match what you saw here.

- [ ] **Step 4: Commit the fixtures**

```bash
git add src-tauri/tests/fixtures/airing.html src-tauri/tests/fixtures/series.html
git commit -m "test: add animeytx HTML fixtures"
```

---

## Task 7: SiteAdapter trait and AnimeytxAdapter

**Files:**
- Create: `src-tauri/src/adapter/mod.rs`
- Create: `src-tauri/src/adapter/animeytx.rs`
- Modify: `src-tauri/src/lib.rs` (add `mod adapter;`)
- Test: inline `#[cfg(test)]` in `animeytx.rs`

- [ ] **Step 1: Write the trait**

Create `src-tauri/src/adapter/mod.rs`:

```rust
use crate::models::{Episode, Series};
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
}
```

Add to `src-tauri/src/lib.rs`:

```rust
mod adapter;
```

- [ ] **Step 2: Write the failing test**

Create `src-tauri/src/adapter/animeytx.rs`:

```rust
use super::SiteAdapter;
use crate::models::{Episode, Series};
use anyhow::Result;
use scraper::{Html, Selector};

pub struct AnimeytxAdapter;

// NOTE: selectors below are DooPlay defaults. Confirm against the fixtures from
// Task 6 and adjust the literal strings if the real markup differs.
const AIRING_CARD: &str = "article.item.tvshows, article.item.movies, .items article";
const AIRING_LINK: &str = ".poster a.lnk-blk, .data h3 a, .poster a";
const AIRING_TITLE: &str = ".data h3, h3";
const AIRING_IMG: &str = ".poster img, img";

const EP_ROW: &str = "#seasons .se-c ul.episodios li, ul.episodios li";
const EP_LINK: &str = ".episodiotitle a, a";
const EP_NUMBER: &str = ".numerando";
const EP_TITLE: &str = ".episodiotitle a";
const EP_DATE: &str = ".date";

fn slug_from_url(url: &str) -> String {
    url.trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("")
        .to_string()
}

impl SiteAdapter for AnimeytxAdapter {
    fn airing_url(&self, base_url: &str) -> String {
        format!("{}/anime-en-emision/", base_url.trim_end_matches('/'))
    }

    fn parse_airing(&self, html: &str) -> Result<Vec<Series>> {
        let doc = Html::parse_document(html);
        let card_sel = Selector::parse(AIRING_CARD).unwrap();
        let link_sel = Selector::parse(AIRING_LINK).unwrap();
        let title_sel = Selector::parse(AIRING_TITLE).unwrap();
        let img_sel = Selector::parse(AIRING_IMG).unwrap();

        let mut out = Vec::new();
        for card in doc.select(&card_sel) {
            let link = card.select(&link_sel).next();
            let url = match link.and_then(|l| l.value().attr("href")) {
                Some(h) => h.to_string(),
                None => continue,
            };
            let title = card
                .select(&title_sel)
                .next()
                .map(|t| t.text().collect::<String>().trim().to_string())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| slug_from_url(&url));
            let cover_url = card.select(&img_sel).next().and_then(|i| {
                i.value()
                    .attr("data-src")
                    .or_else(|| i.value().attr("src"))
                    .map(|s| s.to_string())
            });
            out.push(Series {
                id: 0,
                slug: slug_from_url(&url),
                title,
                url,
                cover_url,
                is_airing: true,
                followed: false,
            });
        }
        Ok(out)
    }

    fn parse_series(&self, html: &str) -> Result<Vec<Episode>> {
        let doc = Html::parse_document(html);
        let row_sel = Selector::parse(EP_ROW).unwrap();
        let link_sel = Selector::parse(EP_LINK).unwrap();
        let num_sel = Selector::parse(EP_NUMBER).unwrap();
        let title_sel = Selector::parse(EP_TITLE).unwrap();
        let date_sel = Selector::parse(EP_DATE).unwrap();

        let mut out = Vec::new();
        for row in doc.select(&row_sel) {
            let url = match row
                .select(&link_sel)
                .next()
                .and_then(|l| l.value().attr("href"))
            {
                Some(h) => h.to_string(),
                None => continue,
            };
            let number = row
                .select(&num_sel)
                .next()
                .map(|n| n.text().collect::<String>().trim().to_string())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| slug_from_url(&url));
            let title = row
                .select(&title_sel)
                .next()
                .map(|t| t.text().collect::<String>().trim().to_string())
                .filter(|s| !s.is_empty());
            let released_at = row
                .select(&date_sel)
                .next()
                .map(|d| d.text().collect::<String>().trim().to_string())
                .filter(|s| !s.is_empty());
            out.push(Episode {
                id: 0,
                series_id: 0,
                number,
                title,
                url,
                released_at,
                seen: false,
            });
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn airing_url_is_built() {
        let a = AnimeytxAdapter;
        assert_eq!(
            a.airing_url("https://wwv.animeytx.net/"),
            "https://wwv.animeytx.net/anime-en-emision/"
        );
    }

    #[test]
    fn parses_airing_fixture() {
        let html = include_str!("../../tests/fixtures/airing.html");
        let out = AnimeytxAdapter.parse_airing(html).unwrap();
        assert!(!out.is_empty(), "expected at least one series");
        for s in &out {
            assert!(!s.url.is_empty());
            assert!(!s.slug.is_empty());
            assert!(!s.title.is_empty());
        }
    }

    #[test]
    fn parses_series_fixture() {
        let html = include_str!("../../tests/fixtures/series.html");
        let out = AnimeytxAdapter.parse_series(html).unwrap();
        assert!(!out.is_empty(), "expected at least one episode");
        for e in &out {
            assert!(!e.url.is_empty());
            assert!(!e.number.is_empty());
        }
    }
}
```

Add to `src-tauri/src/adapter/mod.rs` re-export if desired (already declared `pub mod animeytx;`).

- [ ] **Step 3: Run tests to verify they pass**

Run: `cd src-tauri && cargo test adapter:: && cd ..`
Expected: `airing_url_is_built` passes immediately. The two fixture tests pass **only if the seeded selectors match your fixtures**. If either asserts empty, open the fixture, find the real repeating element/classes (from your Task 6 notes), and edit the `const` selector strings until both fixture tests pass. Do not weaken the assertions.

- [ ] **Step 4: Commit**

```bash
git add src-tauri/src/adapter
git commit -m "feat: add SiteAdapter trait and AnimeytxAdapter with fixture tests"
```

---

## Task 8: Scraper engine (hidden WebView2 → rendered HTML)

This part cannot be unit-tested (needs a live browser + network). Implement, then verify manually.

**Files:**
- Create: `src-tauri/src/scraper_engine.rs`
- Modify: `src-tauri/src/lib.rs` (add `mod scraper_engine;`)

- [ ] **Step 1: Implement the engine**

Create `src-tauri/src/scraper_engine.rs`:

```rust
use anyhow::{anyhow, Result};
use std::sync::mpsc;
use std::time::Duration;
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};

/// Load `url` in a hidden webview, wait for it to settle (letting Cloudflare
/// and JS run), then return the rendered outer HTML.
pub async fn fetch_html(app: &AppHandle, url: &str) -> Result<String> {
    let label = format!("scraper-{}", uuid_like());
    let window = WebviewWindowBuilder::new(
        app,
        &label,
        WebviewUrl::External(url.parse().map_err(|_| anyhow!("bad url: {url}"))?),
    )
    .visible(false)
    .build()?;

    // Give the page time to pass Cloudflare and render.
    tokio::time::sleep(Duration::from_secs(6)).await;

    let (tx, rx) = mpsc::channel::<String>();
    let script = r#"
        window.__ANIMEONTRACK_HTML__ = document.documentElement.outerHTML;
    "#;
    window.eval(script)?;

    // Read it back.
    let window2 = window.clone();
    let (tx2, rx2) = mpsc::channel::<String>();
    window2.eval(
        r#"window.__TAURI_INTERNALS__.invoke('__noop__', {}); "#,
    ).ok();
    // Simpler: use eval with a callback via `run_on_main_thread` is not needed;
    // instead re-read through a dedicated command below.
    let _ = tx; // silence unused in this simplified path
    let _ = rx;
    let _ = tx2;
    let _ = rx2;

    // Pull the stored HTML via a synchronous eval round-trip.
    let html = read_global(&window).await?;
    window.close().ok();
    Ok(html)
}

async fn read_global(window: &tauri::WebviewWindow) -> Result<String> {
    // Poll the JS global we set; return it via an emitted event.
    use std::sync::{Arc, Mutex};
    let slot: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let slot2 = slot.clone();
    let win = window.clone();

    win.listen("animeontrack://html", move |event| {
        if let Ok(s) = serde_json::from_str::<String>(event.payload()) {
            *slot2.lock().unwrap() = Some(s);
        }
    });
    window.eval(
        r#"window.__TAURI__.event.emit('animeontrack://html', window.__ANIMEONTRACK_HTML__);"#,
    )?;

    for _ in 0..50 {
        if let Some(h) = slot.lock().unwrap().take() {
            return Ok(h);
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Err(anyhow!("timed out reading page HTML"))
}

fn uuid_like() -> u128 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()
}
```

> Implementation note: Tauri's `eval` is fire-and-forget, so the round-trip uses the JS event bus (`window.__TAURI__.event.emit`) to hand the HTML back to Rust, where a `listen` handler captures it. Ensure `withGlobalTauri` is enabled so `window.__TAURI__` exists — set `"app": { "withGlobalTauri": true }` in `src-tauri/tauri.conf.json`. Remove the dead `tx/rx` scaffolding lines once the event path is confirmed working; they are left as an explicit marker that the simple channel approach does not work across the JS/Rust boundary.

Add to `src-tauri/src/lib.rs`:

```rust
mod scraper_engine;
```

- [ ] **Step 2: Enable global Tauri in config**

Edit `src-tauri/tauri.conf.json`, in the `"app"` object add:

```json
"withGlobalTauri": true
```

- [ ] **Step 3: Manual verification (deferred to Task 11)**

The engine is exercised end-to-end by the `scan_airing` command in Task 11. Just confirm it compiles now:

Run: `cd src-tauri && cargo build && cd ..`
Expected: compiles (warnings about unused ok).

- [ ] **Step 4: Commit**

```bash
git add src-tauri/src/scraper_engine.rs src-tauri/src/lib.rs src-tauri/tauri.conf.json
git commit -m "feat: add hidden-webview scraper engine"
```

---

## Task 9: Player interface

**Files:**
- Create: `src-tauri/src/player.rs`
- Modify: `src-tauri/src/lib.rs` (add `mod player;`)

- [ ] **Step 1: Implement trait + BrowserPlayer**

Create `src-tauri/src/player.rs`:

```rust
use crate::models::Episode;
use anyhow::Result;
use tauri::AppHandle;
use tauri_plugin_shell::ShellExt;

/// Strategy for "watching" an episode. v1 opens the browser; a future
/// EmbeddedPlayer can implement the same trait.
pub trait EpisodePlayer {
    fn open(&self, app: &AppHandle, episode: &Episode) -> Result<()>;
}

pub struct BrowserPlayer;

impl EpisodePlayer for BrowserPlayer {
    fn open(&self, app: &AppHandle, episode: &Episode) -> Result<()> {
        app.shell().open(&episode.url, None)?;
        Ok(())
    }
}
```

Add to `src-tauri/src/lib.rs`:

```rust
mod player;
```

- [ ] **Step 2: Verify it compiles**

Run: `cd src-tauri && cargo build && cd ..`
Expected: compiles.

- [ ] **Step 3: Commit**

```bash
git add src-tauri/src/player.rs src-tauri/src/lib.rs
git commit -m "feat: add EpisodePlayer trait and BrowserPlayer"
```

---

## Task 10: App state + Tauri command wiring

**Files:**
- Create: `src-tauri/src/commands.rs`
- Modify: `src-tauri/src/lib.rs` (state, plugin, handler registration)

- [ ] **Step 1: Define shared state and commands**

Create `src-tauri/src/commands.rs`:

```rust
use crate::adapter::{animeytx::AnimeytxAdapter, SiteAdapter};
use crate::db::Db;
use crate::diff::new_episodes;
use crate::models::{Episode, Series};
use crate::player::{BrowserPlayer, EpisodePlayer};
use crate::scraper_engine::fetch_html;
use std::sync::Mutex;
use tauri::{AppHandle, State};

pub struct AppState {
    pub db: Mutex<Db>,
    pub source_id: Mutex<Option<i64>>,
}

const SOURCE_NAME: &str = "AnimeYT";

fn adapter() -> AnimeytxAdapter {
    AnimeytxAdapter
}

fn get_source_id(state: &State<AppState>) -> Result<i64, String> {
    state
        .source_id
        .lock()
        .unwrap()
        .ok_or_else(|| "no source configured; run scan_airing first".to_string())
}

/// First-run + manual re-scan: store base_url, scrape airing list, upsert series.
#[tauri::command]
pub async fn scan_airing(
    app: AppHandle,
    state: State<'_, AppState>,
    base_url: String,
) -> Result<Vec<Series>, String> {
    let a = adapter();
    let url = a.airing_url(&base_url);
    let html = fetch_html(&app, &url).await.map_err(|e| e.to_string())?;
    let series = a.parse_airing(&html).map_err(|e| e.to_string())?;
    if series.is_empty() {
        return Err("no series parsed; site layout may have changed".into());
    }
    let db = state.db.lock().unwrap();
    let src = db
        .upsert_source(SOURCE_NAME, base_url.trim_end_matches('/'))
        .map_err(|e| e.to_string())?;
    for s in &series {
        db.upsert_series(src, s).map_err(|e| e.to_string())?;
    }
    *state.source_id.lock().unwrap() = Some(src);
    db.list_airing(src).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn list_airing(state: State<'_, AppState>) -> Result<Vec<Series>, String> {
    let src = get_source_id(&state)?;
    let db = state.db.lock().unwrap();
    db.list_airing(src).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn set_followed(
    state: State<'_, AppState>,
    series_id: i64,
    followed: bool,
) -> Result<(), String> {
    let db = state.db.lock().unwrap();
    db.set_followed(series_id, followed).map_err(|e| e.to_string())
}

/// For each followed series: scrape its page, insert new episodes. Returns count of new episodes.
#[tauri::command]
pub async fn refresh(app: AppHandle, state: State<'_, AppState>) -> Result<i64, String> {
    let src = get_source_id(&state)?;
    let followed = {
        let db = state.db.lock().unwrap();
        db.list_followed(src).map_err(|e| e.to_string())?
    };
    let a = adapter();
    let mut total_new = 0i64;
    for s in followed {
        let html = match fetch_html(&app, &s.url).await {
            Ok(h) => h,
            Err(_) => continue, // unreachable series: skip, keep cached data
        };
        let scraped = match a.parse_series(&html) {
            Ok(eps) => eps,
            Err(_) => continue, // layout change: skip, don't wipe
        };
        let db = state.db.lock().unwrap();
        let known = db.existing_episode_urls(s.id).map_err(|e| e.to_string())?;
        for mut e in new_episodes(&scraped, &known) {
            e.series_id = s.id;
            db.insert_episode(&e).map_err(|e| e.to_string())?;
            total_new += 1;
        }
        drop(db);
        // polite delay between series
        tokio::time::sleep(std::time::Duration::from_millis(800)).await;
    }
    Ok(total_new)
}

#[tauri::command]
pub fn list_pending(state: State<'_, AppState>) -> Result<Vec<PendingItem>, String> {
    let db = state.db.lock().unwrap();
    let rows = db.list_pending().map_err(|e| e.to_string())?;
    Ok(rows
        .into_iter()
        .map(|(s, e)| PendingItem { series: s, episode: e })
        .collect())
}

#[derive(serde::Serialize)]
pub struct PendingItem {
    pub series: Series,
    pub episode: Episode,
}

#[tauri::command]
pub fn pending_count(state: State<'_, AppState>) -> Result<i64, String> {
    let db = state.db.lock().unwrap();
    db.pending_count().map_err(|e| e.to_string())
}

/// Open an episode in the browser and mark it seen.
#[tauri::command]
pub fn open_episode(
    app: AppHandle,
    state: State<'_, AppState>,
    episode_id: i64,
    url: String,
) -> Result<(), String> {
    let ep = Episode {
        id: episode_id,
        series_id: 0,
        number: String::new(),
        title: None,
        url,
        released_at: None,
        seen: false,
    };
    BrowserPlayer.open(&app, &ep).map_err(|e| e.to_string())?;
    let db = state.db.lock().unwrap();
    db.mark_seen(episode_id).map_err(|e| e.to_string())
}
```

- [ ] **Step 2: Wire state, plugin, and handlers in lib.rs**

Edit `src-tauri/src/lib.rs`. Ensure the module declarations exist (from earlier tasks) and replace the generated `run()` with:

```rust
mod adapter;
mod commands;
mod db;
mod diff;
mod models;
mod player;
mod scraper_engine;

use commands::AppState;
use std::sync::Mutex;
use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .setup(|app| {
            let dir = app.path().app_data_dir().expect("app data dir");
            std::fs::create_dir_all(&dir).ok();
            let db_path = dir.join("animeontrack.sqlite");
            let db = db::Db::open(db_path.to_str().unwrap()).expect("open db");
            // Restore last source id if a single source exists.
            let source_id: Option<i64> = db
                .conn
                .query_row("SELECT id FROM sources LIMIT 1", [], |r| r.get(0))
                .ok();
            app.manage(AppState {
                db: Mutex::new(db),
                source_id: Mutex::new(source_id),
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::scan_airing,
            commands::list_airing,
            commands::set_followed,
            commands::refresh,
            commands::list_pending,
            commands::pending_count,
            commands::open_episode,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
```

- [ ] **Step 3: Verify it compiles**

Run: `cd src-tauri && cargo build && cd ..`
Expected: compiles. Fix any borrow/lock issues the compiler flags (e.g. holding the `MutexGuard` across an `.await` — the code deliberately drops guards before awaits).

- [ ] **Step 4: Run all Rust tests**

Run: `cd src-tauri && cargo test && cd ..`
Expected: all previous tests still pass.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/commands.rs src-tauri/src/lib.rs
git commit -m "feat: add app state and Tauri command surface"
```

---

## Task 11: Manual end-to-end scrape check

**Files:** none (verification only).

- [ ] **Step 1: Add a temporary dev button**

In `src/App.tsx`, temporarily add a button that calls `invoke('scan_airing', { baseUrl: 'https://wwv.animeytx.net' })` and `console.log`s the result. (This is scaffolding; the real UI is Task 13.)

- [ ] **Step 2: Run and observe**

Run: `npm run tauri dev`
Click the button. Expected: after a few seconds, the console logs a non-empty array of series with titles and urls. If it errors with "no series parsed", the live markup differs from the fixture — re-capture the fixture (Task 6) and fix selectors (Task 7).

- [ ] **Step 3: Remove the temporary button**

Revert the scaffolding edit to `src/App.tsx`.

- [ ] **Step 4: Commit (if any real fixes were made to selectors/engine)**

```bash
git add -A
git commit -m "fix: adjust scraper/selectors after live verification"
```

---

## Task 12: Frontend API layer and types

**Files:**
- Create: `src/types.ts`
- Create: `src/api.ts`

- [ ] **Step 1: Types**

Create `src/types.ts`:

```ts
export interface Series {
  id: number;
  slug: string;
  title: string;
  url: string;
  cover_url: string | null;
  is_airing: boolean;
  followed: boolean;
}

export interface Episode {
  id: number;
  series_id: number;
  number: string;
  title: string | null;
  url: string;
  released_at: string | null;
  seen: boolean;
}

export interface PendingItem {
  series: Series;
  episode: Episode;
}
```

- [ ] **Step 2: API wrappers**

Create `src/api.ts`:

```ts
import { invoke } from "@tauri-apps/api/core";
import type { Series, PendingItem } from "./types";

export const scanAiring = (baseUrl: string) =>
  invoke<Series[]>("scan_airing", { baseUrl });

export const listAiring = () => invoke<Series[]>("list_airing");

export const setFollowed = (seriesId: number, followed: boolean) =>
  invoke<void>("set_followed", { seriesId, followed });

export const refresh = () => invoke<number>("refresh");

export const listPending = () => invoke<PendingItem[]>("list_pending");

export const pendingCount = () => invoke<number>("pending_count");

export const openEpisode = (episodeId: number, url: string) =>
  invoke<void>("open_episode", { episodeId, url });
```

- [ ] **Step 3: Verify types**

Run: `npx tsc --noEmit`
Expected: no type errors.

- [ ] **Step 4: Commit**

```bash
git add src/types.ts src/api.ts
git commit -m "feat: add frontend API layer and types"
```

---

## Task 13: UI views

**Files:**
- Create: `src/views/Onboarding.tsx`, `src/views/AiringGrid.tsx`, `src/views/Pending.tsx`, `src/views/SeriesDetail.tsx`, `src/views/Settings.tsx`
- Modify: `src/App.tsx`

- [ ] **Step 1: Onboarding view**

Create `src/views/Onboarding.tsx`:

```tsx
import { useState } from "react";
import { scanAiring } from "../api";

export function Onboarding({ onDone }: { onDone: () => void }) {
  const [url, setUrl] = useState("https://wwv.animeytx.net");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function submit() {
    setBusy(true);
    setError(null);
    try {
      await scanAiring(url.trim());
      onDone();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <div style={{ padding: 24, maxWidth: 480 }}>
      <h1>AnimeOnTrack</h1>
      <p>Enter the site URL to scan airing anime.</p>
      <input
        value={url}
        onChange={(e) => setUrl(e.target.value)}
        style={{ width: "100%", padding: 8 }}
      />
      <button disabled={busy} onClick={submit} style={{ marginTop: 12 }}>
        {busy ? "Scanning…" : "Scan"}
      </button>
      {error && <p style={{ color: "crimson" }}>{error}</p>}
    </div>
  );
}
```

- [ ] **Step 2: Airing grid view**

Create `src/views/AiringGrid.tsx`:

```tsx
import { useEffect, useState } from "react";
import { listAiring, setFollowed } from "../api";
import type { Series } from "../types";

export function AiringGrid() {
  const [series, setSeries] = useState<Series[]>([]);

  async function load() {
    setSeries(await listAiring());
  }
  useEffect(() => {
    load();
  }, []);

  async function toggle(s: Series) {
    await setFollowed(s.id, !s.followed);
    await load();
  }

  return (
    <div style={{ display: "grid", gridTemplateColumns: "repeat(auto-fill,minmax(160px,1fr))", gap: 12, padding: 16 }}>
      {series.map((s) => (
        <div key={s.id} style={{ border: "1px solid #ccc", borderRadius: 8, padding: 8 }}>
          {s.cover_url && <img src={s.cover_url} style={{ width: "100%", borderRadius: 4 }} />}
          <div style={{ fontSize: 13, margin: "6px 0" }}>{s.title}</div>
          <button onClick={() => toggle(s)}>
            {s.followed ? "Following ✓" : "Follow"}
          </button>
        </div>
      ))}
    </div>
  );
}
```

- [ ] **Step 3: Pending view**

Create `src/views/Pending.tsx`:

```tsx
import { useEffect, useState } from "react";
import { listPending, openEpisode } from "../api";
import type { PendingItem } from "../types";

export function Pending() {
  const [items, setItems] = useState<PendingItem[]>([]);

  async function load() {
    setItems(await listPending());
  }
  useEffect(() => {
    load();
  }, []);

  async function watch(it: PendingItem) {
    await openEpisode(it.episode.id, it.episode.url);
    await load();
  }

  // group by series title
  const groups = new Map<string, PendingItem[]>();
  for (const it of items) {
    const k = it.series.title;
    (groups.get(k) ?? groups.set(k, []).get(k)!).push(it);
  }

  return (
    <div style={{ padding: 16 }}>
      <h2>Pending ({items.length})</h2>
      {[...groups.entries()].map(([title, eps]) => (
        <div key={title} style={{ marginBottom: 16 }}>
          <h3 style={{ margin: "8px 0" }}>
            {title} <span style={{ color: "#888" }}>({eps.length})</span>
          </h3>
          {eps.map((it) => (
            <div
              key={it.episode.id}
              onClick={() => watch(it)}
              style={{ cursor: "pointer", padding: "6px 8px", borderBottom: "1px solid #eee" }}
            >
              {it.episode.number} {it.episode.title ?? ""}
            </div>
          ))}
        </div>
      ))}
      {items.length === 0 && <p>No pending episodes. Hit refresh.</p>}
    </div>
  );
}
```

- [ ] **Step 4: Series detail view**

Create `src/views/SeriesDetail.tsx`:

```tsx
// Minimal placeholder detail: shows a series title. Expand later to list its
// episode history. Kept intentionally small for v1.
import type { Series } from "../types";

export function SeriesDetail({ series }: { series: Series }) {
  return (
    <div style={{ padding: 16 }}>
      <h2>{series.title}</h2>
      <a href={series.url} target="_blank" rel="noreferrer">
        Open series page
      </a>
    </div>
  );
}
```

- [ ] **Step 5: Settings view**

Create `src/views/Settings.tsx`:

```tsx
import { useState } from "react";
import { scanAiring } from "../api";

export function Settings() {
  const [url, setUrl] = useState("https://wwv.animeytx.net");
  const [msg, setMsg] = useState<string | null>(null);

  async function rescan() {
    setMsg("Scanning…");
    try {
      const s = await scanAiring(url.trim());
      setMsg(`Found ${s.length} airing series.`);
    } catch (e) {
      setMsg(String(e));
    }
  }

  return (
    <div style={{ padding: 16 }}>
      <h2>Settings</h2>
      <label>Source URL</label>
      <input value={url} onChange={(e) => setUrl(e.target.value)} style={{ width: "100%", padding: 8 }} />
      <button onClick={rescan} style={{ marginTop: 8 }}>Re-scan airing</button>
      {msg && <p>{msg}</p>}
    </div>
  );
}
```

- [ ] **Step 6: App shell with view switching + refresh-on-open**

Replace `src/App.tsx` with:

```tsx
import { useEffect, useState } from "react";
import { Onboarding } from "./views/Onboarding";
import { AiringGrid } from "./views/AiringGrid";
import { Pending } from "./views/Pending";
import { Settings } from "./views/Settings";
import { listAiring, refresh } from "./api";

type View = "loading" | "onboarding" | "pending" | "airing" | "settings";

export default function App() {
  const [view, setView] = useState<View>("loading");

  // Decide first screen: onboarding if no source yet, else pending.
  useEffect(() => {
    (async () => {
      try {
        await listAiring(); // throws if no source configured
        await refresh().catch(() => 0); // refresh-on-open, best effort
        setView("pending");
      } catch {
        setView("onboarding");
      }
    })();
  }, []);

  if (view === "loading") return <div style={{ padding: 16 }}>Loading…</div>;
  if (view === "onboarding")
    return <Onboarding onDone={() => setView("airing")} />;

  return (
    <div>
      <nav style={{ display: "flex", gap: 8, padding: 8, borderBottom: "1px solid #ccc" }}>
        <button onClick={() => setView("pending")}>Pending</button>
        <button onClick={() => setView("airing")}>Airing</button>
        <button onClick={() => setView("settings")}>Settings</button>
        <button onClick={async () => { await refresh(); setView("pending"); }}>Refresh</button>
      </nav>
      {view === "pending" && <Pending />}
      {view === "airing" && <AiringGrid />}
      {view === "settings" && <Settings />}
    </div>
  );
}
```

- [ ] **Step 7: Type-check and run**

Run: `npx tsc --noEmit`
Expected: no errors.

Run: `npm run tauri dev`
Expected: first launch shows Onboarding. After scanning + following a series + Refresh, the Pending tab lists new episodes; clicking one opens the browser and removes it from the list.

- [ ] **Step 8: Commit**

```bash
git add src
git commit -m "feat: add UI views and app shell"
```

---

## Task 14: Final full verification

**Files:** none.

- [ ] **Step 1: Rust tests**

Run: `cd src-tauri && cargo test && cd ..`
Expected: all pass.

- [ ] **Step 2: Type check**

Run: `npx tsc --noEmit`
Expected: clean.

- [ ] **Step 3: Full manual smoke**

Run: `npm run tauri dev`. Walk the full flow: onboarding scan → follow 2–3 series → refresh → pending populates with a total count → click an episode → browser opens and count decrements → restart app → data persists and refresh-on-open runs.

- [ ] **Step 4: Commit any fixes**

```bash
git add -A
git commit -m "chore: final verification fixes"
```

---

## Self-review notes (addressed)

- **Spec coverage:** onboarding+scan (T11/T13), follow (T10/T13), refresh+diff (T5/T10), pending list with total counter (T4/T13), browser playback + mark seen (T9/T10/T13), pluggable adapter (T7), error handling for unreachable/layout-change/offline (T10 `refresh` skips and preserves data; `scan_airing` errors on empty parse), SQLite persistence (T3/T4), testing via fixtures (T6/T7). Series-detail is intentionally minimal for v1 (spec lists it as a screen; full episode history deferred, noted in T13 Step 4).
- **Type consistency:** Rust `Series`/`Episode`/`PendingItem` fields match `src/types.ts`; command names in `generate_handler!` match `src/api.ts` invoke strings (`scan_airing`, `list_airing`, `set_followed`, `refresh`, `list_pending`, `pending_count`, `open_episode`).
- **Known risk:** Task 8's webview→Rust HTML round-trip is the most fragile piece; T11 verifies it live. If the event round-trip proves unreliable, the fallback is to serve the extractor via a registered Tauri command the injected JS calls directly.
