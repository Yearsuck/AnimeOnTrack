import { useEffect, useRef } from "react";
import { useT } from "../../../i18n";
import {
  AVAILABLE_FORMATS,
  MIN_SCORE_OPTIONS,
  type ListasFilterState,
  type SortKey,
  type StatusFilter,
} from "./types";

export interface ListasToolbarProps {
  filterState: ListasFilterState;
  onQueryChange: (q: string) => void;
  onSortChange: (sort: SortKey) => void;
  onToggleFormat: (fmt: string) => void;
  onStatusChange: (st: StatusFilter) => void;
  onToggleGenre: (genre: string) => void;
  onMinScoreChange: (score: number | null) => void;
  onYearFromChange: (yr: number | null) => void;
  onYearToChange: (yr: number | null) => void;
  onClearFilters: () => void;
  activeFilterCount: number;
  availableGenres: string[];
  topGenres: string[];
}

export function ListasToolbar({
  filterState,
  onQueryChange,
  onSortChange,
  onToggleFormat,
  onStatusChange,
  onToggleGenre,
  onMinScoreChange,
  onYearFromChange,
  onYearToChange,
  onClearFilters,
  activeFilterCount,
  availableGenres,
  topGenres,
}: ListasToolbarProps) {
  const t = useT();
  const searchInputRef = useRef<HTMLInputElement>(null);

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

  return (
    <div className="listas-toolbar-box" role="region" aria-label={t("discover.searchAriaLabel")}>
      <div className="listas-toolbar-primary">
        <div className="search listas-search-wrap">
          <span className="listas-search-icon" aria-hidden="true">
            ⌕
          </span>
          <label htmlFor="listas-search-input" className="sr-only">
            {t("discover.searchAriaLabel")}
          </label>
          <input
            id="listas-search-input"
            ref={searchInputRef}
            className="input listas-search-input"
            placeholder={t("discover.searchPlaceholderListas")}
            value={filterState.query}
            onChange={(e) => onQueryChange(e.target.value)}
          />
          <kbd className="listas-search-kbd" title={t("discover.listas.searchKeyboardHint")}>
            /
          </kbd>
        </div>

        <select
          className="select listas-filter-select"
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
          className="select listas-filter-select"
          value={filterState.status}
          onChange={(e) => onStatusChange(e.target.value as StatusFilter)}
          aria-label={t("discover.filterAllAiring")}
        >
          <option value="">{t("discover.listas.statusAll")}</option>
          <option value="RELEASING">{t("discover.listas.statusReleasing")}</option>
          <option value="FINISHED">{t("discover.listas.statusFinished")}</option>
          <option value="NOT_YET_RELEASED">{t("discover.listas.statusNotYetReleased")}</option>
          <option value="CANCELLED">{t("discover.listas.statusCancelled")}</option>
        </select>

        <select
          className="select listas-filter-select"
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
            -
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

        {activeFilterCount > 0 && (
          <button
            type="button"
            className="btn btn-sm btn-ghost"
            onClick={onClearFilters}
          >
            {t("discover.listas.clearAllFilters")} ({activeFilterCount})
          </button>
        )}
      </div>

      <div className="listas-toolbar-secondary">
        <div className="listas-chips-wrap">
          <span className="listas-chip-label">{t("discover.listas.filterFormats")}:</span>
          {AVAILABLE_FORMATS.map((fmt) => {
            const active = filterState.selectedFormats.includes(fmt);
            return (
              <button
                key={fmt}
                type="button"
                className={`chip-toggle${active ? " active" : ""}`}
                onClick={() => onToggleFormat(fmt)}
                aria-pressed={active}
              >
                {fmt}
              </button>
            );
          })}
        </div>

        {displayGenres.length > 0 && (
          <div className="listas-chips-wrap">
            <span className="listas-chip-label">{t("discover.listas.filterGenres")}:</span>
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
      </div>
    </div>
  );
}
