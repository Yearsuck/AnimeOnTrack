# Debug sandbox

Run AnimeOnTrack against a **copy** of your real data, isolated from the app you use, and drive it
from scripts. Nothing here ever writes to your real database, WebView2 profile or Google Drive.

Requires **Node >= 22.13** (built-in `node:sqlite` without a flag, with `readOnly`, for the copy; global `WebSocket` for the client) and
Windows (the app's data dirs and WebView2).

## How the isolation works

`start.mjs` launches the app with a Tauri `--config` override of the `identifier`
(`com.animeontrack.sandbox`), passed as a JSON *file* (inline JSON loses its quotes through `cmd`). The
data directory (`%APPDATA%\com.animeontrack.sandbox`) and the WebView2 profile are derived from the
identifier, so the sandbox has its own database, cookies and cover cache.

The real DB is copied once (`--fresh` copies it again) with `VACUUM INTO` from a connection opened
**read-only**: a consistent snapshot even while the real app is running, and the original is never
written. The sandbox cover cache starts empty (copied `file:` cover paths point at the real covers dir,
which the sandbox's narrowed asset scope refuses, so those covers show as placeholders until re-fetched).

### What is removed from the copy, and why

The `google_*`, `gdrive_*` and `backup_*` settings (OAuth client, tokens, remembered backup file id,
"changed since last backup" signature). On startup the app runs an opportunistic Google Drive backup;
with those present, a sandbox could upload **sandbox data over your real cloud backup**, or restore from
it. They are stripped by `sanitize.mjs` (`node --test tools/sandbox` covers it), and the tool refuses to
copy at all if `node:sqlite` is unavailable. Scans still hit the real sites from your IP.

## Quick start

```bash
node tools/sandbox/start.mjs              # dev build, hot reload, CDP on :9222
node tools/sandbox/start.mjs --fresh      # discard sandbox state, re-copy the real DB (close the sandbox app first)
node tools/sandbox/start.mjs --release    # optimised build, frontend embedded (what users run)
```

With `--release` the command only compiles; start `src-tauri/target/release/aot-scaffold.exe` yourself
with the env var it prints (use PowerShell `Start-Process` so it detaches). **It overwrites that exe with
the sandbox identifier baked in** — rebuild without `--config` before installing or shipping it.
Ctrl+C is forwarded so the dev server and app are not orphaned.

## Driving it (Chrome DevTools Protocol)

```bash
node tools/sandbox/cdp.mjs targets                         # list webview targets
node tools/sandbox/cdp.mjs invoke get_active_site '{}'     # call any Tauri command, with timing
node tools/sandbox/cdp.mjs eval "document.title"           # run JS in the page
node tools/sandbox/cdp.mjs probe 60                        # every 3s: renderer + sync command latency
```

Exit codes: `0` ok, `1` the page threw / the command was rejected / bad usage, `2` no app listening or no
main page, `3` timeout. `probe` exits `3` if any sample timed out or errored, so it can gate a script.
It is the quick way to tell a real freeze (sync command times out while the renderer answers) from a
slow command.

## Security: the debug port is full control of the app

`--remote-debugging-port` exposes **every Tauri command** (and the sandbox DB) to anything that can
reach `127.0.0.1:9222`. WebView2 rejects foreign WebSocket origins, which blunts web pages, but any
local process can drive it. This tool opens the port only for the sandbox it starts; never set
`WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS` with a debugging port globally, and do not leave a release exe
running with it.

## Gotchas learned the hard way

- **Keep Tauri's default WebView2 args.** `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS` *replaces* them. Setting
  only `--remote-debugging-port` made the **release** app's IPC stall for ever (every `invoke` hung,
  Rust threads idle, process "not responding") — indistinguishable from a real hang. `start.mjs` and
  the printed release env var include `--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection`
  (hardcoded: re-check it if Tauri changes its defaults).
- Stale `msedgewebview2.exe` processes from a previous run share the profile and keep port 9222. Stop
  only the ones whose command line contains `com.animeontrack.sandbox`.
- To click UI without CDP (to rule CDP out), use Windows UI Automation on the app window.
- The UI-hang watchdog (`src-tauri/src/ui_watchdog.rs`) relaunches a frozen app; set `AOT_NO_WATCHDOG=1`
  when you suspend threads or attach a debugger on purpose.
- A debugger for real hangs: `winget install Microsoft.WinDbg`, then
  `cdb -pv -p <pid> -c "~*kn 28; q"` with `_NT_SYMBOL_PATH=src-tauri\target\release` (offline symbols;
  the Microsoft symbol server makes `cdb` crawl).
- A Windows "application hang" (event id 1002, source *Application Hang*) leaves **no** `panic.log`
  entry: Rust never panicked, a thread just stopped pumping or answering.
