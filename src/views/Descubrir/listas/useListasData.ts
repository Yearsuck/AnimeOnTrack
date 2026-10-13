import { useCallback, useEffect, useRef, useState } from "react";
import { getListEntryMeta, listBacklog, listWatchedExternally } from "../../../api";
import type { ListEntryMeta, Series } from "../../../types";
import type { ListasEntry } from "./types";

export interface UseListasDataReturn {
  want: Series[];
  discarded: Series[];
  watched: Series[];
  metaMap: Map<number, ListEntryMeta>;
  loading: boolean;
  error: string | null;
  reload: () => Promise<void>;
  entriesFor: (list: Series[]) => ListasEntry[];
}

export function useListasData(): UseListasDataReturn {
  const [want, setWant] = useState<Series[]>([]);
  const [discarded, setDiscarded] = useState<Series[]>([]);
  const [watched, setWatched] = useState<Series[]>([]);
  const [metaMap, setMetaMap] = useState<Map<number, ListEntryMeta>>(new Map());
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const loadedOnceRef = useRef(false);

  // Only the first load shows the skeleton: later reloads (after an action)
  // refresh in place so the list keeps its scroll position and card state.
  const reload = useCallback(async () => {
    if (!loadedOnceRef.current) setLoading(true);
    setError(null);
    try {
      const [w, d, wa] = await Promise.all([
        listBacklog("want"),
        listBacklog("discarded"),
        listWatchedExternally(),
      ]);

      setWant(w);
      setDiscarded(d);
      setWatched(wa);

      const allIds = Array.from(new Set([...w, ...d, ...wa].map((s) => s.id)));
      if (allIds.length > 0) {
        try {
          const metaList = await getListEntryMeta(allIds);
          const map = new Map<number, ListEntryMeta>();
          for (const m of metaList) {
            map.set(m.series_id, m);
          }
          setMetaMap(map);
        } catch (metaErr) {
          console.error("Failed to fetch ListEntryMeta", metaErr);
          // Keep empty meta map instead of failing the whole view
          setMetaMap(new Map());
        }
      } else {
        setMetaMap(new Map());
      }
    } catch (err) {
      console.error("Failed to load Discover lists", err);
      setError(String(err));
    } finally {
      loadedOnceRef.current = true;
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    reload();
  }, [reload]);

  const entriesFor = useCallback(
    (list: Series[]): ListasEntry[] => {
      return list.map((series) => ({
        series,
        meta: metaMap.get(series.id),
      }));
    },
    [metaMap]
  );

  return {
    want,
    discarded,
    watched,
    metaMap,
    loading,
    error,
    reload,
    entriesFor,
  };
}
