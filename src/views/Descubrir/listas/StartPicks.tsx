import { useState } from "react";
import { startWatching } from "../../../api";
import { useT } from "../../../i18n";
import { posterMetaLine } from "../../../components/PosterCard";
import { initials, resolveCoverUrl } from "../../../lib/posterFallback";
import type { Series } from "../../../types";
import type { StartPick } from "./pickStart";

const REASON_KEY = {
  airing: "discover.listas.startReasonAiring",
  short: "discover.listas.startReasonShort",
  top: "discover.listas.startReasonTop",
} as const;

/** Quiero ver's "Para empezar hoy" row: a few picks with a direct start. */
export function StartPicks({
  picks,
  onOpenSeries,
  onChanged,
}: {
  picks: StartPick[];
  onOpenSeries: (s: Series) => void;
  onChanged: () => void;
}) {
  const t = useT();
  if (picks.length === 0) return null;
  return (
    <section className="listas-start" aria-labelledby="listas-start-title">
      <div className="cat-shelf-head">
        <h2 id="listas-start-title" className="cat-shelf-title">
          {t("discover.listas.startTitle")}
        </h2>
        <span className="cat-shelf-hint">{t("discover.listas.startHint")}</span>
      </div>
      <div className="listas-start-grid">
        {picks.map((p) => (
          <StartPickCard key={p.entry.series.id} pick={p} onOpenSeries={onOpenSeries} onChanged={onChanged} />
        ))}
      </div>
    </section>
  );
}

function StartPickCard({
  pick,
  onOpenSeries,
  onChanged,
}: {
  pick: StartPick;
  onOpenSeries: (s: Series) => void;
  onChanged: () => void;
}) {
  const t = useT();
  const { series, meta } = pick.entry;
  const [starting, setStarting] = useState(false);
  const [noMatch, setNoMatch] = useState(false);
  const src = resolveCoverUrl(meta?.cover_url ?? series.cover_url ?? null);
  const line = posterMetaLine(t, {
    episodes: meta?.episodes ?? series.site_episode_count,
    airing: pick.reason === "airing",
    score: meta?.score,
  });

  async function start() {
    setStarting(true);
    setNoMatch(false);
    try {
      const outcome = await startWatching(series.id);
      if (outcome.type === "NoMatch") setNoMatch(true);
      else onChanged();
    } catch (err) {
      console.error("startWatching failed for", series.id, err);
    } finally {
      setStarting(false);
    }
  }

  return (
    <article className="listas-start-card">
      {src ? (
        <img src={src} alt="" />
      ) : (
        <span className="listas-start-ph" aria-hidden="true">
          {initials(series.title)}
        </span>
      )}
      <div className="listas-start-body">
        <h3 className="listas-start-name">{series.title}</h3>
        <p className="listas-start-why">{t(REASON_KEY[pick.reason])}</p>
        {line && <p className="muted text-sm listas-start-meta">{line}</p>}
        {noMatch && <p className="text-sm text-danger">{t("discover.listas.startNoMatch")}</p>}
        <div className="listas-start-actions">
          <button type="button" className="btn btn-sm btn-primary" disabled={starting} onClick={start}>
            {starting ? t("common.saving") : t("discover.startWatching")}
          </button>
          <button type="button" className="btn btn-sm" onClick={() => onOpenSeries(series)}>
            {t("discover.listas.startOpen")}
          </button>
        </div>
      </div>
    </article>
  );
}
