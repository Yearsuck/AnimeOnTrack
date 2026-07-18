import { useCallback, useRef, useState } from "react";
import { linkCatalogSeries } from "../../api";
import type { LinkStatus } from "./types";

/// Serializes `linkCatalogSeries` calls through a single promise chain so
/// rapid swiping never spawns unbounded parallel scrapes — the backend's
/// SCRAPE_PERMITS semaphore would otherwise just queue them behind whatever
/// the deck's own prefetching needs, defeating the point of not blocking the
/// swipe on the link. Keeps the most recent 2 statuses for display.
export function useLinkQueue() {
  const chainRef = useRef<Promise<void>>(Promise.resolve());
  const [statuses, setStatuses] = useState<LinkStatus[]>([]);

  const upsert = useCallback((next: LinkStatus) => {
    setStatuses((prev) => [next, ...prev.filter((s) => s.id !== next.id)].slice(0, 2));
  }, []);

  const enqueue = useCallback(
    (seriesId: number, title: string) => {
      upsert({ id: seriesId, title, state: "searching" });
      chainRef.current = chainRef.current
        .then(() => linkCatalogSeries(seriesId))
        .then((outcome) => {
          if (outcome.type === "Linked") {
            upsert({ id: seriesId, title, state: "linked", episodes: outcome.episodes });
          } else if (outcome.type === "NoMatch") {
            upsert({ id: seriesId, title, state: "nomatch" });
          }
          // AlreadyLinked: nothing to show — a freshly-decided card is never
          // already linked, this only matters for the manual retry button.
        })
        .catch((err) => {
          console.error("linkCatalogSeries failed for", seriesId, err);
          upsert({ id: seriesId, title, state: "nomatch" });
        });
    },
    [upsert]
  );

  return { statuses, enqueue };
}
