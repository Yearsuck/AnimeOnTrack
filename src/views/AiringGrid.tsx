import { useEffect, useMemo, useState } from "react";
import { listAiring, setFollowed } from "../api";
import type { Series } from "../types";

// Human label for the next-episode countdown the backend sorting is based
// on — makes the newest-first ordering legible instead of mysterious.
// Computed once per render (no ticking timer): "en 2 h" / "en 3 d" for a
// future release, "hace 5 h" for one that already aired but whose card
// hasn't rolled over yet. Null when the series carries no countdown.
function countdownLabel(nextEpisodeAt: number | null): string | null {
  if (nextEpisodeAt == null) return null;
  const diffMs = nextEpisodeAt * 1000 - Date.now();
  const absHours = Math.abs(diffMs) / 3_600_000;
  const span =
    absHours >= 48
      ? `${Math.round(absHours / 24)} d`
      : absHours >= 1
        ? `${Math.round(absHours)} h`
        : `${Math.max(1, Math.round(Math.abs(diffMs) / 60_000))} min`;
  return diffMs >= 0 ? `en ${span}` : `hace ${span}`;
}

export function AiringGrid({
  onOpenSeries,
  refreshSignal,
}: {
  onOpenSeries: (s: Series) => void;
  refreshSignal?: number;
}) {
  const [series, setSeries] = useState<Series[]>([]);
  const [query, setQuery] = useState("");
  const [onlyFollowed, setOnlyFollowed] = useState(false);

  async function load() {
    setSeries(await listAiring());
  }
  useEffect(() => {
    load();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [refreshSignal]);

  async function toggle(e: React.MouseEvent, s: Series) {
    e.stopPropagation();
    await setFollowed(s.id, !s.followed);
    await load();
  }

  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase();
    return series.filter(
      (s) => (!onlyFollowed || s.followed) && (!q || s.title.toLowerCase().includes(q))
    );
  }, [series, query, onlyFollowed]);

  const followedCount = series.filter((s) => s.followed).length;

  return (
    <div className="page">
      <div className="page-head">
        <h2 className="page-title">En emisión</h2>
        <div className="search">
          <span className="icon">⌕</span>
          <input
            className="input"
            placeholder="Buscar por nombre…"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
          />
        </div>
        <button
          className={`btn ${onlyFollowed ? "btn-success" : "btn-ghost"}`}
          onClick={() => setOnlyFollowed((v) => !v)}
        >
          ★ Siguiendo ({followedCount})
        </button>
        <div className="spacer" />
        <span className="muted">{filtered.length} series</span>
      </div>

      {filtered.length === 0 ? (
        <div className="empty">No hay resultados.</div>
      ) : (
        <div className="grid">
          {filtered.map((s) => (
            <div key={s.id} className="card" onClick={() => onOpenSeries(s)}>
              <div className="poster">
                {s.followed && <span className="chip">SIGUIENDO</span>}
                {countdownLabel(s.next_episode_at) && (
                  <span className="chip chip-countdown">{countdownLabel(s.next_episode_at)}</span>
                )}
                {s.cover_url ? (
                  <img src={s.cover_url} alt={s.title} loading="lazy" />
                ) : null}
              </div>
              <div className="card-body">
                <div className="card-title">{s.title}</div>
                <button
                  className={`btn follow-btn ${s.followed ? "on" : ""}`}
                  onClick={(e) => toggle(e, s)}
                >
                  {s.followed ? "✓ Siguiendo" : "+ Seguir"}
                </button>
              </div>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
