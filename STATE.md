# Estado de AnimeOnTrack

Instantánea del 2026-11-25, con #151-#155 en `develop` sin publicar. Se reescribe tras cada merge a `develop`, cada release y antes de compactar una
conversación (ver "Docs that travel with the code" en `AGENTS.md`). Lo que no está aquí hay que mirarlo en git.

## Publicado
- Última versión: **v0.24.3** (2026-11-07): las filas de catálogo sin enlazar (`anilist-<id>`) ya no se raspan como filas del sitio (animeyt.cc devolvía episodios de otras series que acababan en Pendientes: Goblin, Magic Repo Man...); se ignoran en Pendientes y una reparación única al arrancar borra los episodios ya guardados (510 en la base del autor) tras copiar la base a `animeontrack.sqlite.before-synthetic-repair` (#148).
- Anterior: **v0.24.2** (2026-11-07): animeytx ya no guarda como pendiente el episodio sin emitir (#144).
- Antes: **v0.24.1** (2026-11-02): la temporada se lee también del slug de la página cuando el título no la lleva
  (Black Clover en animeyt; Re:Zero, Mashle, Aharen-san y Classroom of the Elite en jkanime/animeflv) y lo visto ya no se
  sincroniza entre temporadas distintas (#140).
- Antes: **v0.24.0** (2026-10-31; tag en `main`, instaladores firmados de Windows (.exe, .msi), macOS (.dmg) y Linux (.deb, .rpm, AppImage) y `latest.json` del updater en la release; actualización
  automática activa). Contenido en el README ("What's New in v0.24.0"): estadísticas exactas, estimaciones marcadas,
  reparación de temporadas mal enlazadas, una sola cuenta atrás, carga más fluida y la base de Android (#125-#138).
- Android: compila y funciona en un móvil real, pero **no se publica instalador** todavía.

## En `develop` sin publicar
- **Pendientes independiente del sitio** (#153): cambiar de sitio activo ya no cambia la lista; un anime marcado "Ya lo vi" y acabado según AniList no vuelve aunque otro sitio lo liste en emisión (Seihantai 2); los episodios más allá de lo que AniList dice emitido se ocultan al leer (no se borra nada; sin datos de AniList, sin tope).
- **"Ver" en el sitio principal + botones por web** (#155): `episode_sources` devuelve el mismo episodio en cada sitio que lo tiene, primero el activo; clic en el episodio muestra "Abrir en …".
- **Ventana nunca bajo la barra de tareas** (#151): principal y visor caben en el área útil del monitor (antes 41 px tapados a 125 %).
- **Reproductor limpio** (#152): MonosChinos (servidores en un bloque), AnimeYT (iframe perezoso visible), Gogoanime (sin pantalla completa al cargar), AnimeLand (ya no toma la sinopsis por reproductor), servidores conservan su diseño.
- **Coherencia AniList** (#154): una serie en emisión ya no cuenta como "larga" (enlazaba temporadas 2 a la 1), el traspaso de seguimiento veta ids de AniList distintos, la biblioteca usa total y estado de AniList.
- Versión propuesta: **0.25.0** (hay `feat`). Sin verificar: visor en un segundo monitor con otra escala (solo pantalla 1 por petición del autor), AnimeLand y AnimoraTV en vivo (sin episodios), vista Biblioteca en la app.
- Plan y mediciones: `docs/superpowers/plans/2026-11-25-site-independent-pending.md`.

## Abierto
- **Datos**: fechas extrapoladas por animeytx (+1 semana) sin marcar y fechas de estreno parciales de AniList
  (`FuzzyDate`): ambas necesitan una columna nueva (migración). Algunas filas reparadas quedan sin vínculo en vez de
  reenlazarse (ver PR #135).
- **Android pendiente**: scraper con la pantalla apagada (servicio en primer plano), secretos en Keystore (hoy texto
  plano fuera de Windows), OAuth de Drive por deep link, sincronización con el PC, segunda pasada de diseño móvil.
  Compilar Android en Windows necesita el Modo desarrollador (enlaces simbólicos); sin él: compilar la lib, copiar el `.so`
  a `jniLibs/arm64-v8a` y `gradlew assembleArm64Debug -x rustBuildArm64Debug`; `CARGO_TARGET_DIR` corto.
- **Snapshot del catálogo**: publicar la release `catalog-snapshot` con el archivo de `export_catalog_snapshot`.
- **Aniwaves**: las URLs de episodio guardadas dan 404 (la web cambió rutas); el adaptador necesita revisión.
- **Descargas no posibles**: Filemoon/Byse (captcha), Streamwish (anti-bot), Doodstream, Mega (cifrado), 1Fichier/Gofile/
  Fireload, AnimeLand.

## Cómo se publica
Ver "Git workflow" en `AGENTS.md`: versión en `package.json`, `src-tauri/Cargo.toml` (+ `Cargo.lock`) y
`src-tauri/tauri.conf.json`; el tag `vX.Y.Z` dispara `release.yml`. Siempre con el permiso explícito del autor.

## Herramientas de prueba
`node tools/sandbox/start.mjs` abre la app sobre una copia de la base real (ver `tools/sandbox/README.md`).
`AOT_DOWNLOAD_PREFER=<host>` y `AOT_CATALOG_SNAPSHOT_URL=<url>` solo para desarrollo.
