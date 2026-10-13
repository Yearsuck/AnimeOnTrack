import { useT } from "../../../i18n";
import type { ListTab } from "./types";

export interface ListasEmptyProps {
  isFilteredNoResults: boolean;
  activeTab: ListTab;
  onClearFilters: () => void;
  onGoToSwipe?: () => void;
}

export function ListasEmpty({
  isFilteredNoResults,
  activeTab,
  onClearFilters,
  onGoToSwipe,
}: ListasEmptyProps) {
  const t = useT();

  if (isFilteredNoResults) {
    return (
      <div className="listas-empty-box">
        <span className="listas-empty-icon" aria-hidden="true">
          ⌕
        </span>
        <div className="listas-empty-title">{t("discover.listas.noResultsTitle")}</div>
        <div className="listas-empty-desc">{t("discover.listas.noResultsDesc")}</div>
        <button type="button" className="btn btn-primary" onClick={onClearFilters}>
          {t("discover.listas.clearAllFilters")}
        </button>
      </div>
    );
  }

  let title = t("discover.listas.emptyWantTitle");
  let desc = t("discover.listas.emptyWantDesc");
  let icon = "★";

  if (activeTab === "discarded") {
    title = t("discover.listas.emptyDiscardedTitle");
    desc = t("discover.listas.emptyDiscardedDesc");
    icon = "✕";
  } else if (activeTab === "watched") {
    title = t("discover.listas.emptyWatchedTitle");
    desc = t("discover.listas.emptyWatchedDesc");
    icon = "✓";
  }

  return (
    <div className="listas-empty-box">
      <span className="listas-empty-icon" aria-hidden="true">
        {icon}
      </span>
      <div className="listas-empty-title">{title}</div>
      <div className="listas-empty-desc">{desc}</div>
      {onGoToSwipe && (
        <button type="button" className="btn btn-primary" onClick={onGoToSwipe}>
          {t("discover.listas.goToSwipe")}
        </button>
      )}
    </div>
  );
}
