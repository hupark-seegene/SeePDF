#!/usr/bin/env node
/**
 * Writes `latest.json`, the manifest tauri-plugin-updater reads from
 * `https://github.com/<repo>/releases/latest/download/latest.json`.
 *
 *   node scripts/release-latest-json.mjs --dir release-assets --version 0.2.0 --tag v0.2.0 \
 *     --repo hupark-seegene/SeePDF [--notes-file notes.md]
 *
 * Every updater bundle in `--dir` that has a `.sig` beside it becomes a platform entry:
 *
 *   SeePDF_0.2.0_x64-setup.exe(.sig)        windows-x86_64-nsis, and windows-x86_64
 *   SeePDF_0.2.0_x64_en-US.msi(.sig)        windows-x86_64-msi
 *   SeePDF_0.2.0_aarch64.app.tar.gz(.sig)   darwin-aarch64-app, and darwin-aarch64
 *   SeePDF_0.2.0_x64.app.tar.gz(.sig)       darwin-x86_64-app, and darwin-x86_64 (Intel, v0.3)
 *
 * The WebView2-offline installer (`SeePDF_0.3.0_x64-offline-setup.exe`, v0.3) is never an updater
 * target: an installed copy already has WebView2, and the plain setup.exe is the one to update with.
 *
 * The updater first looks for `<os>-<arch>-<installer>` and then `<os>-<arch>`, so the plain
 * Windows key points at the NSIS installer (the per-user one that needs no admin prompt).
 *
 * No `.sig` files (the signing key secret is not configured) → no manifest, exit 0: the release
 * still ships its installers, it just cannot update anyone.
 */
import { existsSync, readdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";

function arg(name, fallback) {
  const i = process.argv.indexOf(`--${name}`);
  if (i === -1) return fallback;
  return process.argv[i + 1];
}

const dir = arg("dir", "release-assets");
const version = arg("version");
const tag = arg("tag");
const repo = arg("repo");
const notesFile = arg("notes-file");
if (!version || !tag || !repo) {
  console.error("usage: release-latest-json.mjs --dir <dir> --version <x.y.z> --tag <tag> --repo <owner/name>");
  process.exit(2);
}

const ARCH = { x64: "x86_64", x86_64: "x86_64", aarch64: "aarch64", arm64: "aarch64" };

/** `{ keys, preferred }` for one updater bundle name, or null for anything else. */
function classify(name) {
  if (/-offline-setup\.exe$/.test(name)) return null;
  let m = name.match(/_(x64|x86_64|aarch64|arm64)-setup\.exe$/);
  if (m) return { keys: [`windows-${ARCH[m[1]]}-nsis`], plain: `windows-${ARCH[m[1]]}`, rank: 2 };
  m = name.match(/_(x64|x86_64|aarch64|arm64)_[A-Za-z-]+\.msi$/);
  if (m) return { keys: [`windows-${ARCH[m[1]]}-msi`], plain: `windows-${ARCH[m[1]]}`, rank: 1 };
  m = name.match(/_(x64|x86_64|aarch64|arm64)\.app\.tar\.gz$/);
  if (m) return { keys: [`darwin-${ARCH[m[1]]}-app`], plain: `darwin-${ARCH[m[1]]}`, rank: 2 };
  return null;
}

const files = existsSync(dir) ? readdirSync(dir) : [];
const platforms = {};
const plainRank = {};
for (const sigName of files.filter((f) => f.endsWith(".sig")).sort()) {
  const asset = sigName.slice(0, -".sig".length);
  if (!files.includes(asset)) {
    console.warn(`[latest.json] ${sigName} has no ${asset} beside it; skipped`);
    continue;
  }
  const kind = classify(asset);
  if (!kind) {
    console.warn(`[latest.json] ${asset}: not an updater bundle this script knows; skipped`);
    continue;
  }
  const entry = {
    signature: readFileSync(join(dir, sigName), "utf8").trim(),
    url: `https://github.com/${repo}/releases/download/${encodeURIComponent(tag)}/${encodeURIComponent(asset)}`,
  };
  for (const key of kind.keys) platforms[key] = entry;
  if ((plainRank[kind.plain] ?? 0) < kind.rank) {
    platforms[kind.plain] = entry;
    plainRank[kind.plain] = kind.rank;
  }
}

if (Object.keys(platforms).length === 0) {
  console.log("[latest.json] no signed updater bundles (TAURI_SIGNING_PRIVATE_KEY not set?) - not written");
  process.exit(0);
}

const notes = notesFile && existsSync(notesFile) ? readFileSync(notesFile, "utf8").trim() : `SeePDF ${version}`;
const manifest = { version, notes, pub_date: new Date().toISOString().replace(/\.\d{3}Z$/, "Z"), platforms };
writeFileSync(join(dir, "latest.json"), JSON.stringify(manifest, null, 2) + "\n");
console.log(`[latest.json] ${Object.keys(platforms).sort().join(", ")}`);
