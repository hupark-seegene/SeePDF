#!/usr/bin/env node
/**
 * OCR accuracy gate for Stage 1 (f) — FEATURES F-21: "the synthetic Korean 300-DPI page reaches
 * Hangul CER ≤ 3 % at PSM 4".
 *
 * Runs tesseract.js against the traineddata that `scripts/prepare-ocr.mjs` put in `public/ocr/`
 * (not the jsdelivr CDN), pipes the raw result through the *same* normaliser the app uses
 * (`src/ocr/normalize.ts`), and scores both the raw `page.text` and the normalised `OcrPage`
 * against `fixtures/out/ocr/korean.txt`. Scoring is the metric of `scripts/spike-ocr.mjs`
 * (Levenshtein CER on whitespace-normalised NFC text, Hangul-only variant, bag-of-words recall).
 *
 * Usage:
 *   node scripts/ocr-accuracy.mjs                       # korean page, kor+eng, PSM 4
 *   node scripts/ocr-accuracy.mjs --psm 6 --image tracemonkey --langs eng
 *   node scripts/ocr-accuracy.mjs --dump-line           # print one raw tesseract line (shape check)
 *   node scripts/ocr-accuracy.mjs --json out.json
 *
 * Exit code 1 when the Hangul CER budget (3 %) is missed, so CI can gate on it.
 *
 * Note: in Node, tesseract.js loads its WASM core with `require('tesseract.js-core/…')` and ignores
 * `corePath`, so this script proves the *models* and the normaliser, not the browser core loading
 * path — that one is proven by the in-bundle probe (docs/STAGE1F_NOTES.md §1).
 */
import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { performance } from "node:perf_hooks";
import { createWorker, OEM } from "tesseract.js";
import { pathToFileURL } from "node:url";

const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..");

// `src/ocr/normalize.ts` has no runtime imports (types only), so Node 24's built-in type stripping
// loads it directly — the script scores exactly the code the app ships.
const { normalizeTesseract, HANGUL_CER_BUDGET } =
  await import(pathToFileURL(join(ROOT, "src", "ocr", "normalize.ts")).href);

const args = {};
{
  const argv = process.argv.slice(2);
  for (let i = 0; i < argv.length; i++) {
    const m = argv[i].match(/^--([^=]+)(?:=(.*))?$/);
    if (!m) continue;
    if (m[2] !== undefined) args[m[1]] = m[2];
    else if (argv[i + 1] !== undefined && !argv[i + 1].startsWith("--")) args[m[1]] = argv[++i];
    else args[m[1]] = true;
  }
}

const OUT_DIR = join(ROOT, "fixtures", "out", "ocr");
const IMAGES = {
  korean: { png: join(OUT_DIR, "korean-300dpi.png"), truth: join(OUT_DIR, "korean.txt"), hangul: true },
  tracemonkey: { png: join(OUT_DIR, "tracemonkey-p1-300dpi.png"), truth: join(OUT_DIR, "tracemonkey-p1.txt"), hangul: false },
};

const imageKey = String(args.image ?? "korean");
const langs = String(args.langs ?? "kor+eng");
const psm = String(args.psm ?? "4");
const dpi = String(args.dpi ?? "300");
const image = IMAGES[imageKey];
if (!image) { console.error(`unknown --image ${imageKey} (korean|tracemonkey)`); process.exit(2); }
if (!existsSync(image.png)) {
  console.error(`missing ${image.png}\n  run: cd src-tauri && cargo run --example spike_ocr_render`);
  process.exit(2);
}

const langPath = join(ROOT, "public", "ocr", "tessdata");
for (const l of langs.split("+")) {
  if (!existsSync(join(langPath, `${l}.traineddata.gz`))) {
    console.error(`missing ${join(langPath, `${l}.traineddata.gz`)}\n  run: npm run prepare:ocr`);
    process.exit(2);
  }
}

// --------------------------------------------------------------------------- metrics (spike-ocr.mjs)
const norm = (s) => s
  .normalize("NFC")
  .replace(/[‘’‚]/g, "'")
  .replace(/[“”„]/g, '"')
  .replace(/[‐-―−]/g, "-")
  .replace(/­/g, "")
  .replace(/\s+/g, " ")
  .trim();

function levenshtein(a, b) {
  const A = Array.from(a); const B = Array.from(b);
  if (!A.length) return B.length;
  if (!B.length) return A.length;
  let prev = new Int32Array(B.length + 1);
  let cur = new Int32Array(B.length + 1);
  for (let j = 0; j <= B.length; j++) prev[j] = j;
  for (let i = 1; i <= A.length; i++) {
    cur[0] = i;
    const ai = A[i - 1];
    for (let j = 1; j <= B.length; j++) {
      cur[j] = Math.min(prev[j] + 1, cur[j - 1] + 1, prev[j - 1] + (ai === B[j - 1] ? 0 : 1));
    }
    [prev, cur] = [cur, prev];
  }
  return prev[B.length];
}

const hangulOnly = (s) => Array.from(s).filter((c) => /[ᄀ-ᇿ㄰-㆏가-힯]/.test(c)).join("");

function score(truth, hyp) {
  const t = norm(truth); const h = norm(hyp);
  const chars = Array.from(t).length;
  const cer = chars ? levenshtein(t, h) / chars : 0;
  const ht = hangulOnly(t); const hh = hangulOnly(h);
  const hangulCer = ht.length ? levenshtein(ht, hh) / Array.from(ht).length : null;
  const tw = t.split(" ").filter(Boolean); const hw = h.split(" ").filter(Boolean);
  const bag = new Map();
  for (const w of tw) bag.set(w, (bag.get(w) ?? 0) + 1);
  let hit = 0;
  for (const w of hw) { const n = bag.get(w) ?? 0; if (n > 0) { hit++; bag.set(w, n - 1); } }
  return {
    cer, hangulCer,
    wordRecall: tw.length ? hit / tw.length : 0,
    wordPrecision: hw.length ? hit / hw.length : 0,
    truthChars: chars, hypChars: Array.from(h).length, truthWords: tw.length, hypWords: hw.length,
  };
}

const pct = (n) => (n === null ? "—" : `${(n * 100).toFixed(2)} %`);

// --------------------------------------------------------------------------- run
console.log(`[ocr-accuracy] ${imageKey} · langs=${langs} · PSM ${psm} · ${dpi} DPI · langPath=public/ocr/tessdata`);

const t0 = performance.now();
const worker = await createWorker(langs, OEM.LSTM_ONLY, {
  langPath,
  gzip: true,
  cacheMethod: "none",
  logger: args.verbose ? (m) => console.log("   ", m.status, m.progress) : () => {},
});
const initMs = performance.now() - t0;
await worker.setParameters({
  tessedit_pageseg_mode: psm,
  user_defined_dpi: dpi,
  preserve_interword_spaces: "1",
});

const t1 = performance.now();
const { data } = await worker.recognize(image.png, {}, { text: true, blocks: true });
const recognizeMs = performance.now() - t1;
await worker.terminate();

const truth = readFileSync(image.truth, "utf8");

// Page geometry: the fixture PNGs are rendered at `dpi`; widthPx/heightPx come from the page bbox.
const pageBox = (data.blocks ?? []).reduce(
  (acc, b) => ({ x1: Math.max(acc.x1, b.bbox.x1), y1: Math.max(acc.y1, b.bbox.y1) }),
  { x1: 0, y1: 0 },
);
const ocrPage = normalizeTesseract(data, {
  page: 0,
  dpi: Number(dpi),
  widthPx: Number(args.width ?? pageBox.x1),
  heightPx: Number(args.height ?? pageBox.y1),
  rotation: 0,
});

const normalizedText = ocrPage.lines.map((l) => l.text).join("\n");

const raw = score(truth, data.text);
const normalized = score(truth, normalizedText);

if (args["dump-line"]) {
  const firstLine = data.blocks?.[0]?.paragraphs?.[0]?.lines?.[0];
  console.log("\n--- raw tesseract line ---");
  console.log(JSON.stringify({
    text: firstLine?.text,
    bbox: firstLine?.bbox,
    baseline: firstLine?.baseline,
    rowAttributes: firstLine?.rowAttributes,
    words: firstLine?.words?.map((w) => ({ text: w.text, bbox: w.bbox, confidence: w.confidence })),
  }, null, 2));
  console.log("\n--- normalised OcrLine ---");
  console.log(JSON.stringify(ocrPage.lines[0], null, 2));
}

console.log(`
  init            ${initMs.toFixed(0)} ms
  recognize       ${recognizeMs.toFixed(0)} ms
  lines / words   ${ocrPage.lines.length} / ${ocrPage.lines.reduce((n, l) => n + l.words.length, 0)}   (raw words ${(data.blocks ?? []).flatMap((b) => b.paragraphs.flatMap((p) => p.lines.flatMap((l) => l.words))).length})
  page size       ${ocrPage.widthPx} × ${ocrPage.heightPx} px @ ${ocrPage.dpi} DPI

                  CER        Hangul CER   word recall   word precision
  raw text        ${pct(raw.cer).padEnd(10)} ${pct(raw.hangulCer).padEnd(12)} ${pct(raw.wordRecall).padEnd(13)} ${pct(raw.wordPrecision)}
  normalised      ${pct(normalized.cer).padEnd(10)} ${pct(normalized.hangulCer).padEnd(12)} ${pct(normalized.wordRecall).padEnd(13)} ${pct(normalized.wordPrecision)}
`);

console.log("--- normalised text ---");
console.log(normalizedText.split("\n").map((l) => "  | " + l).join("\n"));

if (args.json) {
  const path = typeof args.json === "string" ? args.json : join(OUT_DIR, "ocr-accuracy.json");
  writeFileSync(path, JSON.stringify({
    image: imageKey, langs, psm, dpi: Number(dpi), initMs: Math.round(initMs), recognizeMs: Math.round(recognizeMs),
    raw, normalized, lines: ocrPage.lines.length,
  }, null, 2));
  console.log(`\nwrote ${path}`);
}

if (image.hangul) {
  const budget = HANGUL_CER_BUDGET ?? 0.03;
  const worst = Math.max(raw.hangulCer ?? 1, normalized.hangulCer ?? 1);
  if (worst > budget) {
    console.error(`[ocr-accuracy] FAIL Hangul CER ${pct(worst)} > budget ${pct(budget)} (FEATURES F-21)`);
    process.exit(1);
  }
  console.log(`[ocr-accuracy] ok — Hangul CER ${pct(worst)} ≤ budget ${pct(budget)} (FEATURES F-21)`);
}
