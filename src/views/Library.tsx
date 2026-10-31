import { useCallback, useEffect, useMemo, useState, type MouseEvent } from "react";
import { listLibrary, reclassifySeries } from "../api";
import { launchEpisode } from "../lib/launchEpisode";
import { useT } from "../i18n";
import { createFilterCache } from "../lib/createFilterCache";
import { resolveCoverUrl } from "../lib/posterFallback";
import { formatProgress } from "../lib/airCountdown";
import { Menu } from "../components/Menu";
import { PosterCard } from "../components/PosterCard";
import { Reveal } from "../components/Reveal";
import type { LibraryItem, Series } from "../types";

type Status = "completed" | "watching" | "plan";
// Airing / finished filter (lives under "Más filtros"; applies to every tab).
type AiringFilter = "all" | "airing" | "finished";
// The single primary navigation: one tab per derived status, plus "all".
type StatusFilter = "all" | Status;
type SortKey = "recent" | "title" | "progress" | "pending";
const SORT_KEYS: SortKey[] = ["recent", "title", "progress", "pending"];
const STATUS_KEYS: StatusFilter[] = ["watching", "plan", "completed", "all"];
const STATUS_ORDER: Record<Status, number> = { watching: 0, plan: 1, completed: 2 };
const TAB_STORAGE_KEY = "aot.library.tab";
const SORT_STORAGE_KEY = "aot.library.sort";

// localStorage can throw or be empty (private window, blocked data): the view
// must render fine without it.
function readStored<T extends string>(key: string, allowed: readonly T[], fallback: T): T {
  try {
    const v = localStorage.getItem(key);
    if (v && (allowed as readonly string[]).includes(v)) return v as T;
  } catch {
    /* ignore */
  }
  return fallback;
}
function writeStored(key: string, value: string) {
  try {
    localStorage.setItem(key, value);
  } catch {
    /* ignore */
  }
}

// series.kind is dirty scraped free text: real type badges under a Spanish
// alias ("Pelicula") mixed in with the site's quality/version tags
// ("4K"/"Blu-Ray"/"Resubido"/"Sin Censura"/"Yaoi") and null. Fold all of
// that into a fixed, small set for the Tipo filter — the non-type tags (and
// null) bucket under "OTROS" rather than each becoming its own noisy
// option. See docs/superpowers/specs/2026-07-13-library-filters-design.md.
type NormalizedKind = "TV" | "MOVIE" | "OVA" | "ONA" | "SPECIAL" | "OTROS";
const KNOWN_KINDS: NormalizedKind[] = ["TV", "MOVIE", "OVA", "ONA", "SPECIAL"];

function normalizeKind(raw: string | null): NormalizedKind {
  if (!raw) return "OTROS";
  const upper = raw.trim().toUpperCase();
  if (upper === "PELICULA" || upper === "PELÍCULA") return "MOVIE";
  if ((KNOWN_KINDS as string[]).includes(upper)) return upper as NormalizedKind;
  return "OTROS";
}

// Status is derived, never stored (no user-set field exists). A catalog "Ya
// lo vi" swipe (watched_externally) is checked FIRST and unconditionally
// lands in "completed" — it has 0 scraped episodes (AniList catalog Seen
// never scrapes), so the total>0 rule below would otherwise misread it as
// "plan". Completed otherwise means the series is FINISHED (not airing) AND
// everything available is watched — an airing series you're caught up on
// (allSeen && is_airing) stays in "watching", because more episodes are
// still coming (it isn't done). A nonzero total is required so a
// followed-but-never-scraped series (total=0) never reads as completed.
// Everything with >0 seen that isn't completed is "watching" (incl.
// caught-up airing and partly-watched finished shows); 0 seen is "plan",
// never dividing by zero.
export function progressPct(seen: number, total: number): number {
  if (!total || total <= 0) return 0;
  return Math.min(100, Math.max(0, Math.round((seen / total) * 100)));
}

export function statusOf(it: LibraryItem): Status {
  if (it.watched_externally) return "completed";
  const allSeen = it.total_episodes > 0 && it.seen_episodes >= it.total_episodes;
  if (allSeen && !it.series.is_airing) return "completed";
  if (it.seen_episodes > 0) return "watching";
  return "plan";
}

// "Viendo"/"Completadas" sort: most-recently-watched first, NULLs (never
// watched, or watched before the `seen_at` column existed) sort last,
// title as a stable tie-break.
function byRecentWatched(a: LibraryItem, b: LibraryItem): number {
  if (a.last_watched_at == null && b.last_watched_at == null) {
    return a.series.title.localeCompare(b.series.title);
  }
  if (a.last_watched_at == null) return 1;
  if (b.last_watched_at == null) return -1;
  return (
    b.last_watched_at.localeCompare(a.last_watched_at) ||
    a.series.title.localeCompare(b.series.title)
  );
}

function byTitle(a: LibraryItem, b: LibraryItem): number {
  return a.series.title.localeCompare(b.series.title);
}

function progressRatio(it: LibraryItem): number {
  if (it.watched_externally && it.total_episodes === 0) return 1;
  return it.total_episodes ? it.seen_episodes / it.total_episodes : 0;
}

function pendingCount(it: LibraryItem): number {
  return Math.max(0, it.total_episodes - it.seen_episodes);
}

// Most progress first; ties fall back to recency.
function comparatorFor(sort: SortKey): (a: LibraryItem, b: LibraryItem) => number {
  switch (sort) {
    case "title":
      return byTitle;
    case "progress":
      return (a, b) => progressRatio(b) - progressRatio(a) || byRecentWatched(a, b);
    case "pending":
      return (a, b) => pendingCount(b) - pendingCount(a) || byRecentWatched(a, b);
    default:
      return byRecentWatched;
  }
}

function LibraryCard({
  item,
  onOpenSeries,
  onChanged,
  selectMode,
  selected,
  onToggleSelect,
}: {
  item: LibraryItem;
  onOpenSeries: (s: Series) => void;
  onChanged: () => void;
  selectMode: boolean;
  selected: Set<number>;
  onToggleSelect: (id: number) => void;
}) {
  const t = useT();

  // Overflow menu — reclassify actions
  // (see docs/superpowers/specs/2026-07-11-reversibility-classifications-design.md).
  async function unfollow() {
    await reclassifySeries(item.series.id, "None");
    onChanged();
  }

  async function moveToWant() {
    await reclassifySeries(item.series.id, "Want");
    onChanged();
  }

  // Menu: "watching" cards, plus watched-externally cards ("Ya lo vi" from the
  // catalog), which are a manual mark and so always need an undo path.
  // "Plan" and derived-completed cards have nothing to reclassify from.
  const showMenu = statusOf(item) === "watching" || item.watched_externally;
  const isSelected = selected.has(item.series.id);
  const next = item.next_episode;
  const noEpisodes = item.watched_externally && item.total_episodes === 0;
  const pct = noEpisodes
    ? 100
    : progressPct(item.seen_episodes, item.total_episodes);

  // The play button sits in PosterCard's action slot, which already stops
  // click/keys from reaching the card, so Enter on it never also opens the
  // series.
  function playNext(e: MouseEvent<HTMLElement>) {
    if (next) {
      void launchEpisode({
        url: next.url,
        origin: e.currentTarget,
        title: item.series.title,
        episode: next.number,
        cover: resolveCoverUrl(item.series.cover_url),
      });
    }
  }

  return (
    <PosterCard
      title={item.series.title}
      coverUrl={item.series.cover_url}
      genres={item.genres}
      meta={noEpisodes ? t("library.watchedExternally") : formatProgress(item.seen_episodes, item.total_episodes)}
      selectable={selectMode}
      selected={selectMode && isSelected}
      onActivate={() => (selectMode ? onToggleSelect(item.series.id) : onOpenSeries(item.series))}
      corner={
        <>
          {next && <span className="chip lib-ep-pill">{t("library.epPill", { number: next.number })}</span>}
          {showMenu && !selectMode && (
            <div className="card-menu-wrap">
              <Menu
                label={t("library.cardMenuAria")}
                items={
                  item.watched_externally
                    ? [{ label: t("library.markNotWatched"), onSelect: unfollow, danger: true }]
                    : [
                        { label: t("library.unfollow"), onSelect: unfollow, danger: true },
                        { label: t("library.moveToWant"), onSelect: moveToWant },
                      ]
                }
              />
            </div>
          )}
        </>
      }
      action={
        next ? (
          <button type="button" className="btn btn-primary btn-sm" onClick={playNext}>
            {t("library.playNext", { number: next.number })}
          </button>
        ) : undefined
      }
      footer={
        noEpisodes ? (
          <div className="progress lib-progress" aria-hidden="true">
            <span style={{ width: `${pct}%` }} />
          </div>
        ) : (
          <div
            className="progress lib-progress"
            role="progressbar"
            aria-valuenow={item.seen_episodes}
            aria-valuemin={0}
            aria-valuemax={item.total_episodes}
            aria-label={t("library.progressAria", {
              title: item.series.title,
              seen: item.seen_episodes,
              total: item.total_episodes,
            })}
          >
            <span style={{ width: `${pct}%` }} />
          </div>
        )
      }
    />
  );
}

// Survives unmount/remount of this view (tab switch in App.tsx).
const libraryFilterCache = createFilterCache<{
  query: string;
  airingFilter: AiringFilter;
  statusFilter: StatusFilter;
  typeFilter: string;
  includedGenres: Set<string>;
  excludedGenres: Set<string>;
  studioFilter: string;
}>();

export function Library({
  onOpenSeries,
  onGoToListas,
}: {
  onOpenSeries: (s: Series) => void;
  onGoToListas?: () => void;
}) {
  const t = useT();
  const [items, setItems] = useState<LibraryItem[]>([]);
  const [query, setQuery] = useState(() => libraryFilterCache.current?.query ?? "");
  const [debouncedQuery, setDebouncedQuery] = useState(query);

  // Typing is debounced so each keystroke does not re-filter every card;
  // clearing the search applies at once.
  useEffect(() => {
    if (!query) {
      setDebouncedQuery("");
      return;
    }
    const timer = setTimeout(() => {
      setDebouncedQuery(query);
    }, 150);
    return () => clearTimeout(timer);
  }, [query]);
  const [airingFilter, setAiringFilter] = useState<AiringFilter>(() => libraryFilterCache.current?.airingFilter ?? "all");
  const [statusFilter, setStatusFilter] = useState<StatusFilter>(
    () => libraryFilterCache.current?.statusFilter ?? readStored(TAB_STORAGE_KEY, STATUS_KEYS, "watching")
  );
  const [sort, setSort] = useState<SortKey>(() => readStored(SORT_STORAGE_KEY, SORT_KEYS, "recent"));
  useEffect(() => writeStored(TAB_STORAGE_KEY, statusFilter), [statusFilter]);
  useEffect(() => writeStored(SORT_STORAGE_KEY, sort), [sort]);
  const [typeFilter, setTypeFilter] = useState<string>(() => libraryFilterCache.current?.typeFilter ?? "all");
  const [includedGenres, setIncludedGenres] = useState<Set<string>>(() => libraryFilterCache.current?.includedGenres ?? new Set());
  const [excludedGenres, setExcludedGenres] = useState<Set<string>>(() => libraryFilterCache.current?.excludedGenres ?? new Set());
  const [studioFilter, setStudioFilter] = useState<string>(() => libraryFilterCache.current?.studioFilter ?? "all");
  // Purely a display cap, not persisted: the catalog carries dozens of
  // genres and showing them all ate the whole filter bar's screen space.
  const [showAllGenres, setShowAllGenres] = useState(false);
  // "Más filtros" disclosure (genre + studio). Starts open when a cached
  // genre/studio filter is already narrowing the list, so an active filter
  // is never hidden behind a closed panel on return to the tab.
  const [moreOpen, setMoreOpen] = useState(
    () =>
      includedGenres.size > 0 || excludedGenres.size > 0 || studioFilter !== "all" || airingFilter !== "all"
  );
  const secondaryActiveCount =
    includedGenres.size +
    excludedGenres.size +
    (studioFilter !== "all" ? 1 : 0) +
    (airingFilter !== "all" ? 1 : 0);
  // The status tab is navigation, not a filter, so it is not counted here.
  const anyFilterActive = query !== "" || typeFilter !== "all" || secondaryActiveCount > 0;
  function clearAllFilters() {
    setQuery("");
    setAiringFilter("all");
    setTypeFilter("all");
    setStudioFilter("all");
    setIncludedGenres(new Set());
    setExcludedGenres(new Set());
  }

  // Persist filter state to the module-level cache on every change
  useEffect(() => {
    libraryFilterCache.write({ query, airingFilter, statusFilter, typeFilter, includedGenres, excludedGenres, studioFilter });
  }, [query, airingFilter, statusFilter, typeFilter, includedGenres, excludedGenres, studioFilter]);

  const [selectMode, setSelectMode] = useState(false);
  const [selected, setSelected] = useState<Set<number>>(new Set());
  const [batching, setBatching] = useState(false);

  function toggleSelect(id: number) {
    setSelected((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  }

  // A selection is scoped to the set it was made in: every filter here can
  // hide already-selected cards, and the batch bar kept claiming "5
  // seleccionadas" — and kept acting on all 5 — for rows the user could no
  // longer see under the new filter. Search-then-select is one search, so
  // any filter change drops the selection rather than silently carrying it
  // across. `selectMode` deliberately survives (the user is still picking);
  // the batch bar only renders while something is selected. Returning the
  // previous set when it's already empty keeps this from re-rendering on
  // every keystroke in the search box.
  useEffect(() => {
    setSelected((prev) => (prev.size === 0 ? prev : new Set()));
  }, [query, statusFilter, airingFilter, typeFilter, includedGenres, excludedGenres, studioFilter]);

  // "watching"/"completed" are derived from real seen-episode progress
  // (see statusOf() above) — there is no single backend command that sets
  // them directly, only reclassify_series ("None"/"Want"/etc). "plan" is
  // the one Status value with a real 1:1 command: the existing per-item
  // "move to want to watch" action already calls reclassifySeries(id,
  // "Want") (see moveToWant above), so that's the only bulk status action
  // this can honestly offer without a new bulk-marking-episodes-seen
  // feature that's out of scope here.
  // Both bulk actions reconcile the UI in `finally`, never only on success:
  // the loop is one backend call per id, so a rejection on item 3 of 5 still
  // leaves items 1-2 reclassified in the DB. Reloading (and dropping the
  // selection) only in the happy path left those two rendering their
  // pre-action state, with the select-mode bar still up as if nothing had
  // happened.
  async function bulkMoveToPlan() {
    if (selected.size === 0) return;
    setBatching(true);
    try {
      const ids = [...selected];
      for (const id of ids) {
        await reclassifySeries(id, "Want");
      }
    } catch (e) {
      console.error("Bulk status change failed:", e);
    } finally {
      setSelected(new Set());
      setSelectMode(false);
      setBatching(false);
      load();
    }
  }

  async function bulkRemove() {
    if (selected.size === 0) return;
    if (!window.confirm(t("library.bulkRemoveConfirm", { count: selected.size }))) return;
    setBatching(true);
    try {
      const ids = [...selected];
      for (const id of ids) {
        await reclassifySeries(id, "None");
      }
    } catch (e) {
      console.error("Bulk remove failed:", e);
    } finally {
      setSelected(new Set());
      setSelectMode(false);
      setBatching(false);
      load();
    }
  }

  const load = useCallback(() => {
    listLibrary().then(setItems);
  }, []);

  useEffect(() => {
    load();
  }, [load]);

  // Option lists are built from the whole library (not the currently
  // filtered subset), so picking one filter never makes the others' option
  // lists shrink out from under the user.
  const typeOptions = useMemo(() => {
    const present = new Set<NormalizedKind>(items.map((it) => normalizeKind(it.kind)));
    const ordered = KNOWN_KINDS.filter((k) => present.has(k));
    if (present.has("OTROS")) ordered.push("OTROS");
    return ordered;
  }, [items]);

  const genreOptions = useMemo(() => {
    const present = new Set<string>();
    for (const it of items) for (const g of it.genres) present.add(g);
    return [...present].sort((a, b) => a.localeCompare(b));
  }, [items]);

  // Cap the chip row so dozens of genres don't dominate the filter bar; a
  // genre already filtered on stays visible even past the cap, so toggling
  // it off is never hidden behind "show more".
  const GENRE_VISIBLE_CAP = 12;
  const visibleGenreOptions = useMemo(() => {
    if (showAllGenres || genreOptions.length <= GENRE_VISIBLE_CAP) return genreOptions;
    const active = genreOptions.filter((g) => includedGenres.has(g) || excludedGenres.has(g));
    const rest = genreOptions.filter((g) => !includedGenres.has(g) && !excludedGenres.has(g));
    return [...active, ...rest].slice(0, Math.max(GENRE_VISIBLE_CAP, active.length));
  }, [genreOptions, showAllGenres, includedGenres, excludedGenres]);
  const hiddenGenreCount = genreOptions.length - visibleGenreOptions.length;

  // Derived client-side (not a backend facets call, unlike Catálogo's studio
  // select) — Library's full item list is already loaded client-side, same
  // reasoning as genreOptions above. Most rows will have studio: null
  // (unlinked followed series never get native studio data — see
  // LibraryItem.studio's doc comment), so those rows simply never
  // contribute an option here; they still show under the "all" default.
  const studioOptions = useMemo(() => {
    const present = new Set<string>();
    for (const it of items) if (it.studio) present.add(it.studio);
    return [...present].sort((a, b) => a.localeCompare(b));
  }, [items]);

  const filtered = useMemo(() => {
    const q = debouncedQuery.trim().toLowerCase();
    return items.filter((it) => {
      if (q && !it.series.title.toLowerCase().includes(q)) return false;
      if (typeFilter !== "all" && normalizeKind(it.kind) !== typeFilter) return false;
      if (includedGenres.size > 0 && ![...includedGenres].every((g) => it.genres.includes(g))) return false;
      if (excludedGenres.size > 0 && [...excludedGenres].some((g) => it.genres.includes(g))) return false;
      // "all" never hides unlinked (studio: null) rows — only an explicit
      // studio selection filters them out.
      if (studioFilter !== "all" && it.studio !== studioFilter) return false;
      return true;
    });
  }, [items, debouncedQuery, typeFilter, includedGenres, excludedGenres, studioFilter]);

  // The airing/finished filter (under "Más filtros") applies to every tab.
  const scoped = useMemo(
    () =>
      airingFilter === "all"
        ? filtered
        : filtered.filter((it) => (airingFilter === "airing") === it.series.is_airing),
    [filtered, airingFilter]
  );

  // Tab counts follow the other filters, so a tab never promises rows the
  // grid will not show.
  const counts = useMemo(() => {
    const c = { watching: 0, plan: 0, completed: 0, all: scoped.length };
    for (const it of scoped) c[statusOf(it)] += 1;
    return c;
  }, [scoped]);

  // "Todas" is one flat grid: grouped by status (watching, plan, completed)
  // with the chosen sort applied inside each group, except for a non-default
  // sort, which orders the whole list.
  const visible = useMemo(() => {
    const cmp = comparatorFor(sort);
    const list = scoped.filter((it) => statusFilter === "all" || statusOf(it) === statusFilter);
    if (statusFilter === "all" && sort === "recent") {
      return list.sort(
        (a, b) => STATUS_ORDER[statusOf(a)] - STATUS_ORDER[statusOf(b)] || cmp(a, b)
      );
    }
    return list.sort(cmp);
  }, [scoped, statusFilter, sort]);
  const visibleCount = visible.length;

  function getGenreState(genre: string): "neutral" | "include" | "exclude" {
    if (includedGenres.has(genre)) return "include";
    if (excludedGenres.has(genre)) return "exclude";
    return "neutral";
  }

  // Cycle neutral -> include -> exclude -> neutral. Reads the current
  // (closed-over) state to decide the transition, then applies it via
  // functional setState so React's batching stays safe.
  function toggleGenre(genre: string) {
    if (includedGenres.has(genre)) {
      setIncludedGenres((prev) => {
        const next = new Set(prev);
        next.delete(genre);
        return next;
      });
      setExcludedGenres((prev) => new Set(prev).add(genre));
    } else if (excludedGenres.has(genre)) {
      setExcludedGenres((prev) => {
        const next = new Set(prev);
        next.delete(genre);
        return next;
      });
    } else {
      setIncludedGenres((prev) => new Set(prev).add(genre));
    }
  }

  function clearGenreFilters() {
    setIncludedGenres(new Set());
    setExcludedGenres(new Set());
  }

  return (
    <div className="page">
      <div className="page-head">
        <h2 className="page-title">{t("nav.library")}</h2>
        <div className="spacer" />
        <span className="muted">
          {t("common.seriesCount", { count: items.length === 0 ? 0 : visibleCount })}
        </span>
        {!selectMode && items.length > 0 && (
          <button
            type="button"
            className="btn btn-ghost"
            onClick={() => setSelectMode(true)}
          >
            {t("library.selectMode")}
          </button>
        )}
        {selectMode && (
          <button
            type="button"
            className="btn btn-ghost"
            onClick={() => {
              setSelectMode(false);
              setSelected(new Set());
            }}
          >
            {t("library.cancelSelect")}
          </button>
        )}
      </div>

      {items.length === 0 ? (
        <div className="empty">
          {t("library.emptyTitle")}
          <br />
          {t("library.emptyHint")}
        </div>
      ) : (
        <>
          {/* Same shape as Catálogo › Explorar: one row of search + the key
              segmented control + a select + "Más filtros", with the long
              tail (genre chips, studio) behind the disclosure so it never
              wraps the primary controls onto extra lines. */}
          <div className="filter-bar lib-filters">
            <div className="filter-row">
              <div className="search">
                <span className="icon" aria-hidden="true">
                  ⌕
                </span>
                <label htmlFor="library-search" className="sr-only">
                  {t("library.searchAriaLabel")}
                </label>
                <input
                  id="library-search"
                  className="input"
                  placeholder={t("common.searchPlaceholder")}
                  value={query}
                  onChange={(e) => setQuery(e.target.value)}
                />
              </div>
              <div className="seg lib-tabs" role="group" aria-label={t("library.filterStatus")}>
                {STATUS_KEYS.map((f) => (
                  <button
                    key={f}
                    type="button"
                    className={`seg-btn${statusFilter === f ? " active" : ""}`}
                    aria-pressed={statusFilter === f}
                    onClick={() => setStatusFilter(f)}
                  >
                    {f === "all"
                      ? t("library.statusAll")
                      : f === "watching"
                        ? t("library.watching")
                        : f === "plan"
                          ? t("library.tabPlan")
                          : t("library.completed")}
                    <span className="seg-count">{counts[f]}</span>
                  </button>
                ))}
              </div>
              <select
                id="lib-type-select"
                className="input filter-select"
                aria-label={t("library.filterType")}
                value={typeFilter}
                onChange={(e) => setTypeFilter(e.target.value)}
              >
                <option value="all">{t("library.typeAll")}</option>
                {typeOptions.map((k) => (
                  <option key={k} value={k}>
                    {k === "OTROS" ? t("library.typeOther") : k}
                  </option>
                ))}
              </select>
              <select
                className="input filter-select"
                aria-label={t("library.sortLabel")}
                value={sort}
                onChange={(e) => setSort(e.target.value as SortKey)}
              >
                {SORT_KEYS.map((k) => (
                  <option key={k} value={k}>
                    {t(
                      k === "recent"
                        ? "library.sortRecent"
                        : k === "title"
                          ? "library.sortTitle"
                          : k === "progress"
                            ? "library.sortProgress"
                            : "library.sortPending"
                    )}
                  </option>
                ))}
              </select>
              {(
                <button
                  type="button"
                  className={`btn btn-secondary filter-toggle-btn${moreOpen ? " active" : ""}`}
                  onClick={() => setMoreOpen((v) => !v)}
                  aria-expanded={moreOpen}
                  aria-controls="lib-more-filters"
                >
                  {moreOpen ? t("library.lessFilters") : t("library.moreFilters")}
                  {secondaryActiveCount > 0 && (
                    <span className="filter-active-badge">{secondaryActiveCount}</span>
                  )}
                </button>
              )}
              {anyFilterActive && (
                <button type="button" className="btn btn-ghost" onClick={clearAllFilters}>
                  {t("library.clearFilters")}
                </button>
              )}
            </div>

            {moreOpen && (
              <Reveal>
                <div className="filter-collapsible-panel" id="lib-more-filters">
                  <div className="filter-collapsible-row">
                    <span className="filter-label" id="lib-airing-label">
                      {t("library.filterLabel")}
                    </span>
                    <div className="seg" role="group" aria-labelledby="lib-airing-label">
                      {(["all", "airing", "finished"] as AiringFilter[]).map((f) => (
                        <button
                          key={f}
                          type="button"
                          className={`seg-btn${airingFilter === f ? " active" : ""}`}
                          aria-pressed={airingFilter === f}
                          onClick={() => setAiringFilter(f)}
                        >
                          {f === "all"
                            ? t("library.filterAll")
                            : f === "airing"
                              ? t("library.filterAiring")
                              : t("library.filterFinished")}
                        </button>
                      ))}
                    </div>
                  </div>
                  {studioOptions.length > 0 && (
                    <div className="filter-collapsible-row">
                      <label className="filter-label" htmlFor="lib-studio-select">
                        {t("library.filterStudio")}
                      </label>
                      <select
                        id="lib-studio-select"
                        className="input filter-select"
                        value={studioFilter}
                        onChange={(e) => setStudioFilter(e.target.value)}
                      >
                        <option value="all">{t("library.studioAll")}</option>
                        {studioOptions.map((s) => (
                          <option key={s} value={s}>
                            {s}
                          </option>
                        ))}
                      </select>
                    </div>
                  )}
                  {genreOptions.length > 0 && (
                    <div className="filter-group">
                      <span className="filter-label" id="lib-genre-label">
                        {t("library.filterGenre")}
                      </span>                    <div className="chip-row" role="group" aria-labelledby="lib-genre-label">
                        {visibleGenreOptions.map((g) => {
                          const state = getGenreState(g);
                          return (
                            <button
                              key={g}
                              type="button"
                              className={`chip-toggle${state !== "neutral" ? " active" : ""}${state === "include" ? " chip-include" : ""}${state === "exclude" ? " chip-exclude" : ""}`}
                              onClick={() => toggleGenre(g)}
                              aria-pressed={state !== "neutral"}
                              aria-label={t("library.genreChipAria", { genre: g, state: t(`library.genreState.${state}`) })}
                            >
                              {state === "include" && <span className="chip-state" aria-hidden="true">+</span>}
                              {state === "exclude" && <span className="chip-state" aria-hidden="true">−</span>}
                              {g}
                            </button>
                          );
                        })}
                        {hiddenGenreCount > 0 && (
                          <button type="button" className="btn btn-sm btn-ghost" onClick={() => setShowAllGenres(true)}>
                            {t("library.showMoreGenres", { count: hiddenGenreCount })}
                          </button>
                        )}
                        {showAllGenres && genreOptions.length > GENRE_VISIBLE_CAP && (
                          <button type="button" className="btn btn-sm btn-ghost" onClick={() => setShowAllGenres(false)}>
                            {t("library.showLessGenres")}
                          </button>
                        )}
                        {(includedGenres.size > 0 || excludedGenres.size > 0) && (
                          <button type="button" className="btn btn-sm btn-ghost" onClick={clearGenreFilters}>
                            {t("library.clearGenreFilters")}
                          </button>
                        )}
                      </div>
                    </div>
                  )}
                </div>
              </Reveal>
            )}
          </div>

          {visibleCount === 0 ? (
            <div className="empty">
              {anyFilterActive || statusFilter === "all"
                ? t("common.noResults")
                : t(
                    statusFilter === "watching"
                      ? "library.emptyWatching"
                      : statusFilter === "plan"
                        ? "library.emptyPlan"
                        : "library.emptyCompleted"
                  )}
              {statusFilter === "plan" && !anyFilterActive && onGoToListas && (
                <>
                  <br />
                  <button type="button" className="btn btn-primary btn-sm lib-empty-cta" onClick={onGoToListas}>
                    {t("library.backlogBannerBtn")}
                  </button>
                </>
              )}
            </div>
          ) : (
            <>
              {selected.size > 0 && (
                <div className="batch-bar">
                  <span className="muted">{t("library.selectedCount", { count: selected.size })}</span>
                  <div className="spacer" />
                  <button
                    type="button"
                    className="btn btn-ghost"
                    onClick={bulkMoveToPlan}
                    disabled={batching}
                  >
                    {t("library.moveToWant")}
                  </button>
                  <button
                    type="button"
                    className="btn btn-danger"
                    onClick={bulkRemove}
                    disabled={batching}
                  >
                    {t("library.bulkRemove")}
                  </button>
                  <button
                    type="button"
                    className="btn btn-ghost"
                    onClick={() => {
                      setSelected(new Set());
                      setSelectMode(false);
                    }}
                    disabled={batching}
                  >
                    {t("library.deselectAll")}
                  </button>
                </div>
              )}
              {statusFilter === "plan" && onGoToListas && (
                <div className="backlog-shortcut-banner">
                  <div className="backlog-shortcut-text">
                    <span className="backlog-shortcut-title">{t("library.backlogBannerTitle")}</span>
                    <span className="muted text-sm">{t("library.backlogBannerDesc")}</span>
                  </div>
                  <button type="button" className="btn btn-primary btn-sm" onClick={onGoToListas}>
                    {t("library.backlogBannerBtn")} →
                  </button>
                </div>
              )}
              <div className="grid">
                {visible.map((it) => (
                  <LibraryCard
                    key={it.series.id}
                    item={it}
                    onOpenSeries={onOpenSeries}
                    onChanged={load}
                    selectMode={selectMode}
                    selected={selected}
                    onToggleSelect={toggleSelect}
                  />
                ))}
              </div>
            </>
          )}
        </>
      )}
    </div>
  );
}
