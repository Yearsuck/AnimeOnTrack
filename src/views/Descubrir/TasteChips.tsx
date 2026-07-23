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
    <div className="taste-chips">
      <span className="muted taste-chips-label">{t("discover.topGenres")}</span>
      {genres.map((g) => (
        <span key={g.genre} className="taste-chip" style={{ background: categoryColor(g.genre) }}>
          {g.genre}
        </span>
      ))}
    </div>
  );
}
