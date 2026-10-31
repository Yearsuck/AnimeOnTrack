// Pure progress/presence logic shared by the scan bar, the catalog sync block and
// ProgressTrack. The motion itself is CSS (styles.css, "Progress track").

export type ProgressMode = "determinate" | "indeterminate";
export type Presence = "hidden" | "shown" | "leaving";
export type PresenceEvent = "show" | "hide" | "exit";

export interface RefreshProgressPayload {
  current: number;
  total: number;
  title: string;
  done?: boolean;
}

/** How long a leaving element stays mounted: its exit transition is --dur-fast (120ms). */
export const EXIT_MS = 150;

/** How long the scan bar shows "completed" at 100% before it leaves. Longer than the
 *  fill's --dur-slow (320ms) run to the end, so the full bar is actually seen. */
export const DONE_HOLD_MS = 900;

/** value/max clamped to 0..1; 0 when either is unusable. */
export function clampRatio(value: number | undefined, max: number): number {
  if (!Number.isFinite(max) || max <= 0) return 0;
  if (value === undefined || Number.isNaN(value)) return 0;
  return Math.min(1, Math.max(0, value / max));
}

/** Scan bar mode: indeterminate while nothing has finished yet or while the current
 *  series is in a wait of unknown length (page opening, Cloudflare check).
 *  When total <= 0, it is determinate to avoid an indeterminate sweep. */
export function scanMode(current: number, total: number, stage: string | null): ProgressMode {
  if (total <= 0) return "determinate";
  if (current >= total) return "determinate";
  if (current <= 0) return "indeterminate";
  if (stage === "opening" || stage === "verifying") return "indeterminate";
  return "determinate";
}

/** Step pure presence state machine:
 *  - "show": immediately transitions to "shown" from any state.
 *  - "hide": transitions "shown" -> "leaving"; stays "leaving" or "hidden".
 *  - "exit": when exit timer completes, transitions "leaving" -> "hidden".
 */
export function stepPresence(current: Presence, event: PresenceEvent): Presence {
  switch (event) {
    case "show":
      return "shown";
    case "hide":
      return current === "shown" ? "leaving" : current;
    case "exit":
      return current === "leaving" ? "hidden" : current;
  }
}

/** Presence transition helper: hiding something that is shown goes through "leaving" (still
 *  mounted, exit transition running) instead of unmounting at once. */
export function nextPresence(current: Presence, show: boolean): Presence {
  return stepPresence(current, show ? "show" : "hide");
}

/** Exit transition helper for when the exit timer expires. */
export function exitPresence(current: Presence): Presence {
  return stepPresence(current, "exit");
}

/** Whether the progress payload represents completion. */
export function isProgressDone(payload: { done?: boolean } | null | undefined): boolean {
  return payload?.done === true;
}

/** Formats the display label for progress based on completion and current stage. */
export function formatProgressLabel(
  payload: RefreshProgressPayload,
  stage: string | null,
  stageLabels?: Record<string, string>,
  completedLabel = "Completado"
): string {
  if (payload.done === true) return completedLabel;
  const stageSuffix = stage ? ` — ${(stageLabels && stageLabels[stage]) ?? stage}` : "";
  return `${payload.title}${stageSuffix}`;
}
