# Estado de AnimeOnTrack

Instantánea del 2026-10-13. Actualízala al cerrar cada tarea o antes de compactar una conversación
(regla global `session-state`). Lo que no está aquí hay que mirarlo en git, no darlo por sabido.

## Publicado
- Última versión: **v0.7.0** (tag en `main`, 2026-10-13; instaladores de Windows, macOS y Linux en la release).
- Contenido de 0.7.0: filtros del catálogo (puntuación, fecha, estado, orden) y "ocultar vistos" rápido
  (18 s -> 0,1 s), listas de Descubrir rediseñadas, mazo de swipe sin el falso "no hay más", Estadísticas
  rediseñadas (heatmap anual, grafo 3D con clic izquierdo/derecho, portadas de géneros), "En emisión" sin
  OVAs/specials/series paradas, Ajustes en pestañas. PRs #39 a #42.
- Anteriores seguidas: 0.6.0 a 0.6.4 (descargas offline, banners de AniList, selector de monitor del reproductor).

## En `develop` sin publicar
- Nada de código: `develop` y `main` coinciden con 0.7.0 salvo este commit de documentación.

## Abierto
- Decisión pendiente del usuario: si el siguiente release es **1.0.0** (por semver 0.7.0 ya era el mínimo;
  1.0.0 es declarar la app estable). Un release siempre se pide y se aprueba antes de hacerlo.
- Sin verificar de 0.7.0: tema claro de los estilos nuevos de Estadísticas y Listas; "Abrir serie" desde el
  menú del grafo no se probó hasta entrar en la serie; las acciones sueltas de las listas no muestran error
  si fallan; la barra de géneros de Listas solo ofrece los 10 más frecuentes; las pestañas de Listas y
  Ajustes no navegan con flechas.
- Regla de "En emisión" a vigilar: una fila no seguida sin entrada de catálogo desaparece si su última fecha
  scrapeada tiene más de 21 días (`STALE_AIRING_SECS` en `db/airing.rs`).
- Plan `docs/superpowers/plans/2026-10-12-stats-ui-redesign.md`: PR #41 cubre gran parte (heatmap, KPIs,
  grafo), pero **no verificado** punto por punto contra el plan; comprobar antes de darlo por cerrado.
- `package-lock.json` sigue en `0.5.9` (no lo toca el flujo de release); decidir si se sincroniza.
- Ramas locales sin mergear: `Yearsuck/audit-broken-features-gemini` (último commit 2026-10-01) y
  `backup-develop-0.5.22` (por el nombre, una copia de seguridad). Decidir si se borran.
- Carpeta residual `.worktrees/agy-airing` (no es un worktree; Windows no la deja borrar mientras una
  terminal la tenga abierta): borrarla a mano.
- PRs e issues abiertas: ninguna.

## Cómo se publica
Ver la sección "Git workflow" de `AGENTS.md`: sube la versión en `package.json`,
`src-tauri/Cargo.toml` (+ `Cargo.lock`) y `src-tauri/tauri.conf.json`; el tag `vX.Y.Z` dispara
`release.yml`.

## Herramientas de prueba
`node tools/sandbox/start.mjs` abre la app sobre una copia de la base real (ver `tools/sandbox/README.md`).
El sandbox tiene su propia carpeta de portadas: las portadas `file:` de la app real salen como iniciales
hasta copiarlas allí. Capturas por CDP: ojo con `devicePixelRatio` (1,25) al calcular coordenadas de clic.
