import { useT } from "../i18n";
import { categoryColor } from "../lib/categoryColor";

const SIZE = 96;
const STROKE = 8;
const RADIUS = (SIZE - STROKE) / 2;
const CIRCUMFERENCE = 2 * Math.PI * RADIUS;

function Ring({ label, count, max, color }: { label: string; count: number; max: number; color: string }) {
  const frac = max > 0 ? count / max : 0;
  const dash = CIRCUMFERENCE * frac;
  return (
    <div style={{ display: "flex", flexDirection: "column", alignItems: "center", width: SIZE + 24 }}>
      <svg width={SIZE} height={SIZE} viewBox={`0 0 ${SIZE} ${SIZE}`}>
        <circle
          cx={SIZE / 2}
          cy={SIZE / 2}
          r={RADIUS}
          fill="none"
          stroke="var(--surface-3)"
          strokeWidth={STROKE}
        />
        <circle
          cx={SIZE / 2}
          cy={SIZE / 2}
          r={RADIUS}
          fill="none"
          stroke={color}
          strokeWidth={STROKE}
          strokeLinecap="round"
          strokeDasharray={`${dash} ${CIRCUMFERENCE - dash}`}
          transform={`rotate(-90 ${SIZE / 2} ${SIZE / 2})`}
        />
        <text
          x="50%"
          y="50%"
          textAnchor="middle"
          dominantBaseline="central"
          fontSize={22}
          fontWeight={700}
          fill="var(--text)"
        >
          {count}
        </text>
      </svg>
      <div className="muted" style={{ fontSize: 12, marginTop: 6, textAlign: "center" }}>
        {label}
      </div>
    </div>
  );
}

function RingRow({ data, nameKey }: { data: { count: number }[]; nameKey: "genre" | "kind" }) {
  const t = useT();
  if (data.length === 0) {
    return <div className="empty">{t("stats.ringsEmpty")}</div>;
  }
  const max = Math.max(...data.map((d) => d.count));
  return (
    <div style={{ display: "flex", flexWrap: "wrap", gap: 16, padding: "12px 16px" }}>
      {data.map((d) => {
        const name = (d as unknown as Record<string, string>)[nameKey];
        return (
          <Ring key={name} label={name} count={d.count} max={max} color={categoryColor(name)} />
        );
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
  return (
    <>
      <div className="series-block" style={{ marginBottom: 20 }}>
        <div className="series-head">
          <h3 className="card-title">{t("stats.byGenre")}</h3>
        </div>
        <RingRow data={genres} nameKey="genre" />
      </div>

      <div className="series-block">
        <div className="series-head">
          <h3 className="card-title">{t("stats.byType")}</h3>
        </div>
        <RingRow data={types} nameKey="kind" />
      </div>
    </>
  );
}
