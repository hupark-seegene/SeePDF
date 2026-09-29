/**
 * 인식 엔진 (P1-11, v0.3 O3): which recogniser an OCR run uses.
 *
 *   자동             the native engine when the backend lists one (`ocr_capabilities`) and it reads the
 *                    selected languages, Tesseract otherwise (`pickEngine`)
 *   Apple Vision     `ocr_recognize_native` on macOS 13+ — rendered and recognised in the backend
 *   Windows OCR      the same command on Windows (`Windows.Media.Ocr`, v0.3 O3)
 *   Tesseract        the bundled tesseract.js worker pool (every platform, fully offline)
 *
 * A machine has at most one native engine, so the sheet's choice is 자동 / native / Tesseract and the
 * native entry is labelled after whichever the backend listed. Every engine ends in the same `OcrPage`
 * (image pixels, origin top-left) and the same `ocr_apply`, so the choice only changes where the words
 * come from. The capability answer is asked once per session: it is a property of the machine, not of
 * the document.
 *
 * v0.3 O1: the capability answer also says which languages each engine reads (`engineLanguages`); the
 * tesseract list is completed from `public/ocr/tessdata/languages.json`, which `prepare-ocr` writes
 * for whatever traineddata it staged (only the frontend can see its own assets).
 */
import { useEffect, useState } from "react";
import * as api from "../ipc/api";
import { useMock } from "../ipc/env";
import type { OcrCapabilities, OcrEngine } from "../ipc/types";
import {
  BASELINE_LANGUAGES, OCR_LANGUAGES, effectiveLanguages, isOcrLanguage, toggleLanguage, type OcrLanguage,
} from "./languages";
import { ocrAssetUrl } from "./assets";

/** What the user picks in the sheet. `vision` = the machine's native engine (Vision or Windows OCR). */
export type OcrEngineChoice = "auto" | "vision" | "tesseract";
/** The native engines `ocr_recognize_native` drives. */
export type NativeEngine = "vision" | "windows";
/** What a run actually uses. */
export type OcrRunEngine = NativeEngine | "tesseract";

export function isNativeEngine(engine: OcrRunEngine | undefined): engine is NativeEngine {
  return engine === "vision" || engine === "windows";
}

/** The sheet's 인식 엔진 options; the native entry's label follows the machine's engine. */
export function engineChoices(native: NativeEngine | null): { id: OcrEngineChoice; labelKey: string }[] {
  return [
    { id: "auto", labelKey: "ocr.engine.auto" },
    { id: "vision", labelKey: native === "windows" ? "ocr.engine.windows" : "ocr.engine.vision" },
    { id: "tesseract", labelKey: "ocr.engine.tesseract" },
  ];
}

/** Kept for callers of the P1-11 surface: the choices on a Mac with Vision. */
export const ENGINE_CHOICES = engineChoices("vision");

/**
 * 자동's hint under the select, per native engine — or, when 자동 fell back to Tesseract because the
 * native engine cannot read the selected languages (`pickEngine`), that.
 */
export function autoHintKey(native: NativeEngine | null, fellBack = false): string {
  if (fellBack) return native === "windows" ? "ocr.engine.autoFallbackWindows" : "ocr.engine.autoFallback";
  return native === "windows" ? "ocr.engine.autoHintWindows" : "ocr.engine.autoHint";
}

/**
 * Vision, measured on this project's fixtures (Apple M5, 300 DPI): 0.13 s for a sparse 4-line page,
 * 0.36 s for the 10-line Korean page, 3.0 s for a dense two-column English page (736 words), plus
 * ~0.1 s of render — a typical scan sits in between. A second page in flight does not speed it up
 * (Vision serialises on the Neural Engine), so two pages at a time only overlap render and recognition.
 * Windows OCR is used with the same budget (not measured on this Mac).
 */
export const VISION_SECONDS_PER_PAGE = 0.8;
export const VISION_CONCURRENCY = 2;

/**
 * 자동 → the native engine where it exists; an explicit native request on a machine without one →
 * Tesseract. `native` may be a boolean for the P1-11 callers (`true` = Vision).
 */
export function resolveEngine(choice: OcrEngineChoice, native: NativeEngine | boolean | null): OcrRunEngine {
  if (choice === "tesseract") return "tesseract";
  const engine = native === true ? "vision" : native === false ? null : native;
  return engine ?? "tesseract";
}

/** The sheet's tesseract language string (`kor+eng+jpn`) → BCP 47 for the native engines. */
export function visionLanguages(langs: string): string[] {
  const map: Record<string, string> = { kor: "ko-KR", eng: "en-US", jpn: "ja-JP", chi_sim: "zh-Hans" };
  const out: string[] = [];
  for (const code of langs.split("+").map((s) => s.trim()).filter(Boolean)) {
    const lang = map[code] ?? code;
    if (!out.includes(lang)) out.push(lang);
  }
  // Korean always brings English along, as `kor+eng` does for tesseract.
  if (out.includes("ko-KR") && !out.includes("en-US")) out.push("en-US");
  return out.length ? out : ["ko-KR", "en-US"];
}

/** The native engine the backend listed, if any. */
export function nativeEngineOf(caps: OcrCapabilities): NativeEngine | null {
  if (caps.engines.includes("vision")) return "vision";
  if (caps.engines.includes("windows")) return "windows";
  return null;
}

// --- pkg7-ocr, verification round 1 (O3): 자동 looks at the selected languages ---------------------

/**
 * The engine a run uses: [`resolveEngine`], except that 자동 takes the native engine only when it reads
 * the selected languages. A Windows PC with only the en-US recogniser lists Windows OCR, and 자동 used
 * to send a 한국어 + English selection (the default, and 설정's) to it — Korean dropped without a word,
 * where v0.2 ran Tesseract `kor+eng`.
 *
 * The rule: the native engine when it reads every selected language; otherwise whichever engine reads
 * Korean when Korean is selected, then whichever reads more of the selection, the native one on a tie.
 * An explicit engine choice is always honoured.
 *
 * Round 2: "reads" is [`readableLanguages`] — Windows OCR reads one language per run (plus English), so
 * 한국어 + 日本語 is not a selection it covers even with both packs installed.
 */
export function pickEngine(
  choice: OcrEngineChoice, caps: OcrCapabilities | null, selected: readonly string[],
): OcrRunEngine {
  const native = caps ? nativeEngineOf(caps) : null;
  if (choice !== "auto" || !native) return resolveEngine(choice, native);
  const wanted = selected.filter(isOcrLanguage);
  const score = (engine: OcrRunEngine): [number, number] => {
    const reads = readableLanguages(caps, engine, wanted);
    return [reads.includes("kor") ? 1 : 0, reads.length];
  };
  const [nativeKor, nativeCount] = score(native);
  if (nativeCount === wanted.length) return native;
  const [tessKor, tessCount] = score("tesseract");
  if (tessKor !== nativeKor) return tessKor > nativeKor ? "tesseract" : native;
  return tessCount > nativeCount ? "tesseract" : native;
}

/** Whether 자동 settled on Tesseract on a machine that has a native engine (for the hint). */
export function autoFellBack(choice: OcrEngineChoice, caps: OcrCapabilities | null, engine: OcrRunEngine): boolean {
  return choice === "auto" && !!caps && nativeEngineOf(caps) !== null && engine === "tesseract";
}

/**
 * The language chips to offer. Under 자동 every language either engine reads here — the engine follows
 * the chips (`pickEngine`), so a chip must not vanish because the *current* pick cannot read it (turning
 * 한국어 off would otherwise hide it for good). An explicit engine offers its own languages.
 */
export function choosableLanguages(
  choice: OcrEngineChoice, caps: OcrCapabilities | null, engine: OcrRunEngine,
): OcrLanguage[] {
  const native = caps ? nativeEngineOf(caps) : null;
  if (choice !== "auto" || !native) return languagesFor(caps, engine);
  const union = new Set<string>([...languagesFor(caps, "tesseract"), ...languagesFor(caps, native)]);
  return OCR_LANGUAGES.map((l) => l.code).filter((c) => union.has(c));
}

// --- pkg7-ocr, verification round 2 (O3): Windows OCR reads one language per run -------------------

/**
 * `Windows.Media.Ocr` runs one recogniser per call (`winocr::pick_language`), and the ko / ja / zh
 * recognisers also read Latin text. So of `wanted` it reads the first non-English language it has a
 * pack for, plus English when English is wanted; English alone when no other wanted pack is installed.
 */
export function windowsLanguages(reads: readonly string[], wanted: readonly string[]): OcrLanguage[] {
  const ordered = OCR_LANGUAGES.map((l) => l.code).filter((c) => wanted.includes(c));
  const primary = ordered.find((c) => c !== "eng" && reads.includes(c));
  if (primary) return ordered.filter((c) => c === primary || c === "eng");
  return ordered.includes("eng") && reads.includes("eng") ? ["eng"] : [];
}

/** What a run on `engine` actually reads of `selected` (in chip order for Windows OCR). */
export function readableLanguages(
  caps: OcrCapabilities | null, engine: OcrRunEngine, selected: readonly string[],
): OcrLanguage[] {
  const reads = languagesFor(caps, engine);
  const wanted = selected.filter(isOcrLanguage);
  return engine === "windows" ? windowsLanguages(reads, wanted) : wanted.filter((c) => reads.includes(c));
}

/**
 * The selection a run sends, and the chips shown pressed: [`readableLanguages`], or — when none of the
 * selection is readable — Korean + English as far as the engine reads them (`effectiveLanguages`).
 */
export function runLanguages(
  caps: OcrCapabilities | null, engine: OcrRunEngine, selected: readonly string[],
): OcrLanguage[] {
  const readable = readableLanguages(caps, engine, selected);
  if (readable.length) return readable;
  const baseline = readableLanguages(caps, engine, BASELINE_LANGUAGES);
  return baseline.length ? baseline : effectiveLanguages(selected, languagesFor(caps, engine));
}

/**
 * The hint under the engine select: Windows OCR leaving part of the selection out (it reads one language
 * per run) whatever the choice, else 자동's hint ([`autoHintKey`]), else none.
 */
export function engineHintKey(
  choice: OcrEngineChoice, caps: OcrCapabilities | null, engine: OcrRunEngine, selected: readonly string[],
): string | undefined {
  const native = caps ? nativeEngineOf(caps) : null;
  if (engine === "windows") {
    const reads = runLanguages(caps, engine, selected);
    if (selected.some((c) => isOcrLanguage(c) && !reads.includes(c))) return "ocr.engine.windowsOneLanguage";
  }
  return choice === "auto" ? autoHintKey(native, autoFellBack(choice, caps, engine)) : undefined;
}

/**
 * A chip click for `engine`: [`toggleLanguage`], except that when Windows OCR was chosen explicitly a
 * newly pressed non-English language replaces the other one (only one runs) instead of being ignored.
 */
export function toggleLanguageFor(
  choice: OcrEngineChoice, engine: OcrRunEngine, selected: readonly OcrLanguage[], code: OcrLanguage,
): OcrLanguage[] {
  if (choice !== "auto" && engine === "windows" && code !== "eng" && !selected.includes(code)) {
    return toggleLanguage(selected.filter((c) => c === "eng"), code);
  }
  return toggleLanguage(selected, code);
}

// ---------------------------------------------------------------------------------------------------

/** The languages `engine` reads here, in chip order. */
export function languagesFor(caps: OcrCapabilities | null, engine: OcrRunEngine): OcrLanguage[] {
  if (!caps) return [...BASELINE_LANGUAGES];
  const listed: string[] =
    caps.engineLanguages?.[engine]
    ?? (engine === "tesseract" ? caps.languages : BASELINE_LANGUAGES);
  const codes = listed.filter(isOcrLanguage);
  return codes.length ? codes : [...BASELINE_LANGUAGES];
}

/** `public/ocr/tessdata/languages.json` (`prepare-ocr`): the traineddata this build actually ships. */
async function stagedTesseractLanguages(): Promise<string[]> {
  if (useMock()) return [];
  try {
    const res = await fetch(ocrAssetUrl("tessdata/languages.json"));
    if (!res.ok) return [];
    const body = (await res.json()) as { languages?: unknown };
    return Array.isArray(body.languages) ? body.languages.filter((l): l is string => typeof l === "string") : [];
  } catch {
    return [];
  }
}

let cached: Promise<OcrCapabilities> | null = null;

/** `ocr_capabilities`, once per session. A failed call is not cached and reads as "Tesseract only". */
export function loadOcrCapabilities(): Promise<OcrCapabilities> {
  if (!cached) {
    cached = Promise.all([api.ocrCapabilities(), stagedTesseractLanguages()])
      .then(([caps, staged]) => {
        const tesseract = [...new Set([...(caps.engineLanguages?.tesseract ?? caps.languages), ...staged])];
        return {
          ...caps,
          engineLanguages: {
            tesseract,
            vision: caps.engineLanguages?.vision ?? (caps.engines.includes("vision") ? [...BASELINE_LANGUAGES] : []),
            windows: caps.engineLanguages?.windows ?? (caps.engines.includes("windows") ? [...BASELINE_LANGUAGES] : []),
          },
        };
      })
      .catch((e: unknown) => {
        cached = null;
        console.warn("[ocr] ocr_capabilities failed; using tesseract only", e);
        return {
          engines: ["tesseract"] as OcrEngine[], languages: [...BASELINE_LANGUAGES],
          engineLanguages: { tesseract: [...BASELINE_LANGUAGES], vision: [], windows: [] },
        };
      });
  }
  return cached;
}

export async function isVisionAvailable(): Promise<boolean> {
  return (await loadOcrCapabilities()).engines.includes("vision");
}

/** The machine's native engine (`vision` / `windows`) or `null`, from the cached capability answer. */
export async function nativeEngine(): Promise<NativeEngine | null> {
  return nativeEngineOf(await loadOcrCapabilities());
}

/** `null` until the capability answer is in. */
export function useOcrCapabilities(): OcrCapabilities | null {
  const [caps, setCaps] = useState<OcrCapabilities | null>(null);
  useEffect(() => {
    let live = true;
    void loadOcrCapabilities().then((c) => { if (live) setCaps(c); });
    return () => { live = false; };
  }, []);
  return caps;
}

/** `null` until the capability answer is in, then whether Apple Vision can run here. */
export function useVisionAvailable(): boolean | null {
  const caps = useOcrCapabilities();
  return caps ? caps.engines.includes("vision") : null;
}

/** Tests only: forget the cached capability answer. */
export function resetOcrCapabilities(): void {
  cached = null;
}
