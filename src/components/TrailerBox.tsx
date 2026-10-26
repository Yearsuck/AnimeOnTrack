import { useId, useState } from "react";
import { openEpisode } from "../api";
import { useT } from "../i18n";
import type { Trailer } from "../types";
import "../styles/trailer.css";

// `site` and `id` are validated by the backend ([A-Za-z0-9_-], <= 64 chars)
// and the CSP frame-src only allows these two hosts.
function trailerEmbedUrl(tr: Trailer): string {
  return tr.site === "dailymotion"
    ? `https://www.dailymotion.com/embed/video/${tr.id}?autoplay=1`
    : `https://www.youtube-nocookie.com/embed/${tr.id}?autoplay=1&rel=0`;
}

function trailerWatchUrl(tr: Trailer): string {
  return tr.site === "dailymotion"
    ? `https://www.dailymotion.com/video/${tr.id}`
    : `https://www.youtube.com/watch?v=${tr.id}`;
}

export function TrailerBox({ trailer, title }: { trailer: Trailer; title: string }) {
  const t = useT();
  const titleId = useId();
  const [playing, setPlaying] = useState(false);
  const [thumbFailed, setThumbFailed] = useState(false);
  const youtube = trailer.site === "youtube";
  const watchUrl = trailerWatchUrl(trailer);

  return (
    <section className="sheet-trailer" aria-labelledby={titleId}>
      <div className="sheet-trailer-head">
        <h3 id={titleId} className="section-title">
          {t("animeSheet.trailer")}
        </h3>
        <button
          type="button"
          className="btn-text"
          onClick={() => openEpisode(watchUrl).catch((err) => console.error("openEpisode failed for", watchUrl, err))}
        >
          {t(youtube ? "animeSheet.watchYoutube" : "animeSheet.watchDailymotion")}
        </button>
      </div>
      <div className="sheet-trailer-frame">
        {playing ? (
          <iframe
            src={trailerEmbedUrl(trailer)}
            title={t("animeSheet.trailerFrame", { title })}
            allow="autoplay; encrypted-media; picture-in-picture; fullscreen"
            allowFullScreen
            referrerPolicy="strict-origin-when-cross-origin"
          />
        ) : (
          <button type="button" className="sheet-trailer-play" onClick={() => setPlaying(true)}>
            {youtube && !thumbFailed && (
              <img
                src={`https://i.ytimg.com/vi/${trailer.id}/hqdefault.jpg`}
                alt=""
                loading="lazy"
                onError={() => setThumbFailed(true)}
              />
            )}
            <span className="sheet-trailer-icon" aria-hidden="true">
              ▶
            </span>
            <span className="sr-only">{t("animeSheet.playTrailer")}</span>
          </button>
        )}
      </div>
    </section>
  );
}
