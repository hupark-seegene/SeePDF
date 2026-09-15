#!/usr/bin/env node
// OCR feasibility spike: tesseract.js (WASM) on the images produced by
// `cargo run --example spike_ocr_render` (fixtures/out/ocr/*.png).
//
// Usage (from the repo root, Node 24):
//   node scripts/spike-ocr.mjs                 # variant "best_int" (tesseract.js default data)
//   node scripts/spike-ocr.mjs --variant fast  # tessdata_fast from GitHub raw
//   node scripts/spike-ocr.mjs --variant full  # tessdata (legacy+LSTM, 4.0.0) from jsdelivr
//   node scripts/spike-ocr.mjs --langs eng,kor+eng --images tracemonkey,korean
//
// Measures per run: worker init time (core load + traineddata load), recognise time,
// CER (Levenshtein) and word recall/precision vs. the ground truth files, and checks that
// word-level bboxes + confidences come back. Writes fixtures/out/ocr/spike-ocr-results.json.

import { createWorker, OEM, PSM } from 'tesseract.js';
import { readFileSync, writeFileSync, mkdirSync, existsSync, statSync, readdirSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';
import { performance } from 'node:perf_hooks';

const here = dirname(fileURLToPath(import.meta.url));
const root = join(here, '..');
const outDir = join(root, 'fixtures', 'out', 'ocr');

// ---------------------------------------------------------------------------------------------
// CLI
// ---------------------------------------------------------------------------------------------
const args = {};
{
  const argv = process.argv.slice(2);
  for (let i = 0; i < argv.length; i++) {
    const m = argv[i].match(/^--([^=]+)(?:=(.*))?$/);
    if (!m) continue;
    if (m[2] !== undefined) args[m[1]] = m[2];
    else if (argv[i + 1] !== undefined && !argv[i + 1].startsWith('--')) args[m[1]] = argv[++i];
    else args[m[1]] = true;
  }
}
const variant = args.variant ?? 'best_int';
const langsToRun = String(args.langs ?? 'eng,kor+eng').split(',');
const imagesToRun = String(args.images ?? 'tracemonkey,korean').split(',');
// Page segmentation mode: name from tesseract.js PSM (AUTO, SINGLE_BLOCK, SINGLE_COLUMN, SPARSE_TEXT, ...) or a number.
const psmArg = String(args.psm ?? 'AUTO');
const psm = /^\d+$/.test(psmArg) ? Number(psmArg) : PSM[psmArg];
if (psm === undefined) { console.error(`unknown --psm ${psmArg}`); process.exit(2); }
const tag = `${variant}-psm${psm}`;

// Language data source per variant. tesseract.js v7 default (langPath unset + OEM.LSTM_ONLY) is
// https://cdn.jsdelivr.net/npm/@tesseract.js-data/<lang>/4.0.0_best_int (integer-quantised "best").
const VARIANTS = {
  best_int: { langPath: undefined, gzip: true, note: 'tesseract.js default: @tesseract.js-data/<lang>/4.0.0_best_int (jsdelivr)' },
  full: { langPath: 'https://cdn.jsdelivr.net/npm/@tesseract.js-data/{lang}/4.0.0', gzip: true, note: 'tessdata 4.0.0 (legacy+LSTM) via jsdelivr' },
  fast: { langPath: 'https://raw.githubusercontent.com/tesseract-ocr/tessdata_fast/main', gzip: false, note: 'tessdata_fast from GitHub raw (uncompressed)' },
};
if (!VARIANTS[variant]) {
  console.error(`unknown --variant ${variant}; one of ${Object.keys(VARIANTS).join(', ')}`);
  process.exit(2);
}
const cachePath = join(outDir, 'tessdata-cache', variant);
mkdirSync(cachePath, { recursive: true });

const IMAGES = {
  tracemonkey: { png: join(outDir, 'tracemonkey-p1-300dpi.png'), truth: join(outDir, 'tracemonkey-p1.txt') },
  korean: { png: join(outDir, 'korean-300dpi.png'), truth: join(outDir, 'korean.txt') },
};
for (const k of imagesToRun) {
  if (!IMAGES[k]) { console.error(`unknown image ${k}`); process.exit(2); }
  if (!existsSync(IMAGES[k].png)) {
    console.error(`missing ${IMAGES[k].png}; run: cd src-tauri && cargo run --example spike_ocr_render`);
    process.exit(2);
  }
}

// ---------------------------------------------------------------------------------------------
// Metrics
// ---------------------------------------------------------------------------------------------
function normalize(s) {
  return s
    .normalize('NFC')
    .replace(/[‘’‚]/g, "'")
    .replace(/[“”„]/g, '"')
    .replace(/[‐-―−]/g, '-')
    .replace(/­/g, '') // soft hyphen
    .replace(/\s+/g, ' ')
    .trim();
}

// Levenshtein distance on arrays of code points (two-row DP, Int32Array).
function levenshtein(a, b) {
  const A = Array.from(a);
  const B = Array.from(b);
  if (A.length === 0) return B.length;
  if (B.length === 0) return A.length;
  let prev = new Int32Array(B.length + 1);
  let cur = new Int32Array(B.length + 1);
  for (let j = 0; j <= B.length; j++) prev[j] = j;
  for (let i = 1; i <= A.length; i++) {
    cur[0] = i;
    const ai = A[i - 1];
    for (let j = 1; j <= B.length; j++) {
      const cost = ai === B[j - 1] ? 0 : 1;
      cur[j] = Math.min(prev[j] + 1, cur[j - 1] + 1, prev[j - 1] + cost);
    }
    [prev, cur] = [cur, prev];
  }
  return prev[B.length];
}

function wordStats(truth, hyp) {
  const tw = normalize(truth).split(' ').filter(Boolean);
  const hw = normalize(hyp).split(' ').filter(Boolean);
  const bag = new Map();
  for (const w of tw) bag.set(w, (bag.get(w) ?? 0) + 1);
  let hit = 0;
  for (const w of hw) {
    const n = bag.get(w) ?? 0;
    if (n > 0) { hit++; bag.set(w, n - 1); }
  }
  return {
    truthWords: tw.length,
    hypWords: hw.length,
    wordRecall: tw.length ? hit / tw.length : 0,
    wordPrecision: hw.length ? hit / hw.length : 0,
  };
}

// CER restricted to Hangul syllables/jamo (so the Korean page reports Hangul-only accuracy too).
function hangulOnly(s) {
  return Array.from(s).filter((c) => /[ᄀ-ᇿ㄰-㆏가-힯]/.test(c)).join('');
}

function cer(truth, hyp) {
  const t = normalize(truth);
  const h = normalize(hyp);
  const d = levenshtein(t, h);
  return { cer: t.length ? d / Array.from(t).length : 0, distance: d, truthChars: Array.from(t).length, hypChars: Array.from(h).length };
}

function dirSizeBytes(dir) {
  let total = 0;
  if (!existsSync(dir)) return 0;
  for (const f of readdirSync(dir)) {
    const p = join(dir, f);
    const st = statSync(p);
    total += st.isDirectory() ? dirSizeBytes(p) : st.size;
  }
  return total;
}

const mb = (n) => (n / 1024 / 1024).toFixed(2) + ' MB';

// ---------------------------------------------------------------------------------------------
// Run
// ---------------------------------------------------------------------------------------------
const results = { variant, psm, note: VARIANTS[variant].note, node: process.version, runs: [] };

for (const lang of langsToRun) {
  const v = VARIANTS[variant];
  const langPath = v.langPath ? v.langPath.replace('{lang}', '') : undefined; // jsdelivr URL is per-lang; see below
  const perLangPath = (l) => (v.langPath ? v.langPath.replace('{lang}', l) : undefined);

  // For the jsdelivr "full" variant the URL embeds the language name, which tesseract.js cannot
  // template; so for multi-language runs we pre-fetch each file into the cache dir and point
  // langPath at the cache (tesseract.js accepts a local directory in Node).
  let effectiveLangPath = langPath;
  let effectiveGzip = v.gzip;
  if (variant === 'full') {
    for (const l of lang.split('+')) {
      const target = join(cachePath, `${l}.traineddata.gz`);
      if (!existsSync(target)) {
        const url = `${perLangPath(l)}/${l}.traineddata.gz`;
        console.log(`fetching ${url}`);
        const res = await fetch(url);
        if (!res.ok) throw new Error(`fetch ${url}: ${res.status}`);
        writeFileSync(target, Buffer.from(await res.arrayBuffer()));
      }
    }
    effectiveLangPath = cachePath;
    effectiveGzip = true;
  }

  for (let attempt = 0; attempt < 2; attempt++) {
    // attempt 0 = cold (download or first read), attempt 1 = warm (cache hit) init timing
    const t0 = performance.now();
    const worker = await createWorker(lang, OEM.LSTM_ONLY, {
      cachePath,
      langPath: effectiveLangPath,
      gzip: effectiveGzip,
      // logger: (m) => console.log(m),
    });
    const initMs = performance.now() - t0;
    await worker.setParameters({ tessedit_pageseg_mode: String(psm), user_defined_dpi: '300', preserve_interword_spaces: '1' });

    if (attempt === 0) {
      console.log(`\n=== lang=${lang} variant=${variant} psm=${psm} ===`);
    }
    console.log(`[init ${attempt === 0 ? 'cold' : 'warm'}] createWorker('${lang}') took ${initMs.toFixed(0)} ms`);

    if (attempt === 1) {
      // Only timing for the warm init; the recognition numbers below come from attempt 0.
      results.runs.push({ lang, phase: 'init-warm', initMs: Math.round(initMs) });
      await worker.terminate();
      continue;
    }
    results.runs.push({ lang, phase: 'init-cold', initMs: Math.round(initMs) });

    for (const imgKey of imagesToRun) {
      const { png, truth: truthPath } = IMAGES[imgKey];
      const truth = readFileSync(truthPath, 'utf8');
      const t1 = performance.now();
      const { data } = await worker.recognize(png, {}, { text: true, blocks: true });
      const recMs = performance.now() - t1;

      // Flatten words with bbox + confidence.
      const words = [];
      for (const b of data.blocks ?? []) for (const p of b.paragraphs) for (const l of p.lines) for (const w of l.words) words.push(w);
      const withBbox = words.filter((w) => w.bbox && Number.isFinite(w.bbox.x0) && Number.isFinite(w.bbox.x1) && w.bbox.x1 > w.bbox.x0).length;
      const withConf = words.filter((w) => Number.isFinite(w.confidence)).length;
      const meanConf = words.length ? words.reduce((s, w) => s + w.confidence, 0) / words.length : 0;
      const lines = (data.blocks ?? []).flatMap((b) => b.paragraphs.flatMap((p) => p.lines));
      const withBaseline = lines.filter((l) => l.baseline && Number.isFinite(l.baseline.y0)).length;

      const c = cer(truth, data.text);
      const ws = wordStats(truth, data.text);
      const hangul = { truth: hangulOnly(normalize(truth)), hyp: hangulOnly(normalize(data.text)) };
      const hangulCer = hangul.truth.length ? levenshtein(hangul.truth, hangul.hyp) / Array.from(hangul.truth).length : null;

      const row = {
        lang,
        image: imgKey,
        phase: 'recognize',
        recognizeMs: Math.round(recMs),
        cer: +c.cer.toFixed(4),
        hangulCer: hangulCer === null ? null : +hangulCer.toFixed(4),
        distance: c.distance,
        truthChars: c.truthChars,
        hypChars: c.hypChars,
        ...ws,
        wordRecall: +ws.wordRecall.toFixed(4),
        wordPrecision: +ws.wordPrecision.toFixed(4),
        words: words.length,
        wordsWithBbox: withBbox,
        wordsWithConfidence: withConf,
        meanWordConfidence: +meanConf.toFixed(1),
        pageConfidence: data.confidence,
        linesWithBaseline: withBaseline,
        lines: lines.length,
      };
      results.runs.push(row);
      console.log(
        `[${imgKey}] recognize ${recMs.toFixed(0)} ms | CER ${(c.cer * 100).toFixed(2)}%` +
          (hangulCer !== null ? ` (Hangul-only CER ${(hangulCer * 100).toFixed(2)}%)` : '') +
          ` | word recall ${(ws.wordRecall * 100).toFixed(1)}% precision ${(ws.wordPrecision * 100).toFixed(1)}%` +
          ` | ${words.length} words, ${withBbox} with bbox, ${withConf} with confidence, mean conf ${meanConf.toFixed(1)}, page conf ${data.confidence}`,
      );
      // Show a few words so the report can quote the shape.
      const sample = words.slice(0, 3).map((w) => ({ text: w.text, confidence: w.confidence, bbox: w.bbox }));
      console.log('   sample words:', JSON.stringify(sample));
      if (imgKey === 'korean') {
        console.log('   --- OCR text (korean page) ---');
        console.log(data.text.trim().split('\n').map((l) => '   | ' + l).join('\n'));
      }
      writeFileSync(join(outDir, `ocr-${imgKey}-${lang.replace('+', '_')}-${tag}.txt`), data.text);
      writeFileSync(join(outDir, `ocr-${imgKey}-${lang.replace('+', '_')}-${tag}.words.json`), JSON.stringify(words.map((w) => ({ t: w.text, c: w.confidence, b: w.bbox })), null, 0));
    }
    await worker.terminate();
  }
}

// ---------------------------------------------------------------------------------------------
// Bundle-size facts
// ---------------------------------------------------------------------------------------------
const coreDir = join(root, 'node_modules', 'tesseract.js-core');
const distDir = join(root, 'node_modules', 'tesseract.js', 'dist');
const sizes = {};
for (const f of ['tesseract-core-simd-lstm.wasm', 'tesseract-core-simd-lstm.wasm.js', 'tesseract-core-lstm.wasm', 'tesseract-core-simd.wasm', 'tesseract-core-relaxedsimd-lstm.wasm']) {
  const p = join(coreDir, f);
  if (existsSync(p)) sizes[f] = statSync(p).size;
}
for (const f of ['worker.min.js', 'tesseract.esm.min.js']) {
  const p = join(distDir, f);
  if (existsSync(p)) sizes['dist/' + f] = statSync(p).size;
}
const cached = {};
if (existsSync(cachePath)) for (const f of readdirSync(cachePath)) cached[f] = statSync(join(cachePath, f)).size;
results.sizes = { core: sizes, traineddataCache: cached, traineddataCacheTotal: dirSizeBytes(cachePath) };

console.log('\n=== bundle-size facts ===');
for (const [k, v] of Object.entries(sizes)) console.log(`${k.padEnd(45)} ${mb(v)}`);
for (const [k, v] of Object.entries(cached)) console.log(`traineddata cache ${variant}/${k}`.padEnd(45) + ` ${mb(v)} (uncompressed, as written to cache)`);

const outJson = join(outDir, `spike-ocr-results-${tag}.json`);
writeFileSync(outJson, JSON.stringify(results, null, 2));
console.log(`\nwrote ${outJson}`);
