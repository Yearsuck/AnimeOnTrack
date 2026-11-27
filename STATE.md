# Estado de AnimeOnTrack

Instantánea del 2026-11-27 14:00 (v0.27.0 publicada). Se reescribe tras cada merge a `develop`, cada release y antes de compactar una
conversación (ver "Docs that travel with the code" en `AGENTS.md`). Lo que no está aquí hay que mirarlo en git.

## Publicado
- Última versión: **v0.27.0** (2026-11-27): En emisión ordenada por puntuación de AniList, portadas de AnimeFLV y series terminadas fuera de En emisión (#169, #170); webs de cada episodio como botones en Pendientes (#168); auditoría UI/UX aplicada (contraste WCAG AA, objetivos táctiles de 44 px, foco atrapado, estados vacíos y de carga, ajustes solo de escritorio ocultos en Android, errores al guardar ajustes; #175-#190); refactors de arquitectura sin cambio de comportamiento (#174-#198). 14 archivos en la release.
- Anterior: **v0.26.0** (2026-11-26): sincronización entre dispositivos por carpeta o Google Drive (fusión campo a campo con stamps ordenados, sin subir si no cambia; #163, #164); Android: escaneo con la pantalla apagada (servicio en primer plano, #162) que pide una vez el permiso de notificaciones (#165), episodios en el navegador del sistema (#160), secretos de Drive en el Keystore (#161). 14 archivos en la release.
- Anterior: **v0.25.0** (2026-11-25): Pendientes ya no cambia al cambiar de sitio activo (#153); "Ver" abre el sitio principal y clic en el episodio muestra un botón por web (#155); ventana y visor nunca bajo la barra de tareas (#151); reproductor limpio en MonosChinos, AnimeYT, Gogoanime y AnimeLand (#152); coherencia AniList en enlaces, traspasos de seguimiento y biblioteca (#154). 14 archivos en la release (Windows, macOS, Linux, `latest.json`).
- Anterior: **v0.24.3** (2026-11-07): las filas de catálogo sin enlazar (`anilist-<id>`) ya no se raspan como filas del sitio (animeyt.cc devolvía episodios de otras series que acababan en Pendientes: Goblin, Magic Repo Man...); se ignoran en Pendientes y una reparación única al arrancar borra los episodios ya guardados (510 en la base del autor) tras copiar la base a `animeontrack.sqlite.before-synthetic-repair` (#148).
- Antes: **v0.24.2** (2026-11-07): animeytx ya no guarda como pendiente el episodio sin emitir (#144).
- Antes: **v0.24.1** (2026-11-02): la temporada se lee también del slug de la página cuando el título no la lleva
  (Black Clover en animeyt; Re:Zero, Mashle, Aharen-san y Classroom of the Elite en jkanime/animeflv) y lo visto ya no se
  sincroniza entre temporadas distintas (#140).
- Antes: **v0.24.0** (2026-10-31; tag en `main`, instaladores firmados de Windows (.exe, .msi), macOS (.dmg) y Linux (.deb, .rpm, AppImage) y `latest.json` del updater en la release; actualización
  automática activa). Contenido en el README ("What's New in v0.24.0"): estadísticas exactas, estimaciones marcadas,
  reparación de temporadas mal enlazadas, una sola cuenta atrás, carga más fluida y la base de Android (#125-#138).
- Android: compila y funciona en un móvil real, pero **no se publica instalador** todavía.

## En `develop` sin publicar
- Arquitectura (sin cambio de comportamiento): comandos de backup, diagnóstico y descargas con `with_db` (#201, #204); tests de `db/airing.rs` y `db/episodes.rs` a archivos hermanos (#202); `arch-check` ya no avisa por el tamaño de los tests (#203).
- Sin verificar de v0.26.0: sincronización entre dos dispositivos reales y con Drive real (probada en sandbox con carpeta). Sin verificar de v0.25.0: visor en un segundo monitor con otra escala, AnimeLand y AnimoraTV en vivo.

## Abierto
- **Datos**: fechas extrapoladas por animeytx (+1 semana) sin marcar y fechas de estreno parciales de AniList
  (`FuzzyDate`): ambas necesitan una columna nueva (migración). Algunas filas reparadas quedan sin vínculo en vez de
  reenlazarse (ver PR #135).
- **Android pendiente**: OAuth de Drive (necesita un cliente Android de Google Cloud que crea el autor), segunda pasada
  de diseño móvil (en Android salen ajustes solo de escritorio: pantalla y modo ventana del reproductor), prueba de
  sincronización emulador+PC. Seguimientos: wake lock de 30 min, series duplicadas por clave título vs `al:N` entre
  dispositivos, archivo de sincronización duplicado en Drive no se borra.
  Compilar Android en Windows necesita el Modo desarrollador (enlaces simbólicos); sin él: compilar la lib, copiar el `.so`
  a `jniLibs/arm64-v8a` (o `x86_64` para el emulador `aot_test`) y `gradlew assembleArm64Debug -x rustBuildArm64Debug`; `CARGO_TARGET_DIR` corto.
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
