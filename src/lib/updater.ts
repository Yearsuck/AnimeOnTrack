// In-app updates via @tauri-apps/plugin-updater. The app asks GitHub's latest
// release for `latest.json`, verifies the installer's signature against the
// public key in tauri.conf.json, and only installs when the user says so.

import type { Update } from "@tauri-apps/plugin-updater";
import { isMobile } from "./platform";

export const UPDATE_KEY = "aot.autoUpdateCheck";

/** Checking is on unless the user switched it off. */
export function parseUpdatePref(raw: string | null): boolean {
  return !(raw === "0" || raw === "false");
}

export function readUpdatePref(): boolean {
  try {
    return parseUpdatePref(localStorage.getItem(UPDATE_KEY));
  } catch {
    return true;
  }
}

export function writeUpdatePref(v: boolean): void {
  try {
    localStorage.setItem(UPDATE_KEY, v ? "1" : "0");
  } catch {
    // localStorage unavailable — ignore failure.
  }
}

/** Whole-number download progress, or null while the size is unknown. */
export function downloadPercent(downloaded: number, total: number | undefined): number | null {
  if (!total || total <= 0) return null;
  return Math.min(100, Math.floor((downloaded / total) * 100));
}

/** The newer release, or null when up to date. Throws on network/signature errors. */
export async function checkForUpdate(): Promise<Update | null> {
  // The updater plugin is desktop-only; mobile builds are sideloaded.
  if (isMobile) return null;
  const { check } = await import("@tauri-apps/plugin-updater");
  return check();
}

/** Downloads, verifies and installs `update`, then restarts the app. */
export async function installUpdate(
  update: Update,
  onProgress: (percent: number | null) => void,
): Promise<void> {
  let downloaded = 0;
  let total: number | undefined;
  await update.downloadAndInstall((event) => {
    if (event.event === "Started") {
      total = event.data.contentLength;
      onProgress(downloadPercent(0, total));
    } else if (event.event === "Progress") {
      downloaded += event.data.chunkLength;
      onProgress(downloadPercent(downloaded, total));
    }
  });
  const { relaunch } = await import("@tauri-apps/plugin-process");
  await relaunch();
}
