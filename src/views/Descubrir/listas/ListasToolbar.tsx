import { useEffect, useRef, useState, type ReactNode } from "react";
import { useT } from "../../../i18n";
import { formatLabel } from "../../../lib/formatLabel";
import {
  MIN_SCORE_OPTIONS,
  type ListasFilterState,
  type SortKey,
  type StatusFilter,
} from "./types";
import "../../../styles/filters.css";

export interface ListasToolbarProps {
  filterState: ListasFilterState;
  onQueryChange: (q: string) => void;
  onSortChange: (sort: SortKey) => void;
  onFormatChange: (fmt: string) => void;
  onStatusChange: (st: StatusFilter) => void;
  onToggleGenre: (genre: string) => void;
  onMinScoreChange: (score: number | null) => void;
  onYearFromChange: (yr: number | null) => void;
  onYearToChange: (yr: number | null) => void;
  onClearFilters: () => void;
  activeFilterCount: number;
  availableFormats: string[];
  availableGenres: string[];
  topGenres: string[];
  resultCount: number;
  /** Quick-filter chips (Quiero ver only), shown as the first chip row. */
  quickFilters?: ReactNode;
}

// Same structure as Explorar's filter bar (Catalog.tsx): search + format
// select + "Más filtros" + clear on one row, chip rows below, and the
// secondary selects in the collapsible panel.
export function ListasToolbar({
  filterState,
  onQueryChange,
  onSortChange,
  onFormatChange,
  onStatusChange,
  onToggleGenre,
  onMinScoreChange,
  onYearFromChange,
  onYearToChange,
  onClearFilters,
  activeFilterCount,
  availableFormats,
  availableGenres,
  topGenres,
  resultCount,
  quickFilters,
}: ListasToolbarProps) {
  const t = useT();
  const searchInputRef = useRef<HTMLInputElement>(null);
  const [secondaryOpen, setSecondaryOpen] = useState(false);

  // Keyboard shortcut listener: `/` focuses search, Esc clears search
  useEffect(() => {
    function handleKeyDown(e: KeyboardEvent) {
      if (e.key === "/" && document.activeElement !== searchInputRef.current) {
        const tag = (document.activeElement?.tagName || "").toLowerCase();
        if (tag !== "input" && tag !== "textarea" && tag !== "select") {
          e.preventDefault();
          searchInputRef.current?.focus();
        }
      } else if (e.key === "Escape" && document.activeElement === searchInputRef.current) {
        if (filterState.query) {
          e.preventDefault();
          onQueryChange("");
        } else {
          searchInputRef.current?.blur();
        }
      }
    }

    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [filterState.query, onQueryChange]);

  const displayGenres = topGenres.length > 0 ? topGenres : availableGenres.slice(0, 10);
  const format = filterState.selectedFormats[0] ?? "";

  const secondaryActiveCount =
    (filterState.sortBy !== "title" ? 1 : 0) +
    (filterState.status ? 1 : 0) +
    (filterState.minScore != null ? 1 : 0) +
    (filterState.yearFrom != null || filterState.yearTo != null ? 1 : 0);

  return (
    <div className="filter-bar" role="region" aria-label={t("discover.searchAriaLabel")}>
      <div className="filter-row">
        <div className="search">
          <span className="icon" aria-hidden="true">
            ⌕
          </span>
          <input
            ref={searchInputRef}
            className="input"
            placeholder={t("discover.searchPlaceholderListas")}
            aria-label={t("discover.searchAriaLabel")}
            title={t("discover.listas.searchKeyboardHint")}
            value={filterState.query}
            onChange={(e) => onQueryChange(e.target.value)}
          />
        </div>

        <select
          className="input filter-select"
          value={format}
          onChange={(e) => onFormatChange(e.target.value)}
          aria-label={t("seriesDetail.info.format")}
        >
          <option value="">{t("catalog.anyFormat")}</option>
          {availableFormats.map((f) => (
            <option key={f} value={f}>
              {formatLabel(t, f)}
            </option>
          ))}
        </select>

        <button
          type="button"
          className={`btn filter-toggle-btn${secondaryOpen ? " active" : ""}`}
          onClick={() => setSecondaryOpen((prev) => !prev)}
          aria-expanded={secondaryOpen}
        >
          {secondaryOpen ? t("catalog.lessFilters") : t("catalog.moreFilters")}
          {secondaryActiveCount > 0 && <span className="filter-active-badge">{secondaryActiveCount}</span>}
        </button>

        {activeFilterCount > 0 && (
          <button type="button" className="btn btn-ghost" onClick={onClearFilters}>
            {t("catalog.clearFilters")}
          </button>
        )}

        <div className="spacer" />
        {activeFilterCount > 0 && (
          <span className="muted">{t("catalog.resultsCount", { count: resultCount })}</span>
        )}
      </div>

      {quickFilters}

      {displayGenres.length > 0 && (
        <div className="chip-row">
          {displayGenres.map((g) => {
            const active = filterState.selectedGenres.includes(g);
            return (
              <button
                key={g}
                type="button"
                className={`chip-toggle${active ? " active" : ""}`}
                onClick={() => onToggleGenre(g)}
                aria-pressed={active}
              >
                {g}
              </button>
            );
          })}
        </div>
      )}

      {secondaryOpen && (
        <div className="filter-collapsible-panel">
          <div className="filter-collapsible-row">
            <select
              className="input filter-select"
              value={filterState.sortBy}
              onChange={(e) => onSortChange(e.target.value as SortKey)}
              aria-label={t("discover.sortTitle")}
            >
              <option value="title">{t("discover.listas.sortTitle")}</option>
              <option value="score_desc">{t("discover.listas.sortScoreDesc")}</option>
              <option value="newest">{t("discover.listas.sortNewest")}</option>
              <option value="oldest">{t("discover.listas.sortOldest")}</option>
              <option value="episodes">{t("discover.listas.sortEpisodes")}</option>
              <option value="recent">{t("discover.listas.sortRecent")}</option>
            </select>

            <select
              className="input filter-select"
              value={filterState.status}
              onChange={(e) => onStatusChange(e.target.value as StatusFilter)}
              aria-label={t("discover.listas.statusAll")}
            >
              <option value="">{t("discover.listas.statusAll")}</option>
              <option value="RELEASING">{t("discover.listas.statusReleasing")}</option>
              <option value="FINISHED">{t("discover.listas.statusFinished")}</option>
              <option value="NOT_YET_RELEASED">{t("discover.listas.statusNotYetReleased")}</option>
              <option value="CANCELLED">{t("discover.listas.statusCancelled")}</option>
            </select>

            <select
              className="input filter-select"
              value={filterState.minScore ?? ""}
              onChange={(e) => {
                const v = e.target.value;
                onMinScoreChange(v ? Number(v) : null);
              }}
              aria-label={t("discover.listas.minScoreAll")}
            >
              <option value="">{t("discover.listas.minScoreAll")}</option>
              {MIN_SCORE_OPTIONS.map((score) => (
                <option key={score} value={score}>
                  {t("discover.listas.minScoreVal", { score })}
                </option>
              ))}
            </select>

            <div className="listas-year-inputs" role="group" aria-label={t("discover.filtersDateHeading")}>
              <input
                type="number"
                className="input listas-year-input"
                placeholder={t("discover.listas.yearFrom")}
                aria-label={t("discover.listas.yearFrom")}
                value={filterState.yearFrom ?? ""}
                onChange={(e) => {
                  const val = e.target.value.trim();
                  onYearFromChange(val ? Number(val) : null);
                }}
              />
              <span className="muted" aria-hidden="true">
                –
              </span>
              <input
                type="number"
                className="input listas-year-input"
                placeholder={t("discover.listas.yearTo")}
                aria-label={t("discover.listas.yearTo")}
                value={filterState.yearTo ?? ""}
                onChange={(e) => {
                  const val = e.target.value.trim();
                  onYearToChange(val ? Number(val) : null);
                }}
              />
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
