// Pure state machine + timing for the episode-launch curtain. No DOM, no IPC,
// so the "never flickers / dismiss early / error then close" rules are testable.

import type { Box } from "./flip";

/** The curtain never shows for less than this, however fast the window opens. */
export const MIN_SHOW_MS = 450;
/** How long a failure message stays up before the curtain leaves on its own. */
export const ERROR_SHOW_MS = 1800;

export interface LaunchRequest {
  title: string;
  episode: string | null;
  /** Already resolved with resolveCoverUrl; null shows the accent backdrop. */
  cover: string | null;
  site: string | null;
  /** Rect of the clicked element, in viewport px (null: no travel, fade in). */
  origin: Box | null;
}

export type LaunchPhase = "idle" | "opening" | "error" | "closing";

export interface LaunchState {
  phase: LaunchPhase;
  /** Bumped on every start so late events from a previous launch are ignored. */
  seq: number;
  req: LaunchRequest | null;
  error: string | null;
}

export type LaunchEvent =
  | { type: "start"; req: LaunchRequest }
  | { type: "resolved"; seq: number }
  | { type: "failed"; seq: number; message: string }
  | { type: "dismiss" }
  | { type: "exited"; seq: number };

export const INITIAL_LAUNCH_STATE: LaunchState = { phase: "idle", seq: 0, req: null, error: null };

export function launchReducer(state: LaunchState, ev: LaunchEvent): LaunchState {
  switch (ev.type) {
    case "start":
      return { phase: "opening", seq: state.seq + 1, req: ev.req, error: null };
    case "resolved":
      // Ignored once dismissed or superseded: the window still opened.
      if (ev.seq !== state.seq || state.phase !== "opening") return state;
      return { ...state, phase: "closing" };
    case "failed":
      if (ev.seq !== state.seq || state.phase !== "opening") return state;
      return { ...state, phase: "error", error: ev.message };
    case "dismiss":
      if (state.phase !== "opening" && state.phase !== "error") return state;
      return { ...state, phase: "closing" };
    case "exited":
      if (ev.seq !== state.seq || state.phase !== "closing") return state;
      return { ...INITIAL_LAUNCH_STATE, seq: state.seq };
  }
}

/** Milliseconds still to wait so the curtain has been up for MIN_SHOW_MS. */
export function holdRemaining(elapsedMs: number, minMs: number = MIN_SHOW_MS): number {
  return Math.max(0, minMs - Math.max(0, elapsedMs));
}
