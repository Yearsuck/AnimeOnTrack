# Estado de AnimeOnTrack

Instantánea del 2026-10-31 (tarde). Se reescribe tras cada merge a `develop`, cada release y antes de compactar una
conversación (ver "Docs that travel with the code" en `AGENTS.md`). Lo que no está aquí hay que mirarlo en git.

## Publicado
- Última versión: **v0.23.0** (tag en `main`; instaladores firmados en la release, actualización automática activa).
- Historial de cambios por versión: las releases de GitHub y los commits `chore(release)`.

## En `develop` sin publicar (desde v0.23.0)
- **Exactitud de datos y estadísticas** (plan `docs/superpowers/plans/2026-10-30-data-accuracy.md`):
  - #126 números del frontend: meses del heatmap, intensidad, corte de "últimos 3 meses", progreso > 100 %, unidades de
    tiempo y decimales con coma.
  - #128 estadísticas: "pendientes" sin series «Ya lo vi», series abandonadas solo con episodios por ver y agrupadas
    por franquicia, progreso real entre sitios (máximo entre mejor sitio y episodios canónicos distintos acotados al total
    de AniList), duración 0 tratada como desconocida, años en orden. Verificado franquicia a franquicia sobre una copia
    de la base real.
  - #130 barras de progreso suaves, sync del catálogo indeterminada (el total era una estimación de 21.000), botones con
    spinner, y `refresh-progress` con campo `done` explícito.
- **Android** (plan `docs/superpowers/plans/2026-10-30-android.md`):
  - #125 la app compila para Android (plugins y watchdog de escritorio fuera, permisos en `capabilities/desktop.json`,
    proyecto `src-tauri/gen/android`).
  - #127 botón atrás del sistema (pila sobre la History API, `src/lib/backStack.ts`); en escritorio da el atrás del ratón.
  - #129 scraper en Android: plugin propio `src-tauri/plugins/webfetch` (WebView fuera de pantalla), probado en un móvil
    real (horario ~0,8 s).
  - #131 interfaz usable en móvil: márgenes del sistema nativos en `MainActivity.kt`, navegación inferior, reglas en
    `src/styles/mobile.css` limitadas a `html[data-platform="mobile"]`.

  - #133 y #136 estimaciones marcadas: «≈» y % estimado en minutos vistos, «Ya lo vi» aparte, popularidad como
    percentil del catálogo, fechas de próximo episodio extrapoladas (+7 d) con «≈», una sola cuenta atrás,
    "vistos / emitidos (plan)", duración de películas.
  - #135 temporadas: marcadores de temporada en cualquier posición del título; la reparación al arrancar saca de la
    temporada 1 las filas de temporada N (en la base real: 5 filas, comprobado sobre una copia).
  - #132 `AGENTS.md` único archivo de instrucciones, con "Docs that travel with the code".

## Abierto
- **Datos**: fechas extrapoladas por animeytx (+1 semana) sin marcar y fechas de estreno parciales de AniList
  (`FuzzyDate`): ambas necesitan una columna nueva (migración). Algunas filas reparadas quedan sin vínculo en vez de
  reenlazarse (ver PR #135).
- **Android pendiente**: scraper con la pantalla apagada (servicio en primer plano), secretos en Keystore (hoy texto
  plano fuera de Windows), OAuth de Drive por deep link, sincronización con el PC, segunda pasada de diseño móvil.
  Compilar Android en Windows necesita el Modo desarrollador (enlaces simbólicos); sin él: compilar la lib, copiar el `.so`
  a `jniLibs/arm64-v8a` y `gradlew assembleArm64Debug -x rustBuildArm64Debug`; `CARGO_TARGET_DIR` corto.
- **Snapshot del catálogo**: publicar la release `catalog-snapshot` con el archivo de `export_catalog_snapshot`.
- **Descargas no posibles**: Filemoon/Byse (captcha), Streamwish (anti-bot), Doodstream, Mega (cifrado), 1Fichier/Gofile/
  Fireload, AnimeLand.

## Cómo se publica
Ver "Git workflow" en `AGENTS.md`: versión en `package.json`, `src-tauri/Cargo.toml` (+ `Cargo.lock`) y
`src-tauri/tauri.conf.json`; el tag `vX.Y.Z` dispara `release.yml`. Siempre con el permiso explícito del autor.

## Herramientas de prueba
`node tools/sandbox/start.mjs` abre la app sobre una copia de la base real (ver `tools/sandbox/README.md`).
`AOT_DOWNLOAD_PREFER=<host>` y `AOT_CATALOG_SNAPSHOT_URL=<url>` solo para desarrollo.
