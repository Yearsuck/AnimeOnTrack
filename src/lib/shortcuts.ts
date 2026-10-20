export type ShortcutAction =
  | {
      kind: "navigate";
      view:
        | "pending"
        | "airing"
        | "library"
        | "descubrir"
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
  { keys: "1-7", labelKey: "shortcuts.nav" },
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
  | "descubrir"
  | "catalog"
  | "stats"
  | "settings"
> = {
  "1": "pending",
  "2": "airing",
  "3": "library",
  "4": "descubrir",
  "5": "catalog",
  "6": "stats",
  "7": "settings",
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
 * Decides whether the global search shortcut (`/`) should navigate to the
 * Catalog view. When on the "descubrir" view with the "listas" subview active,
 * ListasToolbar handles `/` locally to focus its own search input instead.
 */
export function shouldNavigateToCatalogOnSearch(
  view: string,
  descubrirSubView?: string
): boolean {
  return !(view === "descubrir" && descubrirSubView === "listas");
}
