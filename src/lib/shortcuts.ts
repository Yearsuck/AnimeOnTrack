export type ShortcutAction =
  | {
      kind: "navigate";
      view:
        | "pending"
        | "airing"
        | "library"
        | "catalog"
        | "stats"
        | "settings";
    }
  | { kind: "refresh" }
  | { kind: "search" }
  | { kind: "help" }
  | { kind: "escape" };

export interface ShortcutEvent {
  key: string;
  ctrlKey: boolean;
  metaKey: boolean;
  altKey: boolean;
  shiftKey: boolean;
  targetIsEditable: boolean;
}

export interface ShortcutRow {
  keys: string;
  labelKey: string;
}

export const SHORTCUT_ROWS: ShortcutRow[] = [
  { keys: "1-6", labelKey: "shortcuts.nav" },
  { keys: "R", labelKey: "shortcuts.refresh" },
  { keys: "/", labelKey: "shortcuts.search" },
  { keys: "?", labelKey: "shortcuts.help" },
  { keys: "Esc", labelKey: "shortcuts.escape" },
];

const NAV_VIEWS: Record<
  string,
  | "pending"
  | "airing"
  | "library"
  | "catalog"
  | "stats"
  | "settings"
> = {
  "1": "pending",
  "2": "airing",
  "3": "library",
  "4": "catalog",
  "5": "stats",
  "6": "settings",
};

export function shortcutFor(e: {
  key: string;
  ctrlKey: boolean;
  metaKey: boolean;
  altKey: boolean;
  shiftKey: boolean;
  targetIsEditable: boolean;
}): ShortcutAction | null {
  if (e.targetIsEditable) return null;
  if (e.ctrlKey || e.metaKey || e.altKey) return null;
  if (e.shiftKey && e.key !== "?") return null;

  if (e.key === "Escape") {
    return { kind: "escape" };
  }
  if (e.key === "?") {
    return { kind: "help" };
  }
  if (e.key === "/") {
    return { kind: "search" };
  }
  if (e.key === "r" || e.key === "R") {
    return { kind: "refresh" };
  }
  const view = NAV_VIEWS[e.key];
  if (view) {
    return { kind: "navigate", view };
  }

  return null;
}

/**
 * Decides whether the global search shortcut (`/`) should jump to the Catalog
 * explorer. When Catálogo is already showing its "listas" section,
 * ListasToolbar handles `/` locally to focus its own search input instead.
 */
export function shouldNavigateToCatalogOnSearch(
  view: string,
  catalogSection?: string
): boolean {
  return !(view === "catalog" && catalogSection === "listas");
}
