// Minimal, dependency-free theme layer mirroring src/i18n/index.ts's
// pattern exactly (see docs/superpowers/specs/2026-07-12-theme-light-dark-design.md).
//
// The color ramp lives entirely in CSS custom properties (src/styles.css):
// `:root` is the dark baseline (so the app renders dark even before JS
// runs — no flash), `:root[data-theme="light"]` overrides the color tokens
// for the light theme. This module's only job is to read/persist the
// user's choice and stamp `data-theme` on <html>.
import {
  createContext,
  createElement,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useState,
  type ReactNode,
} from "react";

export type Theme = "dark" | "light";

export const THEMES: { code: Theme; labelKey: "settings.themeDark" | "settings.themeLight" }[] = [
  { code: "dark", labelKey: "settings.themeDark" },
  { code: "light", labelKey: "settings.themeLight" },
];

const STORAGE_KEY = "aot.theme";

function isTheme(value: unknown): value is Theme {
  return value === "dark" || value === "light";
}

// Read synchronously so there is no async flash of the wrong theme on
// startup. Default "dark" when unset or invalid.
function getInitialTheme(): Theme {
  try {
    const stored = localStorage.getItem(STORAGE_KEY);
    if (isTheme(stored)) return stored;
  } catch {
    // localStorage unavailable (privacy mode, etc.) — fall through to default.
  }
  return "dark";
}

// `:root`'s default is already the dark ramp, but we set the attribute
// explicitly for both themes for clarity (and so a future default flip is a
// one-line change, not an implicit-absence bug).
function applyTheme(theme: Theme): void {
  document.documentElement.setAttribute("data-theme", theme);
}

// Apply the initial theme at module load, before React renders, to avoid a
// flash of the wrong theme (mirrors i18n's synchronous getInitialLang read,
// but i18n has no DOM side effect to apply — theme does).
applyTheme(getInitialTheme());

interface ThemeContextValue {
  theme: Theme;
  setTheme: (theme: Theme) => void;
}

const ThemeContext = createContext<ThemeContextValue | null>(null);

export function ThemeProvider({ children }: { children: ReactNode }) {
  const [theme, setThemeState] = useState<Theme>(getInitialTheme);

  // Keep the DOM attribute in sync with state (covers the module-load call
  // above running once, plus any external change to state).
  useEffect(() => {
    applyTheme(theme);
  }, [theme]);

  const setTheme = useCallback((next: Theme) => {
    setThemeState(next);
    try {
      localStorage.setItem(STORAGE_KEY, next);
    } catch {
      // localStorage unavailable — the choice just won't persist.
    }
  }, []);

  const value = useMemo<ThemeContextValue>(() => ({ theme, setTheme }), [theme, setTheme]);

  return createElement(ThemeContext.Provider, { value }, children);
}

export function useTheme(): ThemeContextValue {
  const ctx = useContext(ThemeContext);
  if (!ctx) throw new Error("useTheme must be used within a <ThemeProvider>");
  return ctx;
}
