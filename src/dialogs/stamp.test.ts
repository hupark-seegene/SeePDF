import { describe, expect, it } from "vitest";
import {
  anchorStyle, buildStampSpec, expandTokens, initialStampForm, insertToken, isoDate, roleDefaults, stemOf, switchRole,
  validateStamp,
} from "./stamp";

const WM = "대외비";

describe("stamp.roleDefaults", () => {
  it("watermark: centred, 45°, translucent", () => {
    const d = roleDefaults("watermark", WM);
    expect(d).toMatchObject({ anchor: "mc", rotateDeg: 45, text: WM });
    expect(d.opacityPct).toBeLessThan(100);
  });
  it("header: top centre, upright, the file name", () => {
    expect(roleDefaults("header", WM)).toMatchObject({ anchor: "tc", rotateDeg: 0, text: "{{filename}}", opacityPct: 100 });
  });
  it("footer: bottom centre, upright, page x / total", () => {
    expect(roleDefaults("footer", WM)).toMatchObject({ anchor: "bc", rotateDeg: 0, text: "{{page}} / {{total}}" });
  });
});

describe("stamp.switchRole", () => {
  it("follows the new role's defaults while the text is untouched", () => {
    const next = switchRole(initialStampForm("watermark", WM), "footer", WM);
    expect(next).toMatchObject({ role: "footer", anchor: "bc", rotateDeg: 0, text: "{{page}} / {{total}}" });
  });
  it("keeps text the user typed", () => {
    const edited = { ...initialStampForm("watermark", WM), text: "초안" };
    const next = switchRole(edited, "header", WM);
    expect(next.text).toBe("초안");
    expect(next.anchor).toBe("tc");
  });
  it("keeps margin, source and image across roles", () => {
    const f = { ...initialStampForm("header", WM), marginPt: 20, source: "image" as const, imagePath: "/a.png" };
    expect(switchRole(f, "watermark", WM)).toMatchObject({ marginPt: 20, source: "image", imagePath: "/a.png" });
  });
});

describe("stamp tokens", () => {
  it("inserts at the caret and replaces a selection", () => {
    expect(insertToken("쪽 ", "page")).toEqual({ text: "쪽 {{page}}", caret: 10 });
    expect(insertToken("A-B", "total", 1, 2)).toEqual({ text: "A{{total}}B", caret: 10 });
    expect(insertToken("", "date", 5, 9)).toEqual({ text: "{{date}}", caret: 8 });
  });
  it("expands known tokens and leaves unknown ones literal", () => {
    const ctx = { page: 2, total: 7, date: "2026-09-28", filename: "보고서" };
    expect(expandTokens("{{page}} / {{total}}", ctx)).toBe("2 / 7");
    expect(expandTokens("{{filename}} · {{date}} {{nope}}", ctx)).toBe("보고서 · 2026-09-28 {{nope}}");
  });
  it("formats the date as local YYYY-MM-DD and strips the extension", () => {
    expect(isoDate(new Date(2026, 0, 5))).toBe("2026-01-05");
    expect(stemOf("보고서.final.pdf")).toBe("보고서.final");
    expect(stemOf(".hidden")).toBe(".hidden");
  });
});

describe("stamp.buildStampSpec / validateStamp", () => {
  it("builds a text spec with opacity 0..1 and 'all' pages", () => {
    const f = initialStampForm("watermark", WM);
    const spec = buildStampSpec(f, [0, 1, 2], true);
    expect(spec).toMatchObject({
      role: "watermark", anchor: "mc", rotateDeg: 45, marginPt: 36, pages: "all",
      source: { kind: "text", text: WM, fontSizePt: 60 },
    });
    expect(spec.opacity).toBeCloseTo(0.25);
  });
  it("sends a page list for a partial range and forces 0° for header/footer", () => {
    const f = { ...initialStampForm("footer", WM), rotateDeg: 30 };
    const spec = buildStampSpec(f, [1], false);
    expect(spec.pages).toEqual([1]);
    expect(spec.rotateDeg).toBe(0);
  });
  it("builds an image spec", () => {
    const f = { ...initialStampForm("watermark", WM), source: "image" as const, imagePath: "/logo.png", imageWidthPt: 100 };
    expect(buildStampSpec(f, [0], true).source).toEqual({ kind: "image", path: "/logo.png", widthPt: 100 });
  });
  it("clamps out-of-range numbers", () => {
    const f = { ...initialStampForm("watermark", WM), rotateDeg: 400, opacityPct: 150, marginPt: -3 };
    const spec = buildStampSpec(f, [0], true);
    expect(spec).toMatchObject({ rotateDeg: 180, opacity: 1, marginPt: 0 });
  });
  it("rejects empty text, a missing image and an empty range", () => {
    const f = initialStampForm("header", WM);
    expect(validateStamp(f, [0])).toBeNull();
    expect(validateStamp({ ...f, text: "  " }, [0])).toBe("emptyText");
    expect(validateStamp({ ...f, source: "image" }, [0])).toBe("noImage");
    expect(validateStamp(f, null)).toBe("range");
    expect(validateStamp(f, [])).toBe("range");
  });
});

describe("stamp.anchorStyle", () => {
  it("centres on the middle anchor and insets corners by the margin", () => {
    expect(anchorStyle("mc", 36, 612, 792)).toEqual({ style: { left: "50%", top: "50%" }, translate: "translate(-50%, -50%)" });
    const br = anchorStyle("br", 61.2, 612, 792);
    expect(br.style).toEqual({ right: "10.00%", bottom: "7.73%" });
    expect(br.translate).toBe("translate(0, 0)");
    expect(anchorStyle("tc", 0, 612, 792).style).toEqual({ left: "50%", top: "0.00%" });
  });
});
