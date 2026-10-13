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
key), real download engine, Windows toast notifications, taskbar badge, rotating file log.

## Batch 3 - visible changes (done, validated)
| Change | What it does | How to check it |
|---|---|---|
| Export diagnostics (`feat(diagnostics)`, `feat(settings)`) | Ajustes -> Mantenimiento -> "Exportar diagnóstico" writes one `.txt` in the app data dir (version, OS, counts, the NAMES of the settings keys with secret-looking ones removed, tails of `panic.log` and `hang.log`) and can open its folder. Never writes setting values. | Click it, open the file, confirm no token/secret appears. |
| Continue watching (`feat(pending)`) | Pendientes shows up to 3 series you already started (most recently watched first) with a Play button on the next episode link; the empty Pendientes state has a "Descubrir anime" button. | Watch/mark an episode, open Pendientes; empty the queue to see the button. |
