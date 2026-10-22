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
import { PosterCard, posterMetaLine } from "../../../components/PosterCard";
import type { Series } from "../../../types";
import { OverflowMenu } from "../components";
import type { ListTab, ListasEntry } from "./types";

export interface ListasCardProps {
  entry: ListasEntry;
  activeTab: ListTab;
  selectMode: boolean;
  isSelected: boolean;
  onToggleSelect: () => void;
  onOpenSeries: (s: Series) => void;
  onChanged: () => void;
}

// Same poster card as Explorar (components/PosterCard): format chip top-left,
// overflow menu top-right, the tab's main action on hover at the poster foot.
// Outside selection mode the card opens the series; inside it, it toggles.
export function ListasCard({
  entry,
  activeTab,
  selectMode,
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

  const airing = meta?.status === "RELEASING" || series.is_airing;
  const metaLine = posterMetaLine(t, {
    episodes: meta?.episodes ?? series.site_episode_count,
    airing,
    score: meta?.score,
  });

  const action =
    activeTab === "want" ? (
      <button
        type="button"
        className="btn btn-sm btn-primary"
        disabled={starting || linking}
        onClick={handleStartWatching}
      >
        {starting ? t("common.saving") : t("discover.startWatching")}
      </button>
    ) : activeTab === "discarded" ? (
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
    ) : (
      <button
        type="button"
        className="btn btn-sm"
        onClick={async () => {
          await reclassifySeries(series.id, "None");
          onChanged();
        }}
      >
        {t("discover.markUnwatched")}
      </button>
    );

  const menu = selectMode ? null : (
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
  );

  return (
    <PosterCard
      title={series.title}
      coverUrl={meta?.cover_url ?? series.cover_url}
      format={meta?.format}
      genres={meta?.genres ?? []}
      meta={metaLine}
      selectable={selectMode}
      selected={selectMode && isSelected}
      onActivate={selectMode ? onToggleSelect : () => onOpenSeries(series)}
      corner={menu}
      action={action}
      footer={
        noMatch ? <div className="text-danger text-xs">{t("discover.notFoundInSite")}</div> : null
      }
    />
  );
}
