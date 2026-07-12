import { useEffect, useRef, useState } from "react";
import { useT } from "../i18n";
import { categoryColor } from "../lib/categoryColor";

// Circles <-> bars, persisted like language/theme.
type Shape = "rings" | "bars";
const SHAPE_KEY = "aot.statsShape";

function getInitialShape(): Shape {
  try {
    const s = localStorage.getItem(SHAPE_KEY);
    if (s === "rings" || s === "bars") return s;
  } catch {
    // localStorage unavailable — fall through to default.
  }
  return "rings";
}

// ---- Morph geometry ----------------------------------------------------
// Each category's mark is two stroked paths (recessive track + colored value)
// drawn by "unrolling" a ring into a bar. A single value m in [0,1] (0 = ring,
// 1 = bar) lerps every sample point between its position on a circle and its
// position on a horizontal line, so the arc visibly straightens into the bar.
const VBW = 320; // viewBox width
const VBH = 56; // viewBox height
const R = 22; // ring radius
const CX = VBW / 2; // ring centre x
const CY = VBH / 2; // ring centre y / bar y
const BAR_X0 = 14; // bar left inset
const BAR_W = VBW - BAR_X0 * 2; // bar track length
const SAMPLES = 56;

const TAU = Math.PI * 2;

function lerp(a: number, b: number, t: number): number {
  return a + (b - a) * t;
}

// One sample point of the unrolled shape at arc/line parameter s in [0,1].
function samplePoint(s: number, m: number): [number, number] {
  const angle = (-0.25 + s) * TAU; // start at top (-90deg), clockwise
  const rx = CX + R * Math.cos(angle);
  const ry = CY + R * Math.sin(angle);
  const bx = BAR_X0 + s * BAR_W;
  const by = CY;
  return [lerp(rx, bx, m), lerp(ry, by, m)];
}

function pathFor(fraction: number, m: number): string {
  const f = Math.max(0, Math.min(1, fraction));
  if (f <= 0) return "";
  const n = Math.max(2, Math.round(SAMPLES * f));
  let d = "";
  for (let i = 0; i <= n; i++) {
    const s = (i / n) * f;
    const [x, y] = samplePoint(s, m);
    d += `${i === 0 ? "M" : "L"}${x.toFixed(2)} ${y.toFixed(2)} `;
  }
  return d.trim();
}

function easeInOutCubic(t: number): number {
  return t < 0.5 ? 4 * t * t * t : 1 - Math.pow(-2 * t + 2, 3) / 2;
}

// Tween m toward `target` (0/1) with rAF; snap instantly under reduced motion.
function useMorph(target: number): number {
  const [m, setM] = useState(target);
  const mRef = useRef(target);
  mRef.current = m;
  const rafRef = useRef<number | undefined>(undefined);

  useEffect(() => {
    const reduce =
      typeof window !== "undefined" &&
      window.matchMedia?.("(prefers-reduced-motion: reduce)").matches;
    if (reduce) {
      setM(target);
      return;
    }
    const from = mRef.current;
    const start = performance.now();
    const dur = 500;
    const tick = (now: number) => {
      const t = Math.min(1, (now - start) / dur);
      setM(from + (target - from) * easeInOutCubic(t));
      if (t < 1) rafRef.current = requestAnimationFrame(tick);
    };
    rafRef.current = requestAnimationFrame(tick);
    return () => {
      if (rafRef.current) cancelAnimationFrame(rafRef.current);
    };
  }, [target]);

  return m;
}

function MorphMark({ fraction, color, m }: { fraction: number; color: string; m: number }) {
  return (
    <svg className="stats-mark" viewBox={`0 0 ${VBW} ${VBH}`} role="presentation">
      <path
        d={pathFor(1, m)}
        fill="none"
        stroke="var(--surface-3)"
        strokeWidth={10}
        strokeLinecap="round"
      />
      <path
        d={pathFor(fraction, m)}
        fill="none"
        stroke={color}
        strokeWidth={10}
        strokeLinecap="round"
      />
    </svg>
  );
}

function CategoryRow({ name, count, max, m }: { name: string; count: number; max: number; m: number }) {
  const fraction = max > 0 ? count / max : 0;
  return (
    <div className="stats-cat-row">
      <div className="stats-cat-head">
        <span className="stats-cat-name" title={name}>
          {name}
        </span>
        <span className="stats-cat-count">{count}</span>
      </div>
      <MorphMark fraction={fraction} color={categoryColor(name)} m={m} />
    </div>
  );
}

function CategoryBlock({
  data,
  nameKey,
  m,
}: {
  data: { count: number }[];
  nameKey: "genre" | "kind";
  m: number;
}) {
  const t = useT();
  if (data.length === 0) {
    return <div className="empty">{t("stats.ringsEmpty")}</div>;
  }
  const max = Math.max(...data.map((d) => d.count));
  return (
    <div className="stats-cat-list">
      {data.map((d) => {
        const name = (d as unknown as Record<string, string>)[nameKey];
        return <CategoryRow key={name} name={name} count={d.count} max={max} m={m} />;
      })}
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
  const [shape, setShape] = useState<Shape>(getInitialShape);
  const m = useMorph(shape === "bars" ? 1 : 0);

  function pick(next: Shape) {
    setShape(next);
    try {
      localStorage.setItem(SHAPE_KEY, next);
    } catch {
      // choice just won't persist
    }
  }

  return (
    <>
      <div className="seg" style={{ marginBottom: 20 }} role="tablist" aria-label={t("stats.shapeToggleAria")}>
        <button
          type="button"
          role="tab"
          aria-selected={shape === "rings"}
          className={`seg-btn${shape === "rings" ? " active" : ""}`}
          onClick={() => pick("rings")}
        >
          {t("stats.shapeRings")}
        </button>
        <button
          type="button"
          role="tab"
          aria-selected={shape === "bars"}
          className={`seg-btn${shape === "bars" ? " active" : ""}`}
          onClick={() => pick("bars")}
        >
          {t("stats.shapeBars")}
        </button>
      </div>

      <div className="series-block" style={{ marginBottom: 20 }}>
        <div className="series-head">
          <h3 className="card-title">{t("stats.byGenre")}</h3>
        </div>
        <div style={{ padding: "12px 16px" }}>
          <CategoryBlock data={genres} nameKey="genre" m={m} />
        </div>
      </div>

      <div className="series-block">
        <div className="series-head">
          <h3 className="card-title">{t("stats.byType")}</h3>
        </div>
        <div style={{ padding: "12px 16px" }}>
          <CategoryBlock data={types} nameKey="kind" m={m} />
        </div>
      </div>
    </>
  );
}
