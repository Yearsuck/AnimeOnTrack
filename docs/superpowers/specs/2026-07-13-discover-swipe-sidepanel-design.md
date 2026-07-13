# Descubrir → panel lateral del Swipe (filtros + modo del mazo)

Fecha: 2026-07-13 · Estado: diseñado · Tarea 8 del batch 2026-07-13 (depende de las tareas 2 y 3, ya mergeadas)

## Problema

Los dos controles que gobiernan el mazo viven fuera del sitio donde se usa el mazo:

- Los **baneos** de género/formato están en una sub-pestaña `Filtros` (`FiltersView`,
  `src/views/Descubrir.tsx:945`). Cambiarlos obliga a salir del Swipe, tocar chips, pulsar
  "Guardar filtros" y volver — y ahí se pierde de vista la carta que estabas juzgando.
- El **toggle Recomendado/Aleatorio** (`DiscoverModeToggle`, línea 148) sí está en el Swipe, pero
  suelto encima de la carta, sin relación visual con los filtros, que hacen lo mismo conceptualmente:
  configurar qué sale en el mazo.

Deben vivir juntos, en un lateral de la sección de Swipe, accesibles **mientras** haces swipe.

## Contexto técnico REAL (verificado en el código)

`src/views/Descubrir.tsx`:

- `type SubView = "swipe" | "listas" | "filtros"` (línea 92). El componente raíz `Descubrir`
  (1039) pinta 3 `.tab` y monta `SwipeView` / `ListasView` / `FiltersView` según `subView`.
- `SwipeView` (175) mantiene el mazo en refs, **no en estado**:
  - `queueRef: SwipeCard[]` (prefetch local, `PREFETCH_TARGET = 10`, `REFILL_THRESHOLD = 4`,
    `MAX_FILL_ROUNDS = 5`).
  - `fillQueue()` (213) llama `discoverCatalogCard(discoverMode === "recommended")` en paralelo
    (`need` a la vez) y dedupea contra `queueRef`, `cardUrlRef` y `decidedUrlsRef`.
  - `popNext()` (253) saca la siguiente y rellena en background.
  - Efecto (294): al cambiar `discoverMode` **vacía `queueRef` y refiltra** (`queueRef.current = []; fillQueue()`),
    sin tirar la carta en pantalla y sin tocar `decidedUrlsRef`. Ese es exactamente el patrón que
    necesitan los baneos.
- `FiltersView` (945): estado local `allGenres` (de `getCatalogFacets()`), `bannedGenres`,
  `bannedFormats` (de `getDeckBans()`), botón "Guardar filtros" → `setDeckBans([...g], [...f])`.
  `DECK_FORMATS = ["TV","MOVIE","OVA","ONA","SPECIAL"]` (línea 38).
- `DiscoverModeToggle` (148) ya es **self-contained**: lee/escribe `useDiscoverMode()`
  (localStorage, `src/discoverMode.ts`) sin props — el comentario del código ya anticipa esta tarea
  ("so task 8 can relocate it into a swipe-side panel later without rewiring").

**BUG LATENTE que esta tarea debe cerrar:** hoy los baneos solo surten efecto en el mazo porque
cambiar de sub-pestaña **desmonta `SwipeView`** y al volver se remonta con la cola vacía. Con el
panel embebido en el Swipe, `SwipeView` ya no se desmonta al guardar filtros → si no se invalida la
cola, el usuario seguiría viendo hasta 10 cartas prefetcheadas con los baneos **viejos**. El
backend (`discover_catalog_card` → `random_catalog_anime_in_genre`) lee los baneos de `settings` en
cada llamada, así que basta con invalidar la cola en el cliente.

CSS existente reutilizable (`src/styles.css`): `.swipe-stage` (898, flex column centrado),
`.seg`/`.seg-btn` (525), `.chip-row`/`.chip-toggle` (296–320, con `.active` = baneado),
`.series-block`, `.btn`/`.btn-primary`. Todo tokenizado → theme-aware sin trabajo extra.

i18n (`src/i18n/catalog/es.ts`): ya existen `discover.tabSwipe`, `discover.tabLists`,
`discover.tabFilters`, `discover.filtersIntro`, `discover.filtersGenresHeading`,
`discover.filtersFormatsHeading`, `discover.filtersSave|Saving|Saved`, `discover.mode*`.
`en.ts` es `Record<keyof typeof es, string>`: cada clave nueva debe añadirse a los dos.

## Diseño

### Estructura

1. `SubView` pasa a `"swipe" | "listas"`. Se elimina la pestaña `Filtros` del `.tabs` raíz y el
   montaje de `FiltersView` como vista.
2. `SwipeView` renderiza un layout de dos columnas:
   ```
   <div className="swipe-layout">
     <aside className="deck-panel">   ← "Ajustes del mazo"
        DiscoverModeToggle  (Recomendado | Aleatorio)
        Tipos baneados      (chip-toggle x5)
        Géneros baneados    (chip-toggle, scroll interno max-height)
        [Guardar filtros]   + "Filtros guardados."
     </aside>
     <div className="swipe-stage"> …carta, acciones, hint, historial… </div>
   </div>
   ```
   `FiltersView` se refactoriza a `DeckPanel({ onBansSaved })` — mismo estado y misma llamada
   `setDeckBans`, más el `DiscoverModeToggle` dentro y el `filtersIntro` como texto de ayuda
   compacto. Deja de existir como sub-vista.
3. **Invalidación del prefetch (obligatorio):** `SwipeView` pasa
   `onBansSaved={() => { queueRef.current = []; fillQueue(); }}` y `DeckPanel` lo llama **después**
   de que `setDeckBans` resuelva. No se toca `decidedUrlsRef` (lo ya decidido sigue excluido) ni la
   carta en pantalla (misma semántica que el cambio de modo). Es el mismo cuerpo que el efecto de
   `discoverMode`: extraerlo a un `resetQueue = useCallback(...)` y usarlo en ambos sitios.

### Presentación

- `.swipe-layout`: `display:grid; grid-template-columns: minmax(240px, 280px) 1fr; gap: var(--sp-5);
  align-items:start;`. El `.swipe-stage` sigue centrado dentro de su columna.
- `.deck-panel`: tarjeta (`background: var(--surface-1)`, `border:1px solid var(--border)`,
  `border-radius: var(--radius-md)`, `padding: var(--sp-4)`), `position: sticky; top: 72px` (bajo la
  nav) para que siga visible al hacer scroll con historial largo.
- Panel **colapsable**: cabecera con `<button className="deck-panel-head" aria-expanded={open}
  aria-controls="deck-panel-body">` con título `discover.deckPanelTitle` y un chevron. Estado
  persistido en `localStorage` bajo `aot.deckPanel` (`"open"` | `"closed"`, default `open`),
  siguiendo el patrón de `src/theme.ts` / `src/discoverMode.ts` (leer en `useState` inicial,
  escribir en el setter). No hace falta un módulo nuevo: es estado local del panel.
- Lista de géneros larga (`getCatalogFacets()` devuelve todos los del catálogo AniList sincronizado):
  el `.chip-row` de géneros va en un contenedor con `max-height: 220px; overflow-y: auto;` para que
  el panel no crezca sin fin.
- Badge de recuento de baneos activos en la cabecera cuando el panel está cerrado
  (`{n}` = `bannedGenres.size + bannedFormats.size`, oculto si 0) — así se ve que hay filtros
  puestos sin abrirlo.
- **Responsive:** `@media (max-width: 900px)` → `.swipe-layout { grid-template-columns: 1fr; }` y
  `.deck-panel { position: static; max-width: 520px; margin: 0 auto; }` (el panel pasa a ir encima
  de la carta). Sin `position: fixed` ni overlays.
- Theme-aware por construcción: solo tokens CSS existentes, cero rgba hardcodeado.

### Claves i18n nuevas (es.ts + en.ts)

| clave | es | en |
|---|---|---|
| `discover.deckPanelTitle` | `Ajustes del mazo` | `Deck settings` |
| `discover.deckPanelToggleAria` | `Mostrar u ocultar los ajustes del mazo` | `Show or hide the deck settings` |
| `discover.deckPanelBansBadge` | `{count} filtros activos` | `{count} active filters` |

`discover.tabFilters` deja de usarse como pestaña; **se elimina de ambos catálogos** (si queda sin
referencias, borrarla de es.ts y en.ts a la vez para no dejar claves muertas).

## Criterios de aceptación (verificables)

1. `cargo test --manifest-path src-tauri/Cargo.toml` verde (no se toca Rust: `set_deck_bans`,
   `get_deck_bans`, `discover_catalog_card` quedan igual).
2. `npx tsc --noEmit` limpio y `npm run build` OK (garantiza paridad es/en tras borrar/añadir claves).
3. Ya no existe la sub-pestaña "Filtros": `Descubrir` solo pinta Swipe y Listas.
4. Los baneos y el toggle Recomendado/Aleatorio se ven y se usan **sin salir del Swipe**, con la
   carta en pantalla.
5. Guardar filtros **vacía la cola de prefetch y la rellena**: la siguiente carta que aparezca ya
   respeta los baneos nuevos (verificable: tras banear un género, ninguna carta posterior lleva ese
   `matched_genre`). La carta actualmente en pantalla NO se descarta.
6. Cambiar de modo sigue funcionando igual que antes del refactor (mismo `resetQueue`), y no re-pide
   cartas ya decididas (`decidedUrlsRef` intacto).
7. El estado abierto/cerrado del panel sobrevive a un recargue de la app (localStorage `aot.deckPanel`).
8. Cero scraping nuevo: `discover_catalog_card` es una lectura local de SQLite; no se añade ninguna
   llamada que toque el sitio (regla `project-scraping-scope`).

## Qué verificar en vivo (harness, no ventana Tauri)

Portar el markup exacto (`.swipe-layout` + `.deck-panel` abierto y cerrado + `.swipe-stage` con
carta, acciones e historial) y el CSS a un HTML autocontenido; servirlo con
`python -m http.server <puerto> --bind 127.0.0.1` y mirarlo con claude-in-chrome en **oscuro y
claro**, a ancho ~1400px y ~800px (comprobar el breakpoint de 900px). Comprobar: el panel no empuja
la carta fuera del centro visual, el scroll interno de géneros funciona, el chevron/badge se leen en
ambos temas, y nada desborda horizontalmente. Matar el servidor por PID al acabar.

La ventana Tauri real no es alcanzable por herramientas; el usuario relanza la app para el visto
bueno final (y para comprobar el punto 5 con cartas reales).
