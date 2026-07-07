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
