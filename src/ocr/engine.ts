/**
 * 인식 엔진 (P1-11, v0.3 O3): which recogniser an OCR run uses.
 *
 *   자동             the native engine when the backend lists one (`ocr_capabilities`), Tesseract otherwise
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
import { BASELINE_LANGUAGES, isOcrLanguage, type OcrLanguage } from "./languages";
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

/** 자동's hint under the select, per native engine. */
export function autoHintKey(native: NativeEngine | null): string {
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
