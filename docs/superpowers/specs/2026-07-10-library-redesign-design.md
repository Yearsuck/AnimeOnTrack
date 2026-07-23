# Biblioteca redesign

## Problem

`src/views/Library.tsx` renders followed series as a flat vertical list of `.lib-row`s: a thumbnail, a title, a thin progress bar, `seen/total`. The user finds it neither accessible nor easy to navigate. Concretely:

- **No grouping.** A show you finished last year sits next to the one you're three episodes into. With 119 followed series the list is a wall.
- **Not keyboard-navigable.** The row is a bare `<div onClick>` — no `tabIndex`, no `role`, no Enter/Space handling, no focus ring. Unreachable without a mouse and invisible to a screen reader.
- **The one useful number is buried.** "Next episode to watch" — the reason to open the app — is nowhere; you get a percentage bar and a raw count.
- **Sort labels are unclear.** "Menos visto primero" sorts ascending by percentage, so 0% shows (never started) rank above the one you're mid-way through. That is rarely what "resume watching" means.

## Technical context

- `db.rs::list_library(source_id)` → `LibraryItem { series, total_episodes, seen_episodes, last_added }`, `WHERE followed=1 ORDER BY s.title`. `last_added` is `MAX(episodes.added_at)` — when we *scraped* an episode, not when the user watched it.
- Watching is gap-free (`set_seen_cascade`): seen episodes are always a prefix. So "next episode" is deterministic: the lowest-numbered unseen episode.
- `series.cover_url` is a `data:` URI for followed series whose cover was fetched; otherwise a remote (Cloudflare-blocked) URL or `null`. The grid must degrade gracefully for both.
- Design system: `src/styles.css`, hand-written, custom properties `--bg --surface --surface-2/3/4 --border --text --muted --muted-2 --accent --accent-dim --accent-hover --success --success-dim --danger --sp-1..--sp-8 --radius --radius-sm --radius-round --shadow-sm/md/lg --ease --ease-out --font`. Existing classes to reuse: `.page`, `.page-head`, `.grid`, `.card`, `.poster`, `.chip`, `.btn`, `.progress` (6px track + `> span` fill), `.empty`, `.input`, `.search`. **No component library, no Tailwind. Do not introduce one.**
- `AiringGrid.tsx` is the in-repo precedent for a poster grid — match its `.grid`/`.card`/`.poster` structure so the two tabs read as one app.

## Reference patterns

AniList, MyAnimeList and Trakt all converge on the same three things for a library: **status grouping** ("Watching / Completed / Plan to watch"), **poster-forward cards** with progress overlaid, and a **primary "next episode" affordance** rather than a raw percentage. Adopt those three. Do not copy their chrome, colors, or layout wholesale — this app has its own dark design system, and the point is a coherent app, not a clone.

## Design

### Status derivation (backend, `db.rs`)

Status is derived, not stored — there is no user-set status field and adding one is out of scope.

```
completed  : total_episodes > 0 AND seen_episodes == total_episodes
watching   : seen_episodes > 0 AND seen_episodes < total_episodes
plan       : seen_episodes == 0
```

A followed series with `total_episodes == 0` (linked but never scraped, or scrape failed) is `plan` and must render without dividing by zero.

Extend `LibraryItem` with:
- `next_episode: Option<{ number: String, title: Option<String>, url: String }>` — lowest-numbered unseen episode. Episode `number` is `TEXT` in the schema (the site emits things like `"12"`, `"12.5"`, `"OVA"`), so **order by the same collation the episode list already uses**, not by `CAST(number AS INTEGER)` — check what `SeriesDetail`'s query does and reuse it, so "next" here and "next" there can never disagree.
- `last_watched_at: Option<String>` — `MAX(episodes.added_at)` is *not* this. There is no per-episode watched timestamp today. Either add `episodes.seen_at TEXT` (set by `set_seen_cascade` when marking seen, NULL when unmarking) via `ensure_column`, or drop the "recently watched" sort. **Adding the column is the right call** — "continue watching, most recent first" is the single most useful ordering a tracker has, and it cannot be faked from `added_at`.

Keep `last_added` for backwards compatibility; do not repurpose it.

### Layout (frontend, `Library.tsx` + `styles.css`)

Three sections in fixed order, each a labelled group, each collapsible (`<details>`/`<summary>` gives keyboard + screen-reader behavior for free — use it rather than hand-rolling a disclosure):

1. **Viendo** — default open. Sorted by `last_watched_at` descending, NULLs last, `title` tie-break.
2. **Pendientes de empezar** — default open. Sorted by `title`.
3. **Completadas** — default **collapsed**. Sorted by `last_watched_at` descending.

Each section header shows its count. An empty section is omitted entirely, not rendered empty.

Cards, in a `.grid` matching `AiringGrid`:

- Poster (`.poster`), `loading="lazy"`, with a text-only fallback block when `cover_url` is null (initials or the title, on `--surface-3`) — never a broken `<img>`.
- Title (`.card-title`), full text available via `title` attribute; visually clamped to 2 lines.
- Progress: the existing `.progress` bar plus a text label `"7 / 12"`. The bar is decorative; **the text is the accessible source of truth**. Give the bar `role="progressbar"` with `aria-valuenow/min/max` and an `aria-label` naming the series.
- Primary action, only in **Viendo** and **Pendientes**: a `.btn` labelled `▶ Episodio {n}` that opens the next episode (`openEpisode(next_episode.url)`). This is the whole point of the tab. In **Completadas** the card has no primary action.
- The card itself opens `SeriesDetail` on click/Enter.

### Keyboard + accessibility (non-negotiable, this is half the request)

- Cards are `<button>` elements (or `<div role="button" tabIndex={0}>` with Enter **and** Space handlers). A visible focus ring using `--accent` — do not remove outlines without replacing them.
- The "▶ Episodio n" button is a real nested `<button>`; stop propagation so it doesn't also open the detail view (`AiringGrid`'s follow button and `Descubrir`'s poster click both already had bubbling bugs — commit `909c64d`. Do not repeat that.)
- Arrow-key roving focus across the grid is **out of scope**; native Tab order through cards in DOM order is sufficient and predictable. Say so rather than half-implementing a roving tabindex.
- Contrast: every text/background pair must clear **WCAG AA 4.5:1** (3:1 for text ≥18.66px bold / ≥24px). The existing `--muted-2` on `--surface-2` is the pair most likely to fail — **measure the actual hex values with a contrast formula, don't eyeball it**, and if it fails, fix the token usage in this view (use `--muted`) rather than silently shipping it. Report the computed ratios for the pairs you used.
- The search input gets a real `<label>` (visually hidden is fine) instead of relying on `placeholder`.

### Sorting and filtering

- Search box: unchanged behavior (client-side title substring), now filtering within all three sections.
- The `SortMode` select is **removed**. Grouping plus per-section ordering replaces it; a global sort across mixed statuses is what made the current view hard to read. If a sort control returns later it should be per-section.

## Acceptance criteria (verifiable)

1. `cargo test` passes with new `db.rs` tests: `next_episode` is the lowest-numbered unseen episode and agrees with `SeriesDetail`'s ordering for a series with `"9"`, `"10"`, `"10.5"` episodes; a fully-seen series yields `None`; a series with zero episodes yields `None` and does not panic. `set_seen_cascade` sets `seen_at` when marking and clears it when unmarking (including the cascade rows).
2. `npx tsc --noEmit`, `npm run build` pass.
3. Live: Biblioteca shows the three groups with correct counts; a mid-progress series appears under Viendo with a working `▶ Episodio n` that opens the right episode; a fully-watched one under Completadas (collapsed by default); a followed-but-unstarted one under Pendientes.
4. **Keyboard only, no mouse**: Tab reaches the search box, each section's disclosure, each card, and each play button; Enter/Space activate them; focus is always visibly indicated. Demonstrate this, don't assert it.
5. Contrast ratios for every foreground/background pair used are ≥ 4.5:1 (or ≥ 3:1 where the large-text exemption applies), computed and reported as numbers.

## Live verification required

- Screenshot of the three-section library with real covers and progress.
- Screenshot showing a **visible focus ring** on a card reached by keyboard.
- The computed contrast ratios (list the pairs and the numbers).
- Confirmation that clicking `▶ Episodio n` does not also navigate to the detail view (the bubbling bug).

## Explicitly out of scope

- A user-settable status (dropped/on-hold) — status stays derived.
- Per-section sort controls, drag-to-reorder, custom lists.
- Bulk actions (mark season watched, unfollow many).
- Changing `Pendientes` (the separate queue tab) or `SeriesDetail`.
- Roving-tabindex grid navigation.
