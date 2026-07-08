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

/// A followed series plus its episode counts, for the library view.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LibraryItem {
    pub series: Series,
    pub total_episodes: i64,
    pub seen_episodes: i64,
    /// Insertion timestamp (`datetime('now')`, ISO 8601) of the most recently
    /// scraped episode — used to sort "recently active" first. Not the
    /// episode's air date (that's free-text and not reliably sortable).
    pub last_added: Option<String>,
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
}
