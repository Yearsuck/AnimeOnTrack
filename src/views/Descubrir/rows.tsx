import { useEffect, useState } from "react";
import {
  deleteSeries,
  getSeriesGenres,
  linkCatalogSeries,
  promoteDiscarded,
  reclassifySeries,
  setBacklogStatus,
  startWatching,
} from "../../api";
import { useT } from "../../i18n";
import { isUnlinkedCatalogRow } from "../../lib/catalogLink";
import type { Series } from "../../types";
import { OverflowMenu, PosterThumb } from "./components";

export function WantRow({
  series,
  onChanged,
  onOpenSeries,
}: {
  series: Series;
  onChanged: () => void;
  onOpenSeries: (s: Series) => void;
}) {
  const t = useT();
  const [genres, setGenres] = useState<string[]>([]);
  const [linking, setLinking] = useState(false);
  const [starting, setStarting] = useState(false);
  const [noMatch, setNoMatch] = useState(false);
  const unlinked = isUnlinkedCatalogRow(series);
  useEffect(() => {
    getSeriesGenres(series.id).then(setGenres);
  }, [series.id]);

  const retryLink = async () => {
    setLinking(true);
    try {
      await linkCatalogSeries(series.id);
    } finally {
      setLinking(false);
      onChanged();
    }
  };

  // start_watching links an unlinked catalog row (searches the site) before
  // marking it followed -- the button-press trigger from the design spec,
  // so unlike the swipe-to-Seen path it awaits and shows a spinner. A
  // NoMatch keeps the row in "want" (backend never sets followed) and
  // surfaces the same "not found" message the retry button would show.
  const handleStartWatching = async () => {
    setStarting(true);
    setNoMatch(false);
    try {
      const outcome = await startWatching(series.id);
      if (outcome.type === "NoMatch") {
        setNoMatch(true);
      } else {
        onChanged();
      }
    } finally {
      setStarting(false);
    }
  };

  return (
    <div className="listas-card">
      <PosterThumb series={series} />
      <div className="backlog-main">
        <div className="listas-card-head">
          <span
            className="listas-title"
            onClick={() => onOpenSeries(series)}
            style={{ cursor: "pointer" }}
            title={t("discover.viewDetailsTitle")}
          >
            {series.title}
          </span>
          <span className="listas-chip listas-chip-want">{t("discover.chipWant")}</span>
        </div>
        {genres.length > 0 && <div className="backlog-genres">{genres.join(", ")}</div>}
        {unlinked && (
          <div className="muted" style={{ fontSize: 11 }} title={t("discover.notFoundYetTitle")}>
            {t("discover.unlinked")}
          </div>
        )}
        {noMatch && (
          <div className="muted" style={{ fontSize: 11, color: "var(--danger)" }}>
            {t("discover.notFoundInSite")}
          </div>
        )}
      </div>
      <div className="listas-actions">
        <button className="btn btn-primary" onClick={handleStartWatching} disabled={starting}>
          {starting ? t("discover.searchingBtn") : t("discover.startWatching")}
        </button>
        <OverflowMenu
          label={t("discover.moreActions")}
          items={[
            {
              label: t("discover.discard"),
              onClick: async () => {
                await setBacklogStatus(series.id, "discarded");
                onChanged();
              },
            },
            {
              label: t("discover.removeFromList"),
              onClick: async () => {
                await reclassifySeries(series.id, "None");
                onChanged();
              },
            },
            ...(unlinked
              ? [
                  {
                    label: linking ? t("discover.searchingBtn") : t("discover.searchOnSite"),
                    onClick: retryLink,
                  },
                ]
              : []),
            { label: t("discover.viewDetails"), onClick: () => onOpenSeries(series) },
          ]}
        />
      </div>
    </div>
  );
}

export function WatchedRow({ series, onChanged }: { series: Series; onChanged: () => void }) {
  const t = useT();
  return (
    <div className="listas-card">
      <PosterThumb series={series} />
      <div className="backlog-main">
        <div className="listas-card-head">
          <span className="listas-title">{series.title}</span>
          <span className="listas-chip listas-chip-watched">{t("discover.chipWatched")}</span>
        </div>
      </div>
      <div className="listas-actions">
        <button
          className="btn"
          onClick={async () => {
            await reclassifySeries(series.id, "None");
            onChanged();
          }}
        >
          {t("discover.markUnwatched")}
        </button>
        <OverflowMenu
          label={t("discover.moreActions")}
          items={[
            {
              label: t("discover.moveToWantFromWatched"),
              onClick: async () => {
                await reclassifySeries(series.id, "Want");
                onChanged();
              },
            },
          ]}
        />
      </div>
    </div>
  );
}

export function DiscardedRow({ series, onChanged }: { series: Series; onChanged: () => void }) {
  const t = useT();
  return (
    <div className="listas-card">
      <PosterThumb series={series} />
      <div className="backlog-main">
        <div className="listas-card-head">
          <span className="listas-title">{series.title}</span>
          <span className="listas-chip listas-chip-discarded">{t("discover.chipDiscarded")}</span>
        </div>
      </div>
      <div className="listas-actions">
        <button
          className="btn btn-primary"
          onClick={async () => {
            await promoteDiscarded(series.id);
            onChanged();
          }}
        >
          {t("discover.moveToWant")}
        </button>
        <OverflowMenu
          label={t("discover.moreActions")}
          items={[
            {
              label: t("discover.deleteCompletely"),
              onClick: async () => {
                await deleteSeries(series.id);
                onChanged();
              },
            },
          ]}
        />
      </div>
    </div>
  );
}
