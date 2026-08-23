import { invoke } from "@tauri-apps/api/core";
import type {
  Series,
  AiringItem,
  BingeRecord,
  Episode,
  PendingItem,
  LibraryItem,
  GenreStat,
  TypeStat,
  WatchSummary,
  WatchInsights,
  SeriesGraphNode,
  SwipeCard,
  SwipeDecision,
  GenreAffinity,
  CatalogAnime,
  CatalogPage,
  CatalogFilter,
  CatalogFacets,
  LinkOutcome,
  SiteSummary,
  SiteSwitchResult,
  Classification,
  SwipeHistoryItem,
  DeckBans,
  BackupStatus,
} from "./types";

export const scanAiring = (baseUrl: string) =>
  invoke<Series[]>("scan_airing", { baseUrl });

export const listAiring = () => invoke<Series[]>("list_airing");

// Same series/order as listAiring, each paired with its parsed first-episode
// date (when known) — feeds AiringGrid's "Esta temporada" filter.
export const listAiringSeason = () => invoke<AiringItem[]>("list_airing_season");

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

export const getWatchInsights = () => invoke<WatchInsights>("get_watch_insights");

export const getStatsGraph = () => invoke<SeriesGraphNode[]>("get_stats_graph");

export const getBingeRecord = () => invoke<BingeRecord>("get_binge_record");

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

export const setDeckBans = (
  genres: string[],
  formats: string[],
  hideUpcoming: boolean,
  minStartDate: number | null,
  maxStartDate: number | null,
) => invoke<void>("set_deck_bans", { genres, formats, hideUpcoming, minStartDate, maxStartDate });

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

// Fire-and-forget throttled incremental catalog sync, called on app
// startup so `anilist_catalog.start_date` backfills for "Esta temporada"
// without requiring the user to visit Catálogo and click Sync manually.
export const maybeSyncCatalogIncremental = () =>
  invoke<number | null>("maybe_sync_catalog_incremental");

// Resolves engaged-but-unlinked series to their AniList catalog row against
// the local catalog only — no network. Returns how many were newly linked.
export const linkSeriesToCatalog = () => invoke<number>("link_series_to_catalog");

// Cross-site library import: for every followed series not yet on the active
// site, search+link it there and replay its watched watermark. Paced/live
// scraping — emits "library-import-progress" ({done,total,title}) per series.
export const importLibraryToActiveSite = () =>
  invoke<{ total: number; linked: number; skipped: number }>("import_library_to_active_site");

// Launch the app's own uninstaller (Windows/NSIS) and quit. Resolves before
// exit begins; on a dev/unpacked build it rejects with a "no uninstaller"
// message instead. Irreversible — the caller must confirm first.
export const uninstallApp = () => invoke<void>("uninstall_app");

// Re-fetches catalog rows synced before duration/romaji/studio/start_date
// existed. Long-running (paced against AniList's rate limit) but resumable —
// emits "catalog-backfill-progress" ({ done, total }) per 50-row batch.
export const backfillCatalogMetadata = () =>
  invoke<number>("backfill_catalog_metadata");

export const getCatalogInfoForSeries = (seriesId: number) =>
  invoke<CatalogAnime | null>("get_catalog_info_for_series", { seriesId });

export const discoverCatalogCard = (recommended: boolean) =>
  invoke<SwipeCard | null>("discover_catalog_card", { recommended });

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

export const backupStatus = () => invoke<BackupStatus>("backup_status");
export const connectDrive = () => invoke<BackupStatus>("connect_drive");
export const disconnectDrive = () => invoke<BackupStatus>("disconnect_drive");
export const backupNow = () => invoke<BackupStatus>("backup_now");
export const restoreLatest = () => invoke<void>("restore_latest");

// Stores the Google OAuth client the backup authenticates with, so Drive can
// be set up without rebuilding. Passing empty strings clears it back to
// whatever the build was compiled with.
export const setGoogleCredentials = (clientId: string, clientSecret: string) =>
  invoke<BackupStatus>("set_google_credentials", { clientId, clientSecret });
