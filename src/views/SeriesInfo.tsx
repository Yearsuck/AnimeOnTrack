import { useEffect, useMemo, useState } from "react";
import { openEpisode } from "../api";
import { useLang, useT, type MessageKey } from "../i18n";
import {
  hasSpoilers,
  initialSynopsisSource,
  parseDescription,
  type DescParagraph,
  type SynopsisSource,
} from "../lib/anilistDescription";
import { initials, resolveCoverUrl } from "../lib/posterFallback";
import type { CatalogTag, RelatedAnime, Series } from "../types";

// The series detail view's "about" panel (synopsis + tags) and its
// "Relacionados" row. Everything here is rendered as text nodes: AniList
// markup goes through parseDescription, never dangerouslySetInnerHTML.

const MIN_TAG_RANK = 60;
const MAX_TAGS = 8;

/** Length (or paragraph count) past which the text is clamped behind "Leer más". */
const CLAMP_CHARS = 420;

export function SeriesAbout({
  siteSynopsis,
  siteName,
  anilistDescription,
  anilistLoading,
  tags,
}: {
  siteSynopsis: string | null;
  siteName: string | null;
  anilistDescription: string | null;
  anilistLoading: boolean;
  tags: CatalogTag[];
}) {
  const t = useT();
  const { lang } = useLang();
  const siteParas = useMemo(() => parseDescription(siteSynopsis), [siteSynopsis]);
  const anilistParas = useMemo(() => parseDescription(anilistDescription), [anilistDescription]);
  const hasSite = siteParas.length > 0;
  const hasAnilist = anilistParas.length > 0;

  // `null` = follow the language default; set once the user picks a source.
  const [picked, setPicked] = useState<SynopsisSource | null>(null);
  const [expanded, setExpanded] = useState(false);
  const [showSpoilers, setShowSpoilers] = useState(false);

  const fallback = initialSynopsisSource(lang, hasSite, hasAnilist);
  const source: SynopsisSource | null =
    picked === "site" && hasSite ? "site" : picked === "anilist" && hasAnilist ? "anilist" : fallback;
  const paras: DescParagraph[] = source === "site" ? siteParas : source === "anilist" ? anilistParas : [];

  useEffect(() => {
    setExpanded(false);
    setShowSpoilers(false);
  }, [source, siteSynopsis, anilistDescription]);

  const length = paras.reduce((n, p) => n + p.reduce((m, s) => m + s.text.length, 0), 0);
  const clampable = length > CLAMP_CHARS || paras.length > 3;
  const spoilers = hasSpoilers(paras);

  const topTags = tags.filter((tag) => tag.rank >= MIN_TAG_RANK).slice(0, MAX_TAGS);

  const siteLabel = siteName
    ? t("seriesDetail.source.site", { site: siteName })
    : t("seriesDetail.source.siteGeneric");

  return (
    <section className="detail-about" aria-labelledby="detail-about-title">
      <div className="detail-about-head">
        <h3 id="detail-about-title" className="section-title">
          {t("seriesDetail.synopsis")}
        </h3>
        {hasSite && hasAnilist && (
          <div className="seg" role="group" aria-label={t("seriesDetail.synopsisSourceAria")}>
            <button
              type="button"
              className={`seg-btn${source === "site" ? " active" : ""}`}
              aria-pressed={source === "site"}
              onClick={() => setPicked("site")}
            >
              {siteLabel}
            </button>
            <button
              type="button"
              className={`seg-btn${source === "anilist" ? " active" : ""}`}
              aria-pressed={source === "anilist"}
              onClick={() => setPicked("anilist")}
            >
              {t("seriesDetail.source.anilist")}
            </button>
          </div>
        )}
      </div>

      {paras.length === 0 ? (
        <p className="muted detail-about-empty">
          {anilistLoading ? t("seriesDetail.loadingSynopsis") : t("seriesDetail.noSynopsis")}
        </p>
      ) : (
        <div
          className={`detail-synopsis${clampable && !expanded ? " clamped" : ""}`}
          lang={source === "site" ? "es" : "en"}
        >
          {paras.map((p, i) => (
            <p key={i}>
              {p.map((seg, j) =>
                seg.spoiler && !showSpoilers ? (
                  <span key={j} className="spoiler-hidden">
                    {t("seriesDetail.spoilerHidden")}
                  </span>
                ) : (
                  <span key={j} className={seg.spoiler ? "spoiler-shown" : undefined}>
                    {seg.text}
                  </span>
                )
              )}
            </p>
          ))}
        </div>
      )}

      {(clampable || spoilers) && paras.length > 0 && (
        <div className="detail-about-actions">
          {clampable && (
            <button type="button" className="btn-text" aria-expanded={expanded} onClick={() => setExpanded((v) => !v)}>
              {expanded ? t("seriesDetail.readLess") : t("seriesDetail.readMore")}
            </button>
          )}
          {spoilers && (
            <button
              type="button"
              className="btn-text"
              aria-pressed={showSpoilers}
              onClick={() => setShowSpoilers((v) => !v)}
            >
              {showSpoilers ? t("seriesDetail.hideSpoilers") : t("seriesDetail.showSpoilers")}
            </button>
          )}
        </div>
      )}

      {topTags.length > 0 && (
        <ul className="detail-tags" aria-label={t("seriesDetail.tagsAria")}>
          {topTags.map((tag) => (
            <li key={tag.name} className="tag tag-quiet">
              {tag.name}
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}

const RELATION_KEYS: Record<string, MessageKey> = {
  PREQUEL: "seriesDetail.relation.PREQUEL",
  SEQUEL: "seriesDetail.relation.SEQUEL",
  PARENT: "seriesDetail.relation.PARENT",
  SIDE_STORY: "seriesDetail.relation.SIDE_STORY",
  SPIN_OFF: "seriesDetail.relation.SPIN_OFF",
  ALTERNATIVE: "seriesDetail.relation.ALTERNATIVE",
  SUMMARY: "seriesDetail.relation.SUMMARY",
  COMPILATION: "seriesDetail.relation.COMPILATION",
  CONTAINS: "seriesDetail.relation.CONTAINS",
  CHARACTER: "seriesDetail.relation.CHARACTER",
};

function RelatedCard({ item, onOpenSeries }: { item: RelatedAnime; onOpenSeries?: (s: Series) => void }) {
  const t = useT();
  const cover = resolveCoverUrl(item.cover_url);
  const [failed, setFailed] = useState<string | null>(null);
  const relation = t(RELATION_KEYS[item.relation_type] ?? "seriesDetail.relation.OTHER");
  const inApp = item.series != null && onOpenSeries != null;
  const meta = [item.format, item.year].filter((v) => v != null && v !== "").join(" ");

  function open() {
    if (item.series && onOpenSeries) {
      onOpenSeries(item.series);
      return;
    }
    openEpisode(item.url).catch((err) => console.error("openEpisode failed for", item.url, err));
  }

  return (
    <li className="related-card">
      <button
        type="button"
        className="related-card-btn"
        onClick={open}
        title={inApp ? item.title : `${item.title}. ${t("seriesDetail.related.opensAniList")}`}
      >
        <span className="related-cover">
          {cover && failed !== cover ? (
            <img src={cover} alt="" loading="lazy" onError={() => setFailed(cover)} />
          ) : (
            <span className="related-cover-fallback" aria-hidden="true">
              {initials(item.title)}
            </span>
          )}
          {(item.followed || item.watched) && (
            <span className={`related-badge${item.watched ? " watched" : ""}`}>
              {item.watched ? t("seriesDetail.related.watched") : t("seriesDetail.related.followed")}
            </span>
          )}
        </span>
        <span className="related-relation">{relation}</span>
        <span className="related-title">{item.title}</span>
        {meta && <span className="related-meta">{meta}</span>}
      </button>
    </li>
  );
}

export function RelatedRow({
  items,
  onOpenSeries,
}: {
  items: RelatedAnime[];
  onOpenSeries?: (s: Series) => void;
}) {
  const t = useT();
  if (items.length === 0) return null;
  return (
    <section className="detail-related" aria-labelledby="detail-related-title">
      <h3 id="detail-related-title" className="section-title">
        {t("seriesDetail.related")}
      </h3>
      <ul className="related-row">
        {items.map((item) => (
          <RelatedCard key={item.anilist_id} item={item} onOpenSeries={onOpenSeries} />
        ))}
      </ul>
    </section>
  );
}
