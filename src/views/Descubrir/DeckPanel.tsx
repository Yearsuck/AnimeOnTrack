import { useEffect, useState } from "react";
import { getCatalogFacets, getDeckBans, setDeckBans } from "../../api";
import { useT } from "../../i18n";
import { DECK_FORMATS } from "./constants";
import { DiscoverModeToggle } from "./DiscoverModeToggle";
import { getInitialDeckPanelOpen, persistDeckPanelOpen } from "./helpers";

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
  const [saving, setSaving] = useState(false);
  const [saved, setSaved] = useState(false);
  const [open, setOpen] = useState(getInitialDeckPanelOpen);

  useEffect(() => {
    getDeckBans()
      .then((b) => {
        setBannedGenres(new Set(b.genres));
        setBannedFormats(new Set(b.formats));
        setHideUpcoming(b.hide_upcoming);
        setStatusDataSynced(b.status_data_synced);
      })
      .catch((err) => console.error("getDeckBans failed", err));
    getCatalogFacets()
      .then((f) => setAllGenres(f.genres))
      .catch((err) => console.error("getCatalogFacets failed", err));
  }, []);

  function toggle(set: Set<string>, setter: (s: Set<string>) => void, value: string) {
    const next = new Set(set);
    if (next.has(value)) next.delete(value);
    else next.add(value);
    setter(next);
    setSaved(false);
  }

  async function save() {
    setSaving(true);
    try {
      await setDeckBans([...bannedGenres], [...bannedFormats], hideUpcoming);
      setSaved(true);
      onBansSaved();
    } finally {
      setSaving(false);
    }
  }

  function toggleOpen() {
    setOpen((prev) => {
      const next = !prev;
      persistDeckPanelOpen(next);
      return next;
    });
  }

  const activeBans = bannedGenres.size + bannedFormats.size;

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

          <div className="row deck-panel-save">
            <button className="btn btn-primary" onClick={save} disabled={saving}>
              {saving ? t("discover.filtersSaving") : t("discover.filtersSave")}
            </button>
            {saved && <span className="muted">{t("discover.filtersSaved")}</span>}
          </div>
        </div>
      )}
    </aside>
  );
}
