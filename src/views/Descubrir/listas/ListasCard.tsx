import { useState } from "react";
import {
  deleteSeries,
  linkCatalogSeries,
  promoteDiscarded,
  reclassifySeries,
  setBacklogStatus,
  startWatching,
} from "../../../api";
import { useT } from "../../../i18n";
import { isUnlinkedCatalogRow } from "../../../lib/catalogLink";
import { initials, resolveCoverUrl, useCoverFailure } from "../../../lib/posterFallback";
import type { Series } from "../../../types";
import { OverflowMenu } from "../components";
import type { ListTab, ListasEntry } from "./types";

export interface ListasCardProps {
  entry: ListasEntry;
  activeTab: ListTab;
  isSelected: boolean;
  onToggleSelect: () => void;
  onOpenSeries: (s: Series) => void;
  onChanged: () => void;
}

export function ListasCard({
  entry,
  activeTab,
  isSelected,
  onToggleSelect,
  onOpenSeries,
  onChanged,
}: ListasCardProps) {
  const t = useT();
  const { series, meta } = entry;
  const unlinked = isUnlinkedCatalogRow(series);

  const [starting, setStarting] = useState(false);
  const [linking, setLinking] = useState(false);
  const [noMatch, setNoMatch] = useState(false);

  // Cover fallback resolution
  const coverUrlCandidate = meta?.cover_url ?? series.cover_url;
  const resolvedCover = resolveCoverUrl(coverUrlCandidate);
  const [failed, markFailed] = useCoverFailure(coverUrlCandidate);
  const showFallback = !resolvedCover || failed;

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

  const handleRetryLink = async () => {
    setLinking(true);
    try {
      await linkCatalogSeries(series.id);
    } finally {
      setLinking(false);
      onChanged();
    }
  };

  const year = meta?.start_date
    ? new Date(meta.start_date * 1000).getUTCFullYear()
    : null;

  const episodeCount = meta?.episodes ?? series.site_episode_count;

  return (
    <div className={`listas-card-item${isSelected ? " selected" : ""}`}>
      <div className="listas-card-poster-box">
        {/* Badges top-left: Checkbox & Score */}
        <div className="listas-poster-badge-top-left">
          <button
            type="button"
            className={`listas-select-checkbox-btn${isSelected ? " checked" : ""}`}
            onClick={(e) => {
              e.stopPropagation();
              onToggleSelect();
            }}
            aria-label={t("discover.listas.selectItemAria", { title: series.title })}
            aria-pressed={isSelected}
          >
            {isSelected ? "✓" : ""}
          </button>
          {meta?.score != null && (
            <span className="listas-score-badge" title={t("seriesDetail.info.score")}>
              ★ {meta.score}%
            </span>
          )}
        </div>

        {/* Badges top-right: Format */}
        <div className="listas-poster-badge-top-right">
          {meta?.format && (
            <span className="listas-format-badge" title={t("seriesDetail.info.format")}>
              {meta.format}
            </span>
          )}
        </div>

        {/* Poster Image or Initials */}
        {showFallback ? (
          <div className="listas-card-poster-fallback" aria-hidden="true">
            {initials(series.title)}
          </div>
        ) : (
          <img
            src={resolvedCover!}
            alt={series.title}
            className="listas-card-poster-img"
            loading="lazy"
            onError={markFailed}
          />
        )}

        {/* Hover / Focus-within Quick Action Overlay */}
        <div className="listas-card-quick-actions">
          {activeTab === "want" && (
            <button
              type="button"
              className="btn btn-sm btn-primary listas-quick-btn"
              disabled={starting || linking}
              onClick={handleStartWatching}
            >
              {starting ? t("common.saving") : t("discover.startWatching")}
            </button>
          )}

          {activeTab === "discarded" && (
            <button
              type="button"
              className="btn btn-sm btn-primary listas-quick-btn"
              onClick={async () => {
                await promoteDiscarded(series.id);
                onChanged();
              }}
            >
              {t("discover.moveToWant")}
            </button>
          )}

          {activeTab === "watched" && (
            <button
              type="button"
              className="btn btn-sm btn-ghost listas-quick-btn"
              onClick={async () => {
                await reclassifySeries(series.id, "None");
                onChanged();
              }}
            >
              {t("discover.markUnwatched")}
            </button>
          )}

          <OverflowMenu
            label={t("discover.moreActions")}
            items={[
              ...(activeTab === "want"
                ? [
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
                  ]
                : []),
              ...(activeTab === "discarded"
                ? [
                    {
                      label: t("discover.deleteCompletely"),
                      onClick: async () => {
                        await deleteSeries(series.id);
                        onChanged();
                      },
                    },
                  ]
                : []),
              ...(activeTab === "watched"
                ? [
                    {
                      label: t("discover.moveToWantFromWatched"),
                      onClick: async () => {
                        await reclassifySeries(series.id, "Want");
                        onChanged();
                      },
                    },
                  ]
                : []),
              ...(unlinked
                ? [
                    {
                      label: linking ? t("discover.searchingBtn") : t("discover.searchOnSite"),
                      onClick: handleRetryLink,
                    },
                  ]
                : []),
              {
                label: t("discover.viewDetails"),
                onClick: () => onOpenSeries(series),
              },
            ]}
          />
        </div>
      </div>

      <div className="listas-card-info">
        <button
          type="button"
          className="listas-card-title-link"
          onClick={() => onOpenSeries(series)}
          title={series.title}
        >
          {series.title}
        </button>

        <div className="listas-card-meta">
          {year != null && <span>{year}</span>}
          {year != null && episodeCount != null && <span>·</span>}
          {episodeCount != null && <span>{episodeCount} eps</span>}
          {(meta?.status === "RELEASING" || series.is_airing) && (
            <>
              <span>·</span>
              <span className="text-accent">{t("airing.airingNow") || "En emisión"}</span>
            </>
          )}
        </div>

        {meta?.genres && meta.genres.length > 0 && (
          <div className="listas-card-genres" title={meta.genres.join(", ")}>
            {meta.genres.slice(0, 3).join(", ")}
          </div>
        )}

        {unlinked && (
          <span className="listas-unlinked-tag" title={t("discover.notFoundYetTitle")}>
            {t("discover.listas.unlinkedBadge")}
          </span>
        )}

        {noMatch && (
          <span className="text-danger text-xs">{t("discover.notFoundInSite")}</span>
        )}
      </div>
    </div>
  );
}
