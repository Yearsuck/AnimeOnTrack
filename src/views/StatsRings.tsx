import { useEffect, useState } from "react";
import { useT } from "../i18n";
import { categoryColor } from "../lib/categoryColor";
import { useStatsShape, type Shape } from "../lib/statsShape";

export type Datum = { name: string; count: number };

function toData(data: { count: number }[], nameKey: "genre" | "kind"): Datum[] {
  return data.map((d) => ({
    name: (d as unknown as Record<string, string>)[nameKey],
    count: d.count,
  }));
}

// ---- Horizontal bar chart -------------------------------------------------
// One row per category: label | proportional bar | value column. Bars are
// anchored to a common baseline and share one max per block, so lengths are
// directly comparable; the value lives in its own right-aligned column so it
// never sits on top of the bar. Fills grow in from 0 (CSS transition, which the
// global prefers-reduced-motion rule already neutralizes).
export function BarChart({ data }: { data: Datum[] }) {
  const [grown, setGrown] = useState(false);
  useEffect(() => {
    const id = requestAnimationFrame(() => setGrown(true));
    return () => cancelAnimationFrame(id);
  }, []);
  const max = Math.max(1, ...data.map((d) => d.count));
  return (
    <div className="barchart">
      {data.map((d) => {
        const pct = (d.count / max) * 100;
        return (
          <div className="bar-row" key={d.name}>
            <div className="bar-label" title={d.name}>
              {d.name}
            </div>
            <div
              className="bar-track"
              role="progressbar"
              aria-valuenow={d.count}
              aria-valuemin={0}
              aria-valuemax={max}
              aria-label={d.name}
            >
              <div
                className="bar-fill"
                style={{ width: grown ? `${pct}%` : 0, background: categoryColor(d.name) }}
              />
            </div>
            <div className="bar-val">{d.count}</div>
          </div>
        );
      })}
    </div>
  );
}

// ---- Donut ring grid ------------------------------------------------------
const RING = 88;
const RING_STROKE = 9;
const RING_R = (RING - RING_STROKE) / 2;
const RING_C = 2 * Math.PI * RING_R;

export function RingGrid({ data }: { data: Datum[] }) {
  const [grown, setGrown] = useState(false);
  useEffect(() => {
    const id = requestAnimationFrame(() => setGrown(true));
    return () => cancelAnimationFrame(id);
  }, []);
  const max = Math.max(1, ...data.map((d) => d.count));
  return (
    <div className="ringgrid">
      {data.map((d) => {
        const frac = grown ? d.count / max : 0;
        const dash = RING_C * frac;
        return (
          <div className="ringcell" key={d.name}>
            <svg width={RING} height={RING} viewBox={`0 0 ${RING} ${RING}`} role="img" aria-label={`${d.name}: ${d.count}`}>
              <circle cx={RING / 2} cy={RING / 2} r={RING_R} fill="none" stroke="var(--surface-3)" strokeWidth={RING_STROKE} />
              <circle
                cx={RING / 2}
                cy={RING / 2}
                r={RING_R}
                fill="none"
                stroke={categoryColor(d.name)}
                strokeWidth={RING_STROKE}
                // A zero-length dash still paints under a round linecap, so a
                // count of 0 (routinely the case for "Descartadas" or
                // "Finalizadas") drew a solid dot at 12 o'clock that reads as
                // a sliver of data next to a label saying 0. Square caps have
                // no such artifact, and at zero the arc is invisible either
                // way, so nothing else changes visually.
                strokeLinecap={d.count === 0 ? "butt" : "round"}
                strokeDasharray={`${dash} ${RING_C - dash}`}
                transform={`rotate(-90 ${RING / 2} ${RING / 2})`}
                style={{ transition: "stroke-dasharray 0.6s cubic-bezier(0.16,1,0.3,1)" }}
              />
              <text x="50%" y="50%" textAnchor="middle" dominantBaseline="central" fontSize={20} fontWeight={700} fill="var(--text)">
                {d.count}
              </text>
            </svg>
            <div className="ringcell-label" title={d.name}>
              {d.name}
            </div>
          </div>
        );
      })}
    </div>
  );
}

// Shape-aware bars/rings block, keyed by shape so switching remounts and
// re-triggers the grow-in animation. Shared by StatsRings (géneros/tipos)
// and StatsInsights (embudo, en emisión vs finalizadas) so the app never
// grows a third visual language for "category magnitudes".
export function CategoryBlock({
  data,
  shape,
  emptyMessage,
}: {
  data: Datum[];
  shape: Shape;
  emptyMessage: string;
}) {
  if (data.length === 0) {
    return <div className="empty">{emptyMessage}</div>;
  }
  return shape === "bars" ? (
    <BarChart key="bars" data={data} />
  ) : (
    <RingGrid key="rings" data={data} />
  );
}

// One shared segmented Barras/Círculos toggle, reusable anywhere on the
// Estadísticas screen that needs to let the user flip `aot.statsShape`.
export function ShapeToggle({ shape, onChange }: { shape: Shape; onChange: (s: Shape) => void }) {
  const t = useT();
  return (
    <div className="seg" role="tablist" aria-label={t("stats.shapeToggleAria")}>
      <button
        type="button"
        role="tab"
        aria-selected={shape === "bars"}
        className={`seg-btn${shape === "bars" ? " active" : ""}`}
        onClick={() => onChange("bars")}
      >
        {t("stats.shapeBars")}
      </button>
      <button
        type="button"
        role="tab"
        aria-selected={shape === "rings"}
        className={`seg-btn${shape === "rings" ? " active" : ""}`}
        onClick={() => onChange("rings")}
      >
        {t("stats.shapeRings")}
      </button>
    </div>
  );
}

export function StatsRings({
  genres,
  types,
}: {
  genres: { genre: string; count: number }[];
  types: { kind: string; count: number }[];
}) {
  const t = useT();
  const [shape, setShape] = useStatsShape();

  const genreData = toData(genres, "genre");
  const typeData = toData(types, "kind");

  return (
    <>
      <div style={{ marginBottom: 20 }}>
        <ShapeToggle shape={shape} onChange={setShape} />
      </div>

      <div className="series-block" style={{ marginBottom: 20 }}>
        <div className="series-head">
          <h3 className="card-title">{t("stats.byGenre")}</h3>
        </div>
        <CategoryBlock data={genreData} shape={shape} emptyMessage={t("stats.ringsEmpty")} />
      </div>

      <div className="series-block">
        <div className="series-head">
          <h3 className="card-title">{t("stats.byType")}</h3>
        </div>
        <CategoryBlock data={typeData} shape={shape} emptyMessage={t("stats.ringsEmpty")} />
      </div>
    </>
  );
}
