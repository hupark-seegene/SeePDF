/**
 * WORKPLAN §6 first-day probe for module (f): **does a tesseract.js worker run inside a Tauri
 * bundle under the production CSP?**
 *
 * `tauri dev` serves the HTML from Vite and applies no CSP, so the only honest answer comes from a
 * `tauri build --debug` bundle. This module renders a panel, loads the OCR assets from `/ocr/`
 * exactly the way `ocrJob.ts` does, OCRs the synthetic Korean page, scores it, and — the part that
 * matters — lists every `securitypolicyviolation` the document reports while doing it.
 *
 * How to run it (see docs/STAGE1F_NOTES.md §1):
 *
 *   node scripts/prepare-ocr.mjs --probe
 *   VITE_SEEPDF_OCR_PROBE=1 npx vite build
 *   export PATH="$HOME/.cargo/bin:$PATH"
 *   npx tauri build --debug --bundles app --config '{"build":{"beforeBuildCommand":""}}'
 *   open src-tauri/target/debug/bundle/macos/SeePDF.app
 *
 * For `vite dev`, `VITE_SEEPDF_OCR_PROBE=1 npm run dev` does the same without a bundle.
 * Nothing here is reachable from a normal build: the single entry point (bottom of
 * `src/store/jobStore.ts`) is a `VITE_*` comparison that folds to `false`, so rolldown drops this
 * module and its chunk entirely — and the fixtures only exist when `prepare-ocr.mjs --probe` ran.
 */
import { normalizeTesseract, countWords, meanConfidence, HANGUL_CER_BUDGET } from "./normalize";
import { coreFileName, hasWasmSimd, ocrAssetUrl, TesseractPool } from "./tesseractPool";

const PROBE_IMAGE = "probe/korean-300dpi.png";
const PROBE_TRUTH = "probe/korean.txt";

interface ProbeReport {
  ok: boolean;
  origin: string;
  simd: boolean;
  core: string;
  workerUrl: string;
  langUrl: string;
  fetchMs?: number;
  imageBytes?: number;
  initAndRecognizeMs?: number;
  rawWords?: number;
  words?: number;
  lines?: number;
  meanConfidence?: number;
  cer?: number;
  hangulCer?: number;
  text?: string;
  error?: string;
  cspViolations: string[];
}

// --------------------------------------------------------------------------- scoring (spike-ocr.mjs)

function normalizeForScore(s: string): string {
  return s.normalize("NFC").replace(/\s+/g, " ").trim();
}

function levenshtein(a: string, b: string): number {
  const A = Array.from(a);
  const B = Array.from(b);
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

const hangulOnly = (s: string) =>
  Array.from(s).filter((c) => /[ᄀ-ᇿ㄰-㆏가-힯]/.test(c)).join("");

// --------------------------------------------------------------------------- the probe

export async function runOcrProbe(log: (line: string) => void = () => {}): Promise<ProbeReport> {
  const cspViolations: string[] = [];
  const onViolation = (e: SecurityPolicyViolationEvent) => {
    cspViolations.push(`${e.violatedDirective} blocked ${e.blockedURI || "(inline)"}`);
  };
  document.addEventListener("securitypolicyviolation", onViolation);

  const report: ProbeReport = {
    ok: false,
    origin: location.origin || location.href,
    simd: hasWasmSimd(),
    core: coreFileName(),
    workerUrl: ocrAssetUrl("worker.min.js"),
    langUrl: ocrAssetUrl("tessdata"),
    cspViolations,
  };
  log(`origin ${report.origin}`);
  log(`wasm SIMD ${report.simd ? "yes" : "no"} → ${report.core}`);
  log(`worker ${report.workerUrl}`);

  const pool = new TesseractPool({ workers: 1, langs: "kor+eng", layout: "column", dpi: 300 });
  try {
    const t0 = performance.now();
    const [imageRes, truthRes] = await Promise.all([
      fetch(ocrAssetUrl(PROBE_IMAGE)),
      fetch(ocrAssetUrl(PROBE_TRUTH)),
    ]);
    if (!imageRes.ok) throw new Error(`fetch ${PROBE_IMAGE} → ${imageRes.status}`);
    if (!truthRes.ok) throw new Error(`fetch ${PROBE_TRUTH} → ${truthRes.status}`);
    const blob = await imageRes.blob();
    const truth = await truthRes.text();
    report.fetchMs = Math.round(performance.now() - t0);
    report.imageBytes = blob.size;
    log(`fetched ${PROBE_IMAGE} (${(blob.size / 1024).toFixed(0)} kB) in ${report.fetchMs} ms`);

    const t1 = performance.now();
    const raw = await pool.recognize(blob);
    report.initAndRecognizeMs = Math.round(performance.now() - t1);
    log(`worker init + recognize ${report.initAndRecognizeMs} ms`);

    const bounds = (raw.blocks ?? []).reduce(
      (acc, b) => ({ x1: Math.max(acc.x1, b.bbox?.x1 ?? 0), y1: Math.max(acc.y1, b.bbox?.y1 ?? 0) }),
      { x1: 0, y1: 0 },
    );
    const page = normalizeTesseract(raw, {
      page: 0, dpi: 300, widthPx: bounds.x1, heightPx: bounds.y1, rotation: 0,
    });
    report.rawWords = (raw.blocks ?? [])
      .flatMap((b) => b.paragraphs.flatMap((p) => p.lines.flatMap((l) => l.words))).length;
    report.words = countWords(page);
    report.lines = page.lines.length;
    report.meanConfidence = meanConfidence(page);
    report.text = page.lines.map((l) => l.text).join("\n");

    const t = normalizeForScore(truth);
    const h = normalizeForScore(raw.text ?? "");
    report.cer = Array.from(t).length ? levenshtein(t, h) / Array.from(t).length : 1;
    const ht = hangulOnly(t);
    report.hangulCer = ht.length ? levenshtein(ht, hangulOnly(h)) / Array.from(ht).length : 1;

    report.ok = cspViolations.length === 0 && report.hangulCer <= HANGUL_CER_BUDGET;
    log(`CER ${(report.cer * 100).toFixed(2)} % · Hangul CER ${(report.hangulCer * 100).toFixed(2)} %`);
    log(`${report.rawWords} raw words → ${report.words} words in ${report.lines} lines`);
  } catch (e) {
    report.error = e instanceof Error ? `${e.name}: ${e.message}` : String(e);
    log(`ERROR ${report.error}`);
  } finally {
    await pool.terminate().catch(() => {});
    document.removeEventListener("securitypolicyviolation", onViolation);
  }
  if (cspViolations.length) for (const v of cspViolations) log(`CSP ${v}`);
  return report;
}

/** Full-screen panel so the verdict is readable in a `screencapture` of the bundled app. */
export function mountOcrProbe(): void {
  if (typeof document === "undefined" || document.getElementById("ocr-probe")) return;
  const host = document.createElement("div");
  host.id = "ocr-probe";
  host.setAttribute(
    "style",
    "position:fixed;inset:0;z-index:99999;background:#0b0d10;color:#e6e9ef;font:12px/1.5 ui-monospace,SFMono-Regular,Menlo,monospace;padding:40px 24px 24px;overflow:auto;white-space:pre-wrap;",
  );
  const title = document.createElement("div");
  title.setAttribute("style", "font-size:16px;font-weight:600;margin-bottom:8px;");
  title.textContent = "OCR CSP probe (WORKPLAN §6, module f)";
  const verdict = document.createElement("div");
  verdict.setAttribute("style", "font-size:20px;font-weight:700;margin:8px 0 12px;");
  verdict.textContent = "RUNNING…";
  const body = document.createElement("pre");
  body.setAttribute("style", "margin:0;white-space:pre-wrap;word-break:break-all;");
  host.append(title, verdict, body);
  document.body.appendChild(host);

  const lines: string[] = [];
  const log = (line: string) => {
    lines.push(line);
    body.textContent = lines.join("\n");
    // eslint-disable-next-line no-console
    console.log(`[ocr-probe] ${line}`);
  };

  void runOcrProbe(log).then((report) => {
    verdict.textContent = report.ok ? "PASS — worker + wasm ran under the production CSP" : "FAIL";
    verdict.style.color = report.ok ? "#4ade80" : "#f87171";
    log("");
    log(JSON.stringify(report, null, 2));
    // Anything that can read the window title (or a screenshot) now has the verdict.
    document.title = `OCR probe ${report.ok ? "PASS" : "FAIL"}`;
    (window as unknown as { __OCR_PROBE__?: ProbeReport }).__OCR_PROBE__ = report;
  });
}
