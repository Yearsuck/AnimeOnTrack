import { useEffect, useState } from "react";
import { listAiring, setFollowed } from "../api";
import type { Series } from "../types";

export function AiringGrid() {
  const [series, setSeries] = useState<Series[]>([]);

  async function load() {
    setSeries(await listAiring());
  }
  useEffect(() => {
    load();
  }, []);

  async function toggle(s: Series) {
    await setFollowed(s.id, !s.followed);
    await load();
  }

  return (
    <div style={{ display: "grid", gridTemplateColumns: "repeat(auto-fill,minmax(160px,1fr))", gap: 12, padding: 16 }}>
      {series.map((s) => (
        <div key={s.id} style={{ border: "1px solid #ccc", borderRadius: 8, padding: 8 }}>
          {s.cover_url && <img src={s.cover_url} style={{ width: "100%", borderRadius: 4 }} />}
          <div style={{ fontSize: 13, margin: "6px 0" }}>{s.title}</div>
          <button onClick={() => toggle(s)}>
            {s.followed ? "Following ✓" : "Follow"}
          </button>
        </div>
      ))}
    </div>
  );
}
