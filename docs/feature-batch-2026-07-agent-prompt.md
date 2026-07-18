# Prompt: agente planificador — batch de features/fixes (Descubrir, Biblioteca, Catálogo, Stats 3D)

Pega el bloque de abajo en un **chat/agente nuevo** (contexto limpio) o úsalo como
`/goal`. El planner debe correr en **Opus**. Solo planifica: no toca código.

Modelos de implementación (los fija el plan, no este agente):

| Tipo de tarea | Modelo | Por qué |
|---|---|---|
| Fix puntual con spec cerrada (rename, quitar hardcode, ajustar un parámetro) | **Haiku 4.5** | Cero decisiones de diseño |
| Nueva superficie de datos/UI (filtros studio, duración real AniList, texturas 3D) | **Sonnet** | Requiere criterio de diseño acotado tras el brainstorm |

---

```
GOAL: Diseñar un plan estructurado y por fases para un batch de 8 features/fixes
sobre AnimeOnTrack (Descubrir, Biblioteca, Catálogo, Stats 3D). SOLO el plan — NO
implementar nada de código todavía.

## Contexto
Tauri v2 + Rust (src-tauri/) + React/TS (src/). Lee CLAUDE.md ENTERO antes de
empezar: contiene las invariantes que ningún cambio puede romper. El repo acaba
de pasar por una reorganización (db.rs/commands.rs → módulos por dominio,
Descubrir.tsx → carpeta) — los archivos relevantes YA están divididos por
feature (src-tauri/src/db/{catalog,stats}.rs, src-tauri/src/commands/{scan,
discover}.rs, src/views/Descubrir/{DeckPanel,SwipeView,...}.tsx), así que cada
tarea de abajo debería tocar 1-3 archivos concretos, no un god-file.

El grafo de codebase-memory-mcp está indexado para este repo — úsalo
(get_architecture / search_graph / trace_path) para localizar la implementación
exacta de cada punto antes de escribir la spec de su tarea, en vez de adivinar
nombres de archivo/función.

## Las 8 tareas (tal como las pidió el usuario, en español; agudizalas a spec
cerrada durante el Fase 2 — varias son ambiguas o dependen de qué datos expone
AniList, hay que confirmarlo antes de prometer nada)

1. **Bug — filtro "En emisión, esta temporada"**: hoy filtra por seguidas
   (followed) que estrenaron hace <3 meses. Debe ser simplemente "estrenó hace
   menos de 3 meses", sin importar si está seguida o no. Localiza la lógica
   exacta (probablemente AiringGrid.tsx o similar + su comando backend; hay un
   spec previo en docs/superpowers/specs/2026-07-13-airing-this-season-design.md
   — léelo primero) y quita el requisito de "seguida".

2. **Feature — filtro por estudio (y director si es viable) en Biblioteca y
   Catálogo**: `anilist_catalog` no guarda estudio ni director hoy. AniList
   expone `studios` en su Media query fácilmente (mismo patrón que se usó para
   agregar `status` recientemente: columna nueva + campo en el GraphQL query +
   backfill vía resync). "Director" NO es un campo simple de Media — vive en
   `staff` (una conexión paginada, cara de sincronizar para ~22k títulos).
   Investiga si es viable antes de prometerlo; si no, dejalo fuera del scope y
   decilo explícitamente en el plan. Además: "Biblioteca" (series seguidas/
   scrapeadas del sitio) NO tiene estudio propio — el sitio scrapeado no lo
   expone. Solo se podría filtrar por estudio ahí para series con `anilist_id`
   ya enlazado (via link_catalog_series). Aclara ese límite con el usuario en
   el brainstorm en vez de asumir.

3. **Bug — Hentai/Ecchi excluidos siempre en Descubrir**: HOY es un bug real y
   ya localizado — `EXCLUDED_CATALOG_GENRES: &[&str] = &["Hentai", "Ecchi"]` en
   `src-tauri/src/commands/discover.rs` (usado por `filter_candidate_genres`)
   es una baseline hardcodeada "que no se puede levantar con un setting". El
   usuario quiere que estos géneros se comporten como cualquier otro: excluidos
   SOLO si el usuario los banea manualmente en el panel de bans (Section B, el
   mismo `get_banned_genres`/`set_banned_genres` que ya existe). Fix: eliminar
   la baseline hardcodeada, dejar que Hentai/Ecchi pasen por el mismo camino
   que cualquier género. Cuidado: la baseline también reduce el peso de
   `random_catalog_anime_in_genre` — confirma que quitarla no rompe el resto de
   la query (formato/popularidad/exclusión de decididos siguen aplicando igual).

4. **Rename — "Biblioteca" → "Mi biblioteca"**: cambio de string en
   `src/i18n/catalog/{es,en}.ts` (y el label de navegación si vive aparte).
   Mecánico, Haiku.

5. **Feature — duración real de AniList en vez de estimada**: HOY
   `minutes_per_episode()` en `src-tauri/src/db/stats.rs` ESTIMA minutos por
   episodio según `format` (tabla hardcodeada: TV=24min, MOVIE=100min, etc.) —
   ver su doc comment, dice explícitamente que AniList tiene un campo real
   (`duration`) pero no está sincronizado. Antes de prometer el fix: confirma
   si el conteo de EPISODIOS ya es real hoy (parece que sí — `CatalogAnime.
   episodes` ya se sincroniza, y las series scrapeadas del sitio cuentan
   episodios reales de la tabla `episodes`, no estimados) — si es así, el fix
   real es solo sobre DURACIÓN, no episodios; aclaralo en el plan. Mismo patrón
   que `status` (agregado hace poco): columna `duration` nueva en
   `anilist_catalog`, campo en el GraphQL query, usar el valor real en
   `get_watch_insights`/`get_watch_summary` cuando esté disponible, fallback a
   la estimación cuando sea NULL (pre-resync o película/serie sin dato en
   AniList) — NUNCA asumir 100% de cobertura, es un dato externo.

6. **Tuning + investigación — grafo 3D de Stats**: (a) bajar la fuerza de
   atracción de los nodos/bolas para que se vean mejor las conexiones — ubica
   el parámetro de física (probablemente en `src/views/StatsGraph.tsx`, config
   de force-graph-3d o similar) y bájalo; confirma con el usuario un valor o
   rango antes de fijarlo a ciegas. (b) el usuario reporta un nodo "Siguiendo"
   con contador "(0)" que igual tiene muchas conexiones — esto es raro, hay que
   INVESTIGAR la causa real antes de tocar nada (¿es un nodo de categoría
   agregando conexiones a series individuales pero con el contador calculado
   mal? ¿es un bug de conteo? ¿es esperado y solo falta explicarlo en la UI?)
   — trace_path sobre la construcción del grafo (`get_stats_graph_data` en
   `src-tauri/src/db/stats.rs` + el consumo en StatsGraph.tsx) para entender
   qué arma ese nodo y por qué su count no coincide con sus edges.

7. **Visual — textura de los nodos/planetas**: "ahora mismo es horrible, no
   parece un planeta". Hay specs previos relevantes:
   docs/superpowers/specs/2026-07-12-stats-graph-galaxy-planets-design.md y
   2026-07-13-stats-graph-refresh-planets-light-design.md — léelos primero
   para no repetir contexto de diseño ya decidido. Esto es trabajo visual
   puro (probablemente un canvas/shader procedural en StatsGraph.tsx) — el
   brainstorm debe explorar 2-3 direcciones concretas (textura procedural más
   rica, degradados, ruido/craters, etc.) con el usuario antes de implementar,
   posiblemente con el companion visual del skill de brainstorming si aplica.

8. **Bug de contraste — modo claro del grafo 3D**: las líneas de conexión son
   blancas sobre fondo claro → ilegible. Localiza dónde se fija el color de
   los links (probablemente un valor fijo, no theme-aware) y hazlo depender del
   tema actual (`src/theme.ts` ya tiene el patrón light/dark establecido en el
   resto de la app — reutilízalo, no inventes un mecanismo nuevo).

## Fase 1 — Análisis (grafo ya indexado, no leas archivos enteros a ciegas)
Para cada tarea: `search_graph`/`trace_path` para confirmar la ubicación exacta
antes de escribir su spec. Lee los docs/superpowers/specs/*.md mencionados
arriba cuando aplique — no repitas decisiones de diseño ya tomadas.

## Fase 2 — Brainstorm (obligatorio antes del plan)
Invoca superpowers:brainstorming. Puntos que necesitan decisión del usuario, NO
asumas:
  - Tarea 2: alcance real de "director" (probablemente fuera de scope — costo
    de sync alto) y si "Biblioteca" queda sin filtro de estudio para series no
    enlazadas o eso es aceptable.
  - Tarea 5: confirmar que el fix es solo sobre duración (no episodios) antes
    de prometer "datos objetivos" en general.
  - Tarea 6a: valor/rango de la fuerza de atracción — pedir referencia o dejar
    un slider si el usuario no tiene un número en mente.
  - Tarea 7: dirección visual concreta para la textura (mostrar 2-3 opciones,
    considerá ofrecer el companion visual del skill si el usuario acepta).

## Formato de salida
Escribe el plan en docs/feature-batch-2026-07-plan.md:
  - Fases ordenadas por riesgo/dependencia; cada fase independiente y
    mergeable sola (igual que el batch de reorganización anterior).
  - Por CADA tarea, spec cerrada: archivos exactos, qué cambia línea por
    línea (o el contrato exacto de la nueva función/columna), criterios de
    aceptación (cargo build + cargo test + npx tsc --noEmit + npm run build en
    verde; para las de datos nuevos, tests TDD igual que se hizo con `status`
    hace poco — ver commits recientes en discover.rs/db/catalog.rs como
    referencia de patrón), tamaño estimado, MODELO asignado.
  - Las tareas 2 y 5 (nuevas columnas de `anilist_catalog` sincronizadas desde
    AniList) DEBEN seguir el mismo patrón NULL-safe que `status`: dato ausente
    hasta el próximo resync, nunca romper el comportamiento existente mientras
    tanto, avisar en la UI si hace falta (como se hizo con
    `status_data_synced`).

## Reglas duras
  - Modelos: TÚ (planner) en Opus. Implementación en subagentes con modelo
    bajo (Haiku/Sonnet según la tabla), uno por tarea.
  - Backend: TDD — test que falla primero, luego el fix mínimo.
  - Cada tarea compila y pasa tests antes de pasar a la siguiente; no romper
    los ~258 tests existentes (confirma el número exacto corriendo
    `cargo test` al arrancar, es el baseline).
  - NO romper las invariantes de CLAUDE.md (upsert_series/followed,
    set_seen_cascade gap-free, scrape_via_mirrors fallback, set_mirrors no
    pierde el base_url activo, ExecuteScript síncrono, covers de a una y solo
    followed) ni las agregadas después (NULL-status nunca oculta filas sin
    sync, ver docs/superpowers/specs/2026-07-18-hide-upcoming-releases-design.md).
  - No implementes. Al terminar el plan, páralo y pide revisión antes de
    ejecutar.
```

---

## Cómo lanzarlo
1. Abre un chat nuevo (contexto limpio) con modelo **Opus**.
2. Pega el bloque de arriba (o `/goal <bloque>`).
3. El agente analiza con el grafo → hace brainstorming contigo (ojo con los
   puntos marcados arriba, son decisiones reales, no detalles) → escribe
   `docs/feature-batch-2026-07-plan.md` → para y pide revisión.
4. Tras tu OK, ejecuta con `superpowers:subagent-driven-development` (tareas
   secuenciales, recomendado si comparten archivos como `db/catalog.rs`) o
   `superpowers:dispatching-parallel-agents` (para las claramente
   independientes: el rename de "Mi biblioteca", el fix de Hentai/Ecchi, y el
   contraste del modo claro pueden ir en paralelo sin pisarse).
