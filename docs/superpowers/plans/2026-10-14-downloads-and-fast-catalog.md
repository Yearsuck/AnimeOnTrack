# Plan: first-run catalog snapshot + episode downloads (2026-10-14)

Branch rule: everything lands on `feature/new-additions` only (child branches `feature/na-<slug>`), no release.

## A. Fast catalog for a new user

Problem: a fresh install crawls the whole AniList catalog (~21,000 titles) at ~28 requests/min, 50 titles per
request, i.e. 15+ minutes before Catálogo/Descubrir/Estadísticas are useful. The rate limit is AniList's, so more
parallelism does not help; the only real fix is not to crawl.

Design:
1. `catalog_snapshot.rs`: gzip JSON-Lines snapshot of every `CatalogAnime` (genres, synonyms, tags, relations, all
   scalar columns) + a header (`format`, `generated_at`, `count`). Pure encode/decode, DB export/import (import
   only inserts ids that are missing, never overwrites local data).
2. Export: a command writes a snapshot from a fully synced DB (the owner's, or a CI crawl). The file is published
   as a GitHub Release asset (`catalog-snapshot`, asset `catalog-snapshot.json.gz`); publishing is the owner's call.
3. Bootstrap: on first run (catalog empty and no completed full sync) the app downloads the snapshot (one request,
   a few MB), imports it in chunks with progress events, marks the full sync as completed at the snapshot's date, then
   the existing incremental sync (current year +- 2) closes the gap. Any failure falls back to the old crawl.
4. Relink/backfill steps already in `App.tsx` keep running afterwards.

Verification: unit tests (roundtrip, rejects, no overwrite), import timing at real size (22k rows) in the sandbox,
and a fresh-profile run against a local file URL.

## B. Episode downloads

State today: `commands/download.rs` is a stub that marks the row failed; the UI button, DB columns
(`download_status`, `download_path`), delete-on-seen and size accounting exist.

Findings (checked live 2026-10-14): TioAnime's episode page carries `videos = [[name, embedUrl, ...], ...]`
with several mirrors (YourUpload, Okru, Mega, StreamSB, Mepu, Netu, mail.ru, ...). YourUpload's embed page is
plain HTML with `og:video` / `file: '<mp4>'`; the mp4 answers a 302 to a CDN when fetched with the embed page as
`Referer`. No Cloudflare involved on the embed hosts, so no browser is needed once the server list is known.

Design:
1. `adapter` gets `episode_servers(html) -> Vec<Server{name,url}>` (TioAnime first; other sites follow the same
   research: list their embeds, then add parsers).
2. `download/extract.rs`: per-host resolvers returning `Media{url, referer, kind}`: YourUpload (mp4), Ok.ru
   (`data-options` JSON -> best mp4/hls), mp4upload; unknown hosts are skipped; HLS only if a plain mp4 is not
   offered (no ffmpeg: HLS segments are not playable in the webview, so HLS is out of scope for v1).
3. `download/mod.rs`: fetch the episode page HTML through the existing scraper (Cloudflare), pick the first server
   a resolver supports (preference order configurable), stream to `<app_data>/downloads/<series>/<episode>.mp4`
   with Referer/UA, 5 GB cap, `Content-Type` video/* check, `download-progress` events, cancel flag, `.part` file
   renamed on success, DB status transitions (`downloading` -> `downloaded`/`failed`), one download at a time.
4. UI: progress in the episode row, cancel, "play offline" (opens the file with the system player; in-app `<video>`
   needs the asset scope widened to the downloads dir, kept as a later step), error text per failure reason.
5. Safety: every URL passes `is_safe_external_url`; redirects re-checked; never writes outside the downloads dir.

Risks: hosts change markup (extractors will rot, so each has fixture tests and failures are reported, not hidden);
some mirrors are token-gated or short-lived; copyright of what the sites host is the user's responsibility.

## Order of work
1. A: snapshot module (agy) -> bootstrap command + UI progress (me) -> export + timing check.
2. B: server-list parsing + YourUpload/Ok.ru extractors with fixtures (agy) -> downloader + events (me) -> UI.
