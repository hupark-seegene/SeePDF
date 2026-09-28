/**
 * 인식 엔진 (P1-11): 자동 → Apple Vision where the backend lists it, the language mapping, and the
 * once-per-session capability answer.
 */
import { beforeEach, describe, expect, it, vi } from "vitest";
import { mock } from "../ipc/mock";
import {
  isVisionAvailable, loadOcrCapabilities, resetOcrCapabilities, resolveEngine, visionLanguages,
} from "./engine";

beforeEach(() => {
  resetOcrCapabilities();
  vi.restoreAllMocks();
});

describe("ocr.engine", () => {
  it("자동 is Vision where it exists and Tesseract elsewhere", () => {
    expect(resolveEngine("auto", true)).toBe("vision");
    expect(resolveEngine("auto", false)).toBe("tesseract");
    expect(resolveEngine("tesseract", true)).toBe("tesseract");
    expect(resolveEngine("vision", true)).toBe("vision");
    // A stale choice on a machine without Vision still runs.
    expect(resolveEngine("vision", false)).toBe("tesseract");
  });

  it("maps the sheet's tesseract languages to Vision's", () => {
    expect(visionLanguages("kor+eng")).toEqual(["ko-KR", "en-US"]);
    expect(visionLanguages("kor")).toEqual(["ko-KR", "en-US"]);
    expect(visionLanguages("eng")).toEqual(["en-US"]);
    expect(visionLanguages("")).toEqual(["ko-KR", "en-US"]);
  });

  it("asks ocr_capabilities once per session", async () => {
    const caps = vi.spyOn(mock, "ocrCapabilities").mockResolvedValue({ engines: ["tesseract", "vision"], languages: ["kor", "eng"] });
    expect(await isVisionAvailable()).toBe(true);
    expect(await isVisionAvailable()).toBe(true);
    await loadOcrCapabilities();
    expect(caps).toHaveBeenCalledTimes(1);
  });

  it("reads a failed capability call as Tesseract only, and asks again next time", async () => {
    vi.spyOn(console, "warn").mockImplementation(() => undefined);
    const caps = vi.spyOn(mock, "ocrCapabilities")
      .mockRejectedValueOnce(new Error("backend gone"))
      .mockResolvedValue({ engines: ["tesseract", "vision"], languages: ["kor", "eng"] });
    expect(await isVisionAvailable()).toBe(false);
    expect(await isVisionAvailable()).toBe(true);
    expect(caps).toHaveBeenCalledTimes(2);
  });
});
