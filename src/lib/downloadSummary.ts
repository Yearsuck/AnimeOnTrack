import type { Episode } from "../types";

// The sites whose episode pages the offline downloader understands (keep in
// step with `site_of` in src-tauri/src/commands/download.rs).
const DOWNLOAD_HOSTS = ["tioanime.com", "jkanime.net", "animeyt.cc", "gogoanime.by"];

// Checks whether an episode URL belongs to one of those sites (or a subdomain).
// Never throws on malformed or empty URLs.
export function isDownloadableUrl(url: string): boolean {
  if (!url) return false;
  try {
    const hostname = new URL(url).hostname.toLowerCase();
    return DOWNLOAD_HOSTS.some((h) => hostname === h || hostname.endsWith(`.${h}`));
  } catch {
    return false;
  }
}

export interface DownloadSummary {
  downloadable: number;
  downloaded: number;
  active: number;
  failed: number;
  pending: number;
}

// Rolls up the download states across a series' episodes for the downloads panel.
// `downloadable` counts all episodes hosted on a supported site.
// `pending` counts unwatched, supported episodes that are neither downloaded nor
// currently downloading (failed episodes count so they can be retried in batch).
export function summarizeDownloads(
  episodes: Pick<Episode, "url" | "seen" | "download_status">[]
): DownloadSummary {
  let downloadable = 0;
  let downloaded = 0;
  let active = 0;
  let failed = 0;
  let pending = 0;

  for (const ep of episodes) {
    const supported = isDownloadableUrl(ep.url);
    if (supported) {
      downloadable += 1;
      if (
        !ep.seen &&
        ep.download_status !== "downloaded" &&
        ep.download_status !== "downloading"
      ) {
        pending += 1;
      }
    }

    if (ep.download_status === "downloaded") {
      downloaded += 1;
    } else if (ep.download_status === "downloading") {
      active += 1;
    } else if (ep.download_status === "failed") {
      failed += 1;
    }
  }

  return { downloadable, downloaded, active, failed, pending };
}
