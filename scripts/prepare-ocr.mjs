#!/usr/bin/env node
/**
 * Assemble the offline OCR asset bundle in `public/ocr/` (WORKPLAN §5 "Assets", spike `docs/spikes/ocr.md`).
 *
 * SeePDF must OCR with **no network at runtime** (FEATURES F-21: "no network request is made at any
 * point"), so every file tesseract.js would otherwise pull from jsdelivr is copied next to the app and
 * served same-origin. `src/ocr/tesseractPool.ts` points `workerPath` / `corePath` / `langPath` here.
 *
 * Output (5 files, ~8.1 MB):
 *
 *   public/ocr/worker.min.js                      0.11 MB  tesseract.js worker entry (same-origin, no blob:)
 *   public/ocr/tesseract-core-simd-lstm.wasm.js   3.72 MB  LSTM-only WASM core, base64-embedded (no extra fetch)
 *   public/ocr/tessdata/eng.traineddata.gz        2.80 MB  4.0.0_best_int
 *   public/ocr/tessdata/kor.traineddata.gz        1.49 MB  4.0.0_best_int
 *   public/ocr/LICENSE.txt                        ~2 kB    Apache-2.0 notices for all of the above
 *
 * `--with-fallback-core` adds `tesseract-core-lstm.wasm.js` (+3.90 MB) for WebViews without WASM SIMD
 * (Safari < 16.4, i.e. macOS 12). See docs/STAGE1F_NOTES.md.
 *
 * Usage:
 *   node scripts/prepare-ocr.mjs                 # idempotent; re-runs are a no-op
 *   node scripts/prepare-ocr.mjs --force         # rebuild every file
 *   node scripts/prepare-ocr.mjs --with-fallback-core
 *   node scripts/prepare-ocr.mjs --quiet         # only warnings/errors (postinstall)
 *
 * Traineddata sources, in order: the spike cache (`fixtures/out/ocr/tessdata-cache/best_int`, gzipped
 * here), then jsdelivr `@tesseract.js-data/<lang>/4.0.0_best_int`, then GitHub `tessdata_best` raw.
 * NEVER the plain `4.0.0` (legacy+LSTM) files — they give 90 % CER with the LSTM-only core (spike §4.5).
 */
import { createHash } from "node:crypto";
import { copyFileSync, existsSync, mkdirSync, readFileSync, readdirSync, rmSync, statSync, writeFileSync } from "node:fs";
import { dirname, join, relative } from "node:path";
import { fileURLToPath } from "node:url";
import { gzipSync, gunzipSync } from "node:zlib";

const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..");
const OUT = join(ROOT, "public", "ocr");
const TESSDATA = join(OUT, "tessdata");
const CORE_DIR = join(ROOT, "node_modules", "tesseract.js-core");
const DIST_DIR = join(ROOT, "node_modules", "tesseract.js", "dist");
const SPIKE_CACHE = join(ROOT, "fixtures", "out", "ocr", "tessdata-cache", "best_int");

const argv = process.argv.slice(2);
const FORCE = argv.includes("--force");
const QUIET = argv.includes("--quiet");
const WITH_FALLBACK_CORE = argv.includes("--with-fallback-core");
/** `--probe` also stages the self-test fixture read by `src/ocr/probe.ts` (never in a real build). */
const WITH_PROBE = argv.includes("--probe");

/** The SIMD LSTM-only core is the one WKWebView and WebView2 both run (spike §3a, §4.8). */
const PRIMARY_CORE = "tesseract-core-simd-lstm.wasm.js";
/** Non-SIMD core for Safari < 16.4; opt-in because it is another 3.9 MB. */
const FALLBACK_CORE = "tesseract-core-lstm.wasm.js";
const LANGS = ["eng", "kor"];
/** A traineddata file smaller than this is a 404 page or a truncated download. */
const MIN_TRAINEDDATA_BYTES = 1_000_000;

const log = (...a) => { if (!QUIET) console.log(...a); };
const mb = (n) => (n / 1048576).toFixed(2) + " MB";
const sha8 = (buf) => createHash("sha256").update(buf).digest("hex").slice(0, 8);

function ensureDirs() {
  mkdirSync(TESSDATA, { recursive: true });
}

/** Copy `src` → `dest` unless the destination already holds the same bytes. Returns 'copied'|'skipped'. */
function copyIfChanged(src, dest) {
  if (!existsSync(src)) {
    throw new Error(
      `missing ${relative(ROOT, src)} — run \`npm install\` (tesseract.js 7 / tesseract.js-core 7 must be present)`,
    );
  }
  if (!FORCE && existsSync(dest) && statSync(dest).size === statSync(src).size) return "skipped";
  copyFileSync(src, dest);
  return "copied";
}

/** True when `buf` is a gzip stream that inflates into something that looks like a traineddata file. */
function looksLikeTraineddataGz(buf) {
  if (buf.length < 100_000 || buf[0] !== 0x1f || buf[1] !== 0x8b) return false;
  try {
    return gunzipSync(buf).length >= MIN_TRAINEDDATA_BYTES;
  } catch {
    return false;
  }
}

/** Three attempts with a growing pause: CDN downloads fail transiently on CI runners. */
async function fetchBuffer(url, attempts = 3) {
  for (let i = 1; ; i++) {
    try {
      const res = await fetch(url);
      if (!res.ok) throw new Error(`${url} → HTTP ${res.status}`);
      return Buffer.from(await res.arrayBuffer());
    } catch (e) {
      if (i >= attempts) throw e;
      console.warn(`[ocr] ${e.message} — retry ${i}/${attempts - 1}`);
      await new Promise((r) => setTimeout(r, 2000 * i));
    }
  }
}

/**
 * Put `<lang>.traineddata.gz` in `public/ocr/tessdata/`.
 * Cache (uncompressed, written by scripts/spike-ocr.mjs) → jsdelivr .gz → tessdata_best raw.
 */
async function prepareTraineddata(lang) {
  const dest = join(TESSDATA, `${lang}.traineddata.gz`);
  if (!FORCE && existsSync(dest) && looksLikeTraineddataGz(readFileSync(dest))) {
    return { source: "already present", status: "skipped" };
  }

  const cached = join(SPIKE_CACHE, `${lang}.traineddata`);
  if (existsSync(cached) && statSync(cached).size >= MIN_TRAINEDDATA_BYTES) {
    const raw = readFileSync(cached);
    writeFileSync(dest, gzipSync(raw, { level: 9 }));
    return { source: `cache ${relative(ROOT, cached)}`, status: "gzipped" };
  }

  const attempts = [
    {
      url: `https://cdn.jsdelivr.net/npm/@tesseract.js-data/${lang}/4.0.0_best_int/${lang}.traineddata.gz`,
      gz: true,
    },
    {
      url: `https://raw.githubusercontent.com/tesseract-ocr/tessdata_best/main/${lang}.traineddata`,
      gz: false,
    },
  ];
  const errors = [];
  for (const a of attempts) {
    try {
      log(`  fetching ${a.url}`);
      const buf = await fetchBuffer(a.url);
      const out = a.gz ? buf : gzipSync(buf, { level: 9 });
      if (!looksLikeTraineddataGz(out)) throw new Error("downloaded file is not a valid traineddata archive");
      writeFileSync(dest, out);
      return { source: a.url, status: "downloaded" };
    } catch (e) {
      errors.push(`${a.url}: ${e.message}`);
    }
  }
  throw new Error(
    `could not obtain ${lang}.traineddata (4.0.0_best_int).\n    ` + errors.join("\n    ") +
      `\n    Offline? Run \`node scripts/spike-ocr.mjs --images korean --langs kor+eng\` once on a networked\n` +
      `    machine to fill fixtures/out/ocr/tessdata-cache/best_int, or drop the .gz files into ${relative(ROOT, TESSDATA)}.`,
  );
}

function writeLicense(coreFiles) {
  const notice = [
    "Third-party assets bundled in public/ocr/ by scripts/prepare-ocr.mjs.",
    "",
    "1. worker.min.js — tesseract.js 7.0.0, Apache License 2.0",
    "   https://github.com/naptha/tesseract.js",
    `2. ${coreFiles.join(", ")} — tesseract.js-core 7.0.0 (Tesseract OCR compiled to WebAssembly), Apache License 2.0`,
    "   https://github.com/naptha/tesseract.js-core  https://github.com/tesseract-ocr/tesseract",
    "3. tessdata/eng.traineddata.gz, tessdata/kor.traineddata.gz — Tesseract 4.0.0_best_int trained models,",
    "   Apache License 2.0, https://github.com/tesseract-ocr/tessdata_best",
    "",
    "Apache License 2.0: http://www.apache.org/licenses/LICENSE-2.0",
    "Each file is redistributed unmodified except for gzip re-compression of the traineddata.",
    "",
  ].join("\n");
  const dest = join(OUT, "LICENSE.txt");
  const current = existsSync(dest) ? readFileSync(dest, "utf8") : null;
  if (current === notice) return "skipped";
  writeFileSync(dest, notice);
  return "written";
}

/**
 * Stage the in-bundle probe fixture (`--probe`): the synthetic Korean 300-DPI page and its ground
 * truth, served same-origin so `src/ocr/probe.ts` can OCR them inside a `tauri build --debug` bundle
 * under the production CSP. Never produced by a plain `npm run prepare:ocr`.
 */
function prepareProbe() {
  const dir = join(OUT, "probe");
  if (!WITH_PROBE) {
    if (existsSync(dir)) {
      rmSync(dir, { recursive: true });
      log("  removed probe/ (pass --probe to stage the self-test fixture)");
    }
    return "absent";
  }
  const srcDir = join(ROOT, "fixtures", "out", "ocr");
  const files = ["korean-300dpi.png", "korean.txt"];
  for (const f of files) {
    if (!existsSync(join(srcDir, f))) {
      throw new Error(`--probe needs fixtures/out/ocr/${f} — run: cd src-tauri && cargo run --example spike_ocr_render`);
    }
  }
  mkdirSync(dir, { recursive: true });
  for (const f of files) copyIfChanged(join(srcDir, f), join(dir, f));
  return "staged";
}

/** Remove the opt-in fallback core when it is present but not requested, so the listing stays honest. */
function pruneFallbackCore() {
  const p = join(OUT, FALLBACK_CORE);
  if (!WITH_FALLBACK_CORE && existsSync(p)) {
    rmSync(p);
    log(`  removed ${FALLBACK_CORE} (pass --with-fallback-core to keep it)`);
  }
}

function listing(dir, prefix = "") {
  const rows = [];
  for (const name of readdirSync(dir).sort()) {
    const p = join(dir, name);
    if (statSync(p).isDirectory()) rows.push(...listing(p, `${prefix}${name}/`));
    else rows.push({ file: `${prefix}${name}`, bytes: statSync(p).size, hash: sha8(readFileSync(p)) });
  }
  return rows;
}

async function main() {
  ensureDirs();
  log("[ocr] assembling public/ocr/ (offline tesseract.js assets)");

  const coreFiles = [PRIMARY_CORE, ...(WITH_FALLBACK_CORE ? [FALLBACK_CORE] : [])];
  log(`  worker.min.js                ${copyIfChanged(join(DIST_DIR, "worker.min.js"), join(OUT, "worker.min.js"))}`);
  for (const core of coreFiles) {
    log(`  ${core.padEnd(28)} ${copyIfChanged(join(CORE_DIR, core), join(OUT, core))}`);
  }
  pruneFallbackCore();

  for (const lang of LANGS) {
    const r = await prepareTraineddata(lang);
    log(`  ${`${lang}.traineddata.gz`.padEnd(28)} ${r.status} (${r.source})`);
  }
  log(`  LICENSE.txt                  ${writeLicense(coreFiles)}`);
  log(`  probe/                       ${prepareProbe()}`);

  const rows = listing(OUT);
  const total = rows.reduce((n, r) => n + r.bytes, 0);
  log("");
  for (const r of rows) log(`  ${mb(r.bytes).padStart(9)}  ${r.hash}  ${r.file}`);
  log(`  ${mb(total).padStart(9)}  ${rows.length} files total`);

  const expectedMb = WITH_FALLBACK_CORE ? 12.0 : 8.1;
  if (Math.abs(total / 1048576 - expectedMb) > 1.5) {
    console.warn(`[ocr] WARNING: expected ~${expectedMb} MB, got ${mb(total)} — check the asset list above`);
  }
  for (const lang of LANGS) {
    const p = join(TESSDATA, `${lang}.traineddata.gz`);
    if (!looksLikeTraineddataGz(readFileSync(p))) {
      throw new Error(`${relative(ROOT, p)} is not a usable traineddata archive`);
    }
  }
  log("[ocr] ok — no network needed at runtime");
}

main().catch((e) => {
  console.error(`[ocr] ERROR ${e.message}`);
  process.exit(1);
});
