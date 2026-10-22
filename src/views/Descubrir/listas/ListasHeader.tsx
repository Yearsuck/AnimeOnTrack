import { useT } from "../../../i18n";
import type { ListTab, ViewMode } from "./types";

export interface ListasHeaderProps {
  activeTab: ListTab;
  onTabChange: (tab: ListTab) => void;
  counts: {
    want: number;
    discarded: number;
    watched: number;
  };
  viewMode: ViewMode;
  onViewModeChange: (mode: ViewMode) => void;
  selectMode: boolean;
  onSelectModeChange: (on: boolean) => void;
  canSelect: boolean;
}

// Both switches are the app's one segmented control (`.seg`/`.seg-btn`), the
// same as Catálogo's Explorar/Descubrir/Listas and Mi biblioteca's filters.
export function ListasHeader({
  activeTab,
  onTabChange,
  counts,
  viewMode,
  onViewModeChange,
  selectMode,
  onSelectModeChange,
  canSelect,
}: ListasHeaderProps) {
  const t = useT();

  const tabs: { key: ListTab; label: string; count: number }[] = [
    { key: "want", label: t("discover.wantHeading"), count: counts.want },
    { key: "discarded", label: t("discover.discardedHeading"), count: counts.discarded },
    { key: "watched", label: t("discover.watchedHeading"), count: counts.watched },
  ];

  const modes: { key: ViewMode; label: string }[] = [
    { key: "grid", label: t("discover.listas.viewGrid") },
    { key: "list", label: t("discover.listas.viewList") },
  ];

  return (
    <div className="filter-row">
      <div className="seg" role="tablist" aria-label={t("discover.tabLists")}>
        {tabs.map((tb) => (
          <button
            key={tb.key}
            type="button"
            role="tab"
            aria-selected={activeTab === tb.key}
            className={`seg-btn${activeTab === tb.key ? " active" : ""}`}
            onClick={() => onTabChange(tb.key)}
          >
            {tb.label}
            <span className="seg-count">{tb.count}</span>
          </button>
        ))}
      </div>

      <div className="spacer" />

      {selectMode ? (
        <button type="button" className="btn btn-ghost" onClick={() => onSelectModeChange(false)}>
          {t("library.cancelSelect")}
        </button>
      ) : (
        canSelect && (
          <button type="button" className="btn btn-ghost" onClick={() => onSelectModeChange(true)}>
            {t("library.selectMode")}
          </button>
        )
      )}

      <div className="seg" role="group" aria-label={t("discover.listas.viewModeAria")}>
        {modes.map((m) => (
          <button
            key={m.key}
            type="button"
            className={`seg-btn${viewMode === m.key ? " active" : ""}`}
            onClick={() => onViewModeChange(m.key)}
            aria-pressed={viewMode === m.key}
          >
            {m.label}
          </button>
        ))}
      </div>
    </div>
  );
}
