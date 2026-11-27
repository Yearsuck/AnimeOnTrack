#!/usr/bin/env node
// Architecture fitness checks (no dependencies). Run: node tools/arch-check.mjs
//  - FAIL (exit 1): any file under src-tauri/src/db/ references
//    `crate::commands`, `crate::adapter` or `crate::scraper_engine`
//    (db is the lowest layer and must not import upward).
//  - FAIL (exit 1): pure modules (PURE_MODULES) reference crate::db, crate::commands,
//    tauri, reqwest or tokio.
//  - WARN (informational only): any .rs file under src-tauri/src over 3000 lines.
import { readdirSync, readFileSync, statSync } from "node:fs";
import { join, dirname, relative } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const src = join(root, "src-tauri", "src");
const forbidden = /crate::(commands|adapter|scraper_engine)\b/;
const MAX_LINES = 3000;
// Pure modules: no I/O or framework dependencies.
const PURE_MODULES = ["src-tauri/src/refresh_plan.rs"];
const impure = /crate::(db|commands)|tauri|reqwest|tokio/;

function* walk(dir) {
  for (const name of readdirSync(dir)) {
    const p = join(dir, name);
    if (statSync(p).isDirectory()) yield* walk(p);
    else if (name.endsWith(".rs")) yield p;
  }
}

let failures = 0;
for (const file of walk(src)) {
  const rel = relative(root, file).replaceAll("\\", "/");
  const lines = readFileSync(file, "utf8").split(/\r?\n/);
  if (lines.length > MAX_LINES) console.warn(`warn: ${rel} has ${lines.length} lines (> ${MAX_LINES})`);
  if (PURE_MODULES.includes(rel)) {
    lines.forEach((l, i) => {
      if (impure.test(l)) {
        console.error(`FAIL: ${rel}:${i + 1} pure module must not depend on I/O or framework code: ${l.trim()}`);
        failures++;
      }
    });
  }
  if (rel.startsWith("src-tauri/src/db/") || rel === "src-tauri/src/db.rs") {
    lines.forEach((l, i) => {
      if (forbidden.test(l)) {
        console.error(`FAIL: ${rel}:${i + 1} db must not depend upward: ${l.trim()}`);
        failures++;
      }
    });
  }
}
if (failures) process.exit(1);
console.log("arch-check: ok");
