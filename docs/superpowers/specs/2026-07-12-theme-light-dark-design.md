# Theme selector (light / dark) — design

**Date:** 2026-07-12
**Branch to implement on:** `feat/theme-light-dark`
**Status:** approved (autonomous batch)

## Problem

The app has a single hardcoded dark theme (blue-ish, minimal — the user likes it).
There is no way to switch to a light/white theme. This must land **first** in the
batch so every new UI built in later tasks is born theme-aware, exactly as i18n was
made the foundation in the prior batch.

## Real codebase context (verified)

- **Design system:** `src/styles.css` (994 lines). All colors live as CSS custom
  properties on `:root` (lines 1–41): `--bg #0b1521`, `--surface`, `--surface-2..4`,
  `--border`, `--text`, `--muted`, `--muted-2`, `--accent #4aa8ff`, `--accent-hover`,
  `--accent-dim`, `--success`, `--success-dim`, `--danger`, plus `--shadow-sm/md/lg`,
  spacing (`--sp-*`), radius, ease. **163** `var(--…)` usages already — the ramp is
  the single source of truth for nearly everything.
- **Hardcoded color leaks** (must be tokenized so light theme works). Verified lines
  in `src/styles.css`:
  - Dark backgrounds baked as literal rgba (these break a light theme):
    - L103 `.topbar` `background: rgba(11,21,33,.85)` (blurred bar)
    - L347 `background: rgba(5,18,31,.8)`
    - L427 `border-bottom: 1px solid rgba(36,56,74,.5)`
    - L516 `background: rgba(11,21,33,.92)`
    - L535 `background: rgba(5,18,31,.7)`
    - L627 `background: rgba(5,18,31,.7)`
  - Fine as-is (foreground on accent/success/danger fills, theme-independent):
    L150/195/546/556/759 `color:#05121f`, L536 `color:#fff`, and the *-dim/border
    rgba tints derived from accent/success/danger (L204, L726, L895, L898, L902) —
    these read acceptably on both themes; leave unless a light-theme visual check
    (not tool-reachable here) later shows a problem. Document them, don't churn them.
- **i18n is the pattern to mirror** (`src/i18n/index.ts`): a dependency-free React
  context provider, synchronous `getInitial*()` read from `localStorage` at init (no
  async flash), `localStorage` key `aot.lang`, a `<select>` in `Settings.tsx`, and a
  `useX()` hook. `main.tsx` wraps `<App/>` in `<I18nProvider>`.
- **i18n catalog:** every new UI string must be added to BOTH `src/i18n/catalog/es.ts`
  and `src/i18n/catalog/en.ts` (`Messages = Record<keyof typeof es, string>`; a
  missing key is a `tsc` error). Settings language block is `Settings.tsx` L129–145.

## Design

### Token strategy — theme via `data-theme` on `<html>`

Keep the current dark ramp as the **default** and as the explicit `dark` theme.
Add a `light` theme by overriding the same token names.

In `src/styles.css`:

1. Leave the existing `:root { --bg … }` block as-is (it stays the dark baseline, so
   the app renders dark even before JS runs — no flash).
2. Add a `:root[data-theme="light"] { … }` block overriding **only the color tokens**
   (not spacing/radius/ease) with a light palette. Suggested light values (implementer
   may fine-tune; must keep the blue accent identity and WCAG-AA text contrast):
   - `--bg: #f4f7fb; --surface: #ffffff; --surface-2: #eef2f7; --surface-3: #e2e8f0;`
     `--surface-4: #dbe3ec99; --border: #d3dde8; --text: #17222e; --muted: #5a6b7d;`
     `--muted-2: #8394a5; --accent: #1f88e6; --accent-hover: #1670c8;`
     `--accent-dim: rgba(31,136,230,.12); --success: #1f9d6b;`
     `--success-dim: rgba(31,157,107,.14); --danger: #d83a5a;`
   - Shadows softer on light: `--shadow-sm/md/lg` with lower alpha.
3. **Tokenize the six dark-baked leaks.** Introduce new tokens used by both themes:
   - `--overlay-bg` (topbar/sticky blurred bar bg) → dark `rgba(11,21,33,.85)`,
     light `rgba(244,247,251,.85)`.
   - `--overlay-strong` (L516/L347/L535/L627 modal & scrim backgrounds) → dark
     `rgba(5,18,31,.8)`, light `rgba(255,255,255,.85)`.
   - `--border-soft` (L427) → dark `rgba(36,56,74,.5)`, light `rgba(211,221,232,.6)`.
   Replace the literal rgba at those lines with the new tokens; define both dark
   defaults (on `:root`) and light overrides (on `:root[data-theme="light"]`).
   Note L103 and L516 differ in alpha (.85 vs .92) — use two tokens or accept .85
   for both; implementer decides, document the choice.

### Theme provider (mirror i18n exactly)

New file `src/theme.ts`:
- `export type Theme = "dark" | "light";`
- `THEMES: {code, labelKey}[]` — labels via i18n keys (`settings.themeDark`,
  `settings.themeLight`), not hardcoded.
- `localStorage` key `aot.theme`; `getInitialTheme()` defaults to `"dark"`.
- `applyTheme(theme)` sets `document.documentElement.setAttribute("data-theme", theme)`
  (dark may set the attribute too, or omit — since `:root` default is dark, omitting
  for dark is fine; be explicit and always set it for clarity).
- `ThemeProvider` context + `useTheme()` returning `{ theme, setTheme }`.
- On init AND on every `setTheme`, call `applyTheme` so the DOM attribute tracks state.
- **Apply the initial theme before React renders** to avoid a flash: call
  `applyTheme(getInitialTheme())` at module load in `theme.ts`, and also inside the
  provider effect. (Cheapest: an inline call in `main.tsx` before `createRoot`, or a
  side-effect at top of `theme.ts` imported by `main.tsx`.)
- `main.tsx` wraps `<App/>` with `<ThemeProvider>` (inside or outside `<I18nProvider>`
  — order irrelevant; keep I18n outermost for consistency).

### Settings UI

Add a theme `<select>` block in `Settings.tsx` directly above the language block
(same `.series-block` markup, L129–145 as template). Options from `THEMES`, labels via
`t()`. New i18n keys in es.ts + en.ts: `settings.theme` ("Tema"/"Theme"),
`settings.themeDark` ("Oscuro"/"Dark"), `settings.themeLight` ("Claro"/"Light").

## Acceptance criteria (verifiable without live UI)

1. `npx tsc --noEmit` clean; `npm run build` OK.
2. `cargo test --manifest-path src-tauri/Cargo.toml` still green (no backend change
   expected — pure frontend; run anyway to prove nothing broke).
3. `grep -n 'rgba(11, 21, 33\|rgba(5, 18, 31\|rgba(36, 56, 74' src/styles.css` returns
   **only** token *definitions* (inside `:root` / `:root[data-theme="light"]` blocks),
   no usage in component rules — proves the leaks were tokenized.
4. `src/styles.css` contains a `:root[data-theme="light"]` block overriding every color
   token that `:root` defines (diff the two token lists — no color token missing).
5. New i18n keys present in BOTH catalogs (else tsc fails).
6. `localStorage` key is `aot.theme`; default `"dark"`.

## What to verify live (NOT tool-reachable — state honestly in summary)

- Toggling theme in Settings repaints the whole app with no dark remnants on light
  (check topbar blur, modals, scrims, borders) and persists across app relaunch.
- Text contrast readable on light theme.
Ask the user to relaunch and eyeball; do not fake a screenshot.

## Out of scope

- No "system/auto" theme option (YAGNI; add later if asked).
- No new accent-color customization.
- No churn of the theme-independent `#05121f`/`#fff` foreground colors.
