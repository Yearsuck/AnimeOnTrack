import { useCallback, useEffect, useMemo, useState } from "react";
import { listBacklog, listWatchedExternally } from "../../api";
import { useT } from "../../i18n";
import type { Series } from "../../types";
import type { ListTab } from "./types";
import { norm } from "./helpers";
import { DiscardedRow, WantRow, WatchedRow } from "./rows";

export function ListasView({ onOpenSeries }: { onOpenSeries: (s: Series) => void }) {
  const t = useT();
  const [want, setWant] = useState<Series[]>([]);
  const [discarded, setDiscarded] = useState<Series[]>([]);
  const [watched, setWatched] = useState<Series[]>([]);
  const [tab, setTab] = useState<ListTab>("want");
  const [query, setQuery] = useState("");

  const load = useCallback(async () => {
    const [w, d, wa] = await Promise.all([
      listBacklog("want"),
      listBacklog("discarded"),
      listWatchedExternally(),
    ]);
    setWant(w);
    setDiscarded(d);
    setWatched(wa);
  }, []);

  useEffect(() => {
    load();
  }, [load]);

  const trimmedQuery = query.trim();
  const fWant = useMemo(
    () => (trimmedQuery === "" ? want : want.filter((s) => norm(s.title).includes(norm(trimmedQuery)))),
    [want, trimmedQuery]
  );
  const fDiscarded = useMemo(
    () =>
      trimmedQuery === ""
        ? discarded
        : discarded.filter((s) => norm(s.title).includes(norm(trimmedQuery))),
    [discarded, trimmedQuery]
  );
  const fWatched = useMemo(
    () =>
      trimmedQuery === ""
        ? watched
        : watched.filter((s) => norm(s.title).includes(norm(trimmedQuery))),
    [watched, trimmedQuery]
  );

  const tabs: { key: ListTab; label: string; count: number }[] = [
    { key: "want", label: t("discover.wantHeading"), count: fWant.length },
    { key: "discarded", label: t("discover.discardedHeading"), count: fDiscarded.length },
    { key: "watched", label: t("discover.watchedHeading"), count: fWatched.length },
  ];

  return (
    <>
      <div className="listas-toolbar">
        <div className="search">
          <span className="icon" aria-hidden="true">
            ⌕
          </span>
          <label htmlFor="listas-search" className="sr-only">
            {t("discover.searchAriaLabel")}
          </label>
          <input
            id="listas-search"
            className="input"
            placeholder={t("common.searchPlaceholder")}
            value={query}
            onChange={(e) => setQuery(e.target.value)}
          />
        </div>
      </div>

      <div className="seg listas-seg" role="tablist">
        {tabs.map((tb) => (
          <button
            key={tb.key}
            type="button"
            role="tab"
            aria-selected={tab === tb.key}
            className={`seg-btn${tab === tb.key ? " active" : ""}`}
            onClick={() => setTab(tb.key)}
          >
            {tb.label}
            <span className="listas-seg-count">{tb.count}</span>
          </button>
        ))}
      </div>

      {tab === "want" &&
        (want.length === 0 ? (
          <div className="empty">{t("discover.wantEmpty")}</div>
        ) : fWant.length === 0 ? (
          <div className="empty">{t("discover.searchNoResults")}</div>
        ) : (
          <div className="listas-grid">
            {fWant.map((s) => (
              <WantRow key={s.id} series={s} onChanged={load} onOpenSeries={onOpenSeries} />
            ))}
          </div>
        ))}

      {tab === "discarded" &&
        (discarded.length === 0 ? (
          <div className="empty">{t("discover.discardedEmpty")}</div>
        ) : fDiscarded.length === 0 ? (
          <div className="empty">{t("discover.searchNoResults")}</div>
        ) : (
          <div className="listas-grid">
            {fDiscarded.map((s) => (
              <DiscardedRow key={s.id} series={s} onChanged={load} />
            ))}
          </div>
        ))}

      {tab === "watched" &&
        (watched.length === 0 ? (
          <div className="empty">{t("discover.watchedEmpty")}</div>
        ) : fWatched.length === 0 ? (
          <div className="empty">{t("discover.searchNoResults")}</div>
        ) : (
          <div className="listas-grid">
            {fWatched.map((s) => (
              <WatchedRow key={s.id} series={s} onChanged={load} />
            ))}
          </div>
        ))}
    </>
  );
}
