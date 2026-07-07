import { useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";

interface RefreshProgress {
  current: number;
  total: number;
  title: string;
}

const STAGE_LABEL: Record<string, string> = {
  opening: "Abriendo página…",
  verifying: "Verificando (Cloudflare)…",
  extracting: "Leyendo contenido…",
  covers: "Descargando portadas…",
};

// Native <progress> + accessible label, per the ARIA/WCAG progressbar pattern:
// visible label, aria-valuenow/min/max via the native element's own semantics.
export function ProgressBar() {
  const [progress, setProgress] = useState<RefreshProgress | null>(null);
  const [stage, setStage] = useState<string | null>(null);
  const hideTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(() => {
    const un1 = listen<RefreshProgress>("refresh-progress", (e) => {
      if (hideTimer.current) clearTimeout(hideTimer.current);
      setProgress(e.payload);
      if (e.payload.current >= e.payload.total) {
        setStage(null);
        hideTimer.current = setTimeout(() => setProgress(null), 900);
      }
    });
    const un2 = listen<string>("scrape-stage", (e) => setStage(e.payload));
    return () => {
      un1.then((f) => f());
      un2.then((f) => f());
      if (hideTimer.current) clearTimeout(hideTimer.current);
    };
  }, []);

  if (!progress) return null;

  const label =
    progress.current >= progress.total
      ? "Completado"
      : `${progress.title}${stage ? " — " + (STAGE_LABEL[stage] ?? stage) : ""}`;

  return (
    <div className="scanbar">
      <div className="scanbar-row">
        <span className="scanbar-label">{label}</span>
        <span className="scanbar-count">
          {Math.min(progress.current, progress.total)}/{progress.total}
        </span>
      </div>
      <progress
        className="scanbar-track"
        value={progress.current}
        max={Math.max(progress.total, 1)}
        aria-label={label}
      />
    </div>
  );
}
