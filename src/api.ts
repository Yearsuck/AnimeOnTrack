import { invoke } from "@tauri-apps/api/core";
import type {
  AnimeExtras,
  SeriesSiteInfo,
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
  DustyEntry,
  PopularityBias,
  YearlyActivity,
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
  HourCount,
  GenreCard,
  MonitorOption,
  PlayerSettings,
  ListEntryMeta,
  NextAir,
  DownloadBatch,
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

export const openEpisode = (
  url: string,
  options?: { monitor?: string | null; fullscreen?: boolean | null }
) =>
  invoke<void>("open_episode", {
    url,
    monitor: options?.monitor ?? null,
    fullscreen: options?.fullscreen ?? null,
  });

export const listMonitors = () => invoke<MonitorOption[]>("list_monitors");

export const getPlayerSettings = () => invoke<PlayerSettings>("get_player_settings");

export const setPlayerSettings = (monitor: string | null, fullscreen: boolean, clean: boolean, adblock?: boolean) =>
  invoke<void>("set_player_settings", { monitor, fullscreen, clean, adblock: adblock ?? null });


export const setSeen = (episodeId: number, seen: boolean) =>
  invoke<void>("set_seen", { episodeId, seen });

export const setSeenCascade = (seriesId: number, number: string, seen: boolean) =>
  invoke<void>("set_seen_cascade", { seriesId, number, seen });

export const listEpisodes = (seriesId: number) =>
  invoke<Episode[]>("list_episodes", { seriesId });

export const rescanAiring = () => invoke<Series[]>("rescan_airing");

export const getMirrors = () => invoke<string[]>("get_mirrors");

export const setMirrors = (urls: string[]) => invoke<void>("set_mirrors", { urls });

export const getSitePriority = () => invoke<string[]>("get_site_priority");

export const setSitePriority = (order: string[]) =>
  invoke<void>("set_site_priority", { order });

export const listSites = () => invoke<SiteSummary[]>("list_sites");

export const getActiveSite = () => invoke<SiteSummary>("get_active_site");

// Switches the active site's mirrors/source bookkeeping. Does NOT itself
// scan the airing listing — callers follow up with scanAiring(result.site
// .default_base_url), same as a fresh install's first scan.
export const setActiveSite = (siteId: string) =>
  invoke<SiteSwitchResult>("set_active_site", { siteId });

export const listLibrary = () => invoke<LibraryItem[]>("list_library");

export const getGenreStats = () => invoke<GenreStat[]>("get_genre_stats");

export const getGenreCards = () => invoke<GenreCard[]>("get_genre_cards");

export const getTagCards = () => invoke<GenreCard[]>("get_tag_cards");

export const getTypeStats = () => invoke<TypeStat[]>("get_type_stats");

export const getWatchSummary = () => invoke<WatchSummary>("get_watch_summary");

export const getWatchInsights = () => invoke<WatchInsights>("get_watch_insights");

export const getDustyWatchlist = () => invoke<DustyEntry[]>("get_dusty_watchlist");

export const getAvgCompletionDays = () => invoke<number | null>("get_avg_completion_days");

export const getPopularityBias = () => invoke<PopularityBias>("get_popularity_bias");

export const getYearlyActivity = (year: number) =>
  invoke<YearlyActivity>("get_yearly_activity", { year });

export const getActivityYears = () => invoke<number[]>("get_activity_years");

export const getStatsGraph = () => invoke<SeriesGraphNode[]>("get_stats_graph");

export const getBingeRecord = () => invoke<BingeRecord>("get_binge_record");

export const getHourlyDistribution = () => invoke<HourCount[]>("get_hourly_distribution");

export const backfillGenres = () => invoke<number>("backfill_genres");

export const discoverSwipeCard = () => invoke<SwipeCard | null>("discover_swipe_card");

export const decideSwipe = (seriesUrl: string, decision: SwipeDecision) =>
  invoke<void>("decide_swipe", { seriesUrl, decision });

// Resolves to the title of the decision that was actually reversed, or null
// when nothing was (empty history, or a row already gone). Only show an
// "undone" confirmation on a non-null result — a Ya-lo-vi decision whose
// background site-link merged it into an existing series is reversed on that
// survivor, and before this returned anything the UI reported success even
// when the undo had done nothing at all.
export const undoLastSwipe = () => invoke<string | null>("undo_last_swipe");

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
  minScore: number | null = null,
) => invoke<void>("set_deck_bans", { genres, formats, hideUpcoming, minStartDate, maxStartDate, minScore });

export const getDiscoverNiche = () => invoke<number>("get_discover_niche");

export const setDiscoverNiche = (value: number) =>
  invoke<void>("set_discover_niche", { value });

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

export const getSeriesGenresBatch = (seriesIds: number[]) =>
  invoke<Record<number, string[]>>("get_series_genres_batch", { seriesIds });

export const getListEntryMeta = (seriesIds: number[]) =>
  invoke<ListEntryMeta[]>("get_list_entry_meta", { seriesIds });


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
export const linkUnlinkedViaAnilist = () => invoke<number>("link_unlinked_via_anilist");

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

export const refreshCatalogStatus = () =>
  invoke<number | null>("refresh_catalog_status");

export const getCatalogInfoForSeries = (seriesId: number) =>
  invoke<CatalogAnime | null>("get_catalog_info_for_series", { seriesId });

export const getSeriesSiteInfo = (seriesId: number) =>
  invoke<SeriesSiteInfo>("get_series_site_info", { seriesId });

export const getAnimeExtras = (anilistId: number) =>
  invoke<AnimeExtras>("get_anime_extras", { anilistId });

export const exportCatalogSnapshot = () => invoke<string>("export_catalog_snapshot");

export const getNextAir = (anilistId: number) =>
  invoke<NextAir | null>("get_next_air", { anilistId });

export const discoverCatalogCard = (recommended: boolean) =>
  invoke<SwipeCard | null>("discover_catalog_card", { recommended });

export const discoverCatalogCards = (recommended: boolean, count: number) =>
  invoke<SwipeCard[]>("discover_catalog_cards", { recommended, count });

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

// Offline downloads
export const downloadEpisode = (episodeId: number) =>
  invoke<void>("download_episode", { episodeId });

export const downloadPending = (seriesId: number) =>
  invoke<DownloadBatch>("download_pending", { seriesId });

export const deleteSeriesDownloads = (seriesId: number) =>
  invoke<number>("delete_series_downloads", { seriesId });

export const playDownload = (episodeId: number) =>
  invoke<void>("play_download", { episodeId });

export const cancelDownload = (episodeId: number) =>
  invoke<void>("cancel_download", { episodeId });

export const deleteDownload = (episodeId: number) =>
  invoke<void>("delete_download", { episodeId });

export const getDownloadsSize = () =>
  invoke<number>("get_downloads_size");

export const clearAllDownloads = () =>
  invoke<number>("clear_all_downloads");

export const getSeriesBanner = (seriesId: number) =>
  invoke<string | null>("get_series_banner", { seriesId });

export const exportDiagnostics = () => invoke<string>("export_diagnostics");
export const revealInFolder = (path: string) =>
  invoke<void>("reveal_in_folder", { path });

export interface OrphanFile {
  path: string;
  size_bytes: number;
}

export interface StorageReport {
  downloads_bytes: number;
  sqlite_bytes: number;
  covers_bytes: number;
  logs_bytes: number;
  orphan_bytes: number;
}

export const scanDownloadOrphans = () =>
  invoke<OrphanFile[]>("scan_download_orphans");

export const deleteDownloadOrphans = (paths: string[]) =>
  invoke<number>("delete_download_orphans", { paths });

export const getStorageReport = () =>
  invoke<StorageReport>("get_storage_report");

export const getAutoDeleteSeenDownloads = () =>
  invoke<boolean>("get_auto_delete_seen_downloads");

export const setAutoDeleteSeenDownloads = (enabled: boolean) =>
  invoke<void>("set_auto_delete_seen_downloads", { enabled });

export const getDownloadQuality = () =>
  invoke<string>("get_download_quality");

export const setDownloadQuality = (quality: string) =>
  invoke<void>("set_download_quality", { quality });

