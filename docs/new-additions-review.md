# New additions: what was added and how to try it

Branch `feature/new-additions` (cut from `develop` after v0.8.0). Nothing here is merged into
`develop` or released: this file is the review checklist. Run it with `npm run tauri dev` (or the
debug sandbox `node tools/sandbox/start.mjs`, which uses a COPY of your data).

## Batch 1 - robustness (no visible UI)
| Change | What it does | How to check it |
|---|---|---|
| Vitest + frontend tests (`chore(test)`, `test(frontend)`) | `npm test` runs 28 tests: episode-number parsing, catalog link check, i18n interpolation and key/placeholder parity of es/en/ca. CI runs it. The unused `parseReleasedAt.ts` was removed. | `npm test` |
| AniList season relations (`feat(catalog)`) | The catalog stores each entry's PREQUEL/SEQUEL edges (metadata version 4, so the whole catalog is re-fetched once in the background after the update, ~15-20 min of low traffic). `releasing_sequel_id` follows the SEQUEL chain to the releasing TV entry instead of guessing from title prefixes. | After the background backfill, a multi-season show listed on a single-slug site (TioAnime) relinks to the airing season. Check `anilist_catalog_relations` has rows: `sqlite3 <db> "select count(*) from anilist_catalog_relations"`. |
| Failing-host back-off (`feat(scraper)`) | Two consecutive fetch failures put a host in a 5-minute cool-down (tried last, never dropped; the episode fallback skips it). | With a dead mirror in Settings, the second refresh no longer waits its timeout first. |

## Batch 2 - visible changes (done, validated)
| Change | What it does | How to check it |
|---|---|---|
| Keyboard shortcuts (`feat(shortcuts)`) | `1`-`7` switch tabs (Pendientes, En emisión, Biblioteca, Descubrir, Catálogo, Estadísticas, Ajustes), `r` refreshes, `/` jumps to the catalog search and focuses it, `?` opens a cheat sheet, `Esc` closes the sheet or the open series. Ignored while typing in an input and when ctrl/alt/meta is held. es/en/ca. | Press `?` anywhere outside a text box; press `5` then `/`; type digits in the catalog search and confirm the tab does not change. |
| Tag + studio recommendations (`feat(discover)`) | The Recomendado deck now also scores by the AniList tags (relevance >= 50) and studio of the shows you follow/want/discard, below genre and format. Cold start and Aleatorio mode are unchanged. | Descubrir -> Recomendado: with a library of e.g. isekai follows, cards sharing those tags/studios appear more often. No setting to tune yet. |

## Not done yet (needs a decision or a live check)
ani.zip numbering, AniList progress sync (needs an AniList OAuth client), Tauri updater (needs a signing
key), real download engine, taskbar badge, rotating file log.

## Batch 3 - visible changes (done, validated)
| Change | What it does | How to check it |
|---|---|---|
| Export diagnostics (`feat(diagnostics)`, `feat(settings)`) | Ajustes -> Mantenimiento -> "Exportar diagnóstico" writes one `.txt` in the app data dir (version, OS, counts, the NAMES of the settings keys with secret-looking ones removed, tails of `panic.log` and `hang.log`) and can open its folder. Never writes setting values. | Click it, open the file, confirm no token/secret appears. |
| Continue watching (`feat(pending)`) | Pendientes shows up to 3 series you already started (most recently watched first) with a Play button on the next episode link; the empty Pendientes state has a "Descubrir anime" button. | Watch/mark an episode, open Pendientes; empty the queue to see the button. |

## Batch 4 - visible changes (done, validated)
| Change | What it does | How to check it |
|---|---|---|
| New-episode toast (`feat(notify)`) | After the startup refresh and the Actualizar button, a native Windows notification says "N episodios nuevos" when the window is NOT focused. On/off in Ajustes -> General (stored in localStorage, default on). The plugin is `tauri-plugin-notification` pinned to ~2.4 so `tauri`/`wry` stay as they were. | Minimise the app, trigger a refresh that finds new episodes (or force one with new data in the sandbox). The very first notification asks Windows for permission. |
| Popular <-> cult slider (`feat(discover)`) | Descubrir options panel: a 5-step slider that biases Recomendado toward mainstream or cult titles (popularity, log scale). Centre = neutral = previous ranking. Saved when you release it. | Move it to the right and swipe a few Recomendado cards: lower-popularity titles should show up more. |

## Batch 5 - ani.zip numbering (done, validated by tests; not by live data)
| Change | What it does | How to check it |
|---|---|---|
| ani.zip episode map (`feat(episodes)`) | 30 s after start, a background job stores the per-season/absolute episode map (https://api.ani.zip) of the shows you track (20 per run). No UI. | `anilist_episode_map` table fills in the app DB. |
| Number alignment (`feat(episodes)`) | Sites that number cumulatively (TioAnime 1..27) and per season (animeyt 1..3) are converted to one canonical number before merging Pendientes, cascading "seen" and syncing progress across sites. Fixes a risk after the season relink: a watermark of 27 could mark all animeyt episodes seen. Covered by a Tensei Kizoku fixture test. | Follow the same show on two sites with different numbering; mark an episode seen on one; the other must advance to the same real episode, not further. |

## Batch 6 - visible changes (done, validated by tests)
| Change | What it does | How to check it |
|---|---|---|
| Next episode air date (`feat(episodes)`, `feat(detail)`) | The series detail screen shows "Próximo episodio: Ep N · en X días/h/min" under the info row, from the ani.zip air dates stored in the DB. Nothing is shown when the series has no map yet or nothing upcoming. | Open a followed airing series that has been linked to AniList (the map fills in ~30 s after start, 20 shows per run). |
| Rotating app log (`feat(log)`) | `aot.log` in the app data dir (1 MB, 2 rotated copies) records startup and panics; its tail is part of Ajustes -> Mantenimiento -> Exportar diagnóstico. No setting values are logged. | Start the app, open the data dir, read `aot.log`; export diagnostics and look for the `aot.log` section. |

## Batch 7 - owner feedback round (done, validated by tests/build; UI not seen on screen)
| Change | What it does | How to check it |
|---|---|---|
| Continuar viendo = last 2 | The strip in Pendientes shows exactly the two series you watched most recently (that still have a next episode). | Mark an episode seen in two series; both appear, newest first. |
| AniList page in our window | "Ver en AniList" in the series detail opens an app window, not your browser. | Open any series -> click the link. |
| Descubrir filters apply on their own + reset | Chips, formats, dates, score and "ocultar próximos" save 600 ms after the last change (the deck is refilled); new "Restablecer filtros" button clears all filters and puts the popular/cult slider back to the centre. | Toggle a genre: the deck changes without pressing Guardar; press Restablecer filtros. |
| Maximised episode window | Ajustes -> Reproductor: the second mode is now "Ventana a pantalla completa" (maximised window, keeps the title bar and X) instead of true fullscreen. | Set it, open an episode: window fills the screen and can be closed. |
| Faster navigation | The 3D graph code (three.js, 1.4 MB) loads only when Estadísticas is first opened (prefetched on hover); Catálogo caches its facets between visits; Descubrir shows its first card before filling the whole buffer; dependencies (SQLite) are optimised in dev builds. | Click Descubrir -> Catálogo -> Estadísticas -> repeat; compare with the previous build. |
| Catálogo -> Estadísticas rápido (`fix(catalog)`) | Leaving Catálogo now cancels a load that was still paging (it kept the DB busy and froze Estadísticas). Returning to Catálogo restores at most 4 pages (120 cards) instead of rebuilding every page you had scrolled through; "Cargar más" brings the rest. | Scroll Catálogo far down (15+ pages), go to Estadísticas and back: both should answer at once; the list restarts from 120 cards. |
