#!/usr/bin/env node
/**
 * i18n gate (UI_SPEC §15, WORKPLAN 0.11). Fails CI on:
 *   - a key that exists in one locale and not the other
 *   - `{{placeholder}}` drift between locales
 *   - an `_other` plural without its singular
 *   - an empty string
 *   - a `t("literal.key")` — or a `…Key: "literal.key"` / `…Key="literal.key"` prop — in src/
 *     that no catalogue defines
 *
 * And reports the keys nothing references (v0.3 pkg8, H6). A key counts as used when
 *   - it appears as a string literal anywhere in src/ (non-test .ts/.tsx): `t("…")`, `labelKey`,
 *     `messageKey`, `toast("…")`, `cond ? "a.b" : "c.d"`, key tables — any literal that names a key;
 *   - a template literal starts with its prefix: t(`stamp.role.${r}`) marks every `stamp.role.*`;
 *   - it appears as a string literal in src-tauri/src (the Rust side reads the same catalogues:
 *     undo labels, menu titles, annotation kinds), or a Rust `format!("prefix.{…}")` covers it;
 *   - it is the `_other` plural of a used key.
 *
 * Usage: node scripts/check-i18n.mjs [--quiet] [--list-unused] [--strict]
 *   --list-unused  print every unreferenced key
 *   --strict       unreferenced keys are errors (after the dead-key cleanup, H6)
 */
import { readFileSync, readdirSync, statSync } from "node:fs";
import { dirname, extname, join } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..");
const I18N = join(ROOT, "src", "i18n");
const SRC = join(ROOT, "src");
const RUST = join(ROOT, "src-tauri", "src");
const quiet = process.argv.includes("--quiet");
const listUnused = process.argv.includes("--list-unused");
const strict = process.argv.includes("--strict");

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

function walk(dir, exts, out = []) {
  for (const entry of readdirSync(dir)) {
    const path = join(dir, entry);
    if (statSync(path).isDirectory()) walk(path, exts, out);
    else if (exts.includes(extname(path)) && !/\.(test|spec)\.tsx?$/.test(path)) out.push(path);
  }
  return out;
}

const KEY = /^[a-zA-Z][\w]*(\.[\w]+)+$/;
/** key → first file that references it */
const used = new Map();
/** template / format! prefixes ("stamp.role.") */
const prefixes = new Map();
const mark = (key, file) => {
  if (!used.has(key)) used.set(key, file);
};

for (const file of walk(SRC, [".ts", ".tsx"])) {
  const rel = file.slice(ROOT.length + 1);
  const source = readFileSync(file, "utf8");
  // Explicit uses must resolve: t("…") and …Key props.
  for (const m of source.matchAll(/\bt\(\s*"([a-zA-Z][\w.]*)"/g)) {
    mark(m[1], rel);
    if (!(m[1] in ko)) errors.push(`unknown key used in ${rel}: ${m[1]}`);
  }
  for (const m of source.matchAll(/\b\w*Key\s*(?::|=)\s*\{?\s*"([a-zA-Z][\w]*(?:\.[\w]+)+)"/g)) {
    mark(m[1], rel);
    if (!(m[1] in ko)) errors.push(`unknown key used in ${rel}: ${m[1]}`);
  }
  // Any other string literal that names a key (toast("…"), tables, ternaries).
  for (const m of source.matchAll(/["']([a-zA-Z][\w]*(?:\.[\w]+)+)["']/g)) {
    if (m[1] in ko) mark(m[1], rel);
  }
  // Template literals with a key prefix: `x.y.${…}`.
  for (const m of source.matchAll(/`([a-zA-Z][\w]*(?:\.[\w]+)*\.)\$\{/g)) {
    if (!prefixes.has(m[1])) prefixes.set(m[1], rel);
  }
}

for (const file of walk(RUST, [".rs"])) {
  const rel = file.slice(ROOT.length + 1);
  const source = readFileSync(file, "utf8");
  for (const m of source.matchAll(/"([a-zA-Z][\w]*(?:\.[\w]+)+)"/g)) {
    if (m[1] in ko) mark(m[1], rel);
  }
  // format!("annot.kind.{}", …) / format!("undo.{kind}")
  for (const m of source.matchAll(/"([a-zA-Z][\w]*(?:\.[\w]+)*\.)\{/g)) {
    if (!prefixes.has(m[1])) prefixes.set(m[1], rel);
  }
}

const isUsed = (key) => {
  if (used.has(key)) return true;
  for (const prefix of prefixes.keys()) if (key.startsWith(prefix)) return true;
  if (key.endsWith("_other")) return isUsed(key.slice(0, -"_other".length));
  return false;
};

const unused = Object.keys(ko).filter((k) => KEY.test(k) && !isUsed(k));
if (unused.length) {
  const line = `${unused.length} keys are not referenced`;
  if (strict) errors.push(line);
  else warnings.push(line);
}

if (!quiet) {
  console.log(
    `[i18n] ko ${Object.keys(ko).length} keys · en ${Object.keys(en).length} keys · ` +
      `${Object.keys(ko).length - unused.length} referenced (${used.size} literally, ${prefixes.size} template prefixes)`,
  );
  for (const w of warnings) console.log(`[i18n] note: ${w}`);
  if (listUnused) for (const k of unused) console.log(`[i18n] unused: ${k}`);
}

if (errors.length) {
  for (const e of errors) console.error(`[i18n] ERROR ${e}`);
  console.error(`[i18n] ${errors.length} problem(s)`);
  process.exit(1);
}

if (!quiet) console.log("[i18n] ok");
