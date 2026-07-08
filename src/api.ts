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
} from "./types";

export const scanAiring = (baseUrl: string) =>
  invoke<Series[]>("scan_airing", { baseUrl });

export const listAiring = () => invoke<Series[]>("list_airing");

export const setFollowed = (seriesId: number, followed: boolean) =>
  invoke<void>("set_followed", { seriesId, followed });

export const refresh = () => invoke<number>("refresh");

export const listPending = () => invoke<PendingItem[]>("list_pending");

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

export const startWatching = (seriesId: number) =>
  invoke<void>("start_watching", { seriesId });

export const listBacklog = (status: "want" | "discarded") =>
  invoke<Series[]>("list_backlog", { status });

export const promoteDiscarded = (seriesId: number) =>
  invoke<void>("promote_discarded", { seriesId });

export const deleteSeries = (seriesId: number) =>
  invoke<void>("delete_series", { seriesId });

export const setBacklogStatus = (seriesId: number, status: "want" | "discarded" | null) =>
  invoke<void>("set_backlog_status", { seriesId, status });

export const getSeriesGenres = (seriesId: number) =>
  invoke<string[]>("get_series_genres", { seriesId });

export const getTopGenres = (limit: number) =>
  invoke<GenreAffinity[]>("get_top_genres", { limit });
