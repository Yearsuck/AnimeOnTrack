export interface AutoSyncOptions {
  run: () => Promise<unknown>;
  minIntervalMs: number;
  now?: () => number;
}

/// Throttled background sync: at most one run in flight, and no new run until
/// `minIntervalMs` after the last one started (coming back to the app often
/// fires several visibility events in a row).
export function createAutoSync(opts: AutoSyncOptions): { trigger(): Promise<void> } {
  const getNow = opts.now ?? (() => Date.now());
  let lastStartedAt = -Infinity;
  let inFlight = false;

  return {
    async trigger(): Promise<void> {
      if (inFlight) return;
      const now = getNow();
      if (now - lastStartedAt < opts.minIntervalMs) return;

      lastStartedAt = now;
      inFlight = true;
      try {
        await opts.run();
      } catch {
        // Swallowed: background auto-sync failures are recorded by the backend.
      } finally {
        inFlight = false;
      }
    },
  };
}
