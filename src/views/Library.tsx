import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { listLibrary, openEpisode, reclassifySeries } from "../api";
import { useT } from "../i18n";
import type { LibraryItem, Series } from "../types";

type Status = "completed" | "watching" | "plan";
// Airing filter for the Viendo section.
type AiringFilter = "all" | "airing" | "finished";
// Estado filter — "all" shows every section (current default behavior); a
// specific value hides the other sections entirely.
type StatusFilter = "all" | Status;

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
function statusOf(it: LibraryItem): Status {
  if (it.watched_externally) return "completed";
  const allSeen = it.total_episodes > 0 && it.seen_episodes === it.total_episodes;
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

// Up to two initials from the title, for the poster-less fallback block —
// never render a broken <img> when cover_url is null.
function initials(title: string): string {
  const chars = title
    .trim()
    .split(/\s+/)
    .filter(Boolean)
    .slice(0, 2)
    .map((w) => w[0]?.toUpperCase() ?? "")
    .join("");
  return chars || "?";
}

function LibraryCard({
  item,
  showAction,
  showMenu,
  onOpenSeries,
  onChanged,
}: {
  item: LibraryItem;
  showAction: boolean;
  showMenu: boolean;
  onOpenSeries: (s: Series) => void;
  onChanged: () => void;
}) {
  const t = useT();
  // cover_url is a data: URI only for followed series whose cover was
  // fetched; otherwise it's a remote (Cloudflare-blocked) URL the WebView
  // can't actually load, or null. Both must degrade to the text fallback —
  // null up-front, a broken remote URL via onError — so the grid never
  // shows a broken <img>.
  const [imgFailed, setImgFailed] = useState(false);
  const showFallback = !item.series.cover_url || imgFailed;

  // Overflow "⋯" menu (Watching cards only) — reclassify actions
  // (see docs/superpowers/specs/2026-07-11-reversibility-classifications-design.md).
  // Closed on an outside click, same pattern used nowhere else yet in this
  // codebase so wired by hand here.
  const [menuOpen, setMenuOpen] = useState(false);
  const menuRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!menuOpen) return;
    function onDocMouseDown(e: MouseEvent) {
      if (menuRef.current && !menuRef.current.contains(e.target as Node)) {
        setMenuOpen(false);
      }
    }
    document.addEventListener("mousedown", onDocMouseDown);
    return () => document.removeEventListener("mousedown", onDocMouseDown);
  }, [menuOpen]);

  async function unfollow(e: React.SyntheticEvent) {
    e.stopPropagation();
    setMenuOpen(false);
    await reclassifySeries(item.series.id, "None");
    onChanged();
  }

  async function moveToWant(e: React.SyntheticEvent) {
    e.stopPropagation();
    setMenuOpen(false);
    await reclassifySeries(item.series.id, "Want");
    onChanged();
  }

  const pct = item.total_episodes
    ? Math.round((item.seen_episodes / item.total_episodes) * 100)
    : 0;

  // The card is a div[role=button] rather than a real <button> because it
  // must contain a real nested <button> (the play action) — nesting
  // <button> inside <button> is invalid HTML. Enter and Space are wired by
  // hand to match native button semantics.
  function handleKeyDown(e: React.KeyboardEvent<HTMLDivElement>) {
    if (e.key === "Enter" || e.key === " ") {
      e.preventDefault();
      onOpenSeries(item.series);
    }
  }

  // Stop propagation on both click and keydown: a keydown on the nested
  // button still bubbles to the card's onKeyDown even though the button's
  // own Enter/Space handling stays internal, so without stopping it here
  // Enter on this button would open the detail view *and* play the episode
  // (the bubbling bug that already bit AiringGrid's follow button and
  // Descubrir's poster click — commit 909c64d).
  function playNext(e: React.SyntheticEvent) {
    e.stopPropagation();
    if (item.next_episode) openEpisode(item.next_episode.url);
  }

  // The Completed section normally passes showMenu={false} (nothing to
  // reclassify from "done"), but a watched-externally card ("Ya lo vi" from
  // the catalog) still needs an undo path — it's a manual mark, not a
  // derived state — so it always gets a (single-item) menu regardless of
  // the section's showMenu prop.
  const effectiveShowMenu = showMenu || item.watched_externally;

  return (
    <div
      className="card lib-card"
      role="button"
      tabIndex={0}
      onClick={() => onOpenSeries(item.series)}
      onKeyDown={handleKeyDown}
      title={item.series.title}
    >
      <div className="poster">
        {effectiveShowMenu && (
          <div className="card-menu-wrap" ref={menuRef}>
            <button
              type="button"
              className="card-menu-btn"
              aria-label={t("library.cardMenuAria")}
              onClick={(e) => {
                e.stopPropagation();
                setMenuOpen((v) => !v);
              }}
              onKeyDown={(e) => e.stopPropagation()}
            >
              ⋯
            </button>
            {menuOpen && (
              <div className="card-menu-list" onClick={(e) => e.stopPropagation()}>
                {item.watched_externally ? (
                  <button type="button" className="card-menu-item danger" onClick={unfollow}>
                    {t("library.markNotWatched")}
                  </button>
                ) : (
                  <>
                    <button type="button" className="card-menu-item danger" onClick={unfollow}>
                      {t("library.unfollow")}
                    </button>
                    <button type="button" className="card-menu-item" onClick={moveToWant}>
                      {t("library.moveToWant")}
                    </button>
                  </>
                )}
              </div>
            )}
          </div>
        )}
        {showFallback ? (
          <div className="poster-fallback" aria-hidden="true">
            {initials(item.series.title)}
          </div>
        ) : (
          <img
            src={item.series.cover_url!}
            alt=""
            loading="lazy"
            onError={() => setImgFailed(true)}
          />
        )}
      </div>
      <div className="card-body">
        <div className="card-title">{item.series.title}</div>
        {item.watched_externally && item.total_episodes === 0 ? (
          <div className="lib-progress-text muted">{t("library.watchedExternally")}</div>
        ) : (
          <>
            <div
              className="progress"
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
            <div className="lib-progress-text muted">
              {item.seen_episodes} / {item.total_episodes}
            </div>
          </>
        )}
        {showAction && item.next_episode && (
          <button
            type="button"
            className="btn btn-primary lib-play-btn"
            onClick={playNext}
            onKeyDown={(e) => e.stopPropagation()}
          >
            {t("library.playNext", { number: item.next_episode.number })}
          </button>
        )}
      </div>
    </div>
  );
}

function LibrarySection({
  title,
  items,
  defaultOpen,
  showAction,
  showMenu,
  onOpenSeries,
  onChanged,
}: {
  title: string;
  items: LibraryItem[];
  defaultOpen: boolean;
  showAction: boolean;
  showMenu: boolean;
  onOpenSeries: (s: Series) => void;
  onChanged: () => void;
}) {
  // An empty section (nothing in this status, or nothing left after the
  // search filter) is omitted entirely rather than rendered with a
  // "no results" placeholder — three always-present empty groups would be
  // noisier than the flat list this view replaces.
  if (items.length === 0) return null;
  return (
    <details className="lib-section" open={defaultOpen}>
      <summary className="lib-section-summary">
        <span className="lib-section-title">{title}</span>
        <span className="lib-section-count">{items.length}</span>
      </summary>
      <div className="grid">
        {items.map((it) => (
          <LibraryCard
            key={it.series.id}
            item={it}
            showAction={showAction}
            showMenu={showMenu}
            onOpenSeries={onOpenSeries}
            onChanged={onChanged}
          />
        ))}
      </div>
    </details>
  );
}

export function Library({ onOpenSeries }: { onOpenSeries: (s: Series) => void }) {
  const t = useT();
  const [items, setItems] = useState<LibraryItem[]>([]);
  const [query, setQuery] = useState("");
  const [airingFilter, setAiringFilter] = useState<AiringFilter>("all");
  const [statusFilter, setStatusFilter] = useState<StatusFilter>("all");
  const [typeFilter, setTypeFilter] = useState<string>("all");
  const [genreFilter, setGenreFilter] = useState<string>("all");
  const [studioFilter, setStudioFilter] = useState<string>("all");

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
    const q = query.trim().toLowerCase();
    return items.filter((it) => {
      if (q && !it.series.title.toLowerCase().includes(q)) return false;
      if (typeFilter !== "all" && normalizeKind(it.kind) !== typeFilter) return false;
      if (genreFilter !== "all" && !it.genres.includes(genreFilter)) return false;
      // "all" never hides unlinked (studio: null) rows — only an explicit
      // studio selection filters them out.
      if (studioFilter !== "all" && it.studio !== studioFilter) return false;
      return true;
    });
  }, [items, query, typeFilter, genreFilter, studioFilter]);

  // All "watching" items (before the airing filter) — drives whether the
  // filter control is shown at all.
  const watchingAll = useMemo(
    () => filtered.filter((it) => statusOf(it) === "watching"),
    [filtered]
  );
  const watching = useMemo(() => {
    const w =
      airingFilter === "all"
        ? watchingAll
        : airingFilter === "airing"
          ? watchingAll.filter((it) => it.series.is_airing)
          : watchingAll.filter((it) => !it.series.is_airing);
    return [...w].sort(byRecentWatched);
  }, [watchingAll, airingFilter]);
  const plan = useMemo(
    () => filtered.filter((it) => statusOf(it) === "plan").sort(byTitle),
    [filtered]
  );
  const completed = useMemo(
    () => filtered.filter((it) => statusOf(it) === "completed").sort(byRecentWatched),
    [filtered]
  );

  // Estado scopes which sections render at all; "all" keeps every section
  // (current default behavior).
  const showWatching = statusFilter === "all" || statusFilter === "watching";
  const showPlan = statusFilter === "all" || statusFilter === "plan";
  const showCompleted = statusFilter === "all" || statusFilter === "completed";
  const visibleCount =
    (showWatching ? watching.length : 0) +
    (showPlan ? plan.length : 0) +
    (showCompleted ? completed.length : 0);

  return (
    <div className="page">
      <div className="page-head">
        <h2 className="page-title">{t("nav.library")}</h2>
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
        <div className="spacer" />
        <span className="muted">
          {t("common.seriesCount", { count: items.length === 0 ? 0 : visibleCount })}
        </span>
      </div>

      {items.length === 0 ? (
        <div className="empty">
          {t("library.emptyTitle")}
          <br />
          {t("library.emptyHint")}
        </div>
      ) : (
        <>
          <div className="lib-filter-bar">
            <span className="muted" style={{ fontSize: 12 }}>
              {t("library.filterStatus")}
            </span>
            <div className="seg">
              {(["all", "watching", "plan", "completed"] as StatusFilter[]).map((f) => (
                <button
                  key={f}
                  type="button"
                  className={`seg-btn${statusFilter === f ? " active" : ""}`}
                  onClick={() => setStatusFilter(f)}
                >
                  {f === "all"
                    ? t("library.statusAll")
                    : f === "watching"
                      ? t("library.watching")
                      : f === "plan"
                        ? t("library.planToWatch")
                        : t("library.completed")}
                </button>
              ))}
            </div>

            <label className="sr-only" htmlFor="lib-type-select">
              {t("library.filterType")}
            </label>
            <span className="muted" style={{ fontSize: 12 }}>
              {t("library.filterType")}
            </span>
            <select
              id="lib-type-select"
              className="input"
              style={{ maxWidth: 150 }}
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

            {genreOptions.length > 0 && (
              <>
                <label className="sr-only" htmlFor="lib-genre-select">
                  {t("library.filterGenre")}
                </label>
                <span className="muted" style={{ fontSize: 12 }}>
                  {t("library.filterGenre")}
                </span>
                <select
                  id="lib-genre-select"
                  className="input"
                  style={{ maxWidth: 150 }}
                  value={genreFilter}
                  onChange={(e) => setGenreFilter(e.target.value)}
                >
                  <option value="all">{t("library.genreAll")}</option>
                  {genreOptions.map((g) => (
                    <option key={g} value={g}>
                      {g}
                    </option>
                  ))}
                </select>
              </>
            )}

            {studioOptions.length > 0 && (
              <>
                <label className="sr-only" htmlFor="lib-studio-select">
                  {t("library.filterStudio")}
                </label>
                <span className="muted" style={{ fontSize: 12 }}>
                  {t("library.filterStudio")}
                </span>
                <select
                  id="lib-studio-select"
                  className="input"
                  style={{ maxWidth: 150 }}
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
              </>
            )}
          </div>

          {filtered.length === 0 || visibleCount === 0 ? (
            <div className="empty">{t("common.noResults")}</div>
          ) : (
            <>
              {showWatching && watchingAll.length > 0 && (
                <div className="lib-filter-bar">
                  <span className="muted" style={{ fontSize: 12 }}>
                    {t("library.filterLabel")}
                  </span>
                  <div className="seg">
                    {(["all", "airing", "finished"] as AiringFilter[]).map((f) => (
                      <button
                        key={f}
                        type="button"
                        className={`seg-btn${airingFilter === f ? " active" : ""}`}
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
              )}
              {showWatching &&
                (watchingAll.length > 0 && watching.length === 0 ? (
                  <div className="empty" style={{ padding: "12px 0" }}>
                    {t("library.filterNoResults")}
                  </div>
                ) : (
                  <LibrarySection
                    title={t("library.watching")}
                    items={watching}
                    defaultOpen
                    showAction
                    showMenu
                    onOpenSeries={onOpenSeries}
                    onChanged={load}
                  />
                ))}
              {showPlan && (
                <LibrarySection
                  title={t("library.planToWatch")}
                  items={plan}
                  defaultOpen
                  showAction
                  showMenu={false}
                  onOpenSeries={onOpenSeries}
                  onChanged={load}
                />
              )}
              {showCompleted && (
                <LibrarySection
                  title={t("library.completed")}
                  items={completed}
                  defaultOpen={false}
                  showAction={false}
                  showMenu={false}
                  onOpenSeries={onOpenSeries}
                  onChanged={load}
                />
              )}
            </>
          )}
        </>
      )}
    </div>
  );
}
