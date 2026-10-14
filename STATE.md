# Estado de AnimeOnTrack

Instantánea del 2026-10-14 (release 0.9.0). Actualízala al cerrar cada tarea o antes de compactar una conversación
(regla global `session-state`). Lo que no está aquí hay que mirarlo en git, no darlo por sabido.

## Publicado
- Última versión: **v0.9.0** (tag en `main`, 2026-10-14; instaladores en la release). Trae todo lo de la antigua rama
  `feature/new-additions` (descargas offline, snapshot de catálogo, rastreo AniList ~3 min, fluidez, UI).
- Anterior: **v0.8.0** (2026-10-13).
- Contenido de 0.8.0: AniList como fuente de verdad (títulos, portadas y metadatos), soporte del relanzamiento
  de animeyt.cc, prioridad entre sitios, Pendientes que fusiona sitios por número de episodio, episodios que
  faltan traídos de otro sitio y relink a la temporada en emisión. PR #44.
- Anteriores seguidas: 0.7.0 (filtros de catálogo, listas de Descubrir, estadísticas rediseñadas), 0.6.x.

## En `develop` sin publicar
- Nada: `develop` y `main` coinciden con 0.9.0.

## Contenido de 0.9.0 (antes en `feature/new-additions`, fusionada el 2026-10-14 con aprobación del usuario) La lista completa de cambios, cómo probarlos y qué se validó está en
`docs/new-additions-review.md` (Batches 0 a 15); el plan en `docs/superpowers/plans/2026-10-14-*.md`.
Resumen:
- **Descargas offline** por serie (panel con "descargar pendientes" y "borrar descargas" sin tocar lo visto),
  en TioAnime, JKAnime, animeyt.cc, GogoAnime y el AnimeFLV nuevo; hosts: Mp4upload, YourUpload, Mediafire,
  Streamtape, Mixdrop, Vidhide, VOE, Ok.ru, mp4 directo y un descargador HLS. Cada archivo pasa por una
  cuarentena (contenedor de vídeo real, Defender, Mark of the Web, carpeta sin permiso de ejecución).
- **Primer arranque rápido**: la instalación nueva importa un snapshot gzip del catálogo (22.600 títulos en ~4 s
  en vez de 15+ min de rastreo). **Falta publicarlo** (ver Abierto).
- **Rastreo AniList**: 10 páginas por petición + pausa que descuenta la latencia + 1940-1999 por décadas: 22.619
  títulos en ~3 min (antes 15+).
- **Fluidez**: comandos fuera del hilo de la UI, grafo 3D con carga diferida, Catálogo que no reconstruye decenas
  de páginas, SQLite optimizado también en dev.
- **AnimeFLV** en su nueva dirección `animeflv.or.at` (otro sitio, adaptador reescrito) con migración de las URLs
  guardadas.
- **UI**: filtros de Descubrir que se aplican solos, deslizador popular/culto, "Empezar a ver" desde el
  catálogo, atajos de teclado, aviso de nuevos episodios, fecha del próximo episodio (ani.zip), ventana
  maximizada para el reproductor, bloqueo de anuncios y pop-ups en las ventanas propias.
- **Robustez**: alineado de números entre sitios con ani.zip, breaker de dominios caídos, exportar diagnóstico y
  `aot.log` rotativo, tests de frontend con Vitest.
- Comprobaciones al último commit: 904 tests de Rust, 87 de frontend, clippy y `tsc` limpios, `npm run build` ok.

## Abierto
- **Publicar el snapshot del catálogo**: crear una release con tag `catalog-snapshot` y adjuntar el archivo que
  genera el comando `export_catalog_snapshot` (re-exportarlo desde la app real, no desde el sandbox). Hasta
  entonces las instalaciones nuevas usan el rastreo largo.
- **No hecho / no posible** en descargas: Filemoon/Byse (captcha), Streamwish (anti-bot), Doodstream
  (inalcanzable), Mega (cifrado), 1Fichier/Gofile/Fireload, AnimeLand (redirecciones a páginas falsas), y el HLS en
  MP4 fragmentado de zilla-networks (primer servidor de AnimeFLV; el usuario dijo que no hace falta).
- **No probado de extremo a extremo**: una descarga completa de Ok.ru (su CDN va a ~0,25 MB/s), una detección real de
  Defender (haría falta el archivo EICAR), el alineado de números con tus datos reales (la fila de TioAnime de
  Tensei Kizoku está como visto externamente) y ninguna pantalla nueva en el tema claro.
- **Idea vista y no hecha**: bloquear que una ventana de episodio navegue a otro dominio distinto del que se abrió
  (AnimeLand acabó en una página falsa de "permitir notificaciones"); puede romper enlaces legítimos.
- Pendiente de revisar de 0.7.0 (sin cambios): tema claro de Estadísticas y Listas, "Abrir serie" del menú del
  grafo, errores de acciones sueltas de las listas, barra de géneros de Listas (solo 10), pestañas sin flechas.
- Regla de "En emisión" a vigilar: una fila no seguida sin entrada de catálogo desaparece si su última fecha
  scrapeada tiene más de 21 días (`STALE_AIRING_SECS` en `db/airing.rs`).
- `package-lock.json` sigue en `0.5.9` (no lo toca el flujo de release); decidir si se sincroniza.
- Ramas locales: además de `Yearsuck/audit-broken-features-gemini` y `backup-develop-0.5.22`, hay muchas ramas hijas
  `feature/na-*`, `fix/na-*` y `chore/na-*` ya fusionadas en `feature/new-additions`; borrarlas cuando se
  apruebe la rama. Las carpetas vacías de `.worktrees/` (Windows no las borra mientras una terminal las tenga
  abiertas) se pueden borrar a mano.
- PRs e issues abiertas: ninguna.

## Cómo se publica
Ver la sección "Git workflow" de `AGENTS.md`: sube la versión en `package.json`,
`src-tauri/Cargo.toml` (+ `Cargo.lock`) y `src-tauri/tauri.conf.json`; el tag `vX.Y.Z` dispara
`release.yml`.

## Herramientas de prueba
`node tools/sandbox/start.mjs` abre la app sobre una copia de la base real (ver `tools/sandbox/README.md`).
El sandbox tiene su propia carpeta de portadas: las portadas `file:` de la app real salen como iniciales
hasta copiarlas allí. Capturas por CDP: ojo con `devicePixelRatio` (1,25) al calcular coordenadas de clic.
`AOT_DOWNLOAD_PREFER=<host>` (solo desarrollo) hace que la descarga pruebe primero los mirrors de ese host
(`voe`, `mixdrop`, `vidhide`, `okru`, `mp4upload`...); `AOT_CATALOG_SNAPSHOT_URL=<url>` cambia de dónde se
baja el snapshot del catálogo.
