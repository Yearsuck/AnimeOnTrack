import { useCallback, useEffect, useMemo, useState } from "react";
import { useT } from "../../i18n";
import type { Series } from "../../types";
import "../../styles/listas.css";

import type { ListTab, ViewMode } from "./listas/types";
import { VIEW_MODE_STORAGE_KEY } from "./listas/types";
import { useListasData } from "./listas/useListasData";
import { useListasFilter } from "./listas/useListasFilter";
import { ListasHeader } from "./listas/ListasHeader";
import { SmartGroups } from "./listas/SmartGroups";
import { ListasToolbar } from "./listas/ListasToolbar";
import { ListasCard } from "./listas/ListasCard";
import { ListasDenseRow } from "./listas/ListasDenseRow";
import { BulkActionBar } from "./listas/BulkActionBar";
import { ListasEmpty } from "./listas/ListasEmpty";
import { ListasSkeleton } from "./listas/ListasSkeleton";
import { ListasError } from "./listas/ListasError";

// Cards rendered per step: the discarded list alone can hold thousands of
// entries, and mounting them all at once froze the screen for seconds.
const PAGE_SIZE = 120;

export function ListasView({
  onOpenSeries,
  onSwitchToSwipe,
}: {
  onOpenSeries: (s: Series) => void;
  onSwitchToSwipe?: () => void;
}) {
  const t = useT();
  const [limit, setLimit] = useState(PAGE_SIZE);
  const [activeTab, setActiveTab] = useState<ListTab>("want");
  const [selectedIds, setSelectedIds] = useState<Set<number>>(new Set());

  // View mode persistence in localStorage
  const [viewMode, setViewMode] = useState<ViewMode>(() => {
    try {
      const saved = localStorage.getItem(VIEW_MODE_STORAGE_KEY);
      return saved === "list" ? "list" : "grid";
    } catch {
      return "grid";
    }
  });

  const handleViewModeChange = useCallback((mode: ViewMode) => {
    setViewMode(mode);
    try {
      localStorage.setItem(VIEW_MODE_STORAGE_KEY, mode);
    } catch {
      // localStorage unavailable
    }
  }, []);

  const {
    want,
    discarded,
    watched,
    loading,
    error,
    reload,
    entriesFor,
  } = useListasData();

  const currentRawList = useMemo(() => {
    if (activeTab === "want") return want;
    if (activeTab === "discarded") return discarded;
    return watched;
  }, [activeTab, want, discarded, watched]);

  const currentEntries = useMemo(() => {
    return entriesFor(currentRawList);
  }, [entriesFor, currentRawList]);

  const {
    filterState,
    setQuery,
    setSortBy,
    toggleFormat,
    setStatus,
    toggleGenre,
    setMinScore,
    setYearFrom,
    setYearTo,
    toggleQuickFilter,
    resetFilters,
    availableGenres,
    topGenres,
    smartCounts,
    activeFilterCount,
    filteredEntries,
  } = useListasFilter(currentEntries);

  const handleTabChange = useCallback((tab: ListTab) => {
    setActiveTab(tab);
    setSelectedIds(new Set());
    resetFilters();
  }, [resetFilters]);

  const toggleSelect = useCallback((id: number) => {
    setSelectedIds((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  }, []);

  const selectAllVisible = useCallback(() => {
    setSelectedIds(new Set(filteredEntries.map((e) => e.series.id)));
  }, [filteredEntries]);

  const deselectAll = useCallback(() => {
    setSelectedIds(new Set());
  }, []);

  // New filters or another tab start from the first page again; a reload
  // after acting on a card keeps the current page and scroll position.
  useEffect(() => {
    setLimit(PAGE_SIZE);
  }, [filterState, activeTab]);

  // Never act on entries that are no longer on screen (filtered out, or
  // moved to another list by a single-card action).
  useEffect(() => {
    setSelectedIds((prev) => {
      if (prev.size === 0) return prev;
      const visible = new Set(filteredEntries.map((e) => e.series.id));
      const next = new Set([...prev].filter((id) => visible.has(id)));
      return next.size === prev.size ? prev : next;
    });
  }, [filteredEntries]);
  const shownEntries = useMemo(() => filteredEntries.slice(0, limit), [filteredEntries, limit]);
  const remaining = filteredEntries.length - shownEntries.length;

  return (
    <div className="listas-container">
      {error && <ListasError error={error} onRetry={reload} />}

      <ListasHeader
        activeTab={activeTab}
        onTabChange={handleTabChange}
        counts={{
          want: want.length,
          discarded: discarded.length,
          watched: watched.length,
        }}
        viewMode={viewMode}
        onViewModeChange={handleViewModeChange}
      />

      {activeTab === "want" && (
        <SmartGroups
          quickFilter={filterState.quickFilter}
          onToggleQuickFilter={toggleQuickFilter}
          counts={smartCounts}
        />
      )}

      <ListasToolbar
        filterState={filterState}
        onQueryChange={setQuery}
        onSortChange={setSortBy}
        onToggleFormat={toggleFormat}
        onStatusChange={setStatus}
        onToggleGenre={toggleGenre}
        onMinScoreChange={setMinScore}
        onYearFromChange={setYearFrom}
        onYearToChange={setYearTo}
        onClearFilters={resetFilters}
        activeFilterCount={activeFilterCount}
        availableGenres={availableGenres}
        topGenres={topGenres}
      />

      {loading ? (
        <ListasSkeleton viewMode={viewMode} />
      ) : currentRawList.length === 0 ? (
        <ListasEmpty
          isFilteredNoResults={false}
          activeTab={activeTab}
          onClearFilters={resetFilters}
          onGoToSwipe={onSwitchToSwipe}
        />
      ) : filteredEntries.length === 0 ? (
        <ListasEmpty
          isFilteredNoResults={true}
          activeTab={activeTab}
          onClearFilters={resetFilters}
        />
      ) : viewMode === "grid" ? (
        <div className="listas-cards-grid">
          {shownEntries.map((entry) => (
            <ListasCard
              key={entry.series.id}
              entry={entry}
              activeTab={activeTab}
              isSelected={selectedIds.has(entry.series.id)}
              onToggleSelect={() => toggleSelect(entry.series.id)}
              onOpenSeries={onOpenSeries}
              onChanged={reload}
            />
          ))}
        </div>
      ) : (
        <div className="listas-dense-container">
          {shownEntries.map((entry) => (
            <ListasDenseRow
              key={entry.series.id}
              entry={entry}
              activeTab={activeTab}
              isSelected={selectedIds.has(entry.series.id)}
              onToggleSelect={() => toggleSelect(entry.series.id)}
              onOpenSeries={onOpenSeries}
              onChanged={reload}
            />
          ))}
        </div>
      )}

      {remaining > 0 && !loading && (
        <div className="listas-show-more">
          <button type="button" className="btn" onClick={() => setLimit((l) => l + PAGE_SIZE)}>
            {t("discover.listas.showMore", { count: String(remaining) })}
          </button>
        </div>
      )}

      <BulkActionBar
        activeTab={activeTab}
        selectedIds={selectedIds}
        visibleEntries={filteredEntries}
        onSelectAllVisible={selectAllVisible}
        onDeselectAll={deselectAll}
        onRetainSelection={(ids) => setSelectedIds(new Set(ids))}
        onChanged={reload}
      />
    </div>
  );
}

export default ListasView;
