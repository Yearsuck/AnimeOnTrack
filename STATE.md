# Estado de AnimeOnTrack

Instantánea del 2026-10-31, tras publicar v0.24.0. Se reescribe tras cada merge a `develop`, cada release y antes de compactar una
conversación (ver "Docs that travel with the code" en `AGENTS.md`). Lo que no está aquí hay que mirarlo en git.

## Publicado
- Última versión: **v0.24.0** (2026-10-31; tag en `main`, instaladores firmados de Windows (.exe, .msi), macOS (.dmg) y Linux (.deb, .rpm, AppImage) y `latest.json` del updater en la release; actualización
  automática activa). Contenido en el README ("What's New in v0.24.0"): estadísticas exactas, estimaciones marcadas,
  reparación de temporadas mal enlazadas, una sola cuenta atrás, carga más fluida y la base de Android (#125-#138).
- Android: compila y funciona en un móvil real, pero **no se publica instalador** todavía.

## En `develop` sin publicar
- Nada.

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
