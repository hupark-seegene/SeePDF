#!/usr/bin/env node
/**
 * Prints the CHANGELOG.md section of one version — the release notes of its GitHub release and the
 * `notes` of latest.json (release.yml). Prints nothing (exit 0) when the version has no section, so
 * a release never fails over its notes; `--require` makes a missing section an error instead.
 *
 *   node scripts/changelog-section.mjs 0.3.0 [--file CHANGELOG.md] [--require]
 */
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..");

/** The body of `## [version]` (or `## version`), up to the next `## ` heading / link references. */
export function changelogSection(text, version) {
  const lines = text.split(/\r?\n/);
  const escaped = version.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const heading = new RegExp(`^## \\[?v?${escaped}\\]?(\\s|$)`);
  const start = lines.findIndex((l) => heading.test(l));
  if (start < 0) return "";
  const body = [];
  for (const line of lines.slice(start + 1)) {
    if (/^## /.test(line) || /^\[[^\]]+\]:\s/.test(line)) break;
    body.push(line);
  }
  return body.join("\n").trim();
}

if (process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1]) {
  const args = process.argv.slice(2);
  const version = args.find((a) => !a.startsWith("--") && args[args.indexOf(a) - 1] !== "--file");
  const fileAt = args.indexOf("--file");
  const file = fileAt > -1 ? args[fileAt + 1] : join(ROOT, "CHANGELOG.md");
  if (!version) {
    console.error("usage: node scripts/changelog-section.mjs <version> [--file CHANGELOG.md] [--require]");
    process.exit(2);
  }
  const section = changelogSection(readFileSync(file, "utf8"), version.replace(/^v/, ""));
  if (!section && args.includes("--require")) {
    console.error(`[changelog] no section for ${version} in ${file}`);
    process.exit(1);
  }
  if (section) process.stdout.write(`${section}\n`);
}
