//! Parsing for the site's Spanish free-text episode release dates
//! (`episodes.released_at`), e.g. `"junio 29, 2026"` or `"abril 1, 2026"`.
//! Pure, no I/O — see docs/superpowers/specs/2026-07-13-airing-this-season-design.md.

use chrono::NaiveDate;

/// Parse `"<mes> <día>, <año>"` (Spanish month name, case-insensitive) into a
/// `NaiveDate`. Accepts the `setiembre` spelling variant of `septiembre` seen
/// on the site. Returns `None` for empty/garbage/unparseable input — callers
/// should treat that as "no known date", never a fatal error, since the site
/// occasionally omits or mangles this field (see spec's "11 of 2195 rows are
/// NULL/empty" note).
pub fn parse_spanish_date(s: &str) -> Option<NaiveDate> {
    let s = s.trim();
    // Expected shape: "<month> <day>, <year>"
    let (month_str, rest) = s.split_once(' ')?;
    let (day_str, year_str) = rest.split_once(',')?;

    let month = month_number(month_str)?;
    let day: u32 = day_str.trim().parse().ok()?;
    let year: i32 = year_str.trim().parse().ok()?;

    NaiveDate::from_ymd_opt(year, month, day)
}

fn month_number(name: &str) -> Option<u32> {
    let m = match name.trim().to_lowercase().as_str() {
        "enero" => 1,
        "febrero" => 2,
        "marzo" => 3,
        "abril" => 4,
        "mayo" => 5,
        "junio" => 6,
        "julio" => 7,
        "agosto" => 8,
        "septiembre" | "setiembre" => 9,
        "octubre" => 10,
        "noviembre" => 11,
        "diciembre" => 12,
        _ => return None,
    };
    Some(m)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    #[test]
    fn parses_junio_29_2026() {
        assert_eq!(
            parse_spanish_date("junio 29, 2026"),
            NaiveDate::from_ymd_opt(2026, 6, 29)
        );
    }

    #[test]
    fn parses_abril_1_2026() {
        assert_eq!(
            parse_spanish_date("abril 1, 2026"),
            NaiveDate::from_ymd_opt(2026, 4, 1)
        );
    }

    #[test]
    fn parses_case_insensitively() {
        assert_eq!(
            parse_spanish_date("Junio 29, 2026"),
            NaiveDate::from_ymd_opt(2026, 6, 29)
        );
    }

    #[test]
    fn accepts_setiembre_variant_of_septiembre() {
        assert_eq!(
            parse_spanish_date("setiembre 5, 2025"),
            NaiveDate::from_ymd_opt(2025, 9, 5)
        );
        assert_eq!(
            parse_spanish_date("septiembre 5, 2025"),
            NaiveDate::from_ymd_opt(2025, 9, 5)
        );
    }

    #[test]
    fn empty_string_is_none() {
        assert_eq!(parse_spanish_date(""), None);
    }

    #[test]
    fn garbage_input_is_none() {
        assert_eq!(parse_spanish_date("not a date"), None);
        assert_eq!(parse_spanish_date("2026-06-29"), None);
    }

    #[test]
    fn all_twelve_months_parse() {
        let months = [
            ("enero", 1),
            ("febrero", 2),
            ("marzo", 3),
            ("abril", 4),
            ("mayo", 5),
            ("junio", 6),
            ("julio", 7),
            ("agosto", 8),
            ("septiembre", 9),
            ("octubre", 10),
            ("noviembre", 11),
            ("diciembre", 12),
        ];
        for (name, num) in months {
            let s = format!("{name} 10, 2024");
            assert_eq!(
                parse_spanish_date(&s),
                NaiveDate::from_ymd_opt(2024, num, 10),
                "month {name} failed to parse"
            );
        }
    }
}
