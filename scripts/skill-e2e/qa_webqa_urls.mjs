#!/usr/bin/env node
// Runs the shared allowlist cases (fixtures/webqa/urls.json) against the driver's isAllowedUrl; prints one line per
// wrong verdict and exits 1 when there is one. The Rust side runs the same file (src-tauri/src/store/settings_store.rs).
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { isAllowedUrl } from "../../plugin/scripts/qa-browser.mjs";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");
const { cases } = JSON.parse(fs.readFileSync(path.join(root, "fixtures/webqa/urls.json"), "utf8"));
let wrong = 0;
for (const c of cases) {
  const got = isAllowedUrl(c.url, c.allowlist);
  if (got !== c.allowed) {
    wrong++;
    console.log(`WRONG ${JSON.stringify(c.url)} for ${JSON.stringify(c.allowlist)}: got ${got}, want ${c.allowed} (${c.why})`);
  }
}
console.log(`${cases.length - wrong}/${cases.length} cases agree`);
process.exit(wrong ? 1 : 0);
