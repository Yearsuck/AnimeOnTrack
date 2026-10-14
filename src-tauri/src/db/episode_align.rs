use super::*;
use rusqlite::OptionalExtension;

/// Per-row episode alignment against an AniList catalog entry.
///
/// A row is cumulative when its site numbers episodes continuously across
/// multiple seasons (e.g. TioAnime's "27" for Season 3 Ep 3, where offset=24).
/// A row is non-cumulative when its site restarts numbering at 1 for each
/// season (e.g. animeyt's "3").
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RowAlign {
    pub offset: i64,
    pub cumulative: bool,
}

/// Format an episode number without trailing ".0" for whole numbers (e.g. 3.0 -> "3", 12.5 -> "12.5").
pub fn format_ep_number(n: f64) -> String {
    if n.fract() == 0.0 {
        format!("{}", n as i64)
    } else {
        format!("{n}")
    }
}

impl RowAlign {
    /// Pure constructor holding the alignment decision rule:
    /// A row is CUMULATIVE iff all of:
    /// - `offset > 0`
    /// - row has at least one numeric episode (`row_max` is Some)
    /// - `A` (aired count) is known and > 0 (`aired` is Some and > 0)
    /// - `|m - (offset + A)| < |m - A|` (strictly closer to offset+aired than to aired; ties are NOT cumulative)
    pub fn decide(offset: i64, aired: Option<i64>, row_max: Option<f64>) -> Self {
        let cumulative = if offset > 0 {
            if let (Some(a), Some(m)) = (aired, row_max) {
                if a > 0 {
                    let diff_cum = (m - (offset + a) as f64).abs();
                    let diff_non = (m - a as f64).abs();
                    diff_cum < diff_non
                } else {
                    false
                }
            } else {
                false
            }
        } else {
            false
        };
        RowAlign { offset, cumulative }
    }

    /// Map a raw episode number to canonical space:
    /// - If cumulative: `n - offset` if `n > offset`; `None` if `n <= offset` (earlier season).
    /// - Otherwise: identity `Some(n)`.
    pub fn to_canonical(self, n: f64) -> Option<f64> {
        if self.cumulative {
            if n <= self.offset as f64 {
                None
            } else {
                Some(n - self.offset as f64)
            }
        } else {
            Some(n)
        }
    }

    /// Map a canonical episode number back to this row's raw number:
    /// - If cumulative: `c + offset`.
    /// - Otherwise: `c`.
    pub fn to_row(self, c: f64) -> f64 {
        if self.cumulative {
            c + self.offset as f64
        } else {
            c
        }
    }
}

impl Db {
    /// Compute the `RowAlign` for one series row at timestamp `now`.
    /// Rows without an `anilist_id` return `{ offset: 0, cumulative: false }`.
    pub(crate) fn row_align(&self, series_id: i64, now: i64) -> Result<RowAlign> {
        let anilist_id: Option<i64> = self
            .conn
            .query_row(
                "SELECT anilist_id FROM series WHERE id = ?1",
                [series_id],
                |r| r.get(0),
            )
            .optional()?
            .flatten();

        let Some(al_id) = anilist_id else {
            return Ok(RowAlign {
                offset: 0,
                cumulative: false,
            });
        };

        let offset = self.episode_offset(al_id)?.unwrap_or(0);

        let aired_count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM anilist_episode_map WHERE anilist_id = ?1 AND air_at IS NOT NULL AND air_at <= ?2",
            rusqlite::params![al_id, now],
            |r| r.get(0),
        )?;
        let aired = if aired_count > 0 { Some(aired_count) } else { None };

        let mut stmt = self
            .conn
            .prepare("SELECT number FROM episodes WHERE series_id = ?1")?;
        let numbers = stmt.query_map([series_id], |r| r.get::<_, String>(0))?;
        let mut row_max: Option<f64> = None;
        for num_res in numbers {
            let num_str = num_res?;
            if let Some(v) = crate::db::episodes::parse_ep_number(&num_str) {
                row_max = Some(match row_max {
                    Some(cur) => if v > cur { v } else { cur },
                    None => v,
                });
            }
        }

        Ok(RowAlign::decide(offset, aired, row_max))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn row_align_decide_and_conversion_rules() {
        // Must-pass example 1 from spec:
        let cum = RowAlign::decide(24, Some(3), Some(27.0));
        assert_eq!(cum, RowAlign { offset: 24, cumulative: true });

        assert_eq!(RowAlign::decide(24, Some(3), Some(3.0)), RowAlign { offset: 24, cumulative: false });
        assert_eq!(RowAlign::decide(0, Some(3), Some(27.0)), RowAlign { offset: 0, cumulative: false });
        assert_eq!(RowAlign::decide(24, None, Some(27.0)), RowAlign { offset: 24, cumulative: false });
        assert_eq!(RowAlign::decide(24, Some(3), None), RowAlign { offset: 24, cumulative: false });

        // A tie: midpoint between 3 and 27 is 15. |15 - 27| = 12 == |15 - 3| = 12. Must NOT be cumulative.
        assert_eq!(RowAlign::decide(24, Some(3), Some(15.0)), RowAlign { offset: 24, cumulative: false });

        // Conversions
        assert_eq!(cum.to_canonical(27.0), Some(3.0));
        assert_eq!(cum.to_canonical(24.0), None);
        assert_eq!(cum.to_canonical(1.0), None);
        assert_eq!(cum.to_row(3.0), 27.0);

        let non_cum = RowAlign::decide(24, Some(3), Some(3.0));
        assert_eq!(non_cum.to_canonical(3.0), Some(3.0));
        assert_eq!(non_cum.to_canonical(1.0), Some(1.0));
        assert_eq!(non_cum.to_row(3.0), 3.0);
    }

    #[test]
    fn format_ep_number_cases() {
        assert_eq!(format_ep_number(3.0), "3");
        assert_eq!(format_ep_number(27.0), "27");
        assert_eq!(format_ep_number(12.5), "12.5");
        assert_eq!(format_ep_number(0.0), "0");
    }

    #[test]
    fn db_row_align_with_tensei_fixture() {
        let db = Db::open(":memory:").unwrap();
        let now = 1_700_000_000;
        let src_a = db.upsert_source("AnimeYT", "https://animeyt.cc", "animeytx").unwrap();
        let src_b = db.upsert_source("TioAnime", "https://tioanime.com", "tioanime").unwrap();

        let entries = vec![
            crate::anizip::MapEntry { episode: 1, season: 3, absolute: Some(25), air_at: Some(now - 14 * 86400) },
            crate::anizip::MapEntry { episode: 2, season: 3, absolute: Some(26), air_at: Some(now - 7 * 86400) },
            crate::anizip::MapEntry { episode: 3, season: 3, absolute: Some(27), air_at: Some(now) },
            crate::anizip::MapEntry { episode: 4, season: 3, absolute: Some(28), air_at: Some(now + 7 * 86400) },
        ];
        db.replace_episode_map(185756, &entries, now).unwrap();

        let sid_a = db.upsert_series(src_a, &crate::db::test_support::mk_airing("tensei-a", "Tensei", None)).unwrap();
        db.set_anilist_id(sid_a, 185756).unwrap();
        for i in 1..=3 {
            db.insert_episode(&crate::models::Episode {
                id: 0, series_id: sid_a, number: i.to_string(), title: None,
                url: format!("https://a/{i}"), released_at: None, seen: false,
                download_path: None, download_status: None,
            }).unwrap();
        }

        let sid_b = db.upsert_series(src_b, &crate::db::test_support::mk_airing("tensei-b", "Tensei", None)).unwrap();
        db.set_anilist_id(sid_b, 185756).unwrap();
        for i in 1..=27 {
            db.insert_episode(&crate::models::Episode {
                id: 0, series_id: sid_b, number: i.to_string(), title: None,
                url: format!("https://b/{i}"), released_at: None, seen: false,
                download_path: None, download_status: None,
            }).unwrap();
        }

        let align_a = db.row_align(sid_a, now).unwrap();
        assert_eq!(align_a, RowAlign { offset: 24, cumulative: false });

        let align_b = db.row_align(sid_b, now).unwrap();
        assert_eq!(align_b, RowAlign { offset: 24, cumulative: true });

        // Unlinked row returns offset 0, cumulative false
        let sid_unlinked = db.upsert_series(src_a, &crate::db::test_support::mk_airing("unlinked", "Unlinked", None)).unwrap();
        assert_eq!(db.row_align(sid_unlinked, now).unwrap(), RowAlign { offset: 0, cumulative: false });
    }
}
