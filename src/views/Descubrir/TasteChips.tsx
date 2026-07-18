import { useEffect, useState } from "react";
import { getTopGenres } from "../../api";
import { useT } from "../../i18n";
import { categoryColor } from "../../lib/categoryColor";
import type { GenreAffinity } from "../../types";
import { TOP_GENRES_LIMIT } from "./constants";

export function TasteChips() {
  const t = useT();
  const [genres, setGenres] = useState<GenreAffinity[]>([]);
  useEffect(() => {
    getTopGenres(TOP_GENRES_LIMIT).then(setGenres);
  }, []);
  if (genres.length === 0) return null;
  return (
    <div style={{ display: "flex", alignItems: "center", gap: 8, flexWrap: "wrap", marginBottom: 16 }}>
      <span className="muted" style={{ fontSize: 12 }}>
        {t("discover.topGenres")}
      </span>
      {genres.map((g) => (
        <span
          key={g.genre}
          style={{
            fontSize: 12,
            fontWeight: 600,
            padding: "3px 10px",
            borderRadius: "var(--radius-round)",
            background: categoryColor(g.genre),
            color: "#05121f",
          }}
        >
          {g.genre}
        </span>
      ))}
    </div>
  );
}
