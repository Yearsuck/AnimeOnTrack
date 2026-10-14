// Pure helper to run series start-watching tasks sequentially.
//
// Each startWatching call resolves an unlinked catalog row against the active
// pirate streaming site via WebView2 scraping behind Cloudflare. Launching
// concurrent scraper instances or parallel search queries triggers Cloudflare's
// bot rate-limits or interstitial blocks, so multiple items must be processed
// STRICTLY one at a time in order.
//
// Individual failures or scraping errors must not abort remaining items in a
// batch, so any rejection is caught and recorded as "failed".

export type StartResult = "started" | "alreadyStarted" | "notFound" | "failed";

export interface StartCounts {
  started: number;
  alreadyStarted: number;
  notFound: number;
  failed: number;
}

export interface StartSummary {
  started: number;
  problems: number;
}

export async function startSeriesSequentially<T>(
  items: T[],
  start: (item: T) => Promise<StartResult>,
  onProgress?: (done: number, total: number, result: StartResult) => void
): Promise<StartCounts> {
  const counts: StartCounts = {
    started: 0,
    alreadyStarted: 0,
    notFound: 0,
    failed: 0,
  };

  const total = items.length;
  for (let i = 0; i < total; i++) {
    let result: StartResult;
    try {
      result = await start(items[i]);
    } catch {
      result = "failed";
    }

    counts[result]++;
    if (onProgress) {
      try {
        onProgress(i + 1, total, result);
      } catch {
        // onProgress errors must not interrupt queue processing
      }
    }
  }

  return counts;
}

export function summarizeStart(r: {
  started: number;
  alreadyStarted: number;
  notFound: number;
  failed: number;
}): StartSummary {
  return {
    started: r.started + r.alreadyStarted,
    problems: r.notFound + r.failed,
  };
}
