import { useCallback, useEffect, useState } from "react";
import { listPending, openEpisode, setSeenCascade } from "../api";
import { useT } from "../i18n";
import type { PendingItem, Series } from "../types";
import { countdownLabel } from "./AiringGrid";
import { parseReleasedAtToUnixSeconds } from "../lib/parseReleasedAt";

const REMOVE_MS = 220;
type PendingSort = "remaining_asc" | "remaining_desc";

export function Pending({
  onOpenSeries,
  onChanged,
  refreshSignal,
}: {
  onOpenSeries: (s: Series) => void;
  onChanged: () => void;
  // Bumped by App.tsx whenever something outside this component changed the
  // pending set — a finished refresh() (including the one on startup, which
  // lands after this view has already mounted and fetched), the topbar
  // "Actualizar" button, a site switch, or the SeriesDetail overlay marking
  // episodes seen on top of this list. Without it the list silently
  // contradicted its own tab badge, which App.tsx *did* keep up to date.
  // Same pattern as AiringGrid's identically-named prop.
  refreshSignal?: number;
}) {
  const t = useT();
  const [items, setItems] = useState<PendingItem[]>([]);
  const [removing, setRemoving] = useState<Set<number>>(new Set());
  const [sort, setSort] = useState<PendingSort>("remaining_asc");

  const load = useCallback(async () => {
    setItems(await listPending(sort));
  }, [sort]);
  useEffect(() => {
    load();
    // `refreshSignal` isn't read in the body — it's a bump counter whose only
    // job is to be a dependency, same as in AiringGrid.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [load, refreshSignal]);

  async function watch(it: PendingItem) {
    await openEpisode(it.episode.url);
  }

  function markSeen(it: PendingItem) {
    setRemoving((r) => new Set(r).add(it.episode.id));
    setTimeout(async () => {
      try {
        await setSeenCascade(it.series.id, it.episode.number, true);
        await load();
        onChanged();
      } catch (e) {
        console.error("markSeen failed:", e);
      } finally {
        // Release the fade-out marker. The set only ever grew before, so an
        // id that came BACK into the list — un-marked from the SeriesDetail
        // overlay while this view stays mounted, then reloaded through
        // `refreshSignal` — still matched `.ep-row.removing { opacity: 0 }`
        // and rendered invisible-but-present. In `finally` so a failed
        // cascade (where the row legitimately stays in the list) can't hide
        // it either.
        setRemoving((r) => {
          const next = new Set(r);
          next.delete(it.episode.id);
          return next;
        });
      }
    }, REMOVE_MS);
  }

  // Keyed by series id, not by title: two distinct `series` rows can carry
  // the same title (one show scraped under two slugs on a site, or two
  // unrelated shows named identically). Keying on the text merged them into
  // a single block whose header click-through opened `eps[0].series` — the
  // right show for the first group member and silently the wrong one for
  // every other.
  const groups = new Map<number, PendingItem[]>();
  for (const it of items) {
    const k = it.series.id;
    (groups.get(k) ?? groups.set(k, []).get(k)!).push(it);
  }

  return (
    <div className="page">
      <div className="page-head">
        <h2 className="page-title">{t("nav.pending")}</h2>
        <span className="muted">{t("pending.episodesToWatch", { count: items.length })}</span>
        <div className="spacer" />
        {items.length > 0 && (
          <div className="lib-filter-bar">
            <span className="muted text-sm">
              {t("pending.sortLabel")}
            </span>
            <div className="seg">
              <button
                type="button"
                className={`seg-btn${sort === "remaining_asc" ? " active" : ""}`}
                onClick={() => setSort("remaining_asc")}
              >
                {t("pending.sortFewest")}
              </button>
              <button
                type="button"
                className={`seg-btn${sort === "remaining_desc" ? " active" : ""}`}
                onClick={() => setSort("remaining_desc")}
              >
                {t("pending.sortMost")}
              </button>
            </div>
          </div>
        )}
      </div>

      {items.length === 0 ? (
        <div className="empty">
          {t("pending.empty")}
          <br />
          {t("pending.emptyHint")}
        </div>
      ) : (
        [...groups.entries()].map(([seriesId, eps]) => {
          const series = eps[0].series;
          return (
            <div key={seriesId} className="series-block">
              <div className="series-head clickable" onClick={() => onOpenSeries(series)}>
                {series.cover_url && <img src={series.cover_url} alt="" />}
                <div>
                  <div className="name">{series.title}</div>
                  <div className="count">
                    {t(eps.length === 1 ? "pending.new" : "pending.newPlural", { count: eps.length })}
                  </div>
                </div>
              </div>
              {eps.map((it) => (
                <div
                  key={it.episode.id}
                  className={`ep-row ${removing.has(it.episode.id) ? "removing" : ""}`}
                >
                  <span className="ep-num">{it.episode.number}</span>
                  <div className="ep-main">
                    <div className="ep-title" onClick={() => watch(it)}>
                      {it.episode.title ?? t("common.episodeNumber", { number: it.episode.number })}
                    </div>
                    {it.episode.released_at && (
                      <div className="ep-date">
                        {(() => {
                          const ts = parseReleasedAtToUnixSeconds(it.episode.released_at);
                          return ts != null ? countdownLabel(ts, t) : it.episode.released_at;
                        })()}
                      </div>
                    )}
                  </div>
                  <div className="ep-actions">
                    <button className="btn" onClick={() => watch(it)}>
                      {t("common.watch")}
                    </button>
                    <button
                      className={`check ${removing.has(it.episode.id) ? "on" : ""}`}
                      title={t("common.markSeen")}
                      onClick={() => markSeen(it)}
                    >
                      ✓
                    </button>
                  </div>
                </div>
              ))}
            </div>
          );
        })
      )}
    </div>
  );
}
