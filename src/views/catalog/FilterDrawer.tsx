import { useEffect, useRef } from "react";
import { useT } from "../../i18n";
import { formatLabel, sortFormats } from "../../lib/formatLabel";
import { DateRangeControl, ScoreRangeControl } from "../../components/filters";
import type { CatalogFacets } from "../../types";

const STATUSES = [
  ["RELEASING", "filters.statusReleasing"],
  ["FINISHED", "filters.statusFinished"],
  ["NOT_YET_RELEASED", "filters.statusNotYetReleased"],
  ["HIATUS", "filters.statusHiatus"],
  ["CANCELLED", "filters.statusCancelled"],
] as const;

/**
 * Every Explorar filter in one side panel: what used to be the always-on
 * genre chip wall plus the "Más filtros" drawer. Any change here applies at
 * once (the grid behind it updates live), so there is no apply button.
 */
export function FilterDrawer({
  facets,
  episodeOptions,
  selectedGenres,
  toggleGenre,
  status,
  setStatus,
  format,
  setFormat,
  studio,
  setStudio,
  episodes,
  setEpisodes,
  minScore,
  maxScore,
  onScoreChange,
  minStartDate,
  maxStartDate,
  onDateChange,
  hideWatched,
  setHideWatched,
  autoFocus,
  onClose,
}: {
  facets: CatalogFacets;
  episodeOptions: { value: string; label: string }[];
  selectedGenres: string[];
  toggleGenre: (g: string) => void;
  status: string;
  setStatus: (s: string) => void;
  format: string;
  setFormat: (f: string) => void;
  studio: string;
  setStudio: (s: string) => void;
  episodes: string;
  setEpisodes: (e: string) => void;
  minScore: number | null;
  maxScore: number | null;
  onScoreChange: (min: number | null, max: number | null) => void;
  minStartDate: number | null;
  maxStartDate: number | null;
  onDateChange: (min: number | null, max: number | null) => void;
  hideWatched: boolean;
  setHideWatched: (v: boolean) => void;
  /** Move focus in on mount: only when the user just opened the drawer. */
  autoFocus: boolean;
  onClose: () => void;
}) {
  const t = useT();
  const closeRef = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    if (autoFocus) closeRef.current?.focus();
    // Mount only: a later prop change must not pull focus back.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  return (
    <aside
      id="cat-filter-drawer"
      className="cat-drawer"
      aria-label={t("catalog.filters")}
      onKeyDown={(e) => {
        if (e.key === "Escape") {
          e.stopPropagation();
          onClose();
        }
      }}
    >
      <div className="cat-drawer-head">
        <h2 className="cat-drawer-title">{t("catalog.filters")}</h2>
        <div className="spacer" />
        <button
          ref={closeRef}
          type="button"
          className="btn btn-ghost btn-sm"
          onClick={onClose}
          aria-label={t("catalog.filtersClose")}
        >
          ✕
        </button>
      </div>

      <fieldset className="cat-drawer-group">
        <legend className="cat-drawer-legend">{t("filters.statusLabel")}</legend>
        <div className="chip-row">
          {STATUSES.map(([value, key]) => (
            <button
              key={value}
              type="button"
              className={`chip-toggle${status === value ? " active" : ""}`}
              aria-pressed={status === value}
              onClick={() => setStatus(status === value ? "" : value)}
            >
              {t(key)}
            </button>
          ))}
        </div>
      </fieldset>

      {facets.genres.length > 0 && (
        <fieldset className="cat-drawer-group">
          <legend className="cat-drawer-legend">{t("catalog.genres")}</legend>
          <div className="chip-row">
            {facets.genres.map((g) => (
              <button
                key={g}
                type="button"
                className={`chip-toggle${selectedGenres.includes(g) ? " active" : ""}`}
                aria-pressed={selectedGenres.includes(g)}
                onClick={() => toggleGenre(g)}
              >
                {g}
              </button>
            ))}
          </div>
        </fieldset>
      )}

      <div className="cat-drawer-pair">
        <label className="cat-drawer-field">
          <span className="cat-drawer-legend">{t("catalog.format")}</span>
          <select className="input" value={format} onChange={(e) => setFormat(e.target.value)}>
            <option value="">{t("catalog.anyFormat")}</option>
            {sortFormats(facets.formats).map((f) => (
              <option key={f} value={f}>
                {formatLabel(t, f)}
              </option>
            ))}
          </select>
        </label>
        <label className="cat-drawer-field">
          <span className="cat-drawer-legend">{t("catalog.duration")}</span>
          <select className="input" value={episodes} onChange={(e) => setEpisodes(e.target.value)}>
            {episodeOptions.map((o) => (
              <option key={o.value} value={o.value}>
                {o.label}
              </option>
            ))}
          </select>
        </label>
      </div>

      <label className="cat-drawer-field">
        <span className="cat-drawer-legend">{t("catalog.studio")}</span>
        <select className="input" value={studio} onChange={(e) => setStudio(e.target.value)}>
          <option value="">{t("catalog.anyStudio")}</option>
          {facets.studios.map((s) => (
            <option key={s} value={s}>
              {s}
            </option>
          ))}
        </select>
      </label>

      <ScoreRangeControl
        minScore={minScore}
        maxScore={maxScore}
        onChange={onScoreChange}
        label={t("catalog.scoreRangeLabel")}
      />
      <DateRangeControl
        minStartDate={minStartDate}
        maxStartDate={maxStartDate}
        onChange={onDateChange}
        label={t("catalog.dateRangeLabel")}
      />

      <button
        type="button"
        className={`chip-toggle cat-drawer-hide${hideWatched ? " active" : ""}`}
        onClick={() => setHideWatched(!hideWatched)}
        aria-pressed={hideWatched}
        title={t("catalog.hideWatchedTooltip")}
      >
        {hideWatched ? "✓ " : ""}
        {t("catalog.hideWatched")}
      </button>
    </aside>
  );
}
