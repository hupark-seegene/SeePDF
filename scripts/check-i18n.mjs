#!/usr/bin/env node
/**
 * i18n gate (UI_SPEC §15, WORKPLAN 0.11). Fails CI on:
 *   - a key that exists in one locale and not the other
 *   - `{{placeholder}}` drift between locales
 *   - an `_other` plural without its singular
 *   - an empty string
 *   - a `t("literal.key")` in src/ that no catalogue defines
 *
 * Usage: node scripts/check-i18n.mjs [--quiet]
 */
import { readFileSync, readdirSync, statSync } from "node:fs";
import { dirname, extname, join } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..");
const I18N = join(ROOT, "src", "i18n");
const SRC = join(ROOT, "src");
const quiet = process.argv.includes("--quiet");

const ko = JSON.parse(readFileSync(join(I18N, "ko.json"), "utf8"));
const en = JSON.parse(readFileSync(join(I18N, "en.json"), "utf8"));

const errors = [];
const warnings = [];

const placeholders = (s) => [...s.matchAll(/\{\{(\w+)\}\}/g)].map((m) => m[1]).sort().join(",");

for (const key of Object.keys(ko)) if (!(key in en)) errors.push(`missing in en.json: ${key}`);
for (const key of Object.keys(en)) if (!(key in ko)) errors.push(`missing in ko.json: ${key}`);

for (const [key, value] of Object.entries(ko)) {
  if (!(key in en)) continue;
  if (placeholders(value) !== placeholders(en[key])) {
    errors.push(`placeholder drift: ${key} (ko "${placeholders(value)}" vs en "${placeholders(en[key])}")`);
  }
  if (!value.trim()) errors.push(`empty ko value: ${key}`);
  if (!en[key].trim()) errors.push(`empty en value: ${key}`);
  if (key.endsWith("_other") && !(key.slice(0, -"_other".length) in ko)) {
    errors.push(`plural without singular: ${key}`);
  }
}

// Every literal t("…") in the app must resolve.
function walk(dir, out = []) {
  for (const entry of readdirSync(dir)) {
    const path = join(dir, entry);
    if (statSync(path).isDirectory()) walk(path, out);
    else if ([".ts", ".tsx"].includes(extname(path)) && !/\.(test|spec)\.tsx?$/.test(path)) out.push(path);
  }
  return out;
}

const used = new Map();
for (const file of walk(SRC)) {
  const source = readFileSync(file, "utf8");
  for (const m of source.matchAll(/\bt\(\s*"([a-zA-Z][\w.]*)"/g)) {
    if (!used.has(m[1])) used.set(m[1], file.slice(ROOT.length + 1));
  }
}
for (const [key, file] of used) {
  if (!(key in ko)) errors.push(`unknown key used in ${file}: ${key}`);
}

const unused = Object.keys(ko).filter((k) => !used.has(k) && !k.endsWith("_other"));
if (unused.length) warnings.push(`${unused.length} keys are not referenced yet (Stage 1 owns most of them)`);

if (!quiet) {
  console.log(`[i18n] ko ${Object.keys(ko).length} keys · en ${Object.keys(en).length} keys · ${used.size} referenced`);
  for (const w of warnings) console.log(`[i18n] note: ${w}`);
}

if (errors.length) {
  for (const e of errors) console.error(`[i18n] ERROR ${e}`);
  console.error(`[i18n] ${errors.length} problem(s)`);
  process.exit(1);
}

if (!quiet) console.log("[i18n] ok");
