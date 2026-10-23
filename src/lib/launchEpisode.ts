// Wraps `openEpisode` with a launch curtain (see components/LaunchOverlay.tsx).
// Single module-level store: one overlay host is mounted once in App.tsx.

import { openEpisode } from "../api";
import {
  ERROR_SHOW_MS,
  INITIAL_LAUNCH_STATE,
  holdRemaining,
  launchReducer,
  type LaunchEvent,
  type LaunchState,
} from "./launchState";
import type { Box } from "./flip";

let state: LaunchState = INITIAL_LAUNCH_STATE;
const listeners = new Set<() => void>();
let errorTimer: ReturnType<typeof setTimeout> | undefined;

export function dispatchLaunch(ev: LaunchEvent) {
  const next = launchReducer(state, ev);
  if (next === state) return;
  state = next;
  listeners.forEach((l) => l());
}

export const subscribeLaunch = (l: () => void) => {
  listeners.add(l);
  return () => {
    listeners.delete(l);
  };
};
export const getLaunchState = () => state;

export interface LaunchOptions {
  url: string;
  /** Clicked element (or its rect) the curtain grows out of. */
  origin?: Element | Box | null;
  title: string;
  episode?: string | number | null;
  /** Resolved cover URL; omit when unknown. */
  cover?: string | null;
  site?: string | null;
  monitor?: string | null;
  fullscreen?: boolean | null;
}

function boxOf(origin: Element | Box | null | undefined): Box | null {
  if (!origin) return null;
  if ("getBoundingClientRect" in origin) {
    const r = origin.getBoundingClientRect();
    return { left: r.left, top: r.top, width: r.width, height: r.height };
  }
  return origin;
}

/** Opens the episode window behind a launch curtain. Never rejects. */
export async function launchEpisode(opts: LaunchOptions): Promise<void> {
  clearTimeout(errorTimer);
  dispatchLaunch({
    type: "start",
    req: {
      title: opts.title,
      episode: opts.episode == null ? null : String(opts.episode),
      cover: opts.cover ?? null,
      site: opts.site ?? null,
      origin: boxOf(opts.origin),
    },
  });
  const seq = state.seq;
  const startedAt = Date.now();
  try {
    await openEpisode(opts.url, { monitor: opts.monitor, fullscreen: opts.fullscreen });
    await new Promise((r) => setTimeout(r, holdRemaining(Date.now() - startedAt)));
    dispatchLaunch({ type: "resolved", seq });
  } catch (err) {
    console.error("openEpisode failed for", opts.url, err);
    dispatchLaunch({ type: "failed", seq, message: err instanceof Error ? err.message : String(err) });
    // Only a failure that actually applied owns the dismiss timer; a stale one
    // must not close a newer curtain.
    if (state.seq === seq && state.phase === "error") {
      errorTimer = setTimeout(() => dispatchLaunch({ type: "dismiss" }), ERROR_SHOW_MS);
    }
  }
}
