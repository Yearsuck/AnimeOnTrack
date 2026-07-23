# Cross-platform porting plan (macOS & Linux)

Status: **plan only, not implemented.** Today the app is Windows-only at runtime.

## TL;DR

The app already compiles on macOS and Linux, and everything except the scraper is
portable. There is exactly **one** blocker: `scraper_engine.rs::eval()`, which reads
rendered HTML out of the webview via Windows-only WebView2 COM interop. Port that one
function to WKWebView (macOS) and WebKitGTK (Linux) and the whole app works on all three.
The rest of the work is build/CI matrix, packaging, and — the real cost — **testing on
real hardware**, which a Windows dev box can't do.

## Current state (what's portable vs not)

Audited on `develop`:

| Area | Portable today? | Notes |
|---|---|---|
| UI (React/TS/Vite), i18n, design system | ✅ | No OS assumptions. |
| SQLite (`rusqlite`, bundled) | ✅ | Compiles from C on every OS. |
| App-data path | ✅ | `app_data_dir()` resolves per-OS from the identifier. |
| AniList catalog / discover / stats | ✅ | Pure `reqwest` + local SQL. |
| Google Drive backup | ✅ | `reqwest` + loopback OAuth; loopback works everywhere. |
| Link opening (`player.rs`) | ✅ | Uses `tauri-plugin-opener` (cross-platform). |
| **Scraper (`scraper_engine.rs::eval`)** | ❌ | **Windows-only** — the single blocker. |

The Windows-specific bits are all localized:
- `eval()` has a `#[cfg(windows)]` branch (WebView2 `ExecuteScript`) and a
  `#[cfg(not(windows))]` stub returning *"scraping is only implemented on Windows"*.
- The WebView2 crates are gated behind `[target.'cfg(windows)'.dependencies]` in
  `Cargo.toml` (`webview2-com`, `windows`).

Nothing else in the backend uses `cfg(windows)`, `windows::`, or the Win32 API.

## The one blocker: `eval()`

`eval(window, script, timeout)` runs `script` in the scraper's webview **host-side** (not
page IPC — Tauri doesn't inject IPC into remote pages) and returns its result as a
**JSON string**. It's called from `with_webview(|platform| …)`, which hands back a
`PlatformWebview` whose concrete type differs per OS:

- **Windows** — `platform.controller().CoreWebView2().ExecuteScript(script, handler)`.
  Async; the completion handler delivers a JSON string; bridged to a `oneshot` channel.
- **macOS** — `platform.inner()` is a `WKWebView` pointer. Use
  `evaluateJavaScript:completionHandler:` (objc2 / block2). The completion handler yields
  a native `id` result + `NSError`. **Contract gap:** WebView2 returns a *JSON string*;
  WKWebView returns a *native value*. Fix by wrapping the script so it returns a string
  (`JSON.stringify(...)`) or serializing the `id` to JSON host-side. Async → same oneshot
  bridge.
- **Linux** — `platform.inner()` is a `webkit2gtk` `WebView`. Use
  `webkit_web_view_evaluate_javascript` (WebKitGTK ≥ 2.40) or the older
  `run_javascript`, with a `GAsyncResult` callback; read the `JSCValue`, coerce to a JSON
  string. Async → same oneshot bridge.

All three are the same shape: fire JS, get an async callback, send a `Result<String>`
down the existing `oneshot` channel, honor the existing `tokio::time::timeout`. The
readiness-probe and cover-fetch logic (`document.readyState`, `<canvas>.toDataURL()`) is
plain JS and needs **no** per-OS change once `eval()` works — but must be **re-verified**
against each engine (WebView2/WKWebView/WebKitGTK differ subtly in timing and in what a
cross-origin `<canvas>` read allows).

## Work breakdown

### Phase 0 — Build matrix (low effort, no functionality)
- Add `macos-latest` + `ubuntu-latest` to `ci.yml` (Linux needs
  `libwebkit2gtk-4.1-dev`, `libgtk-3-dev`, `librsvg2-dev`, `build-essential`, etc.).
- Confirm it compiles everywhere (it should — the stub covers non-Windows).
- Result: green cross-platform build, but scraping still returns the stub error. Ships
  nothing useful on its own; it's the scaffolding for the phases below.

### Phase 1 — `eval()` for macOS (WKWebView)
- Add `[target.'cfg(target_os = "macos")'.dependencies]`: `objc2`, `objc2-web-kit`,
  `block2` (or `cocoa`/`objc` if preferred).
- Implement the `#[cfg(target_os = "macos")]` arm: cast `platform.inner()` to
  `WKWebView`, call `evaluateJavaScript:completionHandler:`, marshal result→JSON string,
  send on the oneshot.
- Handle the string-vs-native-value contract gap (wrap script in `JSON.stringify`).

### Phase 2 — `eval()` for Linux (WebKitGTK)
- Add `[target.'cfg(target_os = "linux")'.dependencies]`: `webkit2gtk` (matching the
  version Tauri v2 links).
- Implement the `#[cfg(target_os = "linux")]` arm with the async JS-eval + `JSCValue`
  → JSON string.

### Phase 3 — Re-verify the JS-side assumptions per engine
- Readiness probe (`readyState`, title regex, body-size floor) against each engine.
- Cover fetch via offscreen `<canvas>.toDataURL()` — verify the same-origin image read
  isn't tainted differently on WKWebView/WebKitGTK.
- Cloudflare challenge clearing — the whole reason for the real-browser approach; each
  engine presents a different UA/TLS fingerprint, so a site that clears on WebView2 may
  behave differently on WKWebView/WebKitGTK. **This is the biggest unknown.**

### Phase 4 — Packaging & signing
- **macOS**: `.dmg`/`.app`. For distribution without Gatekeeper warnings, needs an Apple
  Developer ID cert + **notarization** (paid Apple account). Without it, users must
  right-click → Open. Decide: notarize, or ship unsigned with instructions.
- **Linux**: `.deb` + `.AppImage` (and optionally `.rpm`). AppImage is the most portable;
  document the runtime `libwebkit2gtk` dependency.

### Phase 5 — Release CI matrix
- Extend `release.yml` to a matrix (`windows-latest`, `macos-latest`, `ubuntu-latest`);
  `tauri-action` uploads each platform's artifacts to the same GitHub Release.
- Add macOS signing/notarization secrets if Phase 4 chooses to notarize.

### Phase 6 — Real-hardware testing (unavoidable)
- Validate the full flow — Cloudflare clear, airing scan, cover fetch, follow/seen,
  catalog sync, Drive backup/restore — on a **real Mac** and a **real Linux desktop**.
  A Windows dev box (and Windows CI) cannot exercise WKWebView/WebKitGTK. This is the
  gating cost of the whole effort.

## Risks & unknowns

1. **Cloudflare behavior per engine (highest risk).** The bypass depends on a real
   browser clearing the JS challenge; WKWebView and WebKitGTK are different browsers with
   different fingerprints. May need per-engine tuning of the readiness probe or UA.
2. **`with_webview` / `PlatformWebview` inner-type API stability** across Tauri v2 point
   releases — pin the Tauri version and the webkit2gtk/objc2 crate versions together.
3. **macOS notarization** requires a paid Apple Developer account; without it the app is
   annoying to install (Gatekeeper).
4. **Linux fragmentation** — WebKitGTK version skew across distros; AppImage bundling
   mitigates but doesn't fully erase it.
5. **Canvas cover read** could be tainted differently cross-engine, breaking cover
   fetching even when HTML extraction works.

## Effort estimate (rough)

| Phase | Effort |
|---|---|
| 0 — build matrix | ~half a day |
| 1 — macOS eval | 1–2 days (objc FFI + marshaling) |
| 2 — Linux eval | 1–2 days (webkit2gtk FFI) |
| 3 — re-verify JS assumptions | 1–3 days, **needs real hardware** |
| 4 — packaging/signing | 0.5–2 days (more if notarizing) |
| 5 — release matrix | ~half a day |
| 6 — testing | open-ended, **needs real Mac + Linux** |

Architecture-wise it's small (one function, three arms). The real cost is FFI care +
**testing on hardware this project doesn't have**.

## Recommendation

Keep it **Windows-first** until there's access to a Mac and a Linux desktop to test on —
implementing `eval()` blind, without being able to verify Cloudflare clears on those
engines, risks shipping a build that compiles and launches but silently fails every scan.
When hardware is available, do it in this order: Phase 0 → Phase 1 (mac) end-to-end incl.
Phase 3+6 on that OS → then Phase 2 (linux) the same way → then Phases 4/5 to ship. Doing
one OS fully before starting the next keeps the Cloudflare-unknown contained.
