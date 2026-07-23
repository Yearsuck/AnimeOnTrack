import { useCallback, useEffect, useState } from "react";
import { listPending, openEpisode, setSeenCascade } from "../api";
import { useT } from "../i18n";
import type { PendingItem, Series } from "../types";

const REMOVE_MS = 220;
type PendingSort = "remaining_asc" | "remaining_desc";

export function Pending({
  onOpenSeries,
  onChanged,
}: {
  onOpenSeries: (s: Series) => void;
  onChanged: () => void;
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
  }, [load]);

  async function watch(it: PendingItem) {
    await openEpisode(it.episode.url);
  }

  function markSeen(it: PendingItem) {
    setRemoving((r) => new Set(r).add(it.episode.id));
    setTimeout(async () => {
      await setSeenCascade(it.series.id, it.episode.number, true);
      await load();
      onChanged();
    }, REMOVE_MS);
  }

  const groups = new Map<string, PendingItem[]>();
  for (const it of items) {
    const k = it.series.title;
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
        [...groups.entries()].map(([title, eps]) => {
          const series = eps[0].series;
          return (
            <div key={title} className="series-block">
              <div className="series-head clickable" onClick={() => onOpenSeries(series)}>
                {series.cover_url && <img src={series.cover_url} alt="" />}
                <div>
                  <div className="name">{title}</div>
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
                      <div className="ep-date">{it.episode.released_at}</div>
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
