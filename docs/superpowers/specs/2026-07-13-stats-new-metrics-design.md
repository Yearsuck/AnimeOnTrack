# Estadísticas: métricas nuevas (tiempo visto, progreso, embudo, actividad, top series)

Fecha: 2026-07-13 · Estado: diseñado · Tarea 9 del batch 2026-07-13

## Problema

La pantalla de Estadísticas enseña cinco números escalares (`WatchSummary`) + géneros/tipos
(barras/donuts) + el grafo 3D. Se queda corta: no dice **cuánto tiempo** has visto, **cuánto te
falta**, **qué has visto más**, ni **qué haces en Descubrir**. Todo eso es calculable **localmente**
contra la DB — sin scrapear y sin llamar a AniList.

## Contexto técnico REAL (verificado contra el código y la DB de producción)

DB inspeccionada en solo-lectura sobre un `.backup` online de
`%APPDATA%\com.ernes.aot-scaffold\animeontrack.sqlite` (2026-07-13):

| dato | valor real |
|---|---|
| `series` | 4169 |
| `series.followed=1` | 135 (101 con `is_airing=0`, 34 con `is_airing=1`) |
| `series.watched_externally=1` | 524 |
| `series.backlog_status='want'` | 183 |
| `series.backlog_status='discarded'` | 3202 |
| `episodes` | 2220 (2051 con `seen=1`) |
| `episodes.seen_at IS NOT NULL` | **534** (de los 2051 vistos) |
| `DISTINCT DATE(seen_at)` | **3** (2026-07-11: 20, 07-12: 354, 07-13: 160) |
| `anilist_catalog` | 22420 |

Hechos que condicionan el diseño (no adivinados — medidos):

1. **Las 135 series seguidas tienen `anilist_id = NULL`** (0/135). Son filas del sitio scrapeado:
   su único dato de duración/tipo es `series.kind`, que es vocabulario libre del sitio y está
   sucio: `TV`(574), `MOVIE`(27), `4K`(11), `Pelicula`(8), `OVA`(8), `ONA`(7), `Sin Censura`(5),
   `SPECIAL`(4), `Blu-Ray`(3), `Resubido`(2), `Yaoi`(1)… → **no se puede confiar en `kind` para
   estimar duración**, salvo para detectar películas.
2. **Las 524 "Ya vistas" SÍ enlazan con el catálogo**: 518 tienen `anilist_id`, las 518 existen en
   `anilist_catalog`, 495 traen `episodes` no-NULL, sumando **8390 episodios**. `anilist_catalog`
   tiene `format` (`TV`/`MOVIE`/`OVA`/`ONA`/`SPECIAL`/`TV_SHORT`/`MUSIC`) y `episodes`, pero
   **no tiene `duration`** (no se sincroniza) → la duración hay que estimarla por formato.
3. **`episodes.seen_at` es joven y está inflado por la cascada.** Solo 534/2051 filas vistas lo
   tienen (la columna se añadió hace días) y `set_seen_cascade` estampa `datetime('now')` en
   **todas** las filas anteriores que marca de golpe (de ahí los 354 "vistos" del 12/07). Por tanto
   `seen_at` **no** mide "cuándo viste el episodio", mide "cuándo lo marcaste". Cualquier métrica
   basada en él (racha, actividad diaria) debe llamarse *marcados*, no *vistos*, y decir desde
   cuándo hay datos. **No se implementa "racha de días"**: con 3 días de datos y marcado en masa
   sería una métrica falsa.
4. `db.get_watch_summary` (src-tauri/src/db.rs:870) ya calcula `followed_series`, `distinct_anime`
   (vía `franchise_key`, db.rs:127), `episodes_watched`/`episodes_total` (solo sobre `followed=1`),
   `airing_followed`, `pending_to_watch`, `backlog_want`.
5. `get_genre_stats` / `get_type_stats` ya existen y `genres.rs::canonical_genre()` normaliza
   ES/EN (la DB real tiene `Fantasy`=145 y `Fantasía`=112 en `series_genres` — son lo mismo).
   **Reutilizar `canonical_genre`, no volver a agrupar a mano.**
6. Frontend: `src/views/Stats.tsx` recarga con `load()` en montaje, al activarse la pestaña y al
   terminar un `refresh-progress`. `src/views/StatsRings.tsx` ya tiene un **gráfico de barras
   horizontales** (etiqueta | barra proporcional | valor) y una **rejilla de donuts**, ambos
   theme-aware. Reutilizar esos componentes, no inventar otros.

## Diseño

### Backend: un comando nuevo, `get_watch_insights`

`src-tauri/src/models.rs`:

```rust
pub struct DayCount { pub day: String, pub count: i64 }        // "2026-07-13", 160
pub struct TitleCount { pub title: String, pub count: i64 }    // "One Piece…", 114

pub struct WatchInsights {
    pub estimated_minutes_tracked: i64,   // episodios seen=1 de series seguidas
    pub estimated_minutes_external: i64,  // "Ya vistas", vía anilist_catalog.episodes
    pub external_titles_estimated: i64,   // 495 — cuántas "Ya vistas" aportan minutos
    pub external_titles_total: i64,       // 524 — para poder decir "495 de 524"
    pub avg_episodes_per_series: f64,     // media de episodios (todos, no solo vistos) por seguida
    pub followed_airing: i64,             // 34
    pub followed_finished: i64,           // 101
    pub discarded: i64,                   // 3202
    pub want: i64,                        // 183
    pub watched_externally: i64,          // 524
    pub top_series: Vec<TitleCount>,      // top 8 por episodios vistos
    pub marks_by_day: Vec<DayCount>,      // últimos 30 días, solo días con marcas
    pub marks_tracked_since: Option<String>, // MIN(DATE(seen_at)) — para el disclaimer
}
```

`src-tauri/src/db.rs`:

- Función **pura y unit-testeada** para la estimación de duración:
  ```rust
  /// Minutos estimados por episodio según el formato. La DB no guarda duración
  /// real (AniList la tiene, pero no se sincroniza), así que esto es una
  /// estimación explícita, no un dato.
  pub fn minutes_per_episode(format: Option<&str>) -> i64
  ```
  Mapa (case-insensitive, sobre el token normalizado): `MOVIE`/`PELICULA`/`PELÍCULA` → 100,
  `MUSIC` → 5, `TV_SHORT` → 8, `OVA`/`SPECIAL` → 26, resto (incl. `TV`, `ONA`, y todo el ruido del
  sitio: `4K`, `Blu-Ray`, `Resubido`, `Sin Censura`, NULL) → **24**. Tests: cada rama + el ruido
  real observado en la DB + `None`.
- `estimated_minutes_tracked`: `SUM(minutes_per_episode(s.kind))` sobre `episodes e JOIN series s`
  con `e.seen=1 AND s.followed=1 AND s.source_id=?`. Se agrupa por serie en SQL
  (`SELECT s.kind, COUNT(*) … GROUP BY s.id`) y se multiplica en Rust — SQLite no conoce el mapa.
- `estimated_minutes_external`: `SUM(c.episodes * minutes_per_episode(c.format))` sobre
  `series s JOIN anilist_catalog c ON c.id = s.anilist_id` con `s.watched_externally=1 AND
  c.episodes IS NOT NULL`. Devuelve además cuántas filas contaron (`external_titles_estimated`) y
  el total (`external_titles_total`), para que la UI pueda ser honesta sobre la cobertura.
- `top_series`: `SELECT s.title, COUNT(*) c FROM episodes e JOIN series s … WHERE e.seen=1 AND
  s.source_id=? GROUP BY s.id ORDER BY c DESC LIMIT 8`.
- `marks_by_day`: `SELECT DATE(seen_at) d, COUNT(*) FROM episodes e JOIN series s … WHERE
  e.seen_at IS NOT NULL AND DATE(seen_at) >= DATE('now','-30 days') GROUP BY d ORDER BY d`.
- `avg_episodes_per_series`: media de `COUNT(episodes)` por serie con `followed=1` (12.38 real).
- Todo con `source_id` como el resto de `get_watch_summary` (la app es por-sitio).

`src-tauri/src/commands.rs`: `#[tauri::command] async fn get_watch_insights(state) -> Result<WatchInsights, String>`
siguiendo el patrón exacto de `get_watch_summary` (misma resolución de `source_id`). Registrar en
`invoke_handler`. **Ninguna llamada a red: SQLite puro.**

`src/api.ts`: `getWatchInsights(): Promise<WatchInsights>` + tipos en `src/types.ts`.

### Frontend: bloque "Resumen" nuevo en Estadísticas

Nuevo `src/views/StatsInsights.tsx`, montado en `Stats.tsx` **entre** los tiles actuales y el
selector Grafo/Barras, cargado en el mismo `load()` (una llamada más en el `Promise.all`).

1. **Fila de tiles** (mismo `.card`/`.grid` que los tiles actuales):
   - `stats.timeWatched` — **Tiempo visto (estimado)**: `Xh` (o `Xd Yh` si ≥ 48h) =
     `estimated_minutes_tracked + estimated_minutes_external`. Sub-línea con la advertencia:
     "estimación: 24 min/episodio (100 películas); {n} de {m} 'Ya vistas' con datos".
   - `stats.completion` — **Progreso global**: `episodes_watched / episodes_total` en % (de
     `WatchSummary`, ya existente) con una barra de progreso fina debajo.
   - `stats.avgEpisodes` — **Media de episodios por serie**: `avg_episodes_per_series` con 1 decimal.
2. **Barras "Top series"** (reutiliza el componente de barras horizontales de `StatsRings.tsx`,
   extraído a un componente exportable si hace falta): top 8 por episodios vistos.
3. **Donut / barras "Tu embudo de Descubrir"**: `want` / `discarded` / `watched_externally` /
   `followed` — reutiliza la rejilla de donuts o barras según la forma activa
   (`aot.statsShape` en localStorage, ya existente). Enseña de un vistazo la proporción real
   (3202 descartadas vs 183 querer ver).
4. **Barras "Episodios marcados por día (últimos 30 días)"**: `marks_by_day`, con nota al pie
   `stats.marksCaveat`: "cuenta cuándo marcaste el episodio, no cuándo lo viste; marcar una serie
   entera marca todos sus episodios anteriores. Datos desde {date}." Si `marks_by_day` está vacío,
   el bloque **no se renderiza**.
5. **Seguidas: en emisión vs finalizadas** (34/101) como donut de dos segmentos junto al embudo.

Todo theme-aware (tokens existentes, cero rgba nuevo) y con las dos formas ya soportadas
(barras/donuts) respetando `aot.statsShape` — no se añade un tercer lenguaje visual.

### i18n

Claves nuevas en **es.ts y en.ts** (mismas claves; `Messages = Record<keyof typeof es, string>`):
`stats.insightsHeading`, `stats.timeWatched`, `stats.timeWatchedHelp` (con `{done}`/`{total}`),
`stats.completion`, `stats.completionHelp`, `stats.avgEpisodes`, `stats.topSeries`,
`stats.funnelHeading`, `stats.funnelWant`, `stats.funnelDiscarded`, `stats.funnelWatched`,
`stats.funnelFollowed`, `stats.airingVsFinished`, `stats.airing`, `stats.finished`,
`stats.marksHeading`, `stats.marksCaveat` (con `{date}`), `stats.hoursUnit`, `stats.daysUnit`.

## Criterios de aceptación (verificables)

1. `cargo test --manifest-path src-tauri/Cargo.toml` verde, **incluidos tests nuevos**:
   `minutes_per_episode` (cada rama + ruido real `4K`/`Blu-Ray`/`Sin Censura`/`None`), y un test de
   `get_watch_insights` sobre una DB en memoria con datos sembrados (serie seguida con 3 episodios
   vistos + 1 `watched_externally` enlazada a un `anilist_catalog` de 12 episodios formato TV →
   `estimated_minutes_tracked == 72`, `estimated_minutes_external == 288`).
2. `npx tsc --noEmit` limpio y `npm run build` OK.
3. Contra la DB real (solo-lectura sobre un `.backup`), los números del comando cuadran con las
   consultas SQL directas: `followed_airing=34`, `followed_finished=101`, `want=183`,
   `discarded=3202`, `watched_externally=524`, `external_titles_estimated=495`,
   `external_titles_total=524`, top-1 = "One Piece: Arco de Elbaph" con 114.
4. `get_watch_insights` **no hace red**: ni `reqwest`, ni `scraper_engine`, ni WebView2 (revisable
   por lectura del diff — es solo SQL).
5. La UI muestra explícitamente que el tiempo es una **estimación** y que las marcas por día miden
   *marcado*, no *visionado* (criterio de honestidad, no cosmético).
6. Estados vacíos: con 0 episodios vistos no revienta ninguna división (0/0 → 0%), y los bloques sin
   datos no se renderizan.

## Qué verificar en vivo (harness, no ventana Tauri)

Portar el bloque `StatsInsights` completo (tiles + barras top-series + donuts del embudo + barras de
marcas) con datos reales copiados de la DB a un HTML autocontenido; servirlo con
`python -m http.server <puerto> --bind 127.0.0.1` y verlo con claude-in-chrome en **oscuro y
claro**: legibilidad de números grandes, que las barras compartan el mismo máximo, que la nota al
pie no se confunda con un dato, y que nada desborde a ~1400px y ~900px. Matar el servidor por PID.

La ventana Tauri real no es alcanzable por herramientas; el usuario relanza la app para el visto
bueno final.
