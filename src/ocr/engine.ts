/**
 * 인식 엔진 (P1-11): which recogniser an OCR run uses.
 *
 *   자동         Apple Vision when the backend lists it (`ocr_capabilities`), Tesseract otherwise
 *   Apple Vision `ocr_recognize_native` — macOS 13+, rendered and recognised in the backend
 *   Tesseract    the bundled tesseract.js worker pool (every platform, fully offline)
 *
 * Both engines end in the same `OcrPage` (image pixels, origin top-left) and the same `ocr_apply`,
 * so the choice only changes where the words come from. The capability answer is asked once per
 * session: it is a property of the machine, not of the document.
 */
import { useEffect, useState } from "react";
import * as api from "../ipc/api";
import type { OcrEngine } from "../ipc/types";

/** What the user picks in the sheet. */
export type OcrEngineChoice = "auto" | "vision" | "tesseract";
/** What a run actually uses. */
export type OcrRunEngine = "vision" | "tesseract";

export const ENGINE_CHOICES: { id: OcrEngineChoice; labelKey: string }[] = [
  { id: "auto", labelKey: "ocr.engine.auto" },
  { id: "vision", labelKey: "ocr.engine.vision" },
  { id: "tesseract", labelKey: "ocr.engine.tesseract" },
];

/**
 * Vision, measured on this project's fixtures (Apple M5, 300 DPI): 0.13 s for a sparse 4-line page,
 * 0.36 s for the 10-line Korean page, 3.0 s for a dense two-column English page (736 words), plus
 * ~0.1 s of render — a typical scan sits in between. A second page in flight does not speed it up
 * (Vision serialises on the Neural Engine), so two pages at a time only overlap render and recognition.
 */
export const VISION_SECONDS_PER_PAGE = 0.8;
export const VISION_CONCURRENCY = 2;

/** 자동 → Vision where it exists; an explicit Vision request on a machine without it → Tesseract. */
export function resolveEngine(choice: OcrEngineChoice, visionAvailable: boolean): OcrRunEngine {
  if (choice === "tesseract") return "tesseract";
  return visionAvailable ? "vision" : "tesseract";
}

/** The tesseract language string of the sheet (`kor+eng`, `eng`) → Vision's BCP 47 list. */
export function visionLanguages(langs: string): string[] {
  const out: string[] = [];
  for (const code of langs.split("+").map((s) => s.trim()).filter(Boolean)) {
    const lang = code === "kor" ? "ko-KR" : code === "eng" ? "en-US" : code;
    if (!out.includes(lang)) out.push(lang);
  }
  // Korean always brings English along, as `kor+eng` does for tesseract.
  if (out.includes("ko-KR") && !out.includes("en-US")) out.push("en-US");
  return out.length ? out : ["ko-KR", "en-US"];
}

type Capabilities = { engines: OcrEngine[]; languages: string[] };

let cached: Promise<Capabilities> | null = null;

/** `ocr_capabilities`, once per session. A failed call is not cached and reads as "Tesseract only". */
export function loadOcrCapabilities(): Promise<Capabilities> {
  if (!cached) {
    cached = api.ocrCapabilities().catch((e: unknown) => {
      cached = null;
      console.warn("[ocr] ocr_capabilities failed; using tesseract only", e);
      return { engines: ["tesseract"] as OcrEngine[], languages: ["kor", "eng"] };
    });
  }
  return cached;
}

export async function isVisionAvailable(): Promise<boolean> {
  return (await loadOcrCapabilities()).engines.includes("vision");
}

/** `null` until the capability answer is in, then whether Apple Vision can run here. */
export function useVisionAvailable(): boolean | null {
  const [available, setAvailable] = useState<boolean | null>(null);
  useEffect(() => {
    let live = true;
    void isVisionAvailable().then((v) => { if (live) setAvailable(v); });
    return () => { live = false; };
  }, []);
  return available;
}

/** Tests only: forget the cached capability answer. */
export function resetOcrCapabilities(): void {
  cached = null;
}
