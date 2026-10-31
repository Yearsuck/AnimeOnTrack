import { clampRatio } from "../lib/progress";

interface ProgressTrackProps {
  /** Accessible name of the progressbar. */
  label: string;
  /** Amount done, out of `max`. Ignored for ARIA when indeterminate. */
  value?: number;
  max?: number;
  /** Unknown or estimated total, or a wait of unknown length: a sweeping highlight
   *  instead of (and over) the fill, and no aria-valuenow. */
  indeterminate?: boolean;
  className?: string;
}

/**
 * Shared progress bar. The fill is drawn with transform: scaleX(ratio) (the only
 * inline style, data-driven); the sweep is a CSS keyframe. See styles.css "Progress track".
 */
export function ProgressTrack({ label, value, max = 1, indeterminate = false, className }: ProgressTrackProps) {
  const ratio = clampRatio(value, max);
  return (
    <div
      className={className ? `ptrack ${className}` : "ptrack"}
      role="progressbar"
      aria-label={label}
      aria-valuemin={indeterminate ? undefined : 0}
      aria-valuemax={indeterminate ? undefined : max}
      aria-valuenow={indeterminate ? undefined : Math.round(ratio * max)}
      aria-busy={indeterminate || undefined}
      data-mode={indeterminate ? "indeterminate" : "determinate"}
    >
      <span className="ptrack-fill" style={{ transform: `scaleX(${ratio})` }} />
      <span className="ptrack-sweep" />
    </div>
  );
}
