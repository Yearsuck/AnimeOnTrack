# Prompt: agente planificador de reorganización del repo

Pega el bloque de abajo en un **chat/agente nuevo** (contexto limpio) o úsalo como
`/goal`. El planner debe correr en **Opus**. Solo planifica: no toca código.

Modelos de implementación (los fija el plan, no este agente):

| Tipo de tarea | Modelo | Por qué |
|---|---|---|
| Split mecánico: mover símbolos a módulos, re-exportar, partir tests | **Haiku 4.5** | Cero decisiones de diseño; spec cerrada |
| Descomponer vistas (Descubrir.tsx → componentes/hooks), fronteras de módulo con algo de juicio | **Sonnet** | Requiere criterio acotado |

---

```
GOAL: Diseñar un plan estructurado y por fases para reorganizar la arquitectura
del repo AnimeOnTrack. SOLO el plan — NO implementar nada de código todavía.

## Contexto
Tauri v2 + Rust (src-tauri/) + React/TS (src/). App Windows que trackea anime
scrapeando un sitio tras Cloudflare vía WebView2. Lee CLAUDE.md ENTERO antes de
empezar: contiene las invariantes que el refactor NO puede romper.

El repo creció sin arquitectura. God-files reales a atacar (líneas):
  - src-tauri/src/db.rs         4495  (toda la capa DB + tests, un solo archivo)
  - src-tauri/src/commands.rs   2861  (todos los comandos Tauri juntos)
  - src/views/Descubrir.tsx     1196  (vista monolítica: UI + estado + lógica)
  - secundarios: anilist.rs 569, StatsGraph.tsx 568, Library.tsx 560, Catalog.tsx 425

## Fase 1 — Análisis (barato: el grafo YA está indexado en codebase-memory-mcp)
Usa el grafo, NO leas archivos enteros:
  - get_architecture(aspects=['all']) para el mapa general
  - search_graph / trace_path sobre db.rs y commands.rs para ver responsabilidades
    mezcladas, acoplamiento y qué llama a qué
  - Identifica: dominios que conviven en un mismo archivo, ciclos, duplicación,
    fronteras naturales de módulo

## Fase 2 — Diseño del plan
1. Invoca superpowers:brainstorming PRIMERO para fijar objetivos y criterios de
   "hecho" con el usuario antes de escribir el plan.
2. Luego superpowers:writing-plans para redactar el plan.
3. Propón la estructura de módulos objetivo, p.ej.:
   - Rust: partir db.rs por dominio (series / episodes / settings / sources / stats),
     tests junto a cada módulo; partir commands.rs por feature (scan, follow, seen,
     backup, discover...). Mantener una fachada pública estable.
   - Frontend: descomponer Descubrir.tsx en componentes + hooks + lógica pura.

## Formato de salida
Escribe el plan en docs/refactor-plan.md:
  - Fases ordenadas por riesgo y dependencia; cada fase independiente y mergeable sola.
  - Por CADA tarea de implementación, spec cerrada:
      * archivos exactos a crear/mover, y las fronteras del split (qué símbolo va dónde)
      * qué se mueve tal cual y qué NO se toca
      * criterios de aceptación: cargo build + cargo test + npx tsc --noEmit +
        npm run build en verde, y CERO cambios de comportamiento
      * tamaño estimado
      * MODELO asignado: Haiku para splits mecánicos, Sonnet si hay juicio de diseño
  - Objetivo: que un subagente con MODELO BAJO (Sonnet o Haiku) ejecute cada tarea
    SIN ambigüedad — cero decisiones de diseño durante la implementación.

## Reglas duras
  - Modelos: TÚ (planner) en Opus. La implementación va en subagentes con modelo
    bajo (Haiku/Sonnet según la tabla), uno por tarea, con la spec de arriba.
  - Refactor = mover/dividir/renombrar, NUNCA reescribir lógica ni cambiar comportamiento.
  - Cada fase compila y pasa tests antes de pasar a la siguiente.
  - NO romper las invariantes de CLAUDE.md, en concreto:
      * upsert_series excluye `followed` del ON CONFLICT (el scan no des-sigue nada)
      * set_seen_cascade: visto gap-free (marca anteriores, desmarca posteriores)
      * scrape_via_mirrors cae al siguiente mirror en fallo de fetch O parse vacío
      * set_mirrors no deja caer el base_url activo de la lista
      * ExecuteScript solo síncrono (nunca await de promesas en eval())
      * covers de a una imagen y solo para series followed
  - No implementes. Al terminar el plan, páralo y pide revisión antes de ejecutar.
```

---

## Cómo lanzarlo
1. Abre un chat nuevo (contexto limpio) con modelo **Opus**.
2. Pega el bloque de arriba (o `/goal <bloque>`).
3. El agente analiza con el grafo → hace brainstorming contigo → escribe
   `docs/refactor-plan.md` → para y pide revisión.
4. Tras tu OK, ejecuta el plan con `superpowers:subagent-driven-development`
   (tareas secuenciales) o `superpowers:dispatching-parallel-agents` (fases
   independientes en paralelo), asignando Haiku/Sonnet según la tabla.
