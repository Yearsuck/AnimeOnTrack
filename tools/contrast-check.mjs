// WCAG 2.x contrast check of the design tokens in src/styles.css (no deps).
// Usage: node tools/contrast-check.mjs   (exit 1 if a required pair fails)
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const css = readFileSync(join(dirname(fileURLToPath(import.meta.url)), "..", "src", "styles.css"), "utf8");

function block(selectorRe) {
  const m = selectorRe.exec(css);
  if (!m) throw new Error(`block not found: ${selectorRe}`);
  const start = css.indexOf("{", m.index) + 1;
  const end = css.indexOf("\n}", start);
  const tokens = {};
  for (const t of css.slice(start, end).matchAll(/(--[\w-]+)\s*:\s*([^;]+);/g)) tokens[t[1]] = t[2].trim();
  return tokens;
}
const dark = block(/^:root\s*\{/m);
const light = { ...dark, ...block(/^:root\[data-theme="light"\]\s*\{/m) };

function parse(v) {
  const h = /^#([0-9a-f]{6})([0-9a-f]{2})?$/i.exec(v);
  if (!h) throw new Error(`unsupported color: ${v}`);
  const n = parseInt(h[1], 16);
  return { rgb: [n >> 16, (n >> 8) & 255, n & 255], a: h[2] ? parseInt(h[2], 16) / 255 : 1 };
}
const lum = ([r, g, b]) =>
  [r, g, b].map((c) => ((c /= 255) <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4)).reduce((s, c, i) => s + c * [0.2126, 0.7152, 0.0722][i], 0);
const ratio = (a, b) => {
  const [x, y] = [lum(a), lum(b)].sort((p, q) => q - p);
  return (x + 0.05) / (y + 0.05);
};
// Resolve a token to opaque rgb, compositing translucent colors over `under`.
function resolve(t, name, under) {
  const c = parse(t[name]);
  if (c.a >= 1) return c.rgb;
  const u = under;
  return c.rgb.map((v, i) => Math.round(v * c.a + u[i] * (1 - c.a)));
}

const TEXT = 4.5, UI = 3;
const pairs = [
  // [fg, bg, min]
  ["--text", "--bg", TEXT], ["--text", "--surface", TEXT], ["--text", "--surface-2", TEXT],
  ["--muted", "--bg", TEXT], ["--muted", "--surface", TEXT], ["--muted", "--surface-2", TEXT], ["--muted", "--surface-3", TEXT],
  ["--muted-2", "--bg", TEXT], ["--muted-2", "--surface", TEXT],
  ["--accent", "--bg", TEXT], ["--accent", "--surface", TEXT], ["--accent", "--surface-2", TEXT],
  ["--success", "--bg", TEXT], ["--success", "--surface", TEXT], ["--success", "--surface-2", TEXT],
  ["--danger", "--bg", TEXT], ["--danger", "--surface", TEXT], ["--danger", "--surface-2", TEXT],
  ["--on-accent", "--accent-solid", TEXT], ["--on-accent", "--accent-solid-hover", TEXT],
  ["--border-strong", "--surface", UI], ["--border-strong", "--bg", UI],
];
let fail = 0;
for (const [name, t] of [["dark", dark], ["light", light]]) {
  console.log(`\n${name}`);
  for (const [fg, bg, min] of pairs) {
    const b = resolve(t, bg, [0, 0, 0]);
    const r = ratio(resolve(t, fg, b), b);
    const ok = r >= min;
    if (!ok) fail++;
    console.log(`${ok ? "ok  " : "FAIL"} ${r.toFixed(2).padStart(6)} (>= ${min})  ${fg} on ${bg}`);
  }
}
if (fail) { console.error(`\n${fail} pair(s) below threshold`); process.exit(1); }
console.log("\nall pairs pass");
