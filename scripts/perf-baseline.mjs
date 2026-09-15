#!/usr/bin/env node
/**
 * Performance baseline harness — F-30 (WORKPLAN 0.11, ARCHITECTURE §13).
 *
 * Four sources, each measuring the thing where it actually lives:
 *
 *   1. **engine** — `cargo run --release --example perf_bench` drives the real engine thread
 *      against the real bundled PDFium with no window: open, tile/page/thumbnail render
 *      distributions at 2× and 4×, text-layer build, search over every page, save round trip
 *      and RSS on a 500-page document. Headless is both faster and far less noisy than the UI
 *      for these, and it is the same `tiles::render` the `seepdf://` handler calls.
 *   2. **app** — `scripts/dev-bridge.mjs` + `src/dev/testHook.ts` drive the **running** app for
 *      the two rows that only exist in the UI: open → first painted pixel, and the process RSS
 *      after opening 500 pages and scrolling through them. Needs `npm run tauri dev` in another
 *      terminal; skipped (and marked so) when the bridge does not answer.
 *   3. **ocr** — `scripts/ocr-accuracy.mjs --json` recognises the synthetic Korean 300 DPI page
 *      with tesseract.js in Node, which is the same worker pool and the same traineddata the
 *      app loads.
 *   4. **bundle** — `scripts/check-bundle-size.mjs --json`.
 *
 * Usage:
 *   node scripts/perf-baseline.mjs                 # measure and print the table
 *   node scripts/perf-baseline.mjs --write         # + write docs/perf/baseline.{md,json}
 *   node scripts/perf-baseline.mjs --check         # CI: compare against the recorded baseline
 *   node scripts/perf-baseline.mjs --skip-ocr --skip-engine --skip-app
 *
 * `--check` fails only on rows flagged `gate: true` — see GATE_NOTE below for why the
 * wall-clock rows are informational on a shared CI runner.
 */
import { execFileSync } from "node:child_process";
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { arch, cpus, platform, totalmem } from "node:os";

const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..");
const OUT_MD = join(ROOT, "docs", "perf", "baseline.md");
const OUT_JSON = join(ROOT, "docs", "perf", "baseline.json");
const BRIDGE = process.env.SEEPDF_DEV_BRIDGE ?? "http://localhost:1420/__dev/cmd";

const flags = new Set(process.argv.slice(2).filter((a) => a.startsWith("--")));
const write = flags.has("--write");
const check = flags.has("--check");
const TOLERANCE = 0.2;

const GATE_NOTE = `Only the rows marked **gate** are compared by CI. Every wall-clock row is
**informational**: a GitHub-hosted runner shares its CPU with other jobs, and the fastest rows
here are a third of a millisecond — a ±20 % band around 0.34 ms is noise, not a regression
signal. The gated rows are counts and sizes (search hits, serialised bytes, page counts, the
frontend bundle), where ±20 % means "nothing moved" and a real change is always a deliberate
one. The timings are still printed with their delta, so a 3× regression is obvious in the log.`;

/**
 * Every row of the baseline. `target` is the budget from `ARCHITECTURE.md` §13 /
 * `FEATURES.md`; `budgetMs` (when set) is that budget as a number, so the table can say
 * whether the measurement meets it.
 */
const ROWS = [
  // --- frontend bundle (always available once `dist/` exists) -------------------------
  { id: "bundle.criticalGz", scenario: "Frontend critical path (entry JS + CSS, gz)", unit: "kB", target: "≤ 120 kB", budget: 120, source: "bundle", gate: true },
  { id: "bundle.dist", scenario: "Frontend dist total", unit: "kB", target: "≤ 2048 kB", budget: 2048, source: "bundle", gate: true },

  // --- open ---------------------------------------------------------------------------
  { id: "open.tracemonkey.ms", scenario: "Open 14-page PDF (engine only)", unit: "ms", target: "≤ 250 ms", budget: 250, source: "engine" },
  { id: "open.500p.ms", scenario: "Open 500-page PDF (engine only)", unit: "ms", target: "≤ 400 ms", budget: 400, source: "engine" },
  { id: "app.open.tracemonkey.firstPaintMs", scenario: "Open 14-page PDF → first page painted (app)", unit: "ms", target: "≤ 250 ms", budget: 250, source: "app" },
  { id: "app.open.500p.firstPaintMs", scenario: "Open 500-page PDF → first page painted (app)", unit: "ms", target: "≤ 400 ms", budget: 400, source: "app" },

  // --- render -------------------------------------------------------------------------
  { id: "render.page.2x.p50.ms", scenario: "Whole-page render A4 @2× — p50", unit: "ms", target: "≤ 30 ms", budget: 30, source: "engine" },
  { id: "render.page.2x.p95.ms", scenario: "Whole-page render A4 @2× — p95", unit: "ms", target: "≤ 120 ms", budget: 120, source: "engine" },
  { id: "render.tile.2x.p50.ms", scenario: "512 px tile @2× — p50", unit: "ms", target: "≤ 30 ms", budget: 30, source: "engine" },
  { id: "render.tile.2x.p95.ms", scenario: "512 px tile @2× — p95", unit: "ms", target: "≤ 120 ms", budget: 120, source: "engine" },
  { id: "render.tile.4x.p50.ms", scenario: "512 px tile @4× — p50", unit: "ms", target: "≤ 30 ms", budget: 30, source: "engine" },
  { id: "render.tile.4x.p95.ms", scenario: "512 px tile @4× — p95", unit: "ms", target: "≤ 120 ms", budget: 120, source: "engine" },
  { id: "render.thumb.p50.ms", scenario: "Thumbnail render, 240 px — p50", unit: "ms", target: "≤ 36 ms (500/14)", budget: 36, source: "engine" },
  { id: "render.thumb.p95.ms", scenario: "Thumbnail render, 240 px — p95", unit: "ms", target: "—", source: "engine" },
  { id: "render.500p.firstPage.ms", scenario: "500-page doc, page 0 @2×", unit: "ms", target: "≤ 30 ms", budget: 30, source: "engine" },

  // --- text, search -------------------------------------------------------------------
  { id: "text.layer.p50.ms", scenario: "Text layer build, one page — p50", unit: "ms", target: "≤ 16 ms", budget: 16, source: "engine" },
  { id: "text.layer.p95.ms", scenario: "Text layer build, one page — p95", unit: "ms", target: "—", source: "engine" },
  { id: "search.tracemonkey.ms", scenario: 'Search "monkey", all 14 pages, cold layers', unit: "ms", target: "≤ 2500 ms", budget: 2500, source: "engine" },
  { id: "search.tracemonkey.hits", scenario: 'Search "monkey" — hits (F-07 pins 62)', unit: "hits", target: "62", budget: null, source: "engine", gate: true },

  // --- save ---------------------------------------------------------------------------
  { id: "save.serialize.ms", scenario: "Serialise tracemonkey (≈1 MB)", unit: "ms", target: "≤ 300 ms", budget: 300, source: "engine" },
  { id: "save.roundtrip.ms", scenario: "Save round trip: serialise + reopen", unit: "ms", target: "≤ 600 ms", budget: 600, source: "engine" },
  { id: "save.bytes", scenario: "Serialised size of tracemonkey", unit: "B", target: "—", source: "engine", gate: true },

  // --- OCR ----------------------------------------------------------------------------
  { id: "ocr.recognize.ms", scenario: "OCR one A4 page @300 DPI, kor+eng (tesseract.js)", unit: "ms", target: "≤ 2000 ms", budget: 2000, source: "ocr" },
  { id: "ocr.init.ms", scenario: "OCR worker init (first page pays it once)", unit: "ms", target: "—", source: "ocr" },
  { id: "ocr.hangulCerPct", scenario: "OCR Hangul CER (F-21 budget 3 %)", unit: "%", target: "≤ 3 %", budget: 3, source: "ocr" },

  // --- memory -------------------------------------------------------------------------
  { id: "rss.500p.mb", scenario: "Engine RSS after 500-page open + 40 pages rendered", unit: "MB", target: "≤ 400 MB", budget: 400, source: "engine" },
  { id: "app.rss.500p.mb", scenario: "App RSS after opening 500 pages and scrolling", unit: "MB", target: "≤ 400 MB", budget: 400, source: "app" },

  // --- counts, for the gate -----------------------------------------------------------
  { id: "doc.tracemonkey.pages", scenario: "tracemonkey.pdf page count", unit: "pages", target: "14", source: "engine", gate: true },
  { id: "doc.500p.pages", scenario: "gen/500p.pdf page count", unit: "pages", target: "500", source: "engine", gate: true },
];

// ---------------------------------------------------------------------------- sources

function run(cmd, args, opts = {}) {
  return execFileSync(cmd, args, { encoding: "utf8", cwd: ROOT, maxBuffer: 64 * 1024 * 1024, ...opts });
}

function bundleRows() {
  if (!existsSync(join(ROOT, "dist"))) {
    console.error("[perf] no dist/ — run `npm run build` first; bundle rows skipped");
    return {};
  }
  try {
    const report = JSON.parse(
      run(process.execPath, [join(ROOT, "scripts", "check-bundle-size.mjs"), "--json"], {
        stdio: ["ignore", "pipe", "pipe"],
      }),
    );
    return { "bundle.criticalGz": Number(report.criticalGzKb), "bundle.dist": Number(report.totalKb) };
  } catch (e) {
    // Over budget, or a debug `dist/` left behind by `tauri build -- --debug` (exit 2).
    const why = String(e.stderr ?? e.message).trim().split("\n")[0];
    console.error(`[perf] bundle rows skipped — ${why || "check-bundle-size.mjs failed"}`);
    return {};
  }
}

function engineRows() {
  console.error("[perf] engine: cargo run --release --example perf_bench …");
  const env = { ...process.env, PATH: `${process.env.HOME}/.cargo/bin:${process.env.PATH}` };
  const out = run("cargo", ["run", "--release", "--quiet", "--example", "perf_bench"], {
    cwd: join(ROOT, "src-tauri"),
    env,
    stdio: ["ignore", "pipe", "inherit"],
  });
  const line = out.trim().split("\n").filter((l) => l.startsWith("{")).pop();
  if (!line) throw new Error("perf_bench printed no JSON");
  return JSON.parse(line);
}

function ocrRows() {
  console.error("[perf] ocr: tesseract.js, korean-300dpi.png, kor+eng, PSM 4 …");
  const tmp = join(ROOT, "fixtures", "out", "ocr", "perf-ocr.json");
  run(process.execPath, [join(ROOT, "scripts", "ocr-accuracy.mjs"), "--json", tmp], {
    stdio: ["ignore", "ignore", "inherit"],
  });
  const j = JSON.parse(readFileSync(tmp, "utf8"));
  return {
    "ocr.recognize.ms": Number(j.recognizeMs),
    "ocr.init.ms": Number(j.initMs),
    "ocr.hangulCerPct": Number(((j.normalized.hangulCer ?? 0) * 100).toFixed(2)),
  };
}

/** One dev-bridge step; the POST blocks until the page has answered. */
async function bridge(body, timeoutMs = 60_000) {
  const res = await fetch(BRIDGE, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(body),
    signal: AbortSignal.timeout(timeoutMs),
  });
  const json = await res.json();
  if (!json.ok) throw new Error(`dev bridge: ${json.error ?? "unknown error"}`);
  return json.value;
}

async function bridgeAlive() {
  try {
    await bridge({ op: "ping" }, 3000);
    return true;
  } catch {
    return false;
  }
}

async function appRows() {
  if (!(await bridgeAlive())) {
    console.error(`[perf] app: no dev bridge at ${BRIDGE} — run \`npm run tauri dev\` for the app rows`);
    return {};
  }
  console.error("[perf] app: driving the running app through the dev bridge …");
  const out = {};

  const trace = await bridge({ op: "openTimed", path: join(ROOT, "fixtures", "tracemonkey.pdf") });
  out["app.open.tracemonkey.firstPaintMs"] = round(trace.firstPaintMs);
  out["app.open.tracemonkey.documentMs"] = round(trace.documentMs);

  const big = join(ROOT, "fixtures", "gen", "500p.pdf");
  if (existsSync(big)) {
    const five = await bridge({ op: "openTimed", path: big });
    out["app.open.500p.firstPaintMs"] = round(five.firstPaintMs);
    out["app.open.500p.documentMs"] = round(five.documentMs);
    await bridge({ op: "scrollThrough", step: 25, settleMs: 120 }, 180_000);
    const stats = await bridge({ op: "api", fn: "engineStats" });
    out["app.rss.500p.mb"] = round((stats.rssBytes ?? 0) / (1024 * 1024), 1);
    out["app.tile.p50.ms"] = round(stats.tileP50Ms, 2);
    out["app.tile.p95.ms"] = round(stats.tileP95Ms, 2);
  }
  return out;
}

const round = (v, digits = 1) => (v === null || v === undefined || Number.isNaN(v) ? null : Number(Number(v).toFixed(digits)));

// ---------------------------------------------------------------------------- report

function host() {
  return `${platform()}/${arch()} · ${cpus()[0]?.model ?? "cpu"} · ${cpus().length} cores · ${Math.round(totalmem() / 1024 ** 3)} GB`;
}

function meetsBudget(row, value) {
  if (row.budget === null || row.budget === undefined) return null;
  if (value === null || value === undefined || Number.isNaN(value)) return null;
  return value <= row.budget;
}

function render(values, meta) {
  const cell = (row) => {
    const v = values[row.id];
    if (v === null || v === undefined) return { measured: "— *(not measured)*", verdict: "" };
    const measured = `${v}${row.unit === "%" ? " %" : ` ${row.unit}`}`;
    const ok = meetsBudget(row, v);
    return { measured, verdict: ok === null ? "—" : ok ? "✅" : "⚠️ **over budget**" };
  };
  const table = [
    "| id | Scenario | Target | Measured | Budget | CI |",
    "|---|---|---|---|---|---|",
    ...ROWS.map((r) => {
      const { measured, verdict } = cell(r);
      return `| \`${r.id}\` | ${r.scenario} | ${r.target} | ${measured} | ${verdict} | ${r.gate ? "**gate**" : "info"} |`;
    }),
  ].join("\n");

  const missed = ROWS.filter((r) => meetsBudget(r, values[r.id]) === false);

  return `# SeePDF — performance baseline (F-30)

Generated by \`scripts/perf-baseline.mjs\`. Targets are the budgets of \`ARCHITECTURE.md\` §13 and
the perf rows of \`FEATURES.md\`. Re-record with \`node scripts/perf-baseline.mjs --write\` and
commit the diff in the same change that moved a number.

* **Recorded:** ${meta.recorded}
* **Version:** ${meta.version}
* **Machine:** ${meta.host}
* **Build:** \`cargo --release\` engine rows, \`npm run build\` bundle rows${meta.sources.app ? ", app rows through the dev bridge against `npm run tauri dev` (a **debug** binary — treat them as an upper bound)" : ""}
* **Sources measured:** ${Object.entries(meta.sources).filter(([, on]) => on).map(([k]) => k).join(", ") || "none"}

${table}

## Misses

${missed.length === 0 ? "None — every measured row is inside its budget." : missed.map((r) => `* \`${r.id}\` — measured **${values[r.id]} ${r.unit}**, budget ${r.target}.`).join("\n")}

## What is measured where

| source | how |
|---|---|
| \`engine\` | \`cd src-tauri && cargo run --release --example perf_bench\` — the real engine thread, real bundled PDFium, no window. Same \`tiles::render\` the \`seepdf://\` handler calls; the encoded-PNG cache is bypassed so every sample is a real render. |
| \`app\` | \`scripts/dev-bridge.mjs\` + \`src/dev/testHook.ts\` against a running \`npm run tauri dev\`: \`openTimed\` clears \`window.__seepdfFirstPaint\`, starts the clock, opens the file and waits for the first \`<img>\` to land; \`scrollThrough\` walks the 500-page document and \`engine_stats\` reports RSS. |
| \`ocr\` | \`scripts/ocr-accuracy.mjs --json\` — tesseract.js in Node on \`fixtures/out/ocr/korean-300dpi.png\`, the traineddata from \`public/ocr/tessdata\`, PSM 4. |
| \`bundle\` | \`scripts/check-bundle-size.mjs --json\`. |

## The CI gate

${GATE_NOTE}

\`node scripts/perf-baseline.mjs --check\` re-measures and compares against
\`docs/perf/baseline.json\` with a ±${TOLERANCE * 100} % tolerance. It exits non-zero only when a
**gate** row moves; the informational rows are printed with their delta.
`;
}

// ---------------------------------------------------------------------------- main

const sources = {
  bundle: !flags.has("--skip-bundle"),
  engine: !flags.has("--skip-engine"),
  ocr: !flags.has("--skip-ocr"),
  app: !flags.has("--skip-app"),
};

const values = {};
if (sources.bundle) Object.assign(values, bundleRows());
if (sources.engine) Object.assign(values, engineRows());
if (sources.ocr) {
  try {
    Object.assign(values, ocrRows());
  } catch (e) {
    console.error(`[perf] ocr skipped: ${e.message.split("\n")[0]}`);
    sources.ocr = false;
  }
}
if (sources.app) {
  const before = Object.keys(values).length;
  Object.assign(values, await appRows());
  sources.app = Object.keys(values).length > before;
}

const meta = {
  recorded: new Date().toISOString(),
  version: JSON.parse(readFileSync(join(ROOT, "package.json"), "utf8")).version,
  host: host(),
  sources,
  tolerance: TOLERANCE,
};

if (check) {
  if (!existsSync(OUT_JSON)) {
    console.error(`[perf] no ${OUT_JSON} to compare against — run --write first`);
    process.exit(1);
  }
  const base = JSON.parse(readFileSync(OUT_JSON, "utf8"));
  let failed = 0;
  for (const row of ROWS) {
    const now = values[row.id];
    const then = base.values?.[row.id];
    if (now === null || now === undefined || then === null || then === undefined) continue;
    const delta = then === 0 ? (now === 0 ? 0 : 1) : (now - then) / then;
    const pct = `${delta >= 0 ? "+" : ""}${(delta * 100).toFixed(1)} %`;
    const over = Math.abs(delta) > TOLERANCE;
    const tag = row.gate ? (over ? "FAIL " : "ok   ") : "info ";
    console.log(`[perf] ${tag} ${row.id.padEnd(38)} ${String(then).padStart(10)} → ${String(now).padStart(10)} ${row.unit.padEnd(5)} ${pct}`);
    if (row.gate && over) failed++;
  }
  if (failed) {
    console.error(`[perf] ${failed} gated row(s) moved by more than ±${TOLERANCE * 100} %`);
    process.exit(1);
  }
  console.log("[perf] ok — every gated row is within tolerance");
  process.exit(0);
}

const doc = render(values, meta);
if (write) {
  mkdirSync(dirname(OUT_MD), { recursive: true });
  writeFileSync(OUT_MD, doc);
  writeFileSync(OUT_JSON, `${JSON.stringify({ ...meta, values }, null, 2)}\n`);
  console.log(`[perf] wrote ${OUT_MD.slice(ROOT.length + 1)} and ${OUT_JSON.slice(ROOT.length + 1)}`);
} else {
  console.log(doc);
}
