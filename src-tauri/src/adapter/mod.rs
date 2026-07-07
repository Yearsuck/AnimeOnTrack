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
