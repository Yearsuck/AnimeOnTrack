# Progreso de las tareas nocturnas, 2026-11-27

Dos tareas encoladas por el autor (UI/UX y arquitectura), ejecutadas en autónomo con revisión independiente
(subagentes Claude; agy sin cuota hasta ~2026-12-03), CI verde y merge a `develop`. Nada publicado.

## Tarea 1, UI/UX (`docs/design/2026-11-27-ui-audit.md`): plan de 11 PRs completo
- #172 auditoría. #175 contraste WCAG AA de tokens + `tools/contrast-check.mjs`. #176 Android oculta ajustes solo de
  escritorio. #177 eliminado el bloque CSS duplicado (56 reglas idénticas). #179 focus trap en el diálogo de atajos y
  etiquetas en ajustes. #180 dianas táctiles de 44 px con `pointer: coarse`. #181 errores visibles al guardar ajustes del
  reproductor (guardados serializados). #183 microcopy y un solo patrón de error. #185 barras animadas solo con
  `transform`. #187 estados vacíos con una acción y esqueletos que reservan tamaño. #189 literales CSS a tokens +
  `tools/check-css-tokens.mjs` en CI (baseline 36). #190 componentes `Chip` y `SettingsRow`.
- Sin verificar en un Android real; #187 sin verificación visual (estados vacíos no alcanzables con los datos).

## Tarea 2, arquitectura (`docs/architecture/2026-11-27-audit.md`)
- Hito 0: #182 sustituye el test flaky `timing_test_real_size` por comprobaciones de `EXPLAIN QUERY PLAN`
  (la suite completa pasa sin fallos). `tools/arch-check.mjs` en CI (#174).
- Hito 1: #174 `db` ya no importa `commands` ni `adapter`.
- Hito 2 (parcial): #186 `AppError` + `AppState::with_db` (lock envenenado -> error, sin pánico), comandos de biblioteca;
  #191 comandos de seguimiento + los errores de anyhow conservan el texto antiguo; #194 catálogo; #196 seen, stats y
  priority.
- Hito 3 (parcial): #184 decisión skip/fetch de `refresh` en el módulo puro `refresh_plan`; #178 el listado se persiste
  en una sola transacción.
- Hito 4 (parcial): #188 `RetryPolicy` compartida, usada en el 429 de AniList (mismos tiempos, espera tope 120 s).
- Hito 5 (parcial): tests a archivos hermanos: #193 `db/stats.rs` (7098 -> 2819 líneas), #195 `db/catalog.rs`
  (5637 -> 2189) y `db/series.rs` (4181 -> 1766).

## Pendiente
- Hito 2: resto de grupos de comandos (download, scan, sync, mirrors, storage, discover, fallback, backup...).
  Hito 3.3-3.5: puerto `SiteFetcher`, `buffer_unordered`, `ProgressSink`. Hito 4: adoptar la política en
  anizip/catalog/fallback. Hito 5: trocear el código de producción de `db/airing.rs`, `db/catalog.rs` y `db/series.rs`
  por subdominio.
