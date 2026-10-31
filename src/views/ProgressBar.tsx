import { useEffect, useMemo, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { useT } from "../i18n";
import { ProgressTrack } from "../components/ProgressTrack";
import { DONE_HOLD_MS, scanMode } from "../lib/progress";
import { usePresence } from "../lib/usePresence";

interface RefreshProgress {
  current: number;
  total: number;
  title: string;
  done?: boolean;
}

// Visible label + role="progressbar" track (ProgressTrack). Enters with
// @starting-style and leaves through usePresence, so it never pops in or out.
export function ProgressBar() {
  const t = useT();
  const STAGE_LABEL: Record<string, string> = useMemo(
    () => ({
      opening: t("progress.stageOpening"),
      verifying: t("progress.stageVerifying"),
      extracting: t("progress.stageExtracting"),
      covers: t("progress.stageCovers"),
    }),
    [t]
  );
  // The last payload is kept after the bar is hidden so the leaving bar still
  // renders its final ("completed", 100%) state while it fades out.
  const [progress, setProgress] = useState<RefreshProgress | null>(null);
  const [visible, setVisible] = useState(false);
  const [stage, setStage] = useState<string | null>(null);
  const hideTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const presence = usePresence(visible);

  useEffect(() => {
    const un1 = listen<RefreshProgress>("refresh-progress", (e) => {
      if (hideTimer.current) {
        clearTimeout(hideTimer.current);
        hideTimer.current = null;
      }
      setProgress(e.payload);
      setVisible(true);
      if (e.payload.done === true) {
        setStage(null);
        hideTimer.current = setTimeout(() => setVisible(false), DONE_HOLD_MS);
      }
    });
    const un2 = listen<string>("scrape-stage", (e) => setStage(e.payload));
    return () => {
      un1.then((f) => f());
      un2.then((f) => f());
      if (hideTimer.current) {
        clearTimeout(hideTimer.current);
        hideTimer.current = null;
      }
    };
  }, []);

  if (presence === "hidden" || !progress) return null;

  const done = progress.done === true;
  const label = done
    ? t("progress.completed")
    : `${progress.title}${stage ? " — " + (STAGE_LABEL[stage] ?? stage) : ""}`;

  return (
    <div className="scanbar presence" data-leaving={presence === "leaving" ? "" : undefined}>
      <div className="scanbar-row">
        <span className="scanbar-label">{label}</span>
        <span className="scanbar-count">
          {done ? progress.total : Math.min(progress.current, progress.total)}/{progress.total}
        </span>
      </div>
      {/* Announces each series and the completion, not every sub-stage (too chatty). */}
      <span className="sr-only" aria-live="polite">
        {done ? t("progress.completed") : progress.title}
      </span>
      <ProgressTrack
        label={label}
        value={done ? Math.max(progress.total, 1) : progress.current}
        max={Math.max(progress.total, 1)}
        indeterminate={!done && scanMode(progress.current, progress.total, stage) === "indeterminate"}
      />
    </div>
  );
}
