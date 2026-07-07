import { invoke } from "@tauri-apps/api/core";
import type { Series, Episode, PendingItem, LibraryItem } from "./types";

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
