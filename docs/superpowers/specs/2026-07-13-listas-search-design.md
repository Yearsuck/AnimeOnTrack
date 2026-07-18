# Descubrir → Listas: buscador por título

Fecha: 2026-07-13 · Estado: diseñado · Tarea 7 del batch 2026-07-13

## Problema

Las Listas de Descubrir (`Quiero ver` / `Descartados` / `Ya vistas`) se renderizan como una
lista completa sin ningún filtro. Con ~130 filas seguidas y ~60 `watched_externally` en la DB
real, encontrar un título concreto obliga a hacer scroll. Biblioteca y Catálogo sí tienen
buscador; Listas no.

## Contexto técnico REAL (verificado en el código)

- `src/views/Descubrir.tsx:859` `ListasView({ onOpenSeries })`:
  - Estado: `want`, `discarded`, `watched` (`Series[]`), `tab: ListTab = "want" | "discarded" | "watched"`.
  - `load()` (línea 866) hace `Promise.all([listBacklog("want"), listBacklog("discarded"), listWatchedExternally()])`.
    Las tres son lecturas locales de SQLite — **no scrapean** (ver memoria `project-scraping-scope`).
  - `tabs` (881) es un array `{ key, label, count }` que pinta el control segmentado
    `.seg.listas-seg` con `.listas-seg-count`.
  - Cada pestaña renderiza `.listas-grid` con `WantRow` / `DiscardedRow` / `WatchedRow`,
    o `<div className="empty">{t("discover.<x>Empty")}</div>` si la lista está vacía.
- `Series` (src/types.ts) tiene `id`, `title`, `cover_url`, `url`… — el filtro solo necesita `title`.
- Patrón de buscador ya existente, a reutilizar tal cual (`src/views/Library.tsx:390`):
  ```tsx
  <div className="search">
    <span className="icon" aria-hidden="true">⌕</span>
    <label htmlFor="..." className="sr-only">{t("...")}</label>
    <input id="..." className="input" placeholder={t("common.searchPlaceholder")} value={query} onChange={…} />
  </div>
  ```
  CSS ya existe (`.search` en `styles.css:266`, `.search .input` con padding-left 34px, `.search .icon`),
  es theme-aware por tokens (`--muted-2`, `.input` usa `--accent` en focus). **No hace falta CSS nuevo**
  salvo el contenedor de fila.
- i18n: `src/i18n/catalog/es.ts` ya tiene `"common.searchPlaceholder": "Buscar por nombre…"` (línea 31)
  y `library.searchAriaLabel`. Faltan claves nuevas para Listas (ver abajo). `en.ts` es
  `Record<keyof typeof es, string>`: toda clave añadida a `es.ts` debe existir en `en.ts` o `tsc` falla.

## Diseño

Filtro **100% cliente**, sin backend, sin comandos nuevos, sin scraping.

1. `ListasView` gana `const [query, setQuery] = useState("")`.
2. Normalizador local compartido en el fichero:
   ```ts
   const norm = (s: string) =>
     s.toLowerCase().normalize("NFD").replace(/[̀-ͯ]/g, "");
   ```
   (la clase del `replace` es el rango de diacríticos combinantes `U+0300`–`U+036F`; escríbela con
   escapes unicode en el código para que no dependa de la codificación del fichero)
   (insensible a mayúsculas y acentos: "shingeki" encuentra "Shingeki"; "no gou" encuentra "Nō gōu").
   Comparación por **substring** sobre el título, no por prefijo.
3. `useMemo` por lista: `fWant`, `fDiscarded`, `fWatched` = lista filtrada cuando `query.trim() !== ""`.
4. **Los contadores del control segmentado muestran los resultados filtrados** cuando hay query
   (`fWant.length`, etc.), no el total — así el usuario ve en qué pestaña están sus resultados sin
   cambiar de pestaña a ciegas. Con query vacía muestran los totales de siempre.
5. Estado vacío diferenciado: si la lista original tiene filas pero el filtro no deja ninguna,
   se muestra `t("discover.searchNoResults")` en lugar del `discover.*Empty` habitual.
6. El buscador se pinta **encima** del control segmentado, en una fila propia
   (`.listas-toolbar`: flex, `align-items:center`, `gap: var(--sp-3)`, `flex-wrap: wrap`,
   `margin-bottom: var(--sp-3)`), con el `.search` a la izquierda. Un CSS nuevo mínimo en
   `styles.css` junto a `.listas-seg` (línea ~1038), usando solo tokens (theme-aware por construcción).
7. El query **persiste al cambiar de pestaña** (es un filtro global sobre las tres listas). No se
   persiste en localStorage: es un filtro efímero, no una preferencia.
8. Accesibilidad: `<label className="sr-only" htmlFor="listas-search">`, `id` único
   (`listas-search`, distinto de `library-search`).

### Claves i18n nuevas (es.ts + en.ts, mismas claves)

| clave | es | en |
|---|---|---|
| `discover.searchAriaLabel` | `Buscar en las listas` | `Search the lists` |
| `discover.searchNoResults` | `Ningún título coincide con la búsqueda.` | `No title matches your search.` |

(`common.searchPlaceholder` se reutiliza; no se crea otra.)

## Criterios de aceptación (verificables)

1. `npx tsc --noEmit` limpio y `npm run build` OK (garantiza paridad es/en del catálogo).
2. Escribir texto en el buscador filtra las filas visibles de la pestaña activa por substring
   del título, insensible a mayúsculas/acentos.
3. Los contadores de las tres pestañas reflejan los resultados del filtro mientras hay query, y
   vuelven a los totales al vaciarlo.
4. Con query que no casa nada en una lista **no vacía**, sale `discover.searchNoResults` (no el
   texto de "lista vacía").
5. Con la lista realmente vacía y sin query, sigue saliendo el `discover.*Empty` de siempre.
6. Cambiar de pestaña conserva el texto del buscador.
7. Ninguna llamada nueva a `invoke` — no se toca `commands.rs`, no hay scraping (regla
   `project-scraping-scope`).
8. `cargo test --manifest-path src-tauri/Cargo.toml` sigue en verde (no debería tocarse Rust).

## Qué verificar en vivo (harness, no ventana Tauri)

Portar el markup exacto de `ListasView` (toolbar + `.seg.listas-seg` + 3 `.listas-card`) y el CSS
relevante a un HTML autocontenido, servirlo con `python -m http.server <puerto> --bind 127.0.0.1`
y mirarlo con claude-in-chrome en **tema oscuro y claro** (`data-theme` en `:root`): la fila
buscador+segmentado no debe romperse ni desbordar, y el icono `⌕` debe quedar centrado en el input.
Matar el servidor al terminar.

La ventana Tauri real no es alcanzable por herramientas: el usuario relanza la app para la
comprobación final.
