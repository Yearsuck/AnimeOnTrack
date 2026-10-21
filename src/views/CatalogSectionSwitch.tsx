import { useT } from "../i18n";
import { CATALOG_SECTIONS, type CatalogSection } from "../lib/catalogSection";

const LABEL_KEY = {
  explore: "catalog.sectionExplore",
  swipe: "nav.discover",
  listas: "discover.tabLists",
} as const;

// Explorar / Descubrir / Listas segmented control shown in the Catálogo page
// head. `onHover` lets the host warm the lazy chunk of the section.
export function CatalogSectionSwitch({
  value,
  onChange,
  onHover,
}: {
  value: CatalogSection;
  onChange: (s: CatalogSection) => void;
  onHover?: (s: CatalogSection) => void;
}) {
  const t = useT();
  return (
    <div className="seg catalog-sections" role="tablist" aria-label={t("catalog.sectionsAria")}>
      {CATALOG_SECTIONS.map((s) => (
        <button
          key={s}
          type="button"
          role="tab"
          aria-selected={value === s}
          className={`seg-btn${value === s ? " active" : ""}`}
          onClick={() => onChange(s)}
          onMouseEnter={() => onHover?.(s)}
          onFocus={() => onHover?.(s)}
        >
          {t(LABEL_KEY[s])}
        </button>
      ))}
    </div>
  );
}
