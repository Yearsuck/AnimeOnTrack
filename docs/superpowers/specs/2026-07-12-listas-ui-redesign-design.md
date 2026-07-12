# Descubrir "Listas" (Quiero ver / Descartados / Ya vistas) UI/UX redesign

**Date:** 2026-07-12
**Branch to implement on:** `feat/listas-ui-redesign`
**Type:** UI/UX (frontend-only). Theme-aware (Task 1 done), design-system-consistent.
**Status:** approved (autonomous batch)

## Problem

The Listas sub-view of Descubrir (`src/views/Descubrir.tsx`, `ListasView` L675) is three long,
flat, stacked lists (`Quiero ver`, `Descartados`, `Ya vistas`), each a column of `.backlog-row`s
(cover + title + genres + a horizontal row of `btn-ghost` buttons). Weak hierarchy, no counts,
noisy repeated action buttons, no search, and no consistency with the Library view's polished
card language. The user finds it "no convence" — wants better usability: clear hierarchy, clear
actions, and easy moving between lists.

## What exists (verified, `src/views/Descubrir.tsx`)

- `ListasView` fetches `listBacklog("want")`, `listBacklog("discarded")`, `listWatchedExternally()`.
- Row components + their actions (all already wired, reuse them):
  - `WantRow`: open detail (title click), `searchOnSite` (if unlinked), `startWatching`,
    `discard` (`setBacklogStatus(id,"discarded")`), `removeFromList` (`reclassifySeries(id,"None")`).
  - `DiscardedRow`: `moveToWant` (`promoteDiscarded`), `deleteCompletely` (`deleteSeries`).
  - `WatchedRow`: `markUnwatched` (`reclassifySeries(id,"None")`), `moveToWantFromWatched`
    (`reclassifySeries(id,"Want")`).
- The Library card + `card-menu` (⋯ overflow) pattern in `src/views/Library.tsx` is the visual
  target to converge on (poster, title, status chip, overflow menu of secondary actions).
- CSS in `src/styles.css` (`.backlog-row`, `.card`, `.card-menu*`, `.chip*`, `.grid`, `.seg*`).

## Design

Rework `ListasView` (and its three row components) into a single, coherent, card-based screen.
Keep ALL existing actions/handlers — only restructure presentation and grouping.

1. **Section headers with counts** — each list gets a header showing its name + item count
   (reuse Library's `.lib-section-summary` / count-badge styling or the `series-block`
   `series-head` with an added count span). Consider a segmented control at top to switch
   between the three lists (Quiero ver | Descartados | Ya vistas) instead of stacking all three,
   OR keep them stacked but collapsible (`<details>` like Library sections). Pick ONE; the
   segmented switch reduces scroll and reads cleaner — recommended, with the active list's count
   on each segment.
2. **Card layout** — render each row as a poster card consistent with Library
   (`initials()` fallback when `cover_url` null/failed — copy Library's `showFallback` +
   `onError` pattern so no broken `<img>` ever shows; the existing rows render `<img>` only when
   `cover_url` truthy but don't handle load failure). Title, genres (Want only), and a small
   **status chip** ("Quiero ver" / "Descartado" / "Vista").
3. **Primary vs secondary actions** — surface the ONE primary action as a button, move the rest
   into a ⋯ overflow menu (reuse Library's `card-menu` markup + outside-click close):
   - Quiero ver: primary **Empezar a ver** (`startWatching`); menu: Buscar en la web (if
     unlinked), Descartar, Quitar de la lista, Ver detalles.
   - Descartados: primary **Mover a Quiero ver** (`promoteDiscarded`); menu: Eliminar del todo.
   - Ya vistas: primary **Marcar como no vista** (`reclassifySeries None`); menu: Mover a Quiero
     ver.
4. **Empty states** — keep per-list empty messages (existing i18n keys `discover.*Empty`).
5. **Optional search filter** — a title filter input like Library's (reuse `.search`), applied
   to the active list. Nice-to-have; implement if cheap.
6. **Loading feedback** — the async actions currently give no pending state; add a lightweight
   disabled/busy state on the primary button during its await (match `startWatching`'s existing
   `starting` pattern) so double-clicks can't fire twice.

All new strings via i18n `es.ts` + `en.ts` (missing key = tsc error). Reuse existing
`discover.*` keys where they already cover the action labels; add keys only for genuinely new
UI (status chips, segment labels, any new menu items). Everything theme-aware via the existing
CSS custom properties — no hardcoded colors.

## Invoke skills

`artifact-design` and any frontend-design guidance for the layout/hierarchy. This is a
presentation task — prioritize clarity, consistent spacing/typography with the rest of the app,
and accessible controls (keyboard-operable buttons/menus, `aria-label`s like Library's).

## Acceptance criteria (verifiable without live UI)

1. `npx tsc --noEmit` clean; `npm run build` OK.
2. `cargo test --manifest-path src-tauri/Cargo.toml` still green (no backend change expected;
   run to prove nothing broke).
3. Every pre-existing action still reachable (grep the handlers: `startWatching`,
   `setBacklogStatus`, `promoteDiscarded`, `deleteSeries`, `reclassifySeries` with `None`/`Want`,
   `searchOnSite`/retry-link) — none dropped.
4. New i18n keys present in BOTH catalogs.
5. No hardcoded hex/rgba added to `Descubrir.tsx` (use tokens / classes).

## Verify live (NOT tool-reachable — state honestly)

Relaunch, open Descubrir → Listas, confirm the redesigned layout, that actions work, and moving
items between lists behaves. Cannot be screenshot-verified here — the user should eyeball it.

## Out of scope

Backend/list-query changes. The swipe deck and Filtros sub-views. Changing what the actions DO
(only how they're presented).
