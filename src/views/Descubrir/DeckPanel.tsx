import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import { useBackLayer } from "../../lib/backStack";
import { getCatalogFacets, getDeckBans, getDiscoverNiche, setDeckBans, setDiscoverNiche } from "../../api";
import { useT } from "../../i18n";
import { DECK_FORMATS } from "./constants";
import { formatLabel } from "../../lib/formatLabel";
import { DiscoverModeToggle } from "./DiscoverModeToggle";
import { useDiscoverMode } from "../../discoverMode";
import { getInitialDeckPanelOpen, persistDeckPanelOpen } from "./helpers";
import { countActiveFilters, DECK_DEFAULTS } from "./deckReset";
import { DateRangeControl, ScoreRangeControl } from "../../components/filters";
import "../../styles/catalog.css";

// Deck settings panel — lives inside SwipeView (see the design spec at
// docs/superpowers/specs/2026-07-13-discover-swipe-sidepanel-design.md).
// Combines the deck-mode toggle with the genre/type bans (formerly the
// standalone "Filtros" sub-tab, now collapsible and sticky alongside the
// card). Banned chips render "active" (highlighted = excluded from the
// deck). Genres come from the synced catalog facets; formats are the fixed
// deck whitelist. Persisted via setDeckBans; takes effect on the next
// discover_catalog_card PROVIDED the caller also invalidates the local
// prefetch queue — that's what `onBansSaved` is for (SwipeView passes its
// `resetQueue`), since SwipeView no longer unmounts when this panel is used
// (it's embedded, not a separate sub-tab) and would otherwise keep serving
// up to PREFETCH_TARGET already-fetched cards that predate the new bans.
/** What the deck stage can ask of its settings panel. */
export interface DeckApi {
  openSettings: () => void;
  /** How many filters currently narrow the deck. */
  activeBans: number;
}

export function DeckPanel({
  onBansSaved,
  children,
}: {
  onBansSaved: () => void;
  /** The deck stage, given a handle to open the settings drawer. */
  children: (api: DeckApi) => ReactNode;
}) {
  const t = useT();
  const [discoverMode] = useDiscoverMode();
  const [allGenres, setAllGenres] = useState<string[]>([]);
  const [bannedGenres, setBannedGenres] = useState<Set<string>>(new Set());
  const [bannedFormats, setBannedFormats] = useState<Set<string>>(new Set());
  const [hideUpcoming, setHideUpcoming] = useState(false);
  const [statusDataSynced, setStatusDataSynced] = useState(true);
  const [minStartDate, setMinStartDate] = useState<number | null>(null);
  const [maxStartDate, setMaxStartDate] = useState<number | null>(null);
  const [minScore, setMinScore] = useState<number | null>(null);
  const [nicheBias, setNicheBias] = useState(0);
  const nicheBiasRef = useRef(nicheBias);
  nicheBiasRef.current = nicheBias;
  const onBansSavedRef = useRef(onBansSaved);
  onBansSavedRef.current = onBansSaved;
  const [saving, setSaving] = useState(false);
  const [saved, setSaved] = useState(false);
  const [open, setOpen] = useState(getInitialDeckPanelOpen);

  // Guards against saving on mount: stays false until the initial getDeckBans()
  // response has been applied, so loading the saved values never triggers a save or onBansSaved.
  const loadedRef = useRef(false);
  const initialResponseReceivedRef = useRef(false);

  useEffect(() => {
    getDeckBans()
      .then((b) => {
        setBannedGenres(new Set(b.genres));
        setBannedFormats(new Set(b.formats));
        setHideUpcoming(b.hide_upcoming);
        setStatusDataSynced(b.status_data_synced);
        setMinStartDate(b.min_start_date);
        setMaxStartDate(b.max_start_date);
        setMinScore(b.min_score);
        initialResponseReceivedRef.current = true;
      })
      .catch((err) => {
        console.error("getDeckBans failed", err);
        loadedRef.current = true;
      });
    getCatalogFacets()
      .then((f) => setAllGenres(f.genres))
      .catch((err) => console.error("getCatalogFacets failed", err));
    getDiscoverNiche()
      .then((v) => {
        setNicheBias(v);
        nicheBiasRef.current = v;
      })
      .catch((err) => console.error("getDiscoverNiche failed", err));
  }, []);

  function handleNicheChange(e: React.ChangeEvent<HTMLInputElement>) {
    const val = parseFloat(e.target.value);
    setNicheBias(val);
    nicheBiasRef.current = val;
  }

  function persistNiche() {
    setDiscoverNiche(nicheBiasRef.current).catch((err) => {
      console.error("setDiscoverNiche failed", err);
    });
  }

  function toggle(set: Set<string>, setter: (s: Set<string>) => void, value: string) {
    const next = new Set(set);
    if (next.has(value)) next.delete(value);
    else next.add(value);
    setter(next);
    setSaved(false);
  }

  const save = useCallback(async () => {
    setSaved(false);
    setSaving(true);
    try {
      await setDeckBans(
        [...bannedGenres],
        [...bannedFormats],
        hideUpcoming,
        minStartDate,
        maxStartDate,
        minScore
      );
      setSaved(true);
      onBansSavedRef.current();
    } catch (err) {
      console.error("setDeckBans failed", err);
    } finally {
      setSaving(false);
    }
  }, [bannedGenres, bannedFormats, hideUpcoming, minStartDate, maxStartDate, minScore]);

  // Auto-apply: every change of genre chip, format chip, hide-upcoming checkbox,
  // date range or score saves automatically after 600 ms of no further change (debounce).
  useEffect(() => {
    if (!loadedRef.current) {
      if (initialResponseReceivedRef.current) {
        loadedRef.current = true;
      }
      return;
    }

    setSaved(false);
    pendingFlushRef.current = () =>
      setDeckBans([...bannedGenres], [...bannedFormats], hideUpcoming, minStartDate, maxStartDate, minScore);
    const timer = setTimeout(() => {
      pendingFlushRef.current = null;
      save();
    }, 600);

    return () => {
      clearTimeout(timer);
    };
  }, [bannedGenres, bannedFormats, hideUpcoming, minStartDate, maxStartDate, minScore, save]);

  // Leaving the section (or the tab) inside the 600 ms debounce window used to drop the last
  // filter change; persist it on unmount. Only the write is flushed: the parent's refill callback
  // is skipped since the deck it would refill is going away.
  const pendingFlushRef = useRef<(() => Promise<unknown>) | null>(null);
  useEffect(() => {
    return () => {
      const flush = pendingFlushRef.current;
      pendingFlushRef.current = null;
      flush?.().catch((err) => console.error("setDeckBans flush failed", err));
    };
  }, []);

  function handleResetFilters() {
    setSaved(false);
    setBannedGenres(new Set(DECK_DEFAULTS.genres));
    setBannedFormats(new Set(DECK_DEFAULTS.formats));
    setHideUpcoming(DECK_DEFAULTS.hideUpcoming);
    setMinStartDate(DECK_DEFAULTS.minStartDate);
    setMaxStartDate(DECK_DEFAULTS.maxStartDate);
    setMinScore(DECK_DEFAULTS.minScore);
    setNicheBias(DECK_DEFAULTS.nicheBias);
    nicheBiasRef.current = DECK_DEFAULTS.nicheBias;
    setDiscoverNiche(0).catch((err) => {
      console.error("setDiscoverNiche failed", err);
    });
  }

  function setDrawer(next: boolean) {
    setOpen(next);
    persistDeckPanelOpen(next);
  }
  useBackLayer(open, () => setDrawer(false));

  const activeBans = countActiveFilters({
    genres: bannedGenres,
    formats: bannedFormats,
    hideUpcoming,
    minStartDate,
    maxStartDate,
    minScore,
  });

  const api: DeckApi = { openSettings: () => setDrawer(true), activeBans };

  // Deck settings live in one side drawer next to the stage, opened from the
  // toolbar (or from the empty deck's "Ajustar mazo"). All state stays in
  // this one component so the single debounced auto-save still sees every
  // filter.
  return (
    <div className="disc-layout">
      <div className="disc-main">
        <div className="disc-toolbar">
          <DiscoverModeToggle />
          <button
            type="button"
            className={`btn cat-filter-btn${open ? " active" : ""}`}
            aria-expanded={open}
            aria-controls="deck-drawer"
            onClick={() => setDrawer(!open)}
          >
            {t("discover.deckSettings")}
            {activeBans > 0 && <span className="filter-active-badge">{activeBans}</span>}
          </button>
        </div>
        {children(api)}
      </div>

      {open && (
        <aside
          id="deck-drawer"
          className="cat-drawer disc-drawer"
          aria-label={t("discover.deckSettings")}
          onKeyDown={(e) => {
            if (e.key === "Escape") {
              e.stopPropagation();
              setDrawer(false);
            }
          }}
        >
          <div className="cat-drawer-head">
            <h2 className="cat-drawer-title">{t("discover.deckSettings")}</h2>
            <div className="spacer" />
            <button
              type="button"
              className="btn btn-ghost btn-sm"
              onClick={() => setDrawer(false)}
              aria-label={t("discover.deckSettingsClose")}
            >
              ✕
            </button>
          </div>

          {discoverMode === "recommended" && (
            <div className="deck-panel-niche">
              <label htmlFor="deck-niche-slider" className="cat-drawer-legend">
                {t("niche.label")}
              </label>
              <input
                id="deck-niche-slider"
                type="range"
                min="-1"
                max="1"
                step="0.25"
                value={nicheBias}
                onChange={handleNicheChange}
                onPointerUp={persistNiche}
                onKeyUp={persistNiche}
                onBlur={persistNiche}
                className="niche-slider"
              />
              <div className="deck-panel-niche-captions">
                <span className="muted text-sm">{t("niche.mainstream")}</span>
                <span className="muted text-sm">{t("niche.niche")}</span>
              </div>
            </div>
          )}

          <p className="muted text-sm">{t("discover.filtersIntro")}</p>

          <fieldset className="cat-drawer-group">
            <legend className="cat-drawer-legend">{t("discover.filtersFormatsHeading")}</legend>
            <div className="chip-row">
              {DECK_FORMATS.map((f) => (
                <button
                  key={f}
                  type="button"
                  className={`chip-toggle${bannedFormats.has(f) ? " active" : ""}`}
                  aria-pressed={bannedFormats.has(f)}
                  onClick={() => toggle(bannedFormats, setBannedFormats, f)}
                >
                  {formatLabel(t, f)}
                </button>
              ))}
            </div>
          </fieldset>

          {allGenres.length > 0 && (
            <fieldset className="cat-drawer-group">
              <legend className="cat-drawer-legend">{t("discover.filtersGenresHeading")}</legend>
              <div className="chip-row">
                {allGenres.map((g) => (
                  <button
                    key={g}
                    type="button"
                    className={`chip-toggle${bannedGenres.has(g) ? " active" : ""}`}
                    aria-pressed={bannedGenres.has(g)}
                    onClick={() => toggle(bannedGenres, setBannedGenres, g)}
                  >
                    {g}
                  </button>
                ))}
              </div>
            </fieldset>
          )}

          <div className="cat-drawer-group">
            <button
              type="button"
              className={`chip-toggle${hideUpcoming ? " active" : ""}`}
              aria-pressed={hideUpcoming}
              onClick={() => {
                setHideUpcoming((prev) => !prev);
                setSaved(false);
              }}
            >
              {t("discover.filtersHideUpcoming")}
            </button>
            {hideUpcoming && !statusDataSynced && (
              <p className="muted text-sm text-danger deck-panel-warn">
                {t("discover.filtersHideUpcomingNeedsSync")}
              </p>
            )}
          </div>

          <div className="cat-drawer-group">
            <DateRangeControl
              minStartDate={minStartDate}
              maxStartDate={maxStartDate}
              label={t("discover.filtersDateHeading")}
              onChange={(min, max) => {
                setMinStartDate(min);
                setMaxStartDate(max);
                setSaved(false);
              }}
            />
          </div>

          <ScoreRangeControl
            minScore={minScore}
            minOnly={true}
            label={t("discover.filtersScoreHeading")}
            onChange={(min) => {
              setMinScore(min);
              setSaved(false);
            }}
          />

          <div className="disc-drawer-foot">
            <button
              type="button"
              className="btn"
              onClick={handleResetFilters}
              disabled={activeBans === 0 && nicheBias === 0}
            >
              {t("discover.filtersReset")}
            </button>
            <span className="muted text-sm">
              {saving ? t("discover.filtersSaving") : saved ? t("discover.filtersSaved") : t("discover.filtersAutoHint")}
            </span>
          </div>
        </aside>
      )}
    </div>
  );
}
