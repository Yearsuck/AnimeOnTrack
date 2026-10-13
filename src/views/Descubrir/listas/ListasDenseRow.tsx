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

export interface ListasDenseRowProps {
  entry: ListasEntry;
  activeTab: ListTab;
  isSelected: boolean;
  onToggleSelect: () => void;
  onOpenSeries: (s: Series) => void;
  onChanged: () => void;
}

export function ListasDenseRow({
  entry,
  activeTab,
  isSelected,
  onToggleSelect,
  onOpenSeries,
  onChanged,
}: ListasDenseRowProps) {
  const t = useT();
  const { series, meta } = entry;
  const unlinked = isUnlinkedCatalogRow(series);

  const [starting, setStarting] = useState(false);
  const [linking, setLinking] = useState(false);
  const [noMatch, setNoMatch] = useState(false);

  // Cover fallback
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
    <div className={`listas-dense-row-item${isSelected ? " selected" : ""}`}>
      {/* Selection checkbox */}
      <button
        type="button"
        className={`listas-select-checkbox-btn${isSelected ? " checked" : ""}`}
        onClick={onToggleSelect}
        aria-label={t("discover.listas.selectItemAria", { title: series.title })}
        aria-pressed={isSelected}
      >
        {isSelected ? "✓" : ""}
      </button>

      {/* Thumbnail */}
      <div
        className="listas-dense-thumb"
        onClick={() => onOpenSeries(series)}
        role="button"
        tabIndex={0}
        onKeyDown={(e) => {
          if (e.key === "Enter" || e.key === " ") {
            e.preventDefault();
            onOpenSeries(series);
          }
        }}
      >
        {showFallback ? (
          <div className="listas-dense-thumb-fallback" aria-hidden="true">
            {initials(series.title)}
          </div>
        ) : (
          <img
            src={resolvedCover!}
            alt={series.title}
            loading="lazy"
            onError={markFailed}
          />
        )}
      </div>

      {/* Main details */}
      <div className="listas-dense-main">
        <div className="listas-dense-title-row">
          <button
            type="button"
            className="listas-dense-title"
            onClick={() => onOpenSeries(series)}
            title={series.title}
          >
            {series.title}
          </button>

          {meta?.score != null && (
            <span className="listas-score-badge">★ {meta.score}%</span>
          )}

          {meta?.format && (
            <span className="listas-format-badge">{meta.format}</span>
          )}

          {(meta?.status === "RELEASING" || series.is_airing) && (
            <span className="listas-chip listas-chip-airing">
              {t("airing.airingNow") || "En emisión"}
            </span>
          )}
        </div>

        <div className="listas-dense-details">
          {year != null && <span>{year}</span>}
          {episodeCount != null && <span>{episodeCount} eps</span>}
          {meta?.genres && meta.genres.length > 0 && (
            <span className="muted-2">{meta.genres.slice(0, 4).join(", ")}</span>
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

      {/* Row action buttons */}
      <div className="listas-dense-actions">
        {activeTab === "want" && (
          <>
            <button
              type="button"
              className="btn btn-sm btn-primary"
              disabled={starting || linking}
              onClick={handleStartWatching}
            >
              {starting ? t("common.saving") : t("discover.startWatching")}
            </button>
            <button
              type="button"
              className="btn btn-sm"
              onClick={async () => {
                await setBacklogStatus(series.id, "discarded");
                onChanged();
              }}
            >
              {t("discover.discard")}
            </button>
          </>
        )}

        {activeTab === "discarded" && (
          <button
            type="button"
            className="btn btn-sm btn-primary"
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
            className="btn btn-sm btn-ghost"
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
                    label: t("discover.moveToWant"),
                    onClick: async () => {
                      await setBacklogStatus(series.id, "want");
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
  );
}
