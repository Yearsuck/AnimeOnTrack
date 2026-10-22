//! Data for the series detail view's info section that the catalog sync does
//! not carry: the site's own synopsis, the AniList description/relations
//! cache, and the "Relacionados" row (related titles joined to the user's own
//! series rows so a card can be opened and marked followed/watched).

use super::*;
use crate::anilist::{MediaExtras, RelatedMedia};
use std::collections::HashMap;

/// Longest site synopsis kept. Real ones are a paragraph or two; this only
/// guards against a selector that matched a whole page of text.
const MAX_SYNOPSIS_CHARS: usize = 4000;

/// One related title for the detail view.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct RelatedAnime {
    pub anilist_id: i64,
    /// AniList's vocabulary as-is (`PREQUEL`, `SIDE_STORY`, ...); the
    /// frontend translates it.
    pub relation_type: String,
    pub title: String,
    pub cover_url: Option<String>,
    pub format: Option<String>,
    pub year: Option<i64>,
    pub url: String,
    /// The user's own row for this title (any site), to open it in the
    /// detail view. `None` when the title has never been seen in the app.
    pub series: Option<crate::models::Series>,
    pub followed: bool,
    /// Marked "Ya lo vi", or every scraped episode seen.
    pub watched: bool,
}

/// Display order: the watch order first (prequel, sequel), then the parent
/// show and the extras, then the loosely related.
fn relation_rank(relation_type: &str) -> u8 {
    match relation_type {
        "PREQUEL" => 0,
        "SEQUEL" => 1,
        "PARENT" => 2,
        "SIDE_STORY" => 3,
        "SPIN_OFF" => 4,
        "ALTERNATIVE" => 5,
        "SUMMARY" => 6,
        "COMPILATION" => 7,
        "CONTAINS" => 8,
        "CHARACTER" => 9,
        "OTHER" => 10,
        _ => 11,
    }
}

fn year_of(ts: Option<i64>) -> Option<i64> {
    use chrono::Datelike;
    ts.and_then(|t| chrono::DateTime::from_timestamp(t, 0)).map(|d| d.year() as i64)
}

impl Db {
    /// Store the site's synopsis for a series. Blank text is ignored, so a
    /// page whose synopsis selector stopped matching never erases a good one.
    pub fn set_series_synopsis(&self, series_id: i64, synopsis: &str) -> Result<()> {
        let trimmed = synopsis.trim();
        if trimmed.is_empty() {
            return Ok(());
        }
        let capped: String = trimmed.chars().take(MAX_SYNOPSIS_CHARS).collect();
        self.conn.execute("UPDATE series SET synopsis=?1 WHERE id=?2", (capped, series_id))?;
        Ok(())
    }

    /// The site synopsis of this row, else of a canonical twin on another
    /// site (the same show is often only parsed on one of them).
    pub fn series_synopsis(&self, series_id: i64) -> Result<Option<String>> {
        let own: Option<String> = self
            .conn
            .query_row("SELECT synopsis FROM series WHERE id=?1", [series_id], |r| r.get(0))
            .optional()?
            .flatten();
        if own.is_some() {
            return Ok(own);
        }
        for id in self.canonical_series_ids(series_id)? {
            if id == series_id {
                continue;
            }
            let s: Option<String> =
                self.conn.query_row("SELECT synopsis FROM series WHERE id=?1", [id], |r| r.get(0)).optional()?.flatten();
            if s.is_some() {
                return Ok(s);
            }
        }
        Ok(None)
    }

    /// Display name of the site a series row was scraped from (`None` for an
    /// unknown site id).
    pub fn series_site_name(&self, series_id: i64) -> Result<Option<String>> {
        let site_id: Option<String> = self
            .conn
            .query_row(
                "SELECT so.site_id FROM series s JOIN sources so ON so.id = s.source_id WHERE s.id=?1",
                [series_id],
                |r| r.get(0),
            )
            .optional()?
            .flatten();
        Ok(site_id.and_then(|id| {
            crate::adapter::all_sites().iter().find(|s| s.id == id).map(|s| s.name.to_string())
        }))
    }

    /// Cached AniList extras and when they were fetched (unix seconds).
    pub fn cached_media_extras(&self, anilist_id: i64) -> Result<Option<(MediaExtras, i64)>> {
        let row: Option<(Option<String>, String, i64)> = self
            .conn
            .query_row(
                "SELECT description, relations_json, fetched_at FROM anilist_media_extras WHERE anilist_id=?1",
                [anilist_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        Ok(row.map(|(description, json, fetched_at)| {
            let relations: Vec<RelatedMedia> = serde_json::from_str(&json).unwrap_or_default();
            (MediaExtras { description, relations }, fetched_at)
        }))
    }

    /// Cache a fetch result. `None` (AniList has no such title) is stored as
    /// an empty entry so the next open does not ask again.
    pub fn store_media_extras(&self, anilist_id: i64, extras: Option<&MediaExtras>, now: i64) -> Result<()> {
        let (description, relations) = match extras {
            Some(e) => (e.description.clone(), serde_json::to_string(&e.relations)?),
            None => (None, "[]".to_string()),
        };
        self.conn.execute(
            "INSERT INTO anilist_media_extras(anilist_id, description, relations_json, fetched_at) VALUES(?1, ?2, ?3, ?4)
             ON CONFLICT(anilist_id) DO UPDATE SET description=excluded.description,
                relations_json=excluded.relations_json, fetched_at=excluded.fetched_at",
            (anilist_id, description, relations, now),
        )?;
        Ok(())
    }

    /// Related titles of `anilist_id`: the fetched relation list (every type)
    /// merged with the PREQUEL/SEQUEL edges the catalog sync stored, so the
    /// row still works offline or before the first fetch. Local catalog data
    /// (larger cover, canonical title) wins over the fetched node's, and each
    /// entry is joined to the user's own series rows. Ordered prequel →
    /// sequel → the rest, then by year.
    pub fn related_anime(&self, anilist_id: i64, fetched: &[RelatedMedia]) -> Result<Vec<RelatedAnime>> {
        let mut edges: Vec<RelatedMedia> = fetched.to_vec();
        {
            let mut stmt = self
                .conn
                .prepare("SELECT related_id, relation_type FROM anilist_catalog_relations WHERE anilist_id=?1")?;
            let local = stmt
                .query_map([anilist_id], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            for (id, rel) in local {
                if !edges.iter().any(|e| e.id == id) {
                    edges.push(RelatedMedia { id, relation_type: rel, title: String::new(), cover_url: None, format: None, year: None });
                }
            }
        }
        // One entry per title, keeping its most specific relation.
        edges.sort_by_key(|e| relation_rank(&e.relation_type));
        let mut seen_ids = std::collections::HashSet::new();
        edges.retain(|e| e.id != anilist_id && seen_ids.insert(e.id));
        if edges.is_empty() {
            return Ok(Vec::new());
        }

        // al:{id} -> member series ids, from the library's identity bridge.
        let mut members: HashMap<i64, Vec<i64>> = HashMap::new();
        for (sid, key) in self.canonical_group_keys()? {
            if let Some(al) = key.strip_prefix("al:").and_then(|n| n.parse::<i64>().ok()) {
                members.entry(al).or_default().push(sid);
            }
        }

        let mut out = Vec::with_capacity(edges.len());
        for e in edges {
            type LocalRow = (String, Option<String>, Option<String>, Option<i64>, String);
            let local: Option<LocalRow> = self
                .conn
                .query_row(
                    "SELECT title, cover_url, format, start_date, url FROM anilist_catalog WHERE id=?1",
                    [e.id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
                )
                .optional()?;
            let (title, cover_url, format, year, url) = match local {
                Some((t, c, f, sd, u)) => (t, c.or(e.cover_url), f.or(e.format), year_of(sd).or(e.year), u),
                None => (e.title, e.cover_url, e.format, e.year, format!("https://anilist.co/anime/{}", e.id)),
            };
            if title.trim().is_empty() {
                continue; // a stored edge to a title neither synced nor fetched
            }

            let mut followed = false;
            let mut watched = false;
            // (followed, seen episodes, -id) picks the row to open.
            let mut best: Option<((bool, i64, i64), i64)> = None;
            for &sid in members.get(&e.id).map(|v| v.as_slice()).unwrap_or(&[]) {
                let (f, w, total, seen): (bool, bool, i64, i64) = self.conn.query_row(
                    "SELECT s.followed, COALESCE(s.watched_externally, 0),
                            (SELECT COUNT(*) FROM episodes WHERE series_id=s.id),
                            (SELECT COUNT(*) FROM episodes WHERE series_id=s.id AND seen=1)
                     FROM series s WHERE s.id=?1",
                    [sid],
                    |r| Ok((r.get::<_, i64>(0)? != 0, r.get::<_, i64>(1)? != 0, r.get(2)?, r.get(3)?)),
                )?;
                followed |= f;
                watched |= w || (total > 0 && seen == total);
                let score = (f, seen, -sid);
                if best.is_none_or(|(b, _)| score > b) {
                    best = Some((score, sid));
                }
            }
            let series = match best {
                Some((_, sid)) => Some(self.conn.query_row(
                    "SELECT * FROM series WHERE id=?1",
                    [sid],
                    Self::row_to_series,
                )?),
                None => None,
            };
            out.push(RelatedAnime {
                anilist_id: e.id,
                relation_type: e.relation_type,
                title,
                cover_url,
                format,
                year,
                url,
                series,
                followed,
                watched,
            });
        }
        out.sort_by(|a, b| {
            relation_rank(&a.relation_type)
                .cmp(&relation_rank(&b.relation_type))
                .then(a.year.unwrap_or(i64::MAX).cmp(&b.year.unwrap_or(i64::MAX)))
                .then(a.anilist_id.cmp(&b.anilist_id))
        });
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_support::*;

    fn rel(id: i64, t: &str, title: &str) -> RelatedMedia {
        RelatedMedia { id, relation_type: t.into(), title: title.into(), cover_url: None, format: Some("TV".into()), year: None }
    }

    #[test]
    fn synopsis_is_stored_trimmed_never_blanked_and_shared_with_a_twin() {
        let db = Db::open(":memory:").unwrap();
        let a = db.upsert_source("AnimeYT", "a", "animeytx").unwrap();
        let b = db.upsert_source("TioAnime", "b", "tioanime").unwrap();
        let sa = db.upsert_series(a, &mk_airing("x-a", "Show X", None)).unwrap();
        let sb = db.upsert_series(b, &mk_airing("x-b", "Show X", None)).unwrap();
        assert_eq!(db.series_synopsis(sa).unwrap(), None);

        db.set_series_synopsis(sa, "  Una historia.  ").unwrap();
        db.set_series_synopsis(sa, "   ").unwrap();
        assert_eq!(db.series_synopsis(sa).unwrap().as_deref(), Some("Una historia."));
        assert_eq!(db.series_synopsis(sb).unwrap().as_deref(), Some("Una historia."), "twin on another site");

        let long: String = "ñ".repeat(MAX_SYNOPSIS_CHARS + 10);
        db.set_series_synopsis(sb, &long).unwrap();
        assert_eq!(db.series_synopsis(sb).unwrap().unwrap().chars().count(), MAX_SYNOPSIS_CHARS);
    }

    #[test]
    fn site_name_comes_from_the_source_site_id() {
        let db = Db::open(":memory:").unwrap();
        let a = db.upsert_source("whatever", "a", "tioanime").unwrap();
        let s = db.upsert_series(a, &mk_airing("x", "X", None)).unwrap();
        assert_eq!(db.series_site_name(s).unwrap().as_deref(), Some("TioAnime"));
    }

    #[test]
    fn media_extras_cache_round_trips_and_caches_a_miss() {
        let db = Db::open(":memory:").unwrap();
        assert_eq!(db.cached_media_extras(5).unwrap(), None);
        let e = MediaExtras { description: Some("Desc".into()), relations: vec![rel(6, "SEQUEL", "Six")] };
        db.store_media_extras(5, Some(&e), 100).unwrap();
        assert_eq!(db.cached_media_extras(5).unwrap(), Some((e, 100)));
        db.store_media_extras(5, None, 200).unwrap();
        assert_eq!(
            db.cached_media_extras(5).unwrap(),
            Some((MediaExtras { description: None, relations: vec![] }, 200))
        );
    }

    #[test]
    fn related_anime_orders_merges_local_edges_and_marks_the_users_rows() {
        let db = Db::open(":memory:").unwrap();
        // Base 10 with a locally stored PREQUEL edge to 9 (synced) and to 99
        // (never synced, never fetched: dropped).
        let mut base = catalog_anime(10, "Base", &[]);
        base.relations = vec![
            crate::anilist::CatalogRelation { related_id: 9, relation_type: "PREQUEL".into() },
            crate::anilist::CatalogRelation { related_id: 99, relation_type: "PREQUEL".into() },
        ];
        db.upsert_catalog_anime(&base, 0).unwrap();
        let mut prequel = catalog_anime(9, "Base Prequel", &[]);
        prequel.cover_url = Some("https://local/9".into());
        db.upsert_catalog_anime(&prequel, 1).unwrap();

        let src = db.upsert_source("AnimeYT", "a", "animeytx").unwrap();
        let p = db.upsert_series(src, &mk_airing("prequel", "Base Prequel", None)).unwrap();
        db.set_anilist_id(p, 9).unwrap();
        db.set_followed(p, true).unwrap();
        insert_eps_seen_up_to(&db, p, 3, 3);

        let fetched = vec![
            rel(12, "SPIN_OFF", "Spin"),
            rel(11, "SEQUEL", "Base 2"),
            RelatedMedia { cover_url: Some("https://remote/9".into()), ..rel(9, "PREQUEL", "Remote Title") },
            rel(10, "OTHER", "Self loop"),
        ];
        let out = db.related_anime(10, &fetched).unwrap();
        let ids: Vec<i64> = out.iter().map(|r| r.anilist_id).collect();
        assert_eq!(ids, vec![9, 11, 12]);

        let pre = &out[0];
        assert_eq!(pre.relation_type, "PREQUEL");
        assert_eq!(pre.title, "Base Prequel", "local catalog title wins");
        assert_eq!(pre.cover_url.as_deref(), Some("https://local/9"));
        assert!(pre.followed && pre.watched);
        assert_eq!(pre.series.as_ref().map(|s| s.id), Some(p));

        let seq = &out[1];
        assert_eq!(seq.title, "Base 2");
        assert_eq!(seq.url, "https://anilist.co/anime/11");
        assert!(!seq.followed && !seq.watched && seq.series.is_none());
    }
}
