#!/usr/bin/env node
// Downloads prebuilt PDFium (https://github.com/bblanchon/pdfium-binaries) into
// src-tauri/resources/pdfium/ for the current host (or all desktop targets with --all).
import { createWriteStream, existsSync, mkdirSync, readFileSync, renameSync, rmSync } from "node:fs";
import { execSync } from "node:child_process";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { pipeline } from "node:stream/promises";

const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..");
const OUT = join(ROOT, "src-tauri", "resources", "pdfium");
const VERSION_FILE = join(OUT, "VERSION");
const build = readFileSync(VERSION_FILE, "utf8").match(/BUILD=(\d+)/)?.[1] ?? "8057";
const TAG = `chromium/${build}`;

const TARGETS = {
  "aarch64-apple-darwin":       { asset: "pdfium-mac-arm64",  inner: "lib/libpdfium.dylib", out: "libpdfium-aarch64-apple-darwin.dylib" },
  "x86_64-apple-darwin":        { asset: "pdfium-mac-x64",    inner: "lib/libpdfium.dylib", out: "libpdfium-x86_64-apple-darwin.dylib" },
  "x86_64-pc-windows-msvc":     { asset: "pdfium-win-x64",    inner: "bin/pdfium.dll",      out: "pdfium-x86_64-pc-windows-msvc.dll" },
  "aarch64-pc-windows-msvc":    { asset: "pdfium-win-arm64",  inner: "bin/pdfium.dll",      out: "pdfium-aarch64-pc-windows-msvc.dll" },
  "x86_64-unknown-linux-gnu":   { asset: "pdfium-linux-x64",  inner: "lib/libpdfium.so",    out: "libpdfium-x86_64-unknown-linux-gnu.so" },
  "aarch64-unknown-linux-gnu":  { asset: "pdfium-linux-arm64",inner: "lib/libpdfium.so",    out: "libpdfium-aarch64-unknown-linux-gnu.so" },
};

function hostTriple() {
  const explicit = process.env.PDFIUM_TARGET;
  if (explicit) return explicit;
  const arch = process.arch === "arm64" ? "aarch64" : "x86_64";
  if (process.platform === "darwin") return `${arch}-apple-darwin`;
  if (process.platform === "win32") return `${arch}-pc-windows-msvc`;
  return `${arch}-unknown-linux-gnu`;
}

/**
 * GitHub release downloads fail transiently on CI runners now and then (a reset connection or a
 * 5xx from the CDN), which used to fail a whole `npm ci`. Three attempts with a growing pause.
 */
async function withRetry(what, fn, attempts = 3) {
  for (let i = 1; ; i++) {
    try {
      return await fn();
    } catch (e) {
      if (i >= attempts) throw e;
      console.warn(`[pdfium] ${what}: ${e.message} — retry ${i}/${attempts - 1}`);
      await new Promise((r) => setTimeout(r, 2000 * i));
    }
  }
}

async function fetchTarget(triple) {
  const t = TARGETS[triple];
  if (!t) throw new Error(`unsupported target ${triple}`);
  const dest = join(OUT, t.out);
  if (existsSync(dest) && !process.argv.includes("--force")) {
    console.log(`[pdfium] ${t.out} already present`);
    return;
  }
  const url = `https://github.com/bblanchon/pdfium-binaries/releases/download/${encodeURIComponent(TAG)}/${t.asset}.tgz`;
  console.log(`[pdfium] downloading ${url}`);
  const tmpDir = join(OUT, `.tmp-${triple}`);
  rmSync(tmpDir, { recursive: true, force: true });
  mkdirSync(tmpDir, { recursive: true });
  const tgz = join(tmpDir, "pdfium.tgz");
  await withRetry(t.asset, async () => {
    const res = await fetch(url, { redirect: "follow" });
    if (!res.ok) throw new Error(`download failed: ${res.status} ${res.statusText}`);
    await pipeline(res.body, createWriteStream(tgz));
  });
  execSync(`tar -xzf "${tgz}" -C "${tmpDir}"`, { stdio: "inherit" });
  renameSync(join(tmpDir, t.inner), dest);
  rmSync(tmpDir, { recursive: true, force: true });
  console.log(`[pdfium] -> ${dest}`);
}

mkdirSync(OUT, { recursive: true });
const wanted = process.argv.includes("--all") ? Object.keys(TARGETS) : [hostTriple()];
for (const triple of wanted) {
  try { await fetchTarget(triple); } catch (e) { console.error(`[pdfium] ${triple}: ${e.message}`); if (!process.argv.includes("--all")) process.exit(1); }
}
