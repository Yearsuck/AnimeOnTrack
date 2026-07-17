import { useDiscoverMode } from "../../discoverMode";
import { useT } from "../../i18n";

// Recomendado / Aleatorio segmented control — self-contained (reads/writes
// `useDiscoverMode` directly, no props). Lives inside DeckPanel now. Mirrors
// StatsRings.tsx's bars/rings toggle markup (`.seg`/`.seg-btn`,
// `role="tablist"`/`"tab"`).
export function DiscoverModeToggle() {
  const t = useT();
  const [mode, setMode] = useDiscoverMode();
  return (
    <div className="seg" role="tablist" aria-label={t("discover.modeAria")} title={t("discover.modeHint")}>
      <button
        type="button"
        role="tab"
        aria-selected={mode === "recommended"}
        className={`seg-btn${mode === "recommended" ? " active" : ""}`}
        onClick={() => setMode("recommended")}
      >
        {t("discover.modeRecommended")}
      </button>
      <button
        type="button"
        role="tab"
        aria-selected={mode === "random"}
        className={`seg-btn${mode === "random" ? " active" : ""}`}
        onClick={() => setMode("random")}
      >
        {t("discover.modeRandom")}
      </button>
    </div>
  );
}
