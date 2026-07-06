import { invoke } from "@tauri-apps/api/core";
import type { Series, PendingItem } from "./types";

export const scanAiring = (baseUrl: string) =>
  invoke<Series[]>("scan_airing", { baseUrl });

export const listAiring = () => invoke<Series[]>("list_airing");

export const setFollowed = (seriesId: number, followed: boolean) =>
  invoke<void>("set_followed", { seriesId, followed });

export const refresh = () => invoke<number>("refresh");

export const listPending = () => invoke<PendingItem[]>("list_pending");

export const pendingCount = () => invoke<number>("pending_count");

export const openEpisode = (episodeId: number, url: string) =>
  invoke<void>("open_episode", { episodeId, url });
