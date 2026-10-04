// Run AnimeOnTrack against a COPY of your real data, isolated from the real app.
//
//   node tools/sandbox/start.mjs            # dev build (hot reload), reuse existing sandbox data
//   node tools/sandbox/start.mjs --fresh    # re-copy the real DB first (discards sandbox state)
//   node tools/sandbox/start.mjs --release  # optimised build, frontend embedded (what users run)
//
// Isolation comes from overriding the Tauri `identifier` (see `--config`): the data dir
// becomes %APPDATA%\com.animeontrack.sandbox and the WebView2 profile is separate too, so
// the real database, cookies and covers are never touched. The WebView2 remote debugging
// port (default 9222) lets tools/sandbox/cdp.mjs call commands and probe responsiveness.
//
// Requires Node >= 22.13 (built-in `node:sqlite` without a flag, with the `readOnly` option; used
// for a consistent read-only copy).
import { spawn } from 'node:child_process';
import { existsSync, mkdirSync, renameSync, rmSync, writeFileSync } from 'node:fs';
import { cpus, constants, setPriority } from 'node:os';
import { join } from 'node:path';
import { stripCloudSettings } from './sanitize.mjs';

const SANDBOX_ID = 'com.animeontrack.sandbox';
const REAL_ID = 'com.animeontrack.app';
const appdata = process.env.APPDATA;
if (!appdata) { console.error('APPDATA is not set (this tool targets Windows).'); process.exit(1); }

const args = new Set(process.argv.slice(2));
const sandboxDir = join(appdata, SANDBOX_ID);
const realDb = join(appdata, REAL_ID, 'animeontrack.sqlite');
const sandboxDb = join(sandboxDir, 'animeontrack.sqlite');

async function copyRealDbSanitised() {
  let DatabaseSync;
  try {
    ({ DatabaseSync } = await import('node:sqlite'));
  } catch {
    console.error('node:sqlite is unavailable (needs Node >= 22.13). Refusing to copy the DB without being able to strip the cloud-backup credentials.');
    process.exit(1);
  }
  mkdirSync(sandboxDir, { recursive: true });
  // `VACUUM INTO` writes a consistent snapshot even while the real app is running (a raw file
  // copy can catch a torn journal), and the real DB is opened read-only: never written.
  // Work on a partial file and only rename it into place once sanitised: if anything throws
  // midway, a half-sanitised DB must never be left at `sandboxDb` (the next run would see it
  // exists, skip the copy, and start the app with the Drive credentials still inside).
  const partial = `${sandboxDb}.partial`;
  rmSync(partial, { force: true });
  try {
    const real = new DatabaseSync(realDb, { readOnly: true });
    try { real.exec(`VACUUM INTO '${partial.replace(/'/g, "''")}'`); } finally { real.close(); }
    const copy = new DatabaseSync(partial);
    let removed;
    try {
      // Drops the Google Drive credentials/state so the sandbox can never back up over, or
      // restore from, the real cloud copy (see sanitize.mjs).
      removed = stripCloudSettings(copy);
    } finally { copy.close(); }
    renameSync(partial, sandboxDb);
    console.log(`[sandbox] copied ${realDb} -> ${sandboxDb} (read-only snapshot; removed ${removed} cloud-backup setting(s))`);
  } catch (e) {
    rmSync(partial, { force: true });
    console.error(`[sandbox] could not create a sanitised copy: ${e.message}`);
    process.exit(1);
  }
}

if (args.has('--fresh') && existsSync(sandboxDir)) {
  try { rmSync(sandboxDir, { recursive: true, force: true }); }
  catch (e) { console.error(`Cannot reset ${sandboxDir} (${e.code}). Is the sandbox app still running? Close it first.`); process.exit(1); }
}
if (!existsSync(sandboxDb)) {
  if (!existsSync(realDb)) { console.error(`No real DB at ${realDb} to copy.`); process.exit(1); }
  await copyRealDbSanitised();
}

// Passed as a file, not inline JSON: `spawn(..., { shell: true })` does not quote arguments, so
// cmd/npm.cmd on Windows would strip the double quotes of an inline JSON string.
const configPath = join(sandboxDir, 'tauri-sandbox-config.json');
// Only the identifier is overridden: `$APPDATA` in the asset-protocol scope of tauri.conf.json already
// resolves to <Roaming>/<identifier>, so the sandbox's covers dir is covered by the very same scope.
writeFileSync(configPath, JSON.stringify({ identifier: SANDBOX_ID }));

const port = process.env.CDP_PORT || '9222';
// NB: this variable REPLACES Tauri's own default WebView2 arguments instead of adding to
// them. Setting only the debugging port made the packaged (release) app's IPC stall
// forever (every command timed out while Rust sat idle) — a pure tooling artifact that
// looks exactly like an app hang. Always keep Tauri's defaults in front of the port.
const TAURI_DEFAULT_WEBVIEW2_ARGS = '--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection';
const env = {
  ...process.env,
  WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `${TAURI_DEFAULT_WEBVIEW2_ARGS} --remote-debugging-port=${port}`,
  RUST_BACKTRACE: '1',
  // rustc/cargo default to one job per logical core at normal priority: a release build then
  // pegs every core for ~1.5 min and freezes the whole PC. A third of the cores is plenty.
  CARGO_BUILD_JOBS: process.env.CARGO_BUILD_JOBS ?? String(Math.max(2, Math.floor(cpus().length / 3))),
};
const quoted = `"${configPath}"`;
const tauriArgs = args.has('--release')
  ? ['run', 'tauri', 'build', '--', '--no-bundle', '--config', quoted]
  : ['run', 'tauri', 'dev', '--', '--config', quoted];

console.log(`[sandbox] data dir: ${sandboxDir}\n[sandbox] CDP: http://127.0.0.1:${port}  (node tools/sandbox/cdp.mjs targets)`);
if (args.has('--release')) {
  console.log('[sandbox] release build only compiles; then run src-tauri/target/release/aot-scaffold.exe with the same env:\n' +
    `          WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS="${TAURI_DEFAULT_WEBVIEW2_ARGS} --remote-debugging-port=${port}"\n` +
    '          WARNING: this overwrites target/release/aot-scaffold.exe with the SANDBOX identifier baked in;\n' +
    '          rebuild without --config before shipping or installing that binary.\n' +
    '          (use PowerShell Start-Process to detach it)');
}
const child = spawn('npm', tauriArgs, { env, stdio: 'inherit', shell: true });
// Windows priority classes are inherited by processes spawned later, so setting it on npm
// is enough to keep cargo/rustc/vite from starving the desktop. (`tauri dev` also runs the
// app itself under this class, so measure latency/hangs with --release + a normally started exe.)
try { setPriority(child.pid, constants.priority.PRIORITY_BELOW_NORMAL); } catch { /* best effort */ }
// Forward Ctrl+C so the dev server / app are not orphaned (they would keep :9222 and the profile).
for (const sig of ['SIGINT', 'SIGTERM']) process.on(sig, () => child.kill(sig));
child.on('exit', (code) => process.exit(code ?? 0));
