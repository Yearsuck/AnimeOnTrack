import { useT } from "../../i18n";
import "../../styles/filters.css";

export interface StatusSelectProps {
  value: string;
  onChange: (value: string) => void;
  label?: string;
  className?: string;
}

export function StatusSelect({
  value,
  onChange,
  label,
  className = "input filter-select",
}: StatusSelectProps) {
  const t = useT();

  return (
    <label className="filter-select-control">
      {label && <span className="filter-label">{label}</span>}
      <select
        className={className}
        value={value}
        onChange={(e) => onChange(e.target.value)}
        aria-label={label || t("filters.statusLabel")}
      >
        <option value="">{t("filters.statusAll")}</option>
        <option value="RELEASING">{t("filters.statusReleasing")}</option>
        <option value="FINISHED">{t("filters.statusFinished")}</option>
        <option value="NOT_YET_RELEASED">{t("filters.statusNotYetReleased")}</option>
        <option value="CANCELLED">{t("filters.statusCancelled")}</option>
        <option value="HIATUS">{t("filters.statusHiatus")}</option>
      </select>
    </label>
  );
}
