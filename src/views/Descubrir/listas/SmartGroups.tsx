import { useT } from "../../../i18n";
import type { QuickFilter } from "./types";

export interface SmartGroupsProps {
  quickFilter: QuickFilter;
  onToggleQuickFilter: (qf: QuickFilter) => void;
  counts: {
    airing: number;
    topRated: number;
    short: number;
  };
}

// Quick filters as ordinary filter chips (`.chip-toggle`), with the count in
// the same pill the segmented controls use.
export function SmartGroups({
  quickFilter,
  onToggleQuickFilter,
  counts,
}: SmartGroupsProps) {
  const t = useT();

  const groups: { key: QuickFilter; label: string; count: number }[] = [
    { key: "airing", label: t("discover.listas.smartAiring"), count: counts.airing },
    { key: "top_rated", label: t("discover.listas.smartTopRated"), count: counts.topRated },
    { key: "short", label: t("discover.listas.smartShort"), count: counts.short },
  ];

  return (
    <div className="chip-row" role="toolbar" aria-label={t("discover.listas.quickFiltersAria")}>
      {groups.map((grp) => {
        const active = quickFilter === grp.key;
        return (
          <button
            key={grp.key}
            type="button"
            className={`chip-toggle${active ? " active" : ""}`}
            onClick={() => onToggleQuickFilter(grp.key)}
            aria-pressed={active}
          >
            {grp.label}
            <span className="seg-count">{grp.count}</span>
          </button>
        );
      })}
    </div>
  );
}
