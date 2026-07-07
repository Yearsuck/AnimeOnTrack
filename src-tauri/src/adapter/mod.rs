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
