# Download sources: what each site serves (checked live 2026-10-14)

Method: episode pages opened in an app viewer window (Cloudflare passed by the browser engine) and inspected over
CDP; every embed/host page then fetched with plain HTTP (`curl`, same headers as the downloader) to see whether a
browser is needed. Fixtures of the real pages live in `src-tauri/tests/fixtures/`.

| Site | Episode page gives | Resolvable now? | Notes |
|---|---|---|---|
| **TioAnime** | `videos = [[name, embedUrl, ..], ..]` (YourUpload, Okru, Mega, StreamSB, Mepu, Netu, Maru) | **Yes** (YourUpload -> mp4). Shipped. | Okru page returned no player data to plain HTTP; Mega is end-to-end encrypted. |
| **animeyt.cc** | one iframe `mytsumi.com/container.php?id=..`; with `&open=1` the container lists tabs and download panels as plain HTML (no Cloudflare) | **Yes**: a `data-player-kind="video"` tab is a direct mp4 (archive.org, `video/mp4`, 302 to a CDN) | Other tabs: Byse, MEGA, Epsilon, Abyss (embeds, JS players). Download panel offers MEGA, Byse, Fireload (HTML landing, Cloudflare), Gofile (needs API). Direct tab not guaranteed on every episode. |
| **JKAnime** | `servers = [{remote: base64(url), server, lang, size}, ..]` with 11 hosts: Mediafire, Mega, Streamwish, Vidhide, VOE, Filemoon, Mixdrop, Mp4upload, Streamtape, Doodstream, 1Fichier | **Yes** via Mp4upload (`src: ".../video.mp4"`, needs Referer), Mediafire (`#downloadButton` href, `video/mp4`), Streamtape (link assembled by JS: prefix + `substring` chain; the visible element holds a decoy token) | Streamwish/Vidhide/Filemoon/VOE/Mixdrop/Doodstream use packed JS and mostly HLS: later. |
| **GogoAnime (gogoanime.by)** | one server "HD": `gogoanime.by/player/?source=embed&url=<encrypted>` -> iframe `megaplay.su/embed.php?sid=..` -> `https://megaplay.su/uploads/hls/<sid>/index.m3u8` | **Needs HLS**: plain, unencrypted VOD playlist (229 `.ts` segments for a 23 min episode). All plain HTTP once the wrapper page is fetched. | No ffmpeg: segments would be concatenated into a `.ts` file (plays in VLC / Windows Media Player, not in a browser `<video>`). |
| **AnimeLand** | iframe `w2.animeland.tv/muembed.php?file=..` | **No (yet)**: plain HTTP gets a 301 to https and then an empty 200; needs the browser (cookie/JS) | Its page also triggers ad redirects in a normal viewer. |
| **AnimeFLV** | not checked | **Unknown**: the origin answered HTTP 522 (Cloudflare "origin down") during the whole session, same as the app's own scans (`TransientError`) | Re-check when it is back. |

## What this means for the downloader
1. Hosts that give a plain mp4 with a Referer: YourUpload, Mp4upload, Mediafire, Streamtape, archive.org (via mytsumi). Done in the
   parser layer (`download_sources.rs`) and wired into `commands/download.rs`: TioAnime, JKAnime, animeyt.cc.
2. HLS (m3u8) support unlocks GogoAnime (megaplay) and most "player" hosts (Streamwish, Vidhide, Filemoon, VOE...). It needs a
   playlist parser (media + master playlists, pick the best variant), a sequential segment fetcher with the same safety checks, and
   output as `.ts` (no remux). Planned as the next step.
3. Packed-JS hosts (Streamwish, Vidhide, Mixdrop, Doodstream, VOE): need a small `eval(function(p,a,c,k,e,d)` unpacker; planned after HLS.
4. AnimeLand and AnimeFLV need either the browser engine for the embed page or a re-check once the site is reachable.
