/**
 * v0.3 O1 — the language rules the OCR sheet, 여러 파일 OCR and 설정 share, and the per-engine lists.
 */
import { describe, expect, it } from "vitest";
import {
  effectiveLanguages, initialLanguages, langsFor, parseLangs, toggleLanguage,
} from "./languages";
import { engineChoices, languagesFor, nativeEngineOf, resolveEngine, visionLanguages } from "./engine";
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
