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
}

export function ListasHeader({
  activeTab,
  onTabChange,
  counts,
  viewMode,
  onViewModeChange,
}: ListasHeaderProps) {
  const t = useT();

  const tabs: { key: ListTab; label: string; count: number }[] = [
    { key: "want", label: t("discover.wantHeading"), count: counts.want },
    { key: "discarded", label: t("discover.discardedHeading"), count: counts.discarded },
    { key: "watched", label: t("discover.watchedHeading"), count: counts.watched },
  ];

  return (
    <div className="listas-header">
      <div className="seg listas-seg" role="tablist">
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
            <span className="listas-seg-count">{tb.count}</span>
          </button>
        ))}
      </div>

      <div
        className="listas-view-modes"
        role="group"
        aria-label={t("discover.listas.viewModeAria")}
      >
        <button
          type="button"
          className={`listas-view-mode-btn${viewMode === "grid" ? " active" : ""}`}
          onClick={() => onViewModeChange("grid")}
          aria-pressed={viewMode === "grid"}
          title={t("discover.listas.viewGrid")}
        >
          <span aria-hidden="true">⊞</span>
          <span>{t("discover.listas.viewGrid")}</span>
        </button>
        <button
          type="button"
          className={`listas-view-mode-btn${viewMode === "list" ? " active" : ""}`}
          onClick={() => onViewModeChange("list")}
          aria-pressed={viewMode === "list"}
          title={t("discover.listas.viewList")}
        >
          <span aria-hidden="true">☰</span>
          <span>{t("discover.listas.viewList")}</span>
        </button>
      </div>
    </div>
  );
}
