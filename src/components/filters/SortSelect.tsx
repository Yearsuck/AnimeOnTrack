import { useT } from "../../i18n";
import "../../styles/filters.css";

export interface SortSelectProps {
  value: string;
  onChange: (value: string) => void;
  label?: string;
  className?: string;
  options?: { value: string; label: string }[];
}

export function SortSelect({
  value,
  onChange,
  label,
  className = "input filter-select",
  options,
}: SortSelectProps) {
  const t = useT();

  const defaultOptions = [
    { value: "popularity", label: t("filters.sortPopularity") },
    { value: "score_desc", label: t("filters.sortScoreDesc") },
    { value: "score_asc", label: t("filters.sortScoreAsc") },
    { value: "newest", label: t("filters.sortNewest") },
    { value: "oldest", label: t("filters.sortOldest") },
    { value: "title", label: t("filters.sortTitle") },
  ];

  const sortOptions = options ?? defaultOptions;

  return (
    <label className="filter-select-control">
      {label && <span className="filter-label">{label}</span>}
      <select
        className={className}
        value={value}
        onChange={(e) => onChange(e.target.value)}
        aria-label={label || t("filters.sortLabel")}
      >
        {sortOptions.map((opt) => (
          <option key={opt.value} value={opt.value}>
            {opt.label}
          </option>
        ))}
      </select>
    </label>
  );
}
