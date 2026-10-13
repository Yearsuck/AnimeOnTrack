# Estado de AnimeOnTrack

Instantánea del 2026-10-13. Actualízala al cerrar cada tarea o antes de compactar una conversación
(regla global `session-state`). Lo que no está aquí hay que mirarlo en git, no darlo por sabido.

## Publicado
- Última versión: **v0.6.4** (tag en `main`, 2026-10-12). Releases anteriores seguidas: 0.6.0 a 0.6.3.
- Contenido reciente: filtros temáticos y de emisión en listas, búsqueda de isekai en el catálogo,
  selección de monitor y ventana/pantalla completa en el reproductor.

## En `develop` sin publicar
- 3 commits por delante de `main`, todos de documentación: `AGENTS.md` pasa a ser el fichero de
  instrucciones (`CLAUDE.md` = `@AGENTS.md`) y el bloque de delegación a `agy` (PR #37, mergeado).
- No hay cambios de código sin publicar: un release ahora solo publicaría documentación.

## Abierto
- PRs abiertas: ninguna. Issues abiertas: ninguna.
- Plan `docs/superpowers/plans/2026-10-12-stats-ui-redesign.md` (rediseño de Estadísticas): hay commits
  de ese rediseño ya en `develop` (`refactor(stats)`, `fix(stats)`), pero **no verificado** si el plan
  está completo; comprobar contra el plan antes de darlo por cerrado.
- Ramas locales sin mergear en `develop`: `Yearsuck/audit-broken-features-gemini` (último commit
  2026-10-01, arreglos de stats y caché de portadas) y `backup-develop-0.5.22` (por el nombre, una copia de seguridad).
  Decidir si se borran.

## Cómo se publica
Ver la sección "Git workflow" de `AGENTS.md`: sube la versión en `package.json`,
`src-tauri/Cargo.toml` (+ `Cargo.lock`) y `src-tauri/tauri.conf.json`; el tag `vX.Y.Z` dispara
`release.yml`. Un release siempre se pide y se aprueba antes de hacerlo.
