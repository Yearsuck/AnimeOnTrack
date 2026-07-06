import { useEffect, useState } from "react";
import { listPending, openEpisode } from "../api";
import type { PendingItem } from "../types";

export function Pending() {
  const [items, setItems] = useState<PendingItem[]>([]);

  async function load() {
    setItems(await listPending());
  }
  useEffect(() => {
    load();
  }, []);

  async function watch(it: PendingItem) {
    await openEpisode(it.episode.id, it.episode.url);
    await load();
  }

  // group by series title
  const groups = new Map<string, PendingItem[]>();
  for (const it of items) {
    const k = it.series.title;
    (groups.get(k) ?? groups.set(k, []).get(k)!).push(it);
  }

  return (
    <div style={{ padding: 16 }}>
      <h2>Pending ({items.length})</h2>
      {[...groups.entries()].map(([title, eps]) => (
        <div key={title} style={{ marginBottom: 16 }}>
          <h3 style={{ margin: "8px 0" }}>
            {title} <span style={{ color: "#888" }}>({eps.length})</span>
          </h3>
          {eps.map((it) => (
            <div
              key={it.episode.id}
              onClick={() => watch(it)}
              style={{ cursor: "pointer", padding: "6px 8px", borderBottom: "1px solid #eee" }}
            >
              {it.episode.number} {it.episode.title ?? ""}
            </div>
          ))}
        </div>
      ))}
      {items.length === 0 && <p>No pending episodes. Hit refresh.</p>}
    </div>
  );
}
