import { useCallback, useEffect, useRef, useState } from "react";
import { getCatalogFacets, getDeckBans, getDiscoverNiche, setDeckBans, setDiscoverNiche } from "../../api";
import { useT } from "../../i18n";
import { DECK_FORMATS } from "./constants";
import { DiscoverModeToggle } from "./DiscoverModeToggle";
import { getInitialDeckPanelOpen, persistDeckPanelOpen } from "./helpers";
import { countActiveFilters, DECK_DEFAULTS } from "./deckReset";
import { DateRangeControl, ScoreRangeControl } from "../../components/filters";

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
export function DeckPanel({ onBansSaved }: { onBansSaved: () => void }) {
  const t = useT();
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
    const timer = setTimeout(() => {
      save();
    }, 600);

    return () => {
      clearTimeout(timer);
    };
  }, [bannedGenres, bannedFormats, hideUpcoming, minStartDate, maxStartDate, minScore, save]);

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

  function toggleOpen() {
    setOpen((prev) => {
      const next = !prev;
      persistDeckPanelOpen(next);
      return next;
    });
  }

  const activeBans = countActiveFilters({
    genres: bannedGenres,
    formats: bannedFormats,
    hideUpcoming,
    minStartDate,
    maxStartDate,
    minScore,
  });

  return (
    <aside className="deck-panel">
      <button
        type="button"
        className="deck-panel-head"
        aria-expanded={open}
        aria-controls="deck-panel-body"
        aria-label={t("discover.deckPanelToggleAria")}
        onClick={toggleOpen}
      >
        <span className="deck-panel-title">{t("discover.deckPanelTitle")}</span>
        {!open && activeBans > 0 && (
          <span className="deck-panel-badge">{t("discover.deckPanelBansBadge", { count: activeBans })}</span>
        )}
        <span className={`deck-panel-chevron${open ? " open" : ""}`} aria-hidden="true">
          ▾
        </span>
      </button>

      {open && (
        <div id="deck-panel-body" className="deck-panel-body">
          <div className="deck-panel-mode">
            <DiscoverModeToggle />
          </div>

          <div className="deck-panel-niche">
            <label htmlFor="deck-niche-slider" className="deck-panel-niche-label">
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
            <p className="muted text-sm deck-panel-niche-hint">
              {t("niche.hint")}
            </p>
          </div>

          <p className="muted deck-panel-intro">
            {t("discover.filtersIntro")}
          </p>

          <h3 className="section-title deck-panel-heading">
            {t("discover.filtersFormatsHeading")}
          </h3>
          <div className="chip-row deck-panel-chips">
            {DECK_FORMATS.map((f) => (
              <button
                key={f}
                type="button"
                className={`chip-toggle${bannedFormats.has(f) ? " active" : ""}`}
                onClick={() => toggle(bannedFormats, setBannedFormats, f)}
              >
                {f}
              </button>
            ))}
          </div>

          <h3 className="section-title deck-panel-heading">
            {t("discover.filtersGenresHeading")}
          </h3>
          {allGenres.length > 0 && (
            <div className="chip-row deck-panel-genres deck-panel-chips">
              {allGenres.map((g) => (
                <button
                  key={g}
                  type="button"
                  className={`chip-toggle${bannedGenres.has(g) ? " active" : ""}`}
                  onClick={() => toggle(bannedGenres, setBannedGenres, g)}
                >
                  {g}
                </button>
              ))}
            </div>
          )}

          <h3 className="section-title deck-panel-heading">
            {t("discover.filtersOtherHeading")}
          </h3>
          <div className={`chip-row ${hideUpcoming && !statusDataSynced ? "deck-panel-chips-tight" : "deck-panel-chips"}`}>
            <button
              type="button"
              className={`chip-toggle${hideUpcoming ? " active" : ""}`}
              onClick={() => {
                setHideUpcoming((prev) => !prev);
                setSaved(false);
              }}
            >
              {t("discover.filtersHideUpcoming")}
            </button>
          </div>
          {hideUpcoming && !statusDataSynced && (
            <p className="muted text-sm text-danger deck-panel-warn">
              {t("discover.filtersHideUpcomingNeedsSync")}
            </p>
          )}

          <h3 className="section-title deck-panel-heading">
            {t("discover.filtersDateHeading")}
          </h3>
          <DateRangeControl
            minStartDate={minStartDate}
            maxStartDate={maxStartDate}
            onChange={(min, max) => {
              setMinStartDate(min);
              setMaxStartDate(max);
              setSaved(false);
            }}
          />
          <p className="muted text-sm deck-panel-date-hint">
            {t("discover.filtersDateHint")}
          </p>

          <h3 className="section-title deck-panel-heading">
            {t("discover.filtersScoreHeading")}
          </h3>
          <ScoreRangeControl
            minScore={minScore}
            minOnly={true}
            onChange={(min) => {
              setMinScore(min);
              setSaved(false);
            }}
          />

          <div className="row deck-panel-save">
            <button className="btn btn-primary" onClick={save} disabled={saving}>
              {saving ? t("discover.filtersSaving") : t("discover.filtersSave")}
            </button>
            <button
              type="button"
              className="btn"
              onClick={handleResetFilters}
              disabled={activeBans === 0 && nicheBias === 0}
            >
              {t("discover.filtersReset")}
            </button>
            {saved && <span className="muted">{t("discover.filtersSaved")}</span>}
            {!saving && !saved && (
              <span className="muted">{t("discover.filtersAutoHint")}</span>
            )}
          </div>
        </div>
      )}
    </aside>
  );
}
