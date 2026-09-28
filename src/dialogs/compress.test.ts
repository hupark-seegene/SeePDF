import { describe, expect, it } from "vitest";
import { COMPRESS_PRESETS, applyBlock, buildCompressOptions, formatDeltaPct, summarize } from "./compress";

describe("compress.summarize", () => {
  it("reports a saving as a negative delta", () => {
    expect(summarize({ beforeBytes: 1000, afterBytes: 577 })).toEqual({ deltaBytes: -423, deltaPct: -42.3, noGain: false });
  });
  it("flags no gain when the result is the same size or bigger", () => {
    expect(summarize({ beforeBytes: 1000, afterBytes: 1000 }).noGain).toBe(true);
    expect(summarize({ beforeBytes: 1000, afterBytes: 1002 })).toEqual({ deltaBytes: 2, deltaPct: 0.2, noGain: true });
  });
  it("survives an empty document", () => {
    expect(summarize({ beforeBytes: 0, afterBytes: 0 })).toEqual({ deltaBytes: 0, deltaPct: 0, noGain: true });
  });
});

describe("compress.applyBlock", () => {
  it("allows 적용 only for a real saving with at least one image downsampled", () => {
    expect(applyBlock({ beforeBytes: 1000, afterBytes: 577, imagesDownsampled: 3 })).toBeNull();
  });
  it("blocks a result that is not smaller", () => {
    expect(applyBlock({ beforeBytes: 1000, afterBytes: 1000, imagesDownsampled: 3 })).toBe("noGain");
    expect(applyBlock({ beforeBytes: 1000, afterBytes: 1002, imagesDownsampled: 3 })).toBe("noGain");
  });
  it("blocks a result that downsampled nothing, even if the rewrite came out smaller", () => {
    expect(applyBlock({ beforeBytes: 1000, afterBytes: 990, imagesDownsampled: 0 })).toBe("noImages");
  });
  it("reports 'not smaller' first when both hold", () => {
    expect(applyBlock({ beforeBytes: 1000, afterBytes: 1002, imagesDownsampled: 0 })).toBe("noGain");
    expect(applyBlock({ beforeBytes: 0, afterBytes: 0, imagesDownsampled: 0 })).toBe("noGain");
  });
});

describe("compress.formatDeltaPct", () => {
  it("uses a real minus sign, an explicit plus and the locale's decimals", () => {
    expect(formatDeltaPct(-42.3, "ko")).toBe("−42.3%");
    expect(formatDeltaPct(0.2, "en")).toBe("+0.2%");
    expect(formatDeltaPct(0, "ko")).toBe("0%");
    expect(formatDeltaPct(-5, "en")).toBe("−5%");
  });
});

describe("compress.buildCompressOptions", () => {
  it("omits pages for the whole document", () => {
    expect(buildCompressOptions(150, [0, 1, 2], true)).toEqual({ targetDpi: 150 });
    expect(buildCompressOptions(96, [1], false)).toEqual({ targetDpi: 96, pages: [1] });
  });
  it("offers 300 / 150 / 96 DPI in that order", () => {
    expect(COMPRESS_PRESETS.map((p) => p.dpi)).toEqual([300, 150, 96]);
  });
});
