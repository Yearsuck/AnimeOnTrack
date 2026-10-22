import { useState } from "react";
import {
  deleteSeries,
  promoteDiscarded,
  reclassifySeries,
  setBacklogStatus,
  startWatching,
} from "../../../api";
import { useT } from "../../../i18n";
import type { ListTab, ListasEntry } from "./types";

export interface BulkActionBarProps {
  activeTab: ListTab;
  selectedIds: Set<number>;
  visibleEntries: ListasEntry[];
  onSelectAllVisible: () => void;
  onDeselectAll: () => void;
  /** Keep exactly these ids selected (the ones a batch could not complete). */
  onRetainSelection: (ids: number[]) => void;
  onChanged: () => void;
}

export function BulkActionBar({
  activeTab,
  selectedIds,
  visibleEntries,
  onSelectAllVisible,
  onDeselectAll,
  onRetainSelection,
  onChanged,
}: BulkActionBarProps) {
  const t = useT();
  const [processing, setProcessing] = useState(false);
  const [progressMsg, setProgressMsg] = useState<string | null>(null);
  const [failureMsg, setFailureMsg] = useState<string | null>(null);

  // Rendered by ListasView only while selection mode is on, so "Seleccionar
  // visibles" is reachable before anything is picked.
  const count = selectedIds.size;
  const allVisibleSelected =
    visibleEntries.length > 0 &&
    visibleEntries.every((e) => selectedIds.has(e.series.id));

  const runBatch = async (
    action: (id: number) => Promise<unknown>,
    confirmPrompt?: string
  ) => {
    if (confirmPrompt && !window.confirm(confirmPrompt)) {
      return;
    }

    setProcessing(true);
    setFailureMsg(null);
    // Only entries that are still visible; hidden ids are never touched.
    const visible = new Set(visibleEntries.map((e) => e.series.id));
    const ids = Array.from(selectedIds).filter((id) => visible.has(id));
    const failed: number[] = [];
    try {
      for (let i = 0; i < ids.length; i++) {
        setProgressMsg(
          t("discover.listas.bulkActionRunning", { current: i + 1, total: ids.length })
        );
        try {
          const result = await action(ids[i]);
          // start_watching reports "no match" as a value, not an exception.
          if ((result as { type?: string } | null)?.type === "NoMatch") failed.push(ids[i]);
        } catch (e) {
          console.error("Batch action failed for", ids[i], e);
          failed.push(ids[i]);
        }
      }
      if (failed.length > 0) {
        // Keep the failures selected so the user can retry them.
        onRetainSelection(failed);
        setFailureMsg(
          t("discover.listas.bulkPartialFailure", { failed: failed.length, total: ids.length })
        );
      } else {
        onDeselectAll();
      }
      onChanged();
    } finally {
      setProcessing(false);
      setProgressMsg(null);
    }
  };

  return (
    // Same pinned bar as Explorar's and Mi biblioteca's selection (`.batch-bar`).
    <div className="batch-bar" role="toolbar" aria-label={t("discover.listas.bulkSelected", { count })}>
      <span className="muted">{t("discover.listas.bulkSelected", { count })}</span>
      <button
        type="button"
        className="btn btn-ghost"
        onClick={allVisibleSelected ? onDeselectAll : onSelectAllVisible}
        disabled={processing}
      >
        {allVisibleSelected
          ? t("discover.listas.deselectAll")
          : t("discover.listas.selectAllVisible")}
      </button>

      {progressMsg && <span className="muted text-sm">{progressMsg}</span>}
      {failureMsg && !progressMsg && (
        <span className="text-sm text-danger" role="alert">
          {failureMsg}
        </span>
      )}

      <div className="spacer" />
      <div className="listas-bulk-buttons">
        {activeTab === "want" && (
          <>
            <button
              type="button"
              className="btn btn-primary"
              disabled={processing || count === 0}
              onClick={() => runBatch((id) => startWatching(id))}
            >
              {t("discover.listas.bulkStartWatching")}
            </button>
            <button
              type="button"
              className="btn"
              disabled={processing || count === 0}
              onClick={() => runBatch((id) => setBacklogStatus(id, "discarded"))}
            >
              {t("discover.listas.bulkMoveToDiscarded")}
            </button>
            <button
              type="button"
              className="btn"
              disabled={processing || count === 0}
              onClick={() => runBatch((id) => reclassifySeries(id, "WatchedExternally"))}
            >
              {t("discover.listas.bulkMoveToWatched")}
            </button>
            <button
              type="button"
              className="btn btn-ghost"
              disabled={processing || count === 0}
              onClick={() =>
                runBatch(
                  (id) => reclassifySeries(id, "None"),
                  t("discover.listas.bulkRemoveConfirm", { count })
                )
              }
            >
              {t("discover.listas.bulkRemove")}
            </button>
          </>
        )}

        {activeTab === "discarded" && (
          <>
            <button
              type="button"
              className="btn btn-primary"
              disabled={processing || count === 0}
              onClick={() => runBatch((id) => promoteDiscarded(id))}
            >
              {t("discover.listas.bulkMoveToWant")}
            </button>
            <button
              type="button"
              className="btn btn-danger"
              disabled={processing || count === 0}
              onClick={() =>
                runBatch(
                  (id) => deleteSeries(id),
                  t("discover.listas.bulkDeleteConfirm", { count })
                )
              }
            >
              {t("discover.listas.bulkDelete")}
            </button>
          </>
        )}

        {activeTab === "watched" && (
          <>
            <button
              type="button"
              className="btn btn-primary"
              disabled={processing || count === 0}
              onClick={() => runBatch((id) => reclassifySeries(id, "Want"))}
            >
              {t("discover.listas.bulkMoveToWant")}
            </button>
            <button
              type="button"
              className="btn"
              disabled={processing || count === 0}
              onClick={() => runBatch((id) => reclassifySeries(id, "None"))}
            >
              {t("discover.listas.bulkMarkUnwatched")}
            </button>
          </>
        )}
      </div>
    </div>
  );
}
