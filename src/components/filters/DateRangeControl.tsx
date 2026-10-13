import { useT } from "../../i18n";
import { dateInputToEpoch, epochToDateInput } from "./dateHelpers";
import "../../styles/filters.css";

export interface DateRangeControlProps {
  minStartDate: number | null | undefined;
  maxStartDate: number | null | undefined;
  onChange: (min: number | null, max: number | null) => void;
  label?: string;
}

export function DateRangeControl({
  minStartDate,
  maxStartDate,
  onChange,
  label,
}: DateRangeControlProps) {
  const t = useT();
  const currentMin = minStartDate ?? null;
  const currentMax = maxStartDate ?? null;

  const currentYear = new Date().getUTCFullYear();

  const presets = [
    {
      id: "this_year",
      label: t("filters.dateThisYear"),
      min: dateInputToEpoch(`${currentYear}-01-01`),
      max: dateInputToEpoch(`${currentYear}-12-31`),
    },
    {
      id: "last_5_years",
      label: t("filters.dateLast5Years"),
      min: dateInputToEpoch(`${currentYear - 4}-01-01`),
      max: dateInputToEpoch(`${currentYear}-12-31`),
    },
    {
      id: "2020s",
      label: t("filters.date2020s"),
      min: dateInputToEpoch("2020-01-01"),
      max: dateInputToEpoch("2029-12-31"),
    },
    {
      id: "2010s",
      label: t("filters.date2010s"),
      min: dateInputToEpoch("2010-01-01"),
      max: dateInputToEpoch("2019-12-31"),
    },
    {
      id: "2000s",
      label: t("filters.date2000s"),
      min: dateInputToEpoch("2000-01-01"),
      max: dateInputToEpoch("2009-12-31"),
    },
    {
      id: "before_2000",
      label: t("filters.dateBefore2000"),
      min: null,
      max: dateInputToEpoch("1999-12-31"),
    },
  ];

  const hasValue = currentMin !== null || currentMax !== null;

  return (
    <div className="filter-group filter-date-control">
      {label && <span className="filter-label">{label}</span>}
      <div className="filter-date-presets" role="group" aria-label={t("filters.datePresetsAria")}>
        {presets.map((p) => {
          const isActive = currentMin === p.min && currentMax === p.max;
          return (
            <button
              key={p.id}
              type="button"
              className={`filter-preset-chip${isActive ? " active" : ""}`}
              onClick={() => onChange(p.min, p.max)}
              aria-pressed={isActive}
            >
              {p.label}
            </button>
          );
        })}
      </div>

      <div className="filter-date-inputs">
        <label className="filter-date-field">
          <span>{t("filters.dateFrom")}</span>
          <input
            type="date"
            className="filter-date-input"
            value={currentMin !== null ? epochToDateInput(currentMin) : ""}
            onChange={(e) =>
              onChange(e.target.value ? dateInputToEpoch(e.target.value) : null, currentMax)
            }
            aria-label={t("filters.dateFromAria")}
          />
        </label>
        <label className="filter-date-field">
          <span>{t("filters.dateTo")}</span>
          <input
            type="date"
            className="filter-date-input"
            value={currentMax !== null ? epochToDateInput(currentMax) : ""}
            onChange={(e) =>
              onChange(currentMin, e.target.value ? dateInputToEpoch(e.target.value) : null)
            }
            aria-label={t("filters.dateToAria")}
          />
        </label>
        {hasValue && (
          <button
            type="button"
            className="filter-date-clear-btn"
            onClick={() => onChange(null, null)}
            aria-label={t("filters.dateClear")}
          >
            {t("filters.dateClear")}
          </button>
        )}
      </div>
    </div>
  );
}
