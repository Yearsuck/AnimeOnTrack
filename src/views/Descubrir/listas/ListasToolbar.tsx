import { useEffect, useRef, type ReactNode } from "react";
import { useT } from "../../../i18n";
import { formatLabel } from "../../../lib/formatLabel";
import { MIN_SCORE_OPTIONS, type ListasFilterState, type SortKey, type StatusFilter } from "./types";
import "../../../styles/filters.css";
import "../../../styles/catalog.css";

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
  resultCount: number;
  drawerOpen: boolean;
  onToggleDrawer: () => void;
  /** Quick-filter chips (Quiero ver only), first in the chips row. */
  quickFilters?: ReactNode;
}

const STATUS_KEY = {
  RELEASING: "discover.listas.statusReleasing",
  FINISHED: "discover.listas.statusFinished",
  NOT_YET_RELEASED: "discover.listas.statusNotYetReleased",
  CANCELLED: "discover.listas.statusCancelled",
} as const;

// Same shape as Explorar: search + sort + "Filtros" (side drawer) on one row,
// then the quick filters and every active filter as a removable chip.
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
  resultCount,
  drawerOpen,
  onToggleDrawer,
  quickFilters,
}: ListasToolbarProps) {
  const t = useT();
  const searchInputRef = useRef<HTMLInputElement>(null);

  // `/` focuses search, Esc clears it.
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

  const chips: { id: string; label: string; remove: () => void }[] = [];
  if (filterState.status)
    chips.push({ id: "status", label: t(STATUS_KEY[filterState.status]), remove: () => onStatusChange("") });
  for (const f of filterState.selectedFormats)
    chips.push({ id: `fmt:${f}`, label: formatLabel(t, f), remove: () => onFormatChange("") });
  for (const g of filterState.selectedGenres)
    chips.push({ id: `genre:${g}`, label: g, remove: () => onToggleGenre(g) });
  if (filterState.minScore != null)
    chips.push({
      id: "score",
      label: t("discover.listas.minScoreVal", { score: filterState.minScore }),
      remove: () => onMinScoreChange(null),
    });
  if (filterState.yearFrom != null || filterState.yearTo != null)
    chips.push({
      id: "years",
      label: `${filterState.yearFrom ?? "…"}–${filterState.yearTo ?? "…"}`,
      remove: () => {
        onYearFromChange(null);
        onYearToChange(null);
      },
    });

  return (
    <div className="listas-toolbar" role="region" aria-label={t("discover.searchAriaLabel")}>
      <div className="listas-toolbar-row">
        <div className="search listas-search">
          <span className="icon" aria-hidden="true">
            ⌕
          </span>
          <input
            ref={searchInputRef}
            className="input"
            type="search"
            placeholder={t("discover.searchPlaceholderListas")}
            aria-label={t("discover.searchAriaLabel")}
            title={t("discover.listas.searchKeyboardHint")}
            value={filterState.query}
            onChange={(e) => onQueryChange(e.target.value)}
          />
        </div>
        <select
          className="input filter-select"
          value={filterState.sortBy}
          onChange={(e) => onSortChange(e.target.value as SortKey)}
          aria-label={t("discover.sortTitle")}
        >
          <option value="recent">{t("discover.listas.sortRecent")}</option>
          <option value="title">{t("discover.listas.sortTitle")}</option>
          <option value="score_desc">{t("discover.listas.sortScoreDesc")}</option>
          <option value="newest">{t("discover.listas.sortNewest")}</option>
          <option value="oldest">{t("discover.listas.sortOldest")}</option>
          <option value="episodes">{t("discover.listas.sortEpisodes")}</option>
        </select>
        <button
          type="button"
          className={`btn cat-filter-btn${drawerOpen ? " active" : ""}`}
          onClick={onToggleDrawer}
          aria-expanded={drawerOpen}
          aria-controls="listas-filter-drawer"
        >
          {t("catalog.filters")}
          {chips.length > 0 && <span className="filter-active-badge">{chips.length}</span>}
        </button>
      </div>

      <div className="cat-active listas-active">
        <span className="cat-count">{t("catalog.resultsCount", { count: resultCount })}</span>
        {quickFilters}
        {chips.map((c) => (
          <button
            key={c.id}
            type="button"
            className="cat-chip"
            onClick={c.remove}
            aria-label={t("catalog.removeFilter", { label: c.label })}
          >
            {c.label}
            <span aria-hidden="true" className="cat-chip-x">
              ✕
            </span>
          </button>
        ))}
        {activeFilterCount > 0 && (
          <button type="button" className="btn btn-ghost btn-sm" onClick={onClearFilters}>
            {t("catalog.backToBrowse")}
          </button>
        )}
      </div>
    </div>
  );
}

/** Every Listas filter in Explorar's side drawer; changes apply at once. */
export function ListasFilterDrawer({
  filterState,
  onFormatChange,
  onStatusChange,
  onToggleGenre,
  onMinScoreChange,
  onYearFromChange,
  onYearToChange,
  availableFormats,
  availableGenres,
  onClose,
}: Pick<
  ListasToolbarProps,
  | "filterState"
  | "onFormatChange"
  | "onStatusChange"
  | "onToggleGenre"
  | "onMinScoreChange"
  | "onYearFromChange"
  | "onYearToChange"
> & {
  availableFormats: string[];
  availableGenres: string[];
  onClose: () => void;
}) {
  const t = useT();
  const format = filterState.selectedFormats[0] ?? "";
  return (
    <aside
      id="listas-filter-drawer"
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
        <button type="button" className="btn btn-ghost btn-sm" onClick={onClose} aria-label={t("catalog.filtersClose")}>
          ✕
        </button>
      </div>

      <fieldset className="cat-drawer-group">
        <legend className="cat-drawer-legend">{t("filters.statusLabel")}</legend>
        <div className="chip-row">
          {(Object.keys(STATUS_KEY) as Exclude<StatusFilter, "">[]).map((st) => (
            <button
              key={st}
              type="button"
              className={`chip-toggle${filterState.status === st ? " active" : ""}`}
              aria-pressed={filterState.status === st}
              onClick={() => onStatusChange(filterState.status === st ? "" : st)}
            >
              {t(STATUS_KEY[st])}
            </button>
          ))}
        </div>
      </fieldset>

      {availableGenres.length > 0 && (
        <fieldset className="cat-drawer-group">
          <legend className="cat-drawer-legend">{t("catalog.genres")}</legend>
          <div className="chip-row">
            {availableGenres.map((g) => {
              const active = filterState.selectedGenres.includes(g);
              return (
                <button
                  key={g}
                  type="button"
                  className={`chip-toggle${active ? " active" : ""}`}
                  aria-pressed={active}
                  onClick={() => onToggleGenre(g)}
                >
                  {g}
                </button>
              );
            })}
          </div>
        </fieldset>
      )}

      <div className="cat-drawer-pair">
        <label className="cat-drawer-field">
          <span className="cat-drawer-legend">{t("catalog.format")}</span>
          <select className="input" value={format} onChange={(e) => onFormatChange(e.target.value)}>
            <option value="">{t("catalog.anyFormat")}</option>
            {availableFormats.map((f) => (
              <option key={f} value={f}>
                {formatLabel(t, f)}
              </option>
            ))}
          </select>
        </label>
        <label className="cat-drawer-field">
          <span className="cat-drawer-legend">{t("catalog.scoreRangeLabel")}</span>
          <select
            className="input"
            value={filterState.minScore ?? ""}
            onChange={(e) => onMinScoreChange(e.target.value ? Number(e.target.value) : null)}
          >
            <option value="">{t("discover.listas.minScoreAll")}</option>
            {MIN_SCORE_OPTIONS.map((score) => (
              <option key={score} value={score}>
                {t("discover.listas.minScoreVal", { score })}
              </option>
            ))}
          </select>
        </label>
      </div>

      <fieldset className="cat-drawer-group">
        <legend className="cat-drawer-legend">{t("discover.filtersDateHeading")}</legend>
        <div className="listas-year-inputs">
          <input
            type="number"
            className="input listas-year-input"
            placeholder={t("discover.listas.yearFrom")}
            aria-label={t("discover.listas.yearFrom")}
            value={filterState.yearFrom ?? ""}
            onChange={(e) => onYearFromChange(e.target.value.trim() ? Number(e.target.value) : null)}
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
            onChange={(e) => onYearToChange(e.target.value.trim() ? Number(e.target.value) : null)}
          />
        </div>
      </fieldset>
    </aside>
  );
}
