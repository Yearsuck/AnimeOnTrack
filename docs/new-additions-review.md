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
| UI no longer freezes while data loads (`perf(commands)`, `perf(catalog)`, `chore(build)`) | Every Tauri command now runs off the UI thread (they used to run on it, so any wait for the DB froze the window). The startup catalog linking builds its index outside the DB lock (it used to block every query for ~6 s after launch). Dev builds optimise our code and dependencies. Measured in the dev sandbox: six heavy stats/catalog queries in parallel -> max UI stall 13 ms; during the startup linking the slowest `pending_count` was 420 ms (before: 2.9 s, and 6 s without the lock split). | Open the app and switch tabs right away: it should respond even while the badge/lists are still loading. |

## Batch 8 - owner feedback round 2 (done, validated; ad blocker and Start tested live in the sandbox)
| Change | What it does | How to check it |
|---|---|---|
| Aleatorio hides the slider | In the Descubrir options panel the popular/cult slider only shows in Recomendado (it has no effect in Aleatorio). Filters stay visible in both modes. | Switch Recomendado <-> Aleatorio. |
| Start watching from Catálogo | A ▶ button on each catalog card (top-left) and an "Empezar a ver" batch button in the selection bar: the card is added as wanted and started on the active site, one scrape at a time, so it lands in Pendientes (the badge refreshes). A title the site does not have reports "no se encontró" and stays in Quiero ver. Live test in the sandbox: Attack on Titan S2 -> "1 en marcha", Pendientes 71 -> 83, no episode marked seen. | Click ▶ on a card; watch the message under the filters and the Pendientes badge. |
| Ad blocking in our windows (`feat(player)`) | Episode and AniList windows cancel requests to a conservative list of ad/tracker/pop-under networks and deny pop-up windows. Scraper windows are untouched. Live test: a script from googlesyndication.com loads in the main window and errors in a viewer window; `window.open` is denied; jsdelivr and anilist.co still load. The list is in `src-tauri/src/ad_block.rs`. | Open an episode on a site with ads; if a player breaks, tell me which host to take off the list. |

## Batch 9 - fast first run + downloads (done; tested live in the sandbox)
| Change | What it does | How to check it |
|---|---|---|
| Removed | The ▶ button on catalog cards and the "Continuar viendo" strip in Pendientes are gone. Start-from-catalog stays in the selection bar only. | - |
| New-install catalog in seconds (`feat(catalog)`) | A fresh install (catalog under 1000 rows, no completed full sync) downloads ONE gzip snapshot (2.8 MB for 22,602 titles) and imports it in chunks, then marks the full sync done so only the incremental sync remains. Falls back to the old 15-minute crawl if the download or the file fails. Live test in the sandbox with the catalog emptied: 22,602 titles + 109,188 tags imported in 3.9 s. | Needs the snapshot to be published (see below); to try it locally: `export_catalog_snapshot` command writes `catalog-snapshot.json.gz` in the app data dir, serve it and start with `AOT_CATALOG_SNAPSHOT_URL=<url>`. |
| Episode downloads, first version (`feat(download)`) | In the series detail, ⬇ downloads the episode as an mp4 to `<app data>/downloads/<series>/<episode>.mp4` with live percent, cancel (✕), play offline (opens the system player) and delete (🗑); failures show the reason as the button tooltip. Only TioAnime episode pages and YourUpload mirrors work so far. Live test: Death Note 1 -> valid 83.9 MB mp4; an episode whose YourUpload link is dead reports "YourUpload answered HTTP 404". Downloaded files are deleted when the episode is marked seen (existing behaviour). | Series detail of a TioAnime series -> ⬇ on an episode. |

### Needs the owner
- **Publish the catalog snapshot**: a new install downloads `https://github.com/Yearsuck/AnimeOnTrack/releases/download/catalog-snapshot/catalog-snapshot.json.gz`. Create a release tagged `catalog-snapshot` and attach the file produced by `export_catalog_snapshot` (re-export now and then so it stays recent). Until then new installs simply use the old crawl.
- Other sites' downloads need the same mirror research (animeyt.cc, AnimeFLV, JKAnime...) and more hosts (Okru, Mp4Upload...); HLS streams are not supported (no ffmpeg).

## Batch 10 - downloads made easier (done; tested live in the sandbox)
| Change | What it does | How to check it |
|---|---|---|
| Downloads panel in the series detail | Shows "Descargas offline" with a summary ("3 descargados · 1 en cola o descargando · 1 con error") for any series whose episodes can be downloaded (TioAnime pages). | Open a TioAnime series from Pendientes/Biblioteca. |
| Download all pending | "⬇ Descargar pendientes (N)" queues every unwatched, not yet downloaded episode, oldest first, one transfer at a time; reports how many were queued and how many cannot be downloaded from that site. Live test: queued 1 -> "Descargando… 29%" with a progress bar. | Click it; watch the rows. |
| Delete downloads without touching watched state | "🗑 Borrar descargas (N)" with an inline two-step confirmation (no browser dialog) removes the series' files and cancels queued/running ones; each downloaded row also has a 🗑 whose tooltip says the watched state is kept. Live test: confirm -> files gone, panel back to "Nada descargado", episode watched flags unchanged. | Click it, confirm; episodes keep their ✓ state. |
| Clearer rows | Progress bar and "En cola…" while waiting, ▶ Offline as the primary action once downloaded, a "⚠ Reintentar" button and the failure reason under the title when it fails. | Start a download and cancel/fail one. |

## Batch 11 - downloads from more sites (done; tested live in the sandbox)
Research and per-site findings: `docs/superpowers/plans/2026-10-14-download-sources-research.md`.
| Site | Works? | How |
|---|---|---|
| TioAnime | yes | YourUpload / Mp4upload / Mediafire / Streamtape mirrors |
| JKAnime | **yes (new)** | its base64 server list -> Mp4upload, Mediafire, Streamtape. Live test: 104.5 MB mp4 |
| animeyt.cc | **yes (new)** | its mytsumi player container -> the direct mp4 tab (archive.org) or the same hosts. Live test: 153 MB mp4 |
| GogoAnime (gogoanime.by) | not yet | single "HD" server = megaplay HLS stream (229 `.ts` segments, unencrypted): needs the HLS downloader |
| AnimeLand | not yet | the embed answers an empty page to plain HTTP: needs the browser engine |
| AnimeFLV | not checked | its origin was down (HTTP 522) the whole session, like the app's own scans |
Hosts that need more work: Streamwish, Vidhide, Filemoon, VOE, Mixdrop, Doodstream (packed JS / HLS), Mega (encrypted), 1Fichier, Okru.

## Batch 12 - safe downloads + HLS + GogoAnime (done; tested live in the sandbox)
| Change | What it does | How to check it |
|---|---|---|
| Quarantine for every download (`feat(download)`) | Each file stays a `.part` until it passes: (1) its first bytes are a real video container (MP4, MPEG-TS or Matroska) — an exe, script, archive or HTML page renamed to .mp4 is deleted, and the extension comes from the content, never the URL; (2) Windows Defender scans it (`MpCmdRun`, detection or unreadable verdict = deleted; no Defender = that layer is skipped and logged); then the file gets the Mark of the Web (`Zone.Identifier`, ZoneId=3) and the downloads folder denies *execute* on its files (inherit-only ACE for Everyone). Live check: the ACL (`Todos:(OI)(IO)(DENY)(X)`), the `Zone.Identifier` stream and a "scanned by Windows Defender: clean" line in `aot.log` all present, and the file stays readable. | `icacls "%APPDATA%\com.animeontrack.app\downloads"`; `Get-Content <file> -Stream Zone.Identifier`; `aot.log`. |
| HLS downloader (`feat(download)`) | Downloads `.m3u8` streams segment by segment into one `.ts` (master playlist -> best variant; retries flaky segments; refuses encrypted/live/fragmented streams). Same quarantine checks. | GogoAnime episode -> ⬇. |
| GogoAnime downloads | Episode page -> player -> megaplay -> m3u8. Live test: 229 segments -> 127 MB `.ts` with valid MPEG-TS sync bytes, scanned clean. The `.ts` plays in VLC / Windows Media Player (not in the browser). | A gogoanime.by series -> ⬇. |
| Panel for every supported site | The downloads panel now also shows for JKAnime, animeyt.cc and GogoAnime series (it only showed for TioAnime). | Open one. |

What this does NOT do: it is hardening, not a virtual machine. A flaw in the media player that later opens a hostile file is outside what the app can rule out, which is why files are only ever opened by the OS default player (keep it updated) and never executed by the app.

## Batch 13 - the rest of the download hosts (done; each tested live in the sandbox)
| Host | Result |
|---|---|
| Mixdrop (JKAnime) | downloads an mp4 (live test OK) |
| Vidhide (JKAnime) | HLS with PNG-disguised segments -> `.ts` (live test OK; the first attempt was correctly rejected by the quarantine, then the unwrapping fix made it work) |
| VOE (JKAnime) | HLS -> `.ts` (live test OK) |
| Ok.ru (TioAnime) | resolver verified (valid MP4 stream arrives); a full episode is slow on this CDN |
| Not possible | Filemoon/Byse (captcha), Streamwish (anti-bot loader), Doodstream (unreachable), Mega (encrypted), 1Fichier/Gofile/Fireload, AnimeLand (scam redirects), AnimeFLV (origin down). Reasons in `docs/superpowers/plans/2026-10-14-download-sources-research.md`. |
Also fixed: deleting an episode's download while it is still running now stops it (before, it finished and brought the file back); rows left "downloading" by a crash are reset at startup and their half files removed. `AOT_DOWNLOAD_PREFER=<host>` (dev only) tries one host's mirrors first.

## Batch 14 - AnimeFLV at its new address (done; scan and downloads tested live in the sandbox)
AnimeFLV is now https://animeflv.or.at, a different (WordPress) site, so the old adapter could not read it.
| Change | Detail |
|---|---|
| New adapter (`feat(animeflv)`) | Airing list from the "animes en emisión" sidebar (on every page), episodes from the series page's JSON block, search from the main results grid (never the sidebar), genres/synopsis from the series page. Live check on the sandbox copy: 273 series (74 airing) scanned, new episode links stored. |
| Domain migration (`feat(animeflv)`) | The base URL, the series page URLs and the saved mirror lists of existing installs follow the move (same mechanism as AnimeYT). Old episode rows keep their seen state: episodes are matched by number and their links refresh on the next scan. |
| Downloads (`feat(download)`) | Episode pages list servers as base64 buttons; Mp4upload and VOE ones download (live: a 2026-10-11 episode as mp4 and as `.ts`). The first server, a fragmented-MP4 HLS from zilla-networks, is not supported yet, and Byse needs a captcha. Very old episodes may only have dead links. |
