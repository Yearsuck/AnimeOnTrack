import { invoke } from "@tauri-apps/api/core";
import type {
  Series,
  Episode,
  PendingItem,
  LibraryItem,
  GenreStat,
  TypeStat,
  WatchSummary,
  SeriesGraphNode,
  SwipeCard,
  SwipeDecision,
  GenreAffinity,
  CatalogPage,
  CatalogFilter,
  CatalogFacets,
  LinkOutcome,
  SiteSummary,
  SiteSwitchResult,
  Classification,
  SwipeHistoryItem,
  DeckBans,
} from "./types";

export const scanAiring = (baseUrl: string) =>
  invoke<Series[]>("scan_airing", { baseUrl });

export const listAiring = () => invoke<Series[]>("list_airing");

export const setFollowed = (seriesId: number, followed: boolean) =>
  invoke<void>("set_followed", { seriesId, followed });

// force=true ignores the skip rules and re-fetches every followed series
// (Settings' "Forzar recomprobación completa" escape hatch).
export const refresh = (force = false) => invoke<number>("refresh", { force });

export const listPending = (sort?: "remaining_asc" | "remaining_desc") =>
  invoke<PendingItem[]>("list_pending", { sort: sort ?? null });

export const pendingCount = () => invoke<number>("pending_count");

export const openEpisode = (url: string) =>
  invoke<void>("open_episode", { url });

export const setSeen = (episodeId: number, seen: boolean) =>
  invoke<void>("set_seen", { episodeId, seen });

export const setSeenCascade = (seriesId: number, number: string, seen: boolean) =>
  invoke<void>("set_seen_cascade", { seriesId, number, seen });

export const listEpisodes = (seriesId: number) =>
  invoke<Episode[]>("list_episodes", { seriesId });

export const rescanAiring = () => invoke<Series[]>("rescan_airing");

export const getMirrors = () => invoke<string[]>("get_mirrors");

export const setMirrors = (urls: string[]) => invoke<void>("set_mirrors", { urls });

export const listSites = () => invoke<SiteSummary[]>("list_sites");

export const getActiveSite = () => invoke<SiteSummary>("get_active_site");

// Switches the active site's mirrors/source bookkeeping. Does NOT itself
// scan the airing listing — callers follow up with scanAiring(result.site
// .default_base_url), same as a fresh install's first scan.
export const setActiveSite = (siteId: string) =>
  invoke<SiteSwitchResult>("set_active_site", { siteId });

export const listLibrary = () => invoke<LibraryItem[]>("list_library");

export const getGenreStats = () => invoke<GenreStat[]>("get_genre_stats");

export const getTypeStats = () => invoke<TypeStat[]>("get_type_stats");

export const getWatchSummary = () => invoke<WatchSummary>("get_watch_summary");

export const getStatsGraph = () => invoke<SeriesGraphNode[]>("get_stats_graph");

export const backfillGenres = () => invoke<number>("backfill_genres");

export const discoverSwipeCard = () => invoke<SwipeCard | null>("discover_swipe_card");

export const decideSwipe = (seriesUrl: string, decision: SwipeDecision) =>
  invoke<void>("decide_swipe", { seriesUrl, decision });

export const undoLastSwipe = () => invoke<void>("undo_last_swipe");

// Up to 5 most-recent swipe decisions still live in the DB, newest first —
// the Descubrir history strip. `undo_swipe_entry` returns a specific one to
// the deck (hard-deletes its row); reclassification uses `reclassifySeries`.
export const listSwipeHistory = () =>
  invoke<SwipeHistoryItem[]>("list_swipe_history");

export const undoSwipeEntry = (seriesId: number) =>
  invoke<void>("undo_swipe_entry", { seriesId });

// Deck genre/type bans (global). Persisted in settings; the next
// discover_catalog_card call reads them fresh.
export const getDeckBans = () => invoke<DeckBans>("get_deck_bans");

export const setDeckBans = (genres: string[], formats: string[]) =>
  invoke<void>("set_deck_bans", { genres, formats });

export const startWatching = (seriesId: number) =>
  invoke<LinkOutcome>("start_watching", { seriesId });

export const listBacklog = (status: "want" | "discarded") =>
  invoke<Series[]>("list_backlog", { status });

export const promoteDiscarded = (seriesId: number) =>
  invoke<void>("promote_discarded", { seriesId });

export const deleteSeries = (seriesId: number) =>
  invoke<void>("delete_series", { seriesId });

export const setBacklogStatus = (seriesId: number, status: "want" | "discarded" | null) =>
  invoke<void>("set_backlog_status", { seriesId, status });

// The universal "de-classify / move between lists" inverse — clears
// followed/backlog_status/watched_externally then applies `to`. Pure-local,
// never scrapes, never touches episodes/seen rows.
export const reclassifySeries = (seriesId: number, to: Classification) =>
  invoke<void>("reclassify_series", { seriesId, to });

export const listWatchedExternally = () =>
  invoke<Series[]>("list_watched_externally");

export const getSeriesGenres = (seriesId: number) =>
  invoke<string[]>("get_series_genres", { seriesId });

export const getTopGenres = (limit: number) =>
  invoke<GenreAffinity[]>("get_top_genres", { limit });

export const getAnimeCatalog = (page: number, filter?: CatalogFilter) =>
  invoke<CatalogPage>("get_anime_catalog", { page, filter: filter ?? null });

export const getCatalogFacets = () => invoke<CatalogFacets>("get_catalog_facets");

export const syncAnimeCatalog = (forceFull = false) =>
  invoke<number>("sync_anime_catalog", { forceFull });

export const discoverCatalogCard = () => invoke<SwipeCard | null>("discover_catalog_card");

export const decideCatalogCard = (params: {
  anilistId: number;
  title: string;
  url: string;
  posterUrl: string | null;
  genres: string[];
  format: string;
  decision: SwipeDecision;
}) => invoke<number>("decide_catalog_card", params);

export const linkCatalogSeries = (seriesId: number) =>
  invoke<LinkOutcome>("link_catalog_series", { seriesId });
