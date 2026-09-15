#!/usr/bin/env node
/**
 * Frontend size gate (ARCHITECTURE §12 "frontend ~2 MB", §13 "initial JS ≤ 120 kB gz").
 * Run after `npm run build`; exits 1 when a budget is exceeded.
 *
 * Usage: node scripts/check-bundle-size.mjs [--json] [--dist dist]
 */
import { readdirSync, readFileSync, statSync } from "node:fs";
import { gzipSync } from "node:zlib";
import { dirname, extname, join, relative } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..");
const distArg = process.argv.indexOf("--dist");
const DIST = join(ROOT, distArg > -1 ? process.argv[distArg + 1] : "dist");

/** kB, gzipped, of the JS the window must download before the welcome screen paints. */
const BUDGET_ENTRY_JS_GZ_KB = 120;
/** kB, everything the frontend contributes to the bundle (fonts and OCR data are counted elsewhere). */
const BUDGET_TOTAL_KB = 2048;

function walk(dir, out = []) {
  for (const entry of readdirSync(dir)) {
    if (dir === DIST && (entry === "ocr" || entry === "fonts")) continue; // counted elsewhere (OCR data, fonts)
    const path = join(dir, entry);
    if (statSync(path).isDirectory()) walk(path, out);
    else out.push(path);
  }
  return out;
}

let files;
try {
  files = walk(DIST);
} catch {
  console.error(`[bundle] no build output at ${relative(ROOT, DIST)} — run \`npm run build\` first`);
  process.exit(1);
}

const rows = files.map((path) => {
  const bytes = readFileSync(path);
  return {
    file: relative(DIST, path),
    kind: extname(path).replace(".", "") || "other",
    bytes: bytes.length,
    gzBytes: gzipSync(bytes, { level: 9 }).length,
  };
});

const kb = (n) => Math.round((n / 1024) * 10) / 10;
const total = rows.reduce((n, r) => n + r.bytes, 0);
const entry = rows
  .filter((r) => r.kind === "js" && /^assets\/index[-.]/.test(r.file))
  .sort((a, b) => b.bytes - a.bytes)[0]
  ?? rows.filter((r) => r.kind === "js").sort((a, b) => b.bytes - a.bytes)[0];
const css = rows.filter((r) => r.kind === "css").reduce((n, r) => n + r.gzBytes, 0);
const entryGz = (entry?.gzBytes ?? 0) + css;

const report = {
  totalKb: kb(total),
  entryJsGzKb: kb(entry?.gzBytes ?? 0),
  cssGzKb: kb(css),
  criticalGzKb: kb(entryGz),
  budgets: { entryGzKb: BUDGET_ENTRY_JS_GZ_KB, totalKb: BUDGET_TOTAL_KB },
  files: rows.sort((a, b) => b.bytes - a.bytes).slice(0, 10).map((r) => ({ file: r.file, kb: kb(r.bytes), gzKb: kb(r.gzBytes) })),
};

/**
 * `npm run tauri build -- --debug` leaves an **unminified, source-mapped** `dist/` behind
 * (`vite.config.ts` keys `minify`/`sourcemap` off `TAURI_ENV_DEBUG`). Measuring that against a
 * production budget is meaningless — it is 172 kB gz critical path and 3.6 MB total on a tree
 * that ships 106 kB and 597 kB — so say what happened instead of failing with a size the
 * author cannot act on. CI runs `npm run build` between the two steps for the same reason.
 */
const sourcemaps = rows.filter((r) => r.file.endsWith(".map")).length;
if (sourcemaps > 0) {
  console.error(
    `[bundle] ERROR ${relative(ROOT, DIST)} is a **debug** build (${sourcemaps} sourcemaps, not minified).\n` +
      "[bundle]       `npm run tauri build -- --debug` writes one; run `npm run build` before this gate.",
  );
  process.exit(2);
}

const asJson = process.argv.includes("--json");
if (asJson) {
  console.log(JSON.stringify(report, null, 2));
} else {
  console.log(`[bundle] dist total ${report.totalKb} kB (budget ${BUDGET_TOTAL_KB} kB)`);
  console.log(`[bundle] critical path ${report.criticalGzKb} kB gz — entry JS ${report.entryJsGzKb} + CSS ${report.cssGzKb} (budget ${BUDGET_ENTRY_JS_GZ_KB} kB gz)`);
  for (const f of report.files) console.log(`          ${String(f.kb).padStart(8)} kB  ${String(f.gzKb).padStart(7)} kB gz  ${f.file}`);
}

const failures = [];
if (report.criticalGzKb > BUDGET_ENTRY_JS_GZ_KB) {
  failures.push(`critical path ${report.criticalGzKb} kB gz > ${BUDGET_ENTRY_JS_GZ_KB} kB gz`);
}
if (report.totalKb > BUDGET_TOTAL_KB) failures.push(`dist ${report.totalKb} kB > ${BUDGET_TOTAL_KB} kB`);

if (failures.length) {
  for (const f of failures) console.error(`[bundle] ERROR ${f}`);
  process.exit(1);
}
if (!asJson) console.log("[bundle] ok");
