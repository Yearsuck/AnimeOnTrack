// Minimal, dependency-free i18n layer (no react-i18next/i18next/FormatJS —
// see docs/superpowers/specs/2026-07-11-i18n-es-en-design.md).
//
// Adding a third language requires only:
//   1. a new `catalog/xx.ts` file exporting `Messages` (see catalog/en.ts —
//      it's type-checked against `catalog/es.ts`'s key set, so a missing
//      key is a `tsc` error)
//   2. one entry appended to `LANGS` below
//   3. one entry appended to `CATALOGS` below
// No view file needs to change.
import {
  createContext,
  createElement,
  useCallback,
  useContext,
  useMemo,
  useState,
  type ReactNode,
} from "react";
import { es } from "./catalog/es";
import { en } from "./catalog/en";
import { ca } from "./catalog/ca";

// Derived from es's key set. `es` is `as const`, so `typeof es` values are
// string *literals*; mapping through `Record<..., string>` keeps the exact
// key set (a missing or misspelled key in another catalog is a `tsc` error,
// which is the whole point) while allowing each translation to be any
// string rather than forcing it to equal the Spanish literal.
export type MessageKey = keyof typeof es;
export type Messages = Record<MessageKey, string>;

export type Lang = "es" | "en" | "ca";

export const LANGS: { code: Lang; label: string }[] = [
  { code: "es", label: "Español" },
  { code: "en", label: "English" },
  { code: "ca", label: "Català" },
];

const CATALOGS: Record<Lang, Messages> = { es, en, ca };

const STORAGE_KEY = "aot.lang";

function isLang(value: unknown): value is Lang {
  return value === "es" || value === "en" || value === "ca";
}

// Read synchronously at provider init so there is no async flash of the
// wrong language on startup — routing this through the Tauri settings
// table instead would be async and would flicker. Default "es" when unset
// or invalid.
function getInitialLang(): Lang {
  try {
    const stored = localStorage.getItem(STORAGE_KEY);
    if (isLang(stored)) return stored;
  } catch {
    // localStorage unavailable (privacy mode, etc.) — fall through to default.
  }
  return "es";
}

export function interpolate(template: string, params?: Record<string, string | number>): string {
  if (!params) return template;
  return template.replace(/\{(\w+)\}/g, (match, key: string) =>
    Object.prototype.hasOwnProperty.call(params, key) ? String(params[key]) : match
  );
}

interface I18nContextValue {
  lang: Lang;
  setLang: (lang: Lang) => void;
}

const I18nContext = createContext<I18nContextValue | null>(null);

export function I18nProvider({ children }: { children: ReactNode }) {
  const [lang, setLangState] = useState<Lang>(getInitialLang);

  const setLang = useCallback((next: Lang) => {
    setLangState(next);
    try {
      localStorage.setItem(STORAGE_KEY, next);
    } catch {
      // localStorage unavailable — the choice just won't persist.
    }
  }, []);

  const value = useMemo<I18nContextValue>(() => ({ lang, setLang }), [lang, setLang]);

  return createElement(I18nContext.Provider, { value }, children);
}

function useI18nContext(): I18nContextValue {
  const ctx = useContext(I18nContext);
  if (!ctx) throw new Error("useT/useLang must be used within an <I18nProvider>");
  return ctx;
}

export function useLang(): I18nContextValue {
  return useI18nContext();
}

// t identity changes whenever `lang` changes, so consuming components
// re-render on language switch.
export function useT() {
  const { lang } = useI18nContext();
  return useCallback(
    (key: MessageKey, params?: Record<string, string | number>) => {
      const template = CATALOGS[lang][key] ?? CATALOGS.es[key];
      return interpolate(template, params);
    },
    [lang]
  );
}
