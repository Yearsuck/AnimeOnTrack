import { useT } from "../../i18n";
import "../../styles/filters.css";

export interface ScoreRangeControlProps {
  minScore: number | null | undefined;
  maxScore?: number | null | undefined;
  onChange: (min: number | null, max: number | null) => void;
  label?: string;
  minOnly?: boolean;
}

export function ScoreRangeControl({
  minScore,
  maxScore,
  onChange,
  label,
  minOnly = false,
}: ScoreRangeControlProps) {
  const t = useT();
  const currentMin = minScore ?? null;
  const currentMax = maxScore ?? null;

  const isTodos = currentMin === null && currentMax === null;
  const isUnder60 = currentMin === null && currentMax === 59;
  const is60to70 = currentMin === 60 && currentMax === 70;
  const is70to80 = currentMin === 70 && currentMax === 80;
  const is80Plus = currentMin === 80 && currentMax === null;

  function handleMinInput(val: string) {
    const trimmed = val.trim();
    if (trimmed === "") {
      onChange(null, currentMax);
      return;
    }
    const num = Number(trimmed);
    if (!Number.isNaN(num)) {
      const clamped = Math.max(0, Math.min(100, Math.round(num)));
      onChange(clamped, currentMax);
    }
  }

  function handleMaxInput(val: string) {
    const trimmed = val.trim();
    if (trimmed === "") {
      onChange(currentMin, null);
      return;
    }
    const num = Number(trimmed);
    if (!Number.isNaN(num)) {
      const clamped = Math.max(0, Math.min(100, Math.round(num)));
      onChange(currentMin, clamped);
    }
  }

  return (
    <div className="filter-group filter-score-control">
      {label && <span className="filter-label">{label}</span>}
      <div className="filter-score-presets" role="group" aria-label={t("filters.scorePresetsAria")}>
        <button
          type="button"
          className={`filter-preset-chip${isTodos ? " active" : ""}`}
          onClick={() => onChange(null, null)}
          aria-pressed={isTodos}
        >
          {t("filters.scoreAll")}
        </button>
        {!minOnly && (
          <button
            type="button"
            className={`filter-preset-chip${isUnder60 ? " active" : ""}`}
            onClick={() => onChange(null, 59)}
            aria-pressed={isUnder60}
          >
            {t("filters.scoreUnder60")}
          </button>
        )}
        {!minOnly && (
          <button
            type="button"
            className={`filter-preset-chip${is60to70 ? " active" : ""}`}
            onClick={() => onChange(60, 70)}
            aria-pressed={is60to70}
          >
            {t("filters.score60to70")}
          </button>
        )}
        {!minOnly && (
          <button
            type="button"
            className={`filter-preset-chip${is70to80 ? " active" : ""}`}
            onClick={() => onChange(70, 80)}
            aria-pressed={is70to80}
          >
            {t("filters.score70to80")}
          </button>
        )}
        <button
          type="button"
          className={`filter-preset-chip${is80Plus ? " active" : ""}`}
          onClick={() => onChange(80, null)}
          aria-pressed={is80Plus}
        >
          {t("filters.score80Plus")}
        </button>
        {minOnly && (
          <>
            <button
              type="button"
              className={`filter-preset-chip${currentMin === 60 && currentMax === null ? " active" : ""}`}
              onClick={() => onChange(60, null)}
              aria-pressed={currentMin === 60 && currentMax === null}
            >
              60+
            </button>
            <button
              type="button"
              className={`filter-preset-chip${currentMin === 70 && currentMax === null ? " active" : ""}`}
              onClick={() => onChange(70, null)}
              aria-pressed={currentMin === 70 && currentMax === null}
            >
              70+
            </button>
          </>
        )}
      </div>

      <div className="filter-score-inputs">
        <label className="filter-score-field">
          <span>{t("filters.scoreMin")}</span>
          <input
            type="number"
            min="0"
            max="100"
            className="filter-score-input"
            value={currentMin !== null ? currentMin : ""}
            onChange={(e) => handleMinInput(e.target.value)}
            aria-label={t("filters.scoreMinAria")}
            placeholder="0"
          />
        </label>
        {!minOnly && (
          <label className="filter-score-field">
            <span>{t("filters.scoreMax")}</span>
            <input
              type="number"
              min="0"
              max="100"
              className="filter-score-input"
              value={currentMax !== null ? currentMax : ""}
              onChange={(e) => handleMaxInput(e.target.value)}
              aria-label={t("filters.scoreMaxAria")}
              placeholder="100"
            />
          </label>
        )}
      </div>
    </div>
  );
}
