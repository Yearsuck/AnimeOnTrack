// FLIP helpers: make a surface grow out of (and shrink back into) the element
// the user clicked. Pure math so it can be tested without a DOM.

export interface Box {
  left: number;
  top: number;
  width: number;
  height: number;
}

export interface FlipDelta {
  /** Translation (px) that puts the target's centre on the origin's centre. */
  dx: number;
  dy: number;
  /** Uniform scale, so the content is never squashed (clamped to [min, 1]). */
  scale: number;
}

const MIN_SCALE = 0.12;

/**
 * Transform that makes `target` (laid out at its final box) look like it sits
 * on `origin`. Uniform scale from the larger ratio, so the surface fully covers
 * the origin instead of leaving a gap, anchored on the centres
 * (use `transform-origin: center`).
 */
export function flipDelta(origin: Box, target: Box): FlipDelta | null {
  if (target.width <= 0 || target.height <= 0 || origin.width <= 0 || origin.height <= 0) return null;
  const raw = Math.max(origin.width / target.width, origin.height / target.height);
  const scale = Math.min(1, Math.max(MIN_SCALE, raw));
  const dx = origin.left + origin.width / 2 - (target.left + target.width / 2);
  const dy = origin.top + origin.height / 2 - (target.top + target.height / 2);
  return { dx, dy, scale };
}

export function flipTransform(d: FlipDelta): string {
  return `translate(${round(d.dx)}px, ${round(d.dy)}px) scale(${round(d.scale)})`;
}

function round(n: number): number {
  return Math.round(n * 1000) / 1000;
}

/** Closed-state transform when no origin is known: a small settle, no travel. */
export const FALLBACK_TRANSFORM = "scale(0.96)";
