#!/usr/bin/env node
// Ratchet: counts hex / rgb() / rgba() colour literals that sit outside
// :root / [data-theme] blocks in src/styles.css and src/styles/*.css, and fails
// if the total exceeds BASELINE. Colours belong in tokens; the number may only
// go down. After lowering it, update BASELINE to the new count.
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const BASELINE = 36; // frozen after the PR 6 token migration

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const files = [
  "src/styles.css",
  ...fs
    .readdirSync(path.join(root, "src/styles"))
    .filter((f) => f.endsWith(".css"))
    .sort()
    .map((f) => `src/styles/${f}`),
];

const LITERAL = /#[0-9a-fA-F]{3,8}\b|\brgba?\(/g;

function countFile(rel) {
  const src = fs
    .readFileSync(path.join(root, rel), "utf8")
    .replace(/\/\*[\s\S]*?\*\//g, (m) => m.replace(/[^\n]/g, " "));
  const stack = [];
  let count = 0;
  let prelude = "";
  let seg = "";
  const flush = () => {
    if (!stack.some((p) => /:root|\[data-theme/.test(p))) {
      count += (seg.match(LITERAL) || []).length;
    }
    seg = "";
  };
  for (const ch of src) {
    if (ch === "{") {
      stack.push(prelude + seg);
      prelude = "";
      seg = "";
    } else if (ch === "}") {
      flush();
      stack.pop();
      prelude = "";
    } else if (ch === ";") {
      flush();
      prelude = "";
    } else {
      seg += ch;
    }
  }
  return count;
}

let total = 0;
for (const f of files) {
  const n = countFile(f);
  total += n;
  if (n) console.log(`${String(n).padStart(4)}  ${f}`);
}
console.log(`total ${total} (baseline ${BASELINE})`);
if (total > BASELINE) {
  console.error(
    `FAIL: ${total - BASELINE} new colour literal(s) outside :root/[data-theme]. Use a token from :root.`,
  );
  process.exit(1);
}
if (total < BASELINE) {
  console.log(`Lower BASELINE in tools/check-css-tokens.mjs to ${total}.`);
}
