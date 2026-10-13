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
    <div className="listas-smart-groups" role="toolbar" aria-label={t("discover.listas.smartAiring")}>
      <span className="listas-smart-label">⚡</span>
      {groups.map((grp) => {
        const active = quickFilter === grp.key;
        return (
          <button
            key={grp.key}
            type="button"
            className={`listas-smart-chip${active ? " active" : ""}`}
            onClick={() => onToggleQuickFilter(grp.key)}
            aria-pressed={active}
          >
            <span>{grp.label}</span>
            <span className="listas-smart-count">{grp.count}</span>
          </button>
        );
      })}
    </div>
  );
}
