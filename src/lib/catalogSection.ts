// Catálogo hosts three sub-sections. "Descubrir" (swipe deck) and "Listas"
// used to be a top-level tab of their own; they now live here.
export type CatalogSection = "explore" | "swipe" | "listas";

export const CATALOG_SECTIONS: readonly CatalogSection[] = ["explore", "swipe", "listas"];

export const DEFAULT_CATALOG_SECTION: CatalogSection = "explore";

export function isCatalogSection(v: unknown): v is CatalogSection {
  return typeof v === "string" && (CATALOG_SECTIONS as readonly string[]).includes(v);
}

/** The Descubrir module only renders the swipe deck or Listas. */
export function discoverSubViewFor(section: CatalogSection): "swipe" | "listas" {
  return section === "listas" ? "listas" : "swipe";
}
