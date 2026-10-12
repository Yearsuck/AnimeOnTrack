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
  genres: initialGenres,
  onChanged,
  onOpenSeries,
}: {
  series: Series;
  genres?: string[];
  onChanged: () => void;
  onOpenSeries: (s: Series) => void;
}) {
  const t = useT();
  const [genres, setGenres] = useState<string[]>(initialGenres ?? []);
  const [linking, setLinking] = useState(false);
  const [starting, setStarting] = useState(false);
  const [noMatch, setNoMatch] = useState(false);
  const unlinked = isUnlinkedCatalogRow(series);

  useEffect(() => {
    if (initialGenres != null) {
      setGenres(initialGenres);
    } else {
      getSeriesGenres(series.id).then(setGenres);
    }
  }, [series.id, initialGenres]);

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
      <PosterThumb series={series} onClick={() => onOpenSeries(series)} />
      <div className="backlog-main">
        <div className="listas-card-head">
          <span
            className="listas-title clickable"
            onClick={() => onOpenSeries(series)}
            title={t("discover.viewDetailsTitle")}
          >
            {series.title}
          </span>
          <span className="listas-chip listas-chip-want">{t("discover.chipWant")}</span>
          {series.is_airing && (
            <span className="listas-chip listas-chip-airing">
              {t("airing.airingNow") || "En emisión"}
            </span>
          )}
          {series.site_episode_count != null && (
            <span className="listas-chip listas-chip-eps">{series.site_episode_count} eps</span>
          )}
        </div>
        {genres.length > 0 && <div className="backlog-genres">{genres.join(", ")}</div>}
        {unlinked && (
          <div className="muted text-xs" title={t("discover.notFoundYetTitle")}>
            {t("discover.unlinked")}
          </div>
        )}
        {noMatch && (
          <div className="muted text-xs text-danger">
            {t("discover.notFoundInSite")}
          </div>
        )}
      </div>
      <div className="listas-actions">
        <button
          className="btn btn-primary"
          onClick={handleStartWatching}
          disabled={starting || linking}
        >
          {starting ? t("common.saving") : t("discover.startWatching")}
        </button>
        <button
          className="btn"
          onClick={async () => {
            await setBacklogStatus(series.id, "discarded");
            onChanged();
          }}
        >
          {t("discover.discard")}
        </button>
        <OverflowMenu
          label={t("discover.moreActions")}
          items={[
            {
              label: t("discover.moveToWant"),
              onClick: async () => {
                await setBacklogStatus(series.id, "want");
                onChanged();
              },
            },
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

export function WatchedRow({
  series,
  genres = [],
  onChanged,
  onOpenSeries,
}: {
  series: Series;
  genres?: string[];
  onChanged: () => void;
  onOpenSeries?: (s: Series) => void;
}) {
  const t = useT();
  return (
    <div className="listas-card">
      <PosterThumb series={series} onClick={onOpenSeries ? () => onOpenSeries(series) : undefined} />
      <div className="backlog-main">
        <div className="listas-card-head">
          <span
            className={`listas-title ${onOpenSeries ? "clickable" : ""}`}
            onClick={onOpenSeries ? () => onOpenSeries(series) : undefined}
          >
            {series.title}
          </span>
          <span className="listas-chip listas-chip-watched">{t("discover.chipWatched")}</span>
          {series.is_airing && (
            <span className="listas-chip listas-chip-airing">
              {t("airing.airingNow") || "En emisión"}
            </span>
          )}
          {series.site_episode_count != null && (
            <span className="listas-chip listas-chip-eps">{series.site_episode_count} eps</span>
          )}
        </div>
        {genres.length > 0 && <div className="backlog-genres">{genres.join(", ")}</div>}
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
            ...(onOpenSeries ? [{ label: t("discover.viewDetails"), onClick: () => onOpenSeries(series) }] : []),
          ]}
        />
      </div>
    </div>
  );
}

export function DiscardedRow({
  series,
  genres = [],
  onChanged,
  onOpenSeries,
}: {
  series: Series;
  genres?: string[];
  onChanged: () => void;
  onOpenSeries?: (s: Series) => void;
}) {
  const t = useT();
  return (
    <div className="listas-card">
      <PosterThumb series={series} onClick={onOpenSeries ? () => onOpenSeries(series) : undefined} />
      <div className="backlog-main">
        <div className="listas-card-head">
          <span
            className={`listas-title ${onOpenSeries ? "clickable" : ""}`}
            onClick={onOpenSeries ? () => onOpenSeries(series) : undefined}
          >
            {series.title}
          </span>
          <span className="listas-chip listas-chip-discarded">{t("discover.chipDiscarded")}</span>
          {series.is_airing && (
            <span className="listas-chip listas-chip-airing">
              {t("airing.airingNow") || "En emisión"}
            </span>
          )}
          {series.site_episode_count != null && (
            <span className="listas-chip listas-chip-eps">{series.site_episode_count} eps</span>
          )}
        </div>
        {genres.length > 0 && <div className="backlog-genres">{genres.join(", ")}</div>}
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
            ...(onOpenSeries ? [{ label: t("discover.viewDetails"), onClick: () => onOpenSeries(series) }] : []),
          ]}
        />
      </div>
    </div>
  );
}
