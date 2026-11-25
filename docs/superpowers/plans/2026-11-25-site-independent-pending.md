# Pendientes independiente del sitio, reproductor limpio, barra de tareas y coherencia AniList

Fecha: 2026-11-25. Estado: **propuesto, sin código**. Basado en 3 informes de investigación (agy) y
pruebas en el sandbox (`tools/sandbox`, copia de la base real, nunca la app instalada).

## Lo verificado

### Pendientes y cambio de sitio
- `list_pending` ya fusiona todos los sitios por AniList (`db/airing.rs:334`, sin filtro de sitio);
  `source_id` solo es una puerta (`commands/library.rs:23`, `get_source_id` → error si el sitio
  nuevo aún no tiene fuente).
- Sandbox: TioAnime → AnimeYT → AnimeFLV con v0.24.3. Pendientes estable salvo **Seihantai na Kimi
  to Boku 2nd Season**, que aparece (7-13 tras AnimeYT, 13 tras AnimeFLV) aunque está marcado
  "Ya lo vi" en 5 de sus 6 filas. Causa: la fila de TioAnime sale como `is_airing=1` (la web la
  lista en emisión, AniList dice `FINISHED`), y `reconcile_completion_across_sites`
  (`db/library.rs:281`) no propaga "Ya lo vi" a una fila en emisión con progreso. Pendientes
  decide `watched_externally` **por fila** (`airing.rs:453`), mientras que
  `shows_needing_episode_source` lo decide **por grupo**: incoherente.
- "Ver" abre la URL ganadora por `site_priority`, no por el sitio activo (`Pending.tsx:86`).
- `sibling_episodes` (`db/episodes.rs:527`) ya resuelve el mismo episodio en cada sitio con la
  numeración alineada; no está expuesto a la UI.
- Pendientes no limita los episodios a lo que AniList dice que ha salido. `anilist_episode_map`
  puede ir por detrás de `anilist_catalog.next_episode` (Goblin: mapa 7 emitidos, catálogo
  next=9), así que el tope debe ser `max(emitidos_mapa, next_episode-1)`, o `episodes` si `FINISHED`.

### Barra de tareas de Windows
- Medido en el sandbox (1536×864 lógicos, 125 %, área útil 816): **maximizada = 816, no la tapa**
  (el informe que culpaba a `WM_GETMINMAXINFO` estaba equivocado). **Restaurada = 1280×832 en y=25,
  acaba en 857: 41 px detrás de la barra.** La causa es el tamaño por defecto (`tauri.conf.json`
  832 de alto) mayor que el área útil, sin ajuste al arrancar.
- El visor (`player.rs:156-160`) centra con `m.size()` (monitor entero) en vez del área útil.
  Tauri 2.11 tiene `Monitor::work_area()`.

### Reproductor limpio (prueba en vivo, una URL real por web)
| Web | Resultado |
|---|---|
| TioAnime, AnimeFenix, AnimeFLV, JKAnime | OK: reproductor + servidores, nada más |
| AnimeYT | Vídeo OK, **sin selector de servidores** |
| MonosChinos | Funciona, pero los 7 servidores se apilan en vertical (1/3 de la pantalla): `display:block` forzado (`clean_player.rs:367`) |
| Gogoanime | **El iframe ocupa toda la ventana y tapa la barra de título** |
| Aniwaves | **404**: la URL guardada ya no existe (problema del adaptador, no del recorte) |
| AnimeLand, AnimoraTV | Sin episodios en la base: no probado en vivo. AnimeLand usa `.video_wrapper_container`, que en la web es la sinopsis (`adapter/animeland.rs:37`) |

### Coherencia AniList (verificado en código)
- `is_long_runner` devuelve `true` para cualquier anime en emisión (`episodes` NULL) → un "Serie: Subtítulo"
  en emisión puede enlazarse a la temporada 1 (`db/catalog.rs:454`).
- El traspaso de seguimiento al cambiar de sitio empareja solo por título, sin mirar `anilist_id`
  (`db/series.rs:565`, `commands/follow.rs:46`).
- Biblioteca: total = episodios raspados y "completado" usa `is_airing` del sitio, no el estado de AniList
  (`db/series.rs:1033`, `Library.tsx:80`).
- Pendientes y la ficha muestran el número crudo del sitio ("27") aunque `RowAlign` sabe que es el 3.

## Propuesta (un PR por bloque, TDD, cada commit compila y pasa)

### PR 1 — `fix(window)`: nunca detrás de la barra de tareas
1. Al arrancar, si la ventana principal no cabe en `work_area()` del monitor, reducirla y recolocarla dentro.
2. Visor: centrar y dimensionar con `work_area()` en vez de `size()`.
3. Test: función pura `fit_to_work_area(win, work) -> rect` con casos 1080p@125 %, 768p, multi-monitor.
4. Verificación: sandbox, mismas medidas de arriba (bottom ≤ availHeight).

### PR 2 — `fix(pending)`: Pendientes no cambia al cambiar de sitio
1. "Ya lo vi" por anime: si alguna fila del grupo AniList tiene `watched_externally` y AniList dice
   `FINISHED`, el grupo no entra en Pendientes (respeta el guard de One Piece: en emisión no se fuerza).
2. Tope AniList en lectura (no se borra nada): descartar episodios canónicos > `max(emitidos_mapa,
   next_episode-1)`; si `FINISHED`, > `episodes`. Sin datos de AniList → sin tope (como hoy).
3. Quitar la puerta `get_source_id` de `list_pending`/`pending_count` (no usan el valor).
4. Tests: Seihantai (fila en emisión sin flag + filas con flag + AniList FINISHED → fuera), episodio
   no emitido según AniList → fuera hasta su hora, cambio de sitio sin fuente → lista igual.
5. Verificación: repetir TioAnime → AnimeYT → AnimeFLV en el sandbox; Pendientes idéntico antes/después.

### PR 3 — `feat(pending)`: "Ver" en el sitio principal + botones por web
1. Comando `episode_sources(episode_id)` sobre `sibling_episodes`: lista {sitio, url} del mismo episodio.
2. "Ver" abre la copia del sitio activo si existe; si no, la de mayor prioridad (como hoy).
3. Clic en el episodio → botones "Abrir en TioAnime / AnimeFLV / …" solo de las webs que lo tienen. i18n ca/es/en.
4. Búsqueda en todas las webs: ya la hacen `spawn_episode_fallback` y la importación tras cada escaneo;
   no se añade búsqueda nueva en este PR.

### PR 4 — `fix(clean-player)`
1. Gogoanime: el iframe no puede cubrir la barra ni los servidores.
2. Servidores: no forzar `display:block`; conservar el layout (flex) de la web.
3. AnimeYT: selector real del cambiador de servidores (capturar fixture con varias fuentes).
4. AnimeLand: quitar `.video_wrapper_container` / `.video_wrapper` de los selectores de reproductor.
5. `tools/sandbox/clean-check.mjs`: abre un episodio por web y mide reproductor/servidores/restos (+ captura).

### PR 5 — `fix(anilist)`: coherencia
1. Mostrar el número canónico (AniList) en Pendientes y ficha; la URL sigue siendo la del sitio.
2. `is_long_runner`: NULL no significa largo; usar `next_episode`/mapa (> 60).
3. Traspaso de seguimiento: vetar emparejar filas con `anilist_id` distintos.
4. Biblioteca: total y "completado" según AniList (`episodes`, `status`) cuando hay enlace.

### Aparte (investigar antes de proponer)
- Aniwaves: URLs de episodio dan 404; el adaptador probablemente necesita actualizarse.
- Probar AnimeLand y AnimoraTV en vivo cuando haya episodios.

## Fuera de alcance
Migraciones destructivas: todo es filtro en lectura o flags; ninguna fila se borra ni se reescribe.
