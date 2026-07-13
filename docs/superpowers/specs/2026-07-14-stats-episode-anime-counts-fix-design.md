# BUG — Estadísticas: "Episodios vistos" y "Animes" no cuadran (las horas sí)

Fecha: 2026-07-14 · Estado: diseñado · Bug reportado por el usuario tras el batch 2026-07-13

## Síntoma

El usuario: *"los números de episodios reales y animes vistos no cuadran; las horas parece que sí
son correctas"*. El tile **Tiempo visto** suma dos mundos (episodios scrapeados vistos + episodios
de catálogo de las "Ya vistas"), pero los tiles **Episodios vistos** y **Animes** no — cada uno mide
un universo distinto, y ninguno mide "lo que he visto".

## Causa raíz (evidencia real, `.backup` de solo lectura de la DB de producción, 2026-07-14)

`db.get_watch_summary` (src-tauri/src/db.rs:876) calcula:

```sql
SELECT SUM(e.seen=1), COUNT(e.id) FROM episodes e JOIN series s ON s.id=e.series_id
WHERE s.source_id=?1 AND s.followed=1          -- ← el filtro culpable
```

Números medidos:

| consulta | valor |
|---|---|
| episodios seen/total con `followed=1` (lo que pinta el tile) | **1549 / 1671** |
| episodios `seen=1` en series `followed=0, watched_externally=1` (37 series) | **497** |
| episodios `seen=1` en otras series `followed=0` | 5 |
| `seen=1` totales en la DB (todas las series) | **2051** |
| episodios de catálogo de las "Ya vistas" (`anilist_catalog.episodes`) | **9144** |
| filas `watched_externally=1` | 566 (42 de ellas tienen episodios scrapeados) |
| filas `followed=1` | 135 |
| filas `followed=1 AND watched_externally=1` | **5** |
| filas que alimentan `distinct_anime` (`followed=1 OR watched_externally=1`) | **696** |

Tres defectos distintos:

1. **`episodes_watched` ignora todo lo que no sigues.** Al marcar "Ya lo vi" en Descubrir
   (`decide_catalog_card` → `Seen`), la fila queda `watched_externally=1, followed=0` y —si el
   enlace con el sitio funcionó— sus episodios se scrapean y se marcan vistos. Son **497 episodios
   vistos reales** que el tile no cuenta. Y las 566 "Ya vistas" (9144 episodios de catálogo) tampoco
   aparecen por ningún lado, aunque **sí** entran en el tile de horas (`estimated_minutes_external`
   en `get_watch_insights`). De ahí la incoherencia exacta que ve el usuario.
2. **`distinct_anime` ("Animes", ayuda "temporadas contadas como una") mide biblioteca, no
   visionado**: es `followed=1 OR watched_externally=1`, así que incluye series que sigues sin haber
   visto ni un episodio. La etiqueta promete "animes (vistos)".
3. **Doble conteo en las horas**: las 5 filas `followed=1 AND watched_externally=1` suman sus
   episodios reales en `estimated_minutes_tracked` **y** sus episodios de catálogo en
   `estimated_minutes_external`. Igualmente, una "Ya vista" con episodios scrapeados y vistos
   contaría por catálogo aunque tengamos el dato real (hoy no se solapa porque `tracked` filtra
   `followed=1`, pero al arreglar el defecto 1 sí se solaparía → hay que resolverlo a la vez).

## Diseño

Regla única, aplicada igual en los tres sitios: **un episodio se cuenta una sola vez, y se prefiere
el dato real (fila `episodes` con `seen=1`) sobre la estimación de catálogo.**

### `get_watch_summary` (db.rs)

- `episodes_watched` → `SELECT COUNT(*) FROM episodes e JOIN series s … WHERE s.source_id=?1 AND
  e.seen=1` (**sin** `followed=1`): cuenta los episodios vistos de verdad, sigas la serie o no
  → 2051 con la DB actual.
- `episodes_total` → mismo `JOIN` sin el filtro `followed=1`, pero **restringido a series con al
  menos un episodio visto o seguidas** para que el denominador siga significando algo
  ("de lo que tienes en la app, cuánto llevas visto"). Concretamente:
  `WHERE s.source_id=?1 AND (s.followed=1 OR EXISTS(SELECT 1 FROM episodes x WHERE x.series_id=s.id AND x.seen=1))`.
  Así las series descartadas con episodios scrapeados no inflan el total.
- **Nuevo campo `episodes_watched_external: i64`** = episodios de catálogo de las "Ya vistas" que
  **no** aportan episodios reales vistos:
  ```sql
  SELECT COALESCE(SUM(c.episodes),0)
  FROM series s JOIN anilist_catalog c ON c.id = s.anilist_id
  WHERE s.source_id=?1 AND s.watched_externally=1 AND c.episodes IS NOT NULL
    AND NOT EXISTS (SELECT 1 FROM episodes e WHERE e.series_id=s.id AND e.seen=1)
  ```
- **`distinct_anime` pasa a medir visionado**: franquicias (`franchise_key`) de las series con
  **evidencia de visionado** — `watched_externally=1` **OR** con ≥1 episodio `seen=1`. Las seguidas
  sin ningún episodio visto dejan de contar aquí (ya salen en "Siguiendo en emisión" y en el embudo).

### `get_watch_insights` (db.rs) — coherencia de horas

- `estimated_minutes_tracked` → deja de filtrar `followed=1`: suma **todos** los episodios `seen=1`
  del source (por serie, `minutes_per_episode(s.kind)`).
- `estimated_minutes_external` → añade el mismo `NOT EXISTS (… seen=1)` que arriba, de modo que una
  serie con datos reales no vuelve a contarse por catálogo. Esto elimina el doble conteo de las 5
  filas `followed=1 AND watched_externally=1` y mantiene el total de horas prácticamente igual (por
  eso al usuario "las horas le cuadran": el error de las horas es pequeño; el de los episodios es
  estructural).
- `external_titles_estimated` / `external_titles_total` se recalculan con el mismo criterio
  (estimadas = las que aportan minutos por catálogo; total = todas las `watched_externally=1`).

### UI (`Stats.tsx`, `StatsInsights.tsx`) + i18n

- Tile **Episodios vistos**: pasa a mostrar `episodes_watched + episodes_watched_external` (número
  único, no `X/Y`), con sub-línea `stats.episodesWatchedHelp`:
  ES `"{real} con seguimiento real · {external} estimados de «Ya vistas»"`,
  EN `"{real} tracked · {external} estimated from “Already watched”"`.
- El par `X/Y` de progreso sigue existiendo, pero donde ya vive: el tile **Progreso global** de
  `StatsInsights` (`episodes_watched / episodes_total`). Ya no se repite arriba.
- Tile **Animes** → etiqueta `stats.distinctAnime` = ES `"Animes vistos"` / EN `"Anime watched"`;
  ayuda `stats.distinctAnimeHelp` = ES `"temporadas contadas como una · incluye «Ya vistas»"` /
  EN `"seasons counted once · includes “Already watched”"`.
- Claves nuevas en **es.ts y en.ts**: `stats.episodesWatchedHelp`.

## Criterios de aceptación (verificables)

1. Tests nuevos en `cargo test` (TDD, DB en memoria):
   - una serie `followed=0, watched_externally=1` con 3 episodios `seen=1` **cuenta** en
     `episodes_watched` y **no** aporta minutos/episodios por catálogo (aunque tenga `anilist_id`
     con 12 episodios en `anilist_catalog`);
   - una serie `watched_externally=1` **sin** episodios cuenta 12 en `episodes_watched_external` y
     `12 * minutes_per_episode("TV")` en `estimated_minutes_external`;
   - una serie `followed=1` sin episodios vistos **no** cuenta en `distinct_anime`;
   - `followed=1 AND watched_externally=1` con episodios vistos se cuenta **una sola vez** en horas.
2. Contra la DB real (solo-lectura sobre `.backup`): `episodes_watched == 2051`,
   `episodes_watched_external == 9144 menos los episodios de catálogo de las "Ya vistas" que tienen
   episodios vistos reales` (calcularlo con el SQL del spec y comprobar que el comando devuelve lo
   mismo), y el total de horas **no baja más de ~2%** respecto al valor actual (el doble conteo era
   pequeño: 5 filas).
3. `npx tsc --noEmit` limpio, `npm run build` OK, `cargo test` verde.
4. Las horas y los episodios cuentan **el mismo universo**: `episodes_watched +
   episodes_watched_external` debe ser exactamente el número de episodios sobre el que se calculan
   `estimated_minutes_tracked + estimated_minutes_external` (verificable en test: minutos totales ==
   suma de `episodios × minutes_per_episode` de cada serie contada, sin solapes).

## Qué verificar en vivo

Harness HTML en loopback con los tiles reales (oscuro/claro) para la nueva sub-línea. La ventana
Tauri no es alcanzable por herramientas: el usuario relanza la app y comprueba que "Episodios
vistos" ya incluye sus "Ya vistas" y que "Animes vistos" no cuenta las seguidas sin empezar.
