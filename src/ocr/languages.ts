/**
 * OCR languages (v0.3 O1): the four the sheet can offer, and the rules every entry point shares — the
 * OCR sheet, 여러 파일 OCR and Settings' defaults (U1).
 *
 * Codes are tesseract's (`kor`, `eng`, `jpn`, `chi_sim`), which is also what `Settings.ocrLanguages`
 * stores and what `ocr_capabilities.engineLanguages` answers in. A chip shows only when the engine
 * that will run reads the language: Apple Vision always has 日本語 and 中文 (macOS 13+), Windows OCR
 * when the language pack is installed, Tesseract only when `prepare-ocr --langs jpn,chi_sim` staged
 * the traineddata (the release ships `kor` + `eng`; STAGE notes).
 */

export type OcrLanguage = "kor" | "eng" | "jpn" | "chi_sim";

/** Chip order, and the order languages are joined in. */
export const OCR_LANGUAGES: { code: OcrLanguage; labelKey: string }[] = [
  { code: "kor", labelKey: "ocr.language.ko" },
  { code: "eng", labelKey: "ocr.language.en" },
  { code: "jpn", labelKey: "ocr.language.ja" },
  { code: "chi_sim", labelKey: "ocr.language.zh" },
];

/** What every build reads: the traineddata `prepare-ocr` always fetches. */
export const BASELINE_LANGUAGES: OcrLanguage[] = ["kor", "eng"];

const ORDER = OCR_LANGUAGES.map((l) => l.code as string);

export function isOcrLanguage(code: string): code is OcrLanguage {
  return ORDER.includes(code);
}

/**
 * The selected chips → the tesseract language string (`kor+eng+jpn`), in chip order. Korean always
 * brings English (`kor` alone reads English as digits, spike §4.2); nothing selected means Korean +
 * English.
 */
export function langsFor(selected: readonly string[]): string {
  const set = new Set<string>(selected.filter(isOcrLanguage));
  if (set.has("kor")) set.add("eng");
  if (set.size === 0) return "kor+eng";
  return ORDER.filter((c) => set.has(c)).join("+");
}

/** `kor+eng` → `["kor", "eng"]` (unknown codes dropped). */
export function parseLangs(langs: string): OcrLanguage[] {
  return langs.split("+").map((s) => s.trim()).filter(isOcrLanguage);
}

/**
 * The chips to start with: the user's `Settings.ocrLanguages` (U1) limited to what the engine reads,
 * or Korean + English when nothing of it is available.
 */
export function initialLanguages(settings: readonly string[] | undefined, available: readonly string[]): OcrLanguage[] {
  const wanted = (settings?.length ? settings : BASELINE_LANGUAGES).filter(isOcrLanguage);
  const usable = wanted.filter((c) => available.includes(c));
  return usable.length ? usable : BASELINE_LANGUAGES.filter((c) => available.includes(c));
}

/** The selection an engine can actually run: unavailable chips dropped, Korean + English if none is left. */
export function effectiveLanguages(selected: readonly string[], available: readonly string[]): OcrLanguage[] {
  const usable = selected.filter(isOcrLanguage).filter((c) => available.includes(c));
  return usable.length ? usable : [...BASELINE_LANGUAGES];
}

/**
 * A chip click: toggles `code`, but the last selected chip stays on. (English may be turned off next
 * to Korean; `langsFor` still sends `kor+eng`.)
 */
export function toggleLanguage(selected: readonly OcrLanguage[], code: OcrLanguage): OcrLanguage[] {
  if (selected.includes(code)) {
    const next = selected.filter((c) => c !== code);
    return next.length ? next : [...selected];
  }
  return ORDER.filter((c) => c === code || selected.includes(c as OcrLanguage)) as OcrLanguage[];
}
