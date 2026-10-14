import type { Episode } from "../types";

// Checks whether an episode URL belongs to a site supported by the offline
// downloader. Currently only tioanime.com (and subdomains) supports downloads.
// Never throws on malformed or empty URLs.
export function isDownloadableUrl(url: string): boolean {
  if (!url) return false;
  try {
    const hostname = new URL(url).hostname.toLowerCase();
    return hostname === "tioanime.com" || hostname.endsWith(".tioanime.com");
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
