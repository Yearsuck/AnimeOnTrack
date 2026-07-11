# i18n — modular es/en support across the whole UI

**Date:** 2026-07-11
**Branch to implement on:** `feat/i18n-es-en` (from `develop`)
**Task:** #1 of the 2026-07-11 batch. Foundation task — must land first so every later
UI change is born internationalized.

## Problem

The entire UI is hardcoded in Spanish (nav tabs in `App.tsx`, every `src/views/*.tsx`
label, button, and status message). There is no language layer. We want:

1. A clean, modular i18n layer: all strings in an es/en catalog.
2. A language selector in Ajustes (Settings).
3. The choice persisted across restarts.
4. Adding a third language must be trivial (add one catalog file + one registry entry).
5. Cover backend user-facing text where it exists.

No heavy dependency (react-i18next / i18next / FormatJS) unless a lightweight approach
can't do the job — it can, so we build a ~80-line custom layer that fits the existing
hand-rolled design system (no component library, no Tailwind — see CLAUDE.md).

## Real codebase context (verified)

- **Mount point:** `src/main.tsx` renders `<React.StrictMode><App /></React.StrictMode>`.
  No provider currently wraps `App`.
- **Strings live in:** `src/App.tsx` (nav tabs `Pendientes`/`En emisión`/`Biblioteca`/
  `Descubrir`/`Catálogo`/`Estadísticas`/`Ajustes`, `↻ Actualizar`, `Actualizando…`,
  `Cargando…`) and each `src/views/*.tsx`. Line counts: App 184, AiringGrid 110,
  Catalog 298, Descubrir 565, Library 268, Onboarding 57, Pending 102, ProgressBar 64,
  SeriesDetail 182, Settings 220, Stats 143, StatsRings 93, StatsGraph 344 (StatsGraph
  is mostly three.js — few user strings).
- **Dynamic messages** are built frontend-side, e.g. `Settings.tsx:57`
  ``setMsg(`Cambiado a ${result.site.name}. Encontradas ${s.length} series en emisión.`)``
  and `:71`, `:85`, `:112`. These need interpolation, not just static lookup.
- **Backend user-facing strings** are only error messages returned via `Result::Err`,
  surfaced in the UI as `setMsg(String(e))`. Examples in `src-tauri/src/commands.rs`:
  `:178` `"url inválida: {url}"`, `:322` mirror-mismatch, `:330`
  `"ninguna web funcionó; último error: {last_err}"`, `:486` `"…todavía no tiene un
  adaptador implementado"`, `:980` `"no se encontraron géneros; reintenta el escaneo"`.
  These are rare, technical, and constructed dynamically in Rust. Full Rust-side i18n is
  out of scope (fragile, disproportionate). See "Backend strings" below.
- No `Intl`/locale formatting today; countdowns/dates are formatted ad-hoc in views.

## Design

### Module layout — `src/i18n/`

```
src/i18n/
  index.ts        # types, Lang, LANGS registry, context, I18nProvider, useT, useLang, interpolate
  catalog/es.ts   # source of truth: Messages object, Spanish
  catalog/en.ts   # English, typed `Messages` so missing/extra keys are a COMPILE error
```

- `es.ts` exports `const es = { ... } as const` — the canonical key set.
- `type Messages = typeof es` derived from es. `en.ts`: `const en: Messages = { ... }`.
  A missing or misspelled key in `en` is a `tsc` error → guarantees full coverage.
- Keys are flat dotted strings grouped by area:
  `nav.pending`, `nav.airing`, …, `common.refresh`, `common.refreshing`, `common.loading`,
  `common.cancel`, `settings.title`, `settings.activeSite`, `settings.foundSeries`,
  `library.watching`, `pending.title`, etc. Grouping is by convention in the key string;
  the object itself is flat (`{ "nav.pending": "Pendientes", … }`) to keep `Messages`
  typing simple and lookups O(1).

### Runtime — `index.ts`

```ts
export type Lang = "es" | "en";
export const LANGS: { code: Lang; label: string }[] = [
  { code: "es", label: "Español" },
  { code: "en", label: "English" },
];
const CATALOGS: Record<Lang, Messages> = { es, en };
const STORAGE_KEY = "aot.lang";
```

- **Persistence:** `localStorage[STORAGE_KEY]`. Read *synchronously* at provider init
  (`getInitialLang()`), so there is no async flash of the wrong language on startup
  (unlike routing it through the Tauri settings table, which is async and would flicker).
  Default `"es"` when unset or invalid. On `setLang`, write localStorage + update state.
- **Context:** `I18nProvider` holds `{ lang, setLang }` in state; `useLang()` exposes it.
- **`useT()`** returns a `t` function bound to the current lang:
  `t(key: keyof Messages, params?: Record<string, string | number>) => string`.
  Lookup current catalog → fall back to `es` catalog if key somehow missing at runtime →
  interpolate `{name}` placeholders from `params`. `t` identity changes when `lang`
  changes so consuming components re-render.
- **`interpolate(template, params)`**: replace `{key}` tokens. Pure, unit-testable.

### Wiring

- `main.tsx`: wrap `<App/>` in `<I18nProvider>`.
- Every hardcoded Spanish literal in `App.tsx` + `src/views/*.tsx` replaced with
  `t("...")`. Dynamic ones use params: e.g.
  `t("settings.foundSeries", { count: s.length })` →
  es `"Encontradas {count} series en emisión."` /
  en `"Found {count} airing series."`.
- The es catalog values are the *current exact Spanish strings* (copy them verbatim so
  the es build is a visual no-op). en values are faithful translations.

### Language selector (Ajustes)

New block at the top of `Settings.tsx` (its own `.series-block`): a labelled `<select>`
built from `LANGS`, value = current `lang`, `onChange` → `setLang(code)`. Switch is live
(React re-render), no reload. Label itself is translated (`settings.language`).

### Backend strings

Rust command errors stay in Rust (Spanish), but the frontend stops surfacing raw
`String(e)`. Add `t("errors.generic", { detail })` = es `"Error: {detail}"` /
en `"Error: {detail}"` and wrap the ~6 `setMsg(String(e))` call sites so the framing is
localized while the technical detail passes through. This satisfies "cover backend
user-facing text" proportionately without a fragile Rust i18n layer. (If a later task
adds new user-facing Rust prose, revisit — but today it's only error detail.)

### Formatting

Out of scope for this task beyond text. Do NOT refactor date/countdown formatting here;
that's noise. A follow-up can pass `lang` to `Intl.*`. Keep the diff to string extraction
+ the i18n module + the selector.

## Acceptance criteria (verifiable)

1. `npx tsc --noEmit` clean. Deleting one key from `en.ts` MUST produce a tsc error
   (proves compile-time coverage).
2. `npm run build` succeeds. No new runtime dependency added to `package.json`.
3. With lang=es the UI is byte-for-byte the same Spanish as before (es catalog copied
   verbatim) — verify by screenshot diff of at least nav + Settings + Pending.
4. Switching to English in Ajustes flips ALL visible chrome live (nav tabs, refresh
   button, Ajustes, and at least Pending + Library + Descubrir + Catalog content) with no
   reload and no leftover Spanish in those views.
5. Restarting the app keeps the last chosen language (localStorage persists).
6. Adding a hypothetical third language requires only: new `catalog/xx.ts` + one `LANGS`
   entry + one `CATALOGS` entry — no view edits. State this in the module's top comment.

## Live verification (required, with real screenshots)

App.tsx fires `refresh()` on startup (scrapes). Temporarily disable that startup effect
to verify UI without scraping, then revert so `git diff` is clean before commit (per
project rules — no synthetic OS clicks; passive screenshots only; interaction via
`window.eval()` if needed).

Steps: launch app → screenshot (es) of nav + Ajustes + Pending → in Ajustes switch to
English via the selector → screenshot the same three → confirm no Spanish remains in
translated views → restart app → confirm it comes up in English.

## Out of scope

- Translating Rust error *message bodies* (only their framing).
- Locale-aware date/number formatting.
- Auto-detecting OS language (default stays es; user opts into en). Could be a trivial
  later addition via `navigator.language`.
