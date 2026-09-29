/**
 * v0.3 O1 — the language rules the OCR sheet, 여러 파일 OCR and 설정 share, and the per-engine lists.
 */
import { describe, expect, it } from "vitest";
import {
  effectiveLanguages, initialLanguages, langsFor, parseLangs, toggleLanguage,
} from "./languages";
import {
  autoFellBack, choosableLanguages, engineChoices, engineHintKey, languagesFor, nativeEngineOf, pickEngine,
  readableLanguages, resolveEngine, runLanguages, toggleLanguageFor, visionLanguages, windowsLanguages,
} from "./engine";
import type { OcrCapabilities } from "../ipc/types";

const CAPS: OcrCapabilities = {
  engines: ["tesseract", "vision"], languages: ["kor", "eng"],
  engineLanguages: { tesseract: ["kor", "eng", "jpn"], vision: ["kor", "eng", "jpn", "chi_sim"], windows: [] },
};

describe("ocr.languages", () => {
  it("joins in chip order, Korean always with English", () => {
    expect(langsFor(["kor", "eng"])).toBe("kor+eng");
    expect(langsFor(["kor"])).toBe("kor+eng");
    expect(langsFor(["jpn", "kor", "eng"])).toBe("kor+eng+jpn");
    expect(langsFor(["chi_sim", "jpn"])).toBe("jpn+chi_sim");
    expect(langsFor(["eng"])).toBe("eng");
    expect(langsFor(["xyz"])).toBe("kor+eng");
    expect(parseLangs("kor+eng+jpn")).toEqual(["kor", "eng", "jpn"]);
  });

  it("the last chip cannot be turned off; a new one takes its place in chip order", () => {
    expect(toggleLanguage(["eng"], "eng")).toEqual(["eng"]);
    expect(toggleLanguage(["kor", "eng"], "eng")).toEqual(["kor"]);
    expect(toggleLanguage(["eng"], "kor")).toEqual(["kor", "eng"]);
    expect(toggleLanguage(["kor", "eng"], "chi_sim")).toEqual(["kor", "eng", "chi_sim"]);
  });

  it("settings seed the chips, limited to what the engine reads", () => {
    expect(initialLanguages(["eng"], ["kor", "eng"])).toEqual(["eng"]);
    expect(initialLanguages(["jpn"], ["kor", "eng"])).toEqual(["kor", "eng"]);
    expect(initialLanguages(undefined, ["kor", "eng"])).toEqual(["kor", "eng"]);
    expect(effectiveLanguages(["kor", "jpn"], ["kor", "eng"])).toEqual(["kor"]);
    expect(effectiveLanguages(["jpn"], ["kor", "eng"])).toEqual(["kor", "eng"]);
  });

  it("each engine reads its own list", () => {
    expect(languagesFor(CAPS, "tesseract")).toEqual(["kor", "eng", "jpn"]);
    expect(languagesFor(CAPS, "vision")).toEqual(["kor", "eng", "jpn", "chi_sim"]);
    expect(languagesFor(null, "vision")).toEqual(["kor", "eng"]);
    // An older backend without engineLanguages: the tesseract baseline.
    expect(languagesFor({ engines: ["tesseract"], languages: ["kor", "eng"] }, "tesseract")).toEqual(["kor", "eng"]);
  });

  it("maps to the native engines' BCP 47 tags", () => {
    expect(visionLanguages("kor+eng+jpn")).toEqual(["ko-KR", "en-US", "ja-JP"]);
    expect(visionLanguages("chi_sim")).toEqual(["zh-Hans"]);
  });

  it("the native engine is Vision on a Mac and Windows OCR on Windows", () => {
    expect(nativeEngineOf(CAPS)).toBe("vision");
    expect(nativeEngineOf({ engines: ["tesseract", "windows"], languages: [] })).toBe("windows");
    expect(nativeEngineOf({ engines: ["tesseract"], languages: [] })).toBeNull();
    expect(resolveEngine("auto", "windows")).toBe("windows");
    expect(resolveEngine("vision", null)).toBe("tesseract");
    expect(engineChoices("windows")[1].labelKey).toBe("ocr.engine.windows");
  });
});

describe("자동 follows the selected languages (O3, verification round 1)", () => {
  const windows = (langs: string[]): OcrCapabilities => ({
    engines: ["tesseract", "windows"], languages: ["kor", "eng"],
    engineLanguages: { tesseract: ["kor", "eng"], vision: [], windows: langs },
  });

  it("takes the native engine only when it reads every selected language", () => {
    expect(pickEngine("auto", windows(["kor", "eng"]), ["kor", "eng"])).toBe("windows");
    expect(pickEngine("auto", windows(["eng"]), ["kor", "eng"])).toBe("tesseract");
    expect(pickEngine("auto", windows(["eng"]), ["eng"])).toBe("windows");
    // Korean decides first: a jpn pack does not outweigh a missing kor one.
    expect(pickEngine("auto", windows(["eng", "jpn"]), ["kor", "eng", "jpn"])).toBe("tesseract");
    // Neither reads everything and both read Korean: the one that reads more.
    expect(pickEngine("auto", windows(["kor", "eng", "jpn"]), ["kor", "eng", "jpn", "chi_sim"])).toBe("windows");
    expect(pickEngine("auto", null, ["kor"])).toBe("tesseract");
  });

  it("an explicit choice is honoured", () => {
    expect(pickEngine("vision", windows(["eng"]), ["kor", "eng"])).toBe("windows");
    expect(pickEngine("tesseract", windows(["kor", "eng"]), ["eng"])).toBe("tesseract");
  });

  it("under 자동 the chips are what either engine reads; the hint says when it fell back", () => {
    const caps = windows(["eng", "jpn"]);
    expect(choosableLanguages("auto", caps, "tesseract")).toEqual(["kor", "eng", "jpn"]);
    expect(choosableLanguages("vision", caps, "windows")).toEqual(["eng", "jpn"]);
    expect(autoFellBack("auto", caps, "tesseract")).toBe(true);
    expect(autoFellBack("auto", caps, "windows")).toBe(false);
    expect(autoFellBack("tesseract", caps, "tesseract")).toBe(false);
  });
});

// --- pkg7-ocr, verification round 2 (O3): Windows OCR reads one language per run --------------------
describe("Windows OCR reads one language (plus English) per run (O3, verification round 2)", () => {
  const caps = (tesseract: string[], windows: string[]): OcrCapabilities => ({
    engines: ["tesseract", "windows"], languages: ["kor", "eng"],
    engineLanguages: { tesseract, vision: [], windows },
  });
  const KO_EN_JA = ["kor", "eng", "jpn"];

  it("one recogniser: the first non-English language it has, plus English when wanted", () => {
    expect(windowsLanguages(KO_EN_JA, ["kor", "jpn"])).toEqual(["kor"]);
    expect(windowsLanguages(KO_EN_JA, ["kor", "eng", "jpn"])).toEqual(["kor", "eng"]);
    expect(windowsLanguages(KO_EN_JA, ["eng", "jpn"])).toEqual(["eng", "jpn"]);
    expect(windowsLanguages(KO_EN_JA, ["jpn", "eng"])).toEqual(["eng", "jpn"]);
    // The ja recogniser reads English even without the en-US pack.
    expect(windowsLanguages(["jpn"], ["eng", "jpn"])).toEqual(["eng", "jpn"]);
    expect(windowsLanguages(["eng"], ["kor", "eng"])).toEqual(["eng"]);
    expect(windowsLanguages(["kor"], ["eng"])).toEqual([]);
    // Vision and Tesseract read the whole selection in one pass.
    const c = { ...caps(KO_EN_JA, KO_EN_JA), engines: ["tesseract", "vision"] as OcrCapabilities["engines"] };
    c.engineLanguages = { tesseract: KO_EN_JA, vision: KO_EN_JA, windows: [] };
    expect(readableLanguages(c, "vision", ["kor", "jpn"])).toEqual(["kor", "jpn"]);
  });

  it("자동 takes Windows OCR only when one recogniser covers the selection", () => {
    // The verifier's case: kor + jpn with the ko, en and ja packs used to pick Windows (jpn dropped).
    expect(pickEngine("auto", caps(KO_EN_JA, KO_EN_JA), ["kor", "jpn"])).toBe("tesseract");
    expect(pickEngine("auto", caps(KO_EN_JA, KO_EN_JA), ["eng", "jpn"])).toBe("windows");
    expect(pickEngine("auto", caps(KO_EN_JA, KO_EN_JA), ["kor", "eng"])).toBe("windows");
    expect(pickEngine("auto", caps(KO_EN_JA, KO_EN_JA), ["jpn"])).toBe("windows");
    // Tesseract lacks jpn: neither reads both, both read Korean, a tie → Windows (faster), with the hint.
    expect(pickEngine("auto", caps(["kor", "eng"], KO_EN_JA), ["kor", "jpn"])).toBe("windows");
    expect(runLanguages(caps(["kor", "eng"], KO_EN_JA), "windows", ["kor", "jpn"])).toEqual(["kor"]);
    expect(engineHintKey("auto", caps(["kor", "eng"], KO_EN_JA), "windows", ["kor", "jpn"]))
      .toBe("ocr.engine.windowsOneLanguage");
    expect(engineHintKey("auto", caps(KO_EN_JA, KO_EN_JA), "windows", ["eng", "jpn"])).toBe("ocr.engine.autoHintWindows");
    expect(engineHintKey("auto", caps(KO_EN_JA, KO_EN_JA), "tesseract", ["kor", "jpn"]))
      .toBe("ocr.engine.autoFallbackWindows");
    expect(engineHintKey("tesseract", caps(KO_EN_JA, KO_EN_JA), "tesseract", ["kor", "jpn"])).toBeUndefined();
  });

  it("an explicit Windows OCR choice swaps the non-English chip instead of ignoring the click", () => {
    expect(toggleLanguageFor("vision", "windows", ["kor", "eng"], "jpn")).toEqual(["eng", "jpn"]);
    expect(toggleLanguageFor("vision", "windows", ["kor"], "jpn")).toEqual(["jpn"]);
    expect(toggleLanguageFor("vision", "windows", ["kor"], "eng")).toEqual(["kor", "eng"]);
    // Under 자동 the chips add up (the engine follows them), and other engines are unchanged.
    expect(toggleLanguageFor("auto", "windows", ["kor", "eng"], "jpn")).toEqual(["kor", "eng", "jpn"]);
    expect(toggleLanguageFor("vision", "vision", ["kor", "eng"], "jpn")).toEqual(["kor", "eng", "jpn"]);
  });
});
