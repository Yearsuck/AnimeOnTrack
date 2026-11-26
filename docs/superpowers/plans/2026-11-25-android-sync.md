# Plan 2026-11-25: Android forward + device sync

Two workstreams requested by the author: (A) move the Android version forward (implement, test,
validate) and (B) synchronise the user's data between devices (PC <-> Android, PC <-> PC).
Implementation was pre-approved; decisions an owner might disagree with are listed under
"Decisiones a confirmar". Research: two agy research agents (reports kept by the supervisor, claims
spot-checked against the code below).

## Verified facts

### Android today (baseline = `develop` at 0b9a04a, v0.25.0)
- An x86_64 debug APK builds without Developer Mode (`tauri android build` compiles the `.so`, then
  copy it to `jniLibs/x86_64` and `gradlew assembleX86_64Debug -x rustBuildX86_64Debug`). Installed
  on a fresh emulator (AVD `aot_test`, Android 16, x86_64): onboarding renders, scanning animeyt.cc
  finds 71 series, following one and refreshing lists 21 pending episodes. Screenshots
  `C:/t/shots/base-0*.png` (local only).
- The WebView exposes Chrome DevTools in debug builds: `adb forward tcp:9333
  localabstract:webview_devtools_remote_<pid>` and `CDP_PORT=9333 node tools/sandbox/cdp.mjs invoke ...`
  drives the Android app exactly like the desktop sandbox.
- **Bug: opening an episode traps the user.** `open_episode` (`commands/seen.rs:13`) always uses
  `AppWindowPlayer` (`player.rs:102`), a `WebviewWindowBuilder` window: on Android it covers the whole
  app with the scraped page (ads included, `ad_block::install` is a no-op off Windows,
  `ad_block.rs:396`), and the back key does nothing (verified on the emulator, `base-05/06`). Only
  killing the app gets out.
- Secrets (`google_client_secret`, `gdrive_refresh_token`) are plain text off Windows:
  `secure_store.rs` only has DPAPI. A Rust -> Kotlin call (`run_mobile_plugin`) runs on the Android
  main thread and blocks the caller, and `unprotect` is called while holding the DB mutex
  (`backup/mod.rs:139,184`), so a per-secret Kotlin call could deadlock with a synchronous command.
- Scans are started from JS only (`App.tsx` startup, Actualizar, 30 min interval); with the screen
  off Android freezes the off-screen WebView (spike notes in `2026-10-30-android.md`). The manifest
  only has `INTERNET`.
- Drive OAuth is a desktop loopback flow (`backup/oauth.rs:227`, `http://127.0.0.1:<port>`) with a
  Desktop client id + secret; it cannot work on Android. Google's Android clients have no secret and
  are bound to package name + signing SHA-1.
- Downloads opened with `opener().open_path` go through a `FileProvider` whose `file_paths.xml` has
  no `<files-path>` (not verified on device).

### Data model relevant to sync
- Row ids differ per device. The site-agnostic `library` table (`db.rs` ~549, `db/library.rs`) is
  rebuilt from `series` on every open (`sync_library_from_series`): canonical key `al:<anilist id>`,
  else `t:<normalized title>` (`canonical_group_keys`, `db/episodes.rs:396`), followed / watched
  OR-ed, backlog by precedence, seen watermark = max canonical episode number (`RowAlign`,
  `db/episode_align.rs`). Discarded swipe rows are left out of it on purpose.
- No change timestamps exist for follow, "Ya lo vi" or backlog; `episodes.seen_at` exists only for
  seen rows (cleared on un-mark).
- Absent shows can be materialised as synthetic `anilist-<id>` rows exactly like
  `run_library_import` (`commands/follow.rs` ~527) does; the post-scan library import then links
  them to the active site.
- Drive code (`backup/drive.rs`) only knows the DB file name; reusable for a second file once the
  name is a parameter. Restoring a whole DB onto another OS also carries DPAPI blobs that cannot be
  decrypted there.

## Design decisions

1. **Sync unit: a per-show state document, not the DB.** `animeontrack-sync.json` holds, per
   canonical key, four stamped fields (`followed`, `watched`, `backlog`, `watermark`), each
   `{v, at, dev}`. Whole-DB restore stays as the backup feature. Why: platform-independent, tiny,
   no restart, no secrets or paths, merges instead of overwriting. Types: `src-tauri/src/sync/doc.rs`.
2. **Three-way merge with a local base, not per-command timestamps.** Each device stores, per key,
   what it published (`merged`) and what its own state was right after applying it (`observed`) in a
   new `sync_base` table. A local change = current state != `observed`; a remote change = remote
   stamp != `merged`. Why: no edits to the ~12 write paths (follow, seen cascade, swipes, backlog,
   reconciles), works for existing data, and a remote value this device cannot apply yet (watermark
   beyond the scraped episodes) is not mistaken for a local un-mark.
3. **Conflicts (both devices changed the same field since their last sync) resolve to the more
   engaged value**: follow/"Ya lo vi" `true` wins, watermark max wins, backlog want > discarded >
   none. Why: a device only knows that its change happened since its last sync, not when, so
   timestamps would just favour whoever syncs later; losing an unfollow or un-mark costs one tap,
   losing progress or a follow is worse.
4. **Deletion is a value.** A remote never deletes a stamp; a missing or reset remote file cannot
   wipe local state. Keys whose local rows disappeared (e.g. re-keyed from `t:` to `al:` after
   linking) are "unknown", never "unfollowed".
5. **Materialise only what has an AniList id.** A remote `al:` follow/backlog/"Ya lo vi" missing
   locally creates the synthetic row; `t:` keys only apply to rows that already exist.
6. **Transport: one trait, two implementations.** `SyncRemote { get, put(if_match) }` with etag
   optimistic concurrency and a retry loop. (a) Local folder (any folder OneDrive/Syncthing/Drive for
   desktop replicates: PC <-> PC today, and the test transport); (b) Google Drive `appDataFolder`
   reusing the existing OAuth client (PC <-> PC now; Android once Android OAuth exists).
7. **Android episodes open in the system browser.** Smallest fix for the trap; no in-app player on
   mobile for now.
8. **Android secrets: Keystore-wrapped data key, AES-GCM in Rust.** Kotlin returns a 32-byte data key
   wrapped by a non-exportable `AndroidKeyStore` key once at startup; Rust encrypts with `ring`
   (already in the lockfile) under prefix `aks1:`. Why: no Kotlin call per secret (deadlock risk
   above); legacy plain text keeps working and is upgraded in place.
9. **Screen-off scans: a `dataSync` foreground service + partial wake lock held only while a scan
   runs**, started/stopped from Rust around `refresh` through the existing `webfetch` plugin.

## Phases (each a PR into `develop`)
1. `fix/android-open-episode`: system browser on mobile.
2. `feature/sync-core` -> doc types, pure merge, `SyncRemote` + local folder (agy `sync-core`).
3. `feature/sync-db` -> `sync_base` table, projection, apply ops (agy `sync-db`).
4. `feature/android-keystore` -> `aks1:` secrets (agy `android-keystore`).
5. `feature/sync-command` -> `sync_now`/status commands, Drive remote, settings UI, auto-sync after
   changes/at startup, README/AGENTS.
6. `feature/android-scan-service` -> foreground service.
7. Android OAuth for Drive (needs a Google Cloud Android client: user step) and a second mobile
   design pass: only if the above land with time left.

## Tests and proofs
- Unit: merge cases (first sync, remote unfollow, local/remote un-mark, shortfall retry, conflicts,
  orthogonal edits, lost remote, re-keyed show, determinism), local folder etag/conflict/retry,
  projection/apply on in-memory DBs, `aks1:` seal/open/tamper.
- Behaviour: two sandbox profiles on the PC (`com.animeontrack.sandbox` with a copy of the real DB,
  `com.animeontrack.sandbox-empty`) syncing through a temp folder; PC sandbox <-> emulator through
  the same document pushed with `adb`. Never the real DB, never the real Drive backup.
- Emulator: install the debug APK, onboarding, scan, pending, open an episode (must open the
  browser and come back with the back key), screenshots.

## Needs the author
- Google Cloud: an Android OAuth client (package `com.animeontrack.app` + SHA-1 of the signing key)
  before Drive can work on the phone; consent on each device.
- Release signing key for a sideloadable release APK.

## Decisiones a confirmar
- Conflict rule "more engaged wins" (decision 3) instead of last-writer-wins.
- Settings (language, theme, filters, active site) are NOT synced; only show decisions.
- `t:` (unlinked) shows are not materialised on other devices.
- Episodes on Android open in the external browser (no ad block, no clean player there).
- Discarded swipe decisions are synced (they can be thousands; the document stays a few hundred KB).

## Progress
(updated as PRs merge)
