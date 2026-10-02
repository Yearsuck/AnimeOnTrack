// Tiny CDP client for the sandboxed Tauri app (WebView2 --remote-debugging-port=9222).
//
// Usage: node tools/sandbox/cdp.mjs targets
//        node tools/sandbox/cdp.mjs probe [seconds]                   ping renderer + a sync command every 3s
//        node tools/sandbox/cdp.mjs eval "<js expression>" [timeoutMs]    (awaits promises)
//        node tools/sandbox/cdp.mjs invoke <command> '<json args>' [timeoutMs]
//
// Exit codes: 0 ok, 1 the page threw / the command was rejected / bad usage, 2 no app or no main
// page, 3 timeout (the page did not answer: a real freeze looks like this).
// Needs Node >= 22 (global WebSocket).
const USAGE = 'usage: cdp.mjs targets | probe [seconds] | eval "<js>" [timeoutMs] | invoke <command> \'<json>\' [timeoutMs]';
const PORT = process.env.CDP_PORT || 9222;
const [, , mode, a, b, c] = process.argv;

if (!['targets', 'probe', 'eval', 'invoke'].includes(mode)) { console.error(USAGE); process.exit(1); }
if (typeof WebSocket === 'undefined') { console.error('This tool needs Node >= 22 (global WebSocket).'); process.exit(1); }
if ((mode === 'eval' && !a) || (mode === 'invoke' && !a)) { console.error(USAGE); process.exit(1); }

let targets;
try {
  targets = await (await fetch(`http://127.0.0.1:${PORT}/json`)).json();
} catch {
  console.error(`Nothing is listening on 127.0.0.1:${PORT}. Start the sandbox first (node tools/sandbox/start.mjs).`);
  process.exit(2);
}
if (mode === 'targets') {
  for (const t of targets) console.log(t.type, t.url.slice(0, 90), t.title);
  process.exit(0);
}
const page = targets.find((t) => t.type === 'page' && /localhost:1420|tauri\.localhost/.test(t.url));
if (!page) { console.error('no main page target (is the app window open?)'); process.exit(2); }

const ws = new WebSocket(page.webSocketDebuggerUrl);
try {
  await new Promise((resolve, reject) => { ws.onopen = resolve; ws.onerror = reject; });
} catch {
  console.error('Could not open the DevTools WebSocket (another client attached, or the page is gone).');
  process.exit(2);
}
// A socket that dies mid-run would leave every pending call waiting for its full timeout.
ws.onclose = () => { console.error('DevTools WebSocket closed.'); process.exit(2); };
let nextId = 0;
const pending = new Map();
ws.onmessage = (m) => {
  const msg = JSON.parse(m.data);
  if (msg.id && pending.has(msg.id)) { pending.get(msg.id)(msg); pending.delete(msg.id); }
};
const evaluate = (expression) => new Promise((resolve) => {
  const id = ++nextId;
  pending.set(id, resolve);
  ws.send(JSON.stringify({ id, method: 'Runtime.evaluate', params: { expression, awaitPromise: true, returnByValue: true } }));
});

if (mode === 'probe') {
  // One probe call: how long did the page take, and did it actually succeed? A CDP reply that
  // carries `exceptionDetails` (e.g. __TAURI_INTERNALS__ missing, command rejected) is an ERR, not "ok".
  const call = async (expression) => {
    const t0 = Date.now();
    const reply = await Promise.race([
      evaluate(expression),
      new Promise((resolve) => setTimeout(() => resolve(null), 4000)),
    ]);
    if (reply === null) return 'TIMEOUT>4s';
    if (reply.error) return `ERR(${reply.error.message})`;
    if (reply.result?.exceptionDetails) return `ERR(${reply.result.exceptionDetails.exception?.description?.split('\n')[0] ?? 'exception'})`;
    return `${Date.now() - t0}ms`;
  };
  const iterations = Math.max(1, Math.ceil(Number(a || 30) / 3));
  let bad = 0;
  for (let n = 0; n < iterations; n++) {
    const renderer = await call('document.title');
    const command = await call("window.__TAURI_INTERNALS__.invoke('get_active_site', {})");
    if (/TIMEOUT|ERR/.test(renderer + command)) bad++;
    console.log(new Date().toTimeString().slice(0, 8), `renderer=${renderer} syncCommand=${command}`);
    await new Promise((resolve) => setTimeout(resolve, 3000));
  }
  process.exit(bad ? 3 : 0);
}

let expression;
let timeout = 60000;
if (mode === 'eval') { expression = a; timeout = Number(b || 60000); }
else {
  let parsed = {};
  try { parsed = JSON.parse(b || '{}'); } catch { console.error(`invoke args are not valid JSON: ${b}`); process.exit(1); }
  expression = `window.__TAURI_INTERNALS__.invoke(${JSON.stringify(a)}, ${JSON.stringify(parsed)})`;
  timeout = Number(c || 60000);
}
const t0 = Date.now();
const timer = setTimeout(() => { console.log(`TIMEOUT after ${Date.now() - t0}ms (page evaluate did not return)`); process.exit(3); }, timeout);
const reply = await evaluate(expression);
clearTimeout(timer);
console.log(`done in ${Date.now() - t0}ms`);
if (reply.error) {
  console.log(`CDP error: ${reply.error.message}`);
  process.exit(1);
}
if (reply.result?.exceptionDetails) {
  const e = reply.result.exceptionDetails;
  console.log(JSON.stringify(e.exception?.value ?? e.exception?.description ?? e.text, null, 1));
  process.exit(1);
}
console.log(JSON.stringify(reply.result?.result?.value ?? reply.result?.result, null, 1)?.slice(0, 3000));
process.exit(0);
