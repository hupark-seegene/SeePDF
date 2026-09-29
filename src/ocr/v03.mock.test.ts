/**
 * v0.3 pkg7-ocr: the mock adapter keeps the engine's O2 / O1 contract (`tests/ocr.rs` is the engine side),
 * so a flow that passes against the mock also works against PDFium.
 */
import { describe, expect, it } from "vitest";
import * as api from "../ipc/api";
import { mockEvents } from "../ipc/mock";
import type { DocChangedEvent, OcrPage } from "../ipc/types";

const SAMPLE = "/Users/veri/Documents/SeePDF-샘플.pdf";

describe("mock ↔ engine (v0.3 OCR)", () => {
  it("ocr_apply's setRotation turns the page in the same undo step, structure: true", async () => {
    const info = await api.openDocument({ path: SAMPLE });
    const geom = info.pages[0];
    const ocr = await api.ocrRecognizeNative({ docId: info.docId, page: 0, dpi: 300, languages: ["ko-KR"], rotate: 90 });
    expect(ocr.rotation).toBe(90);
    expect([ocr.widthPx, ocr.heightPx]).toEqual([
      Math.round((geom.heightPt * 300) / 72), Math.round((geom.widthPt * 300) / 72),
    ]);

    const events: DocChangedEvent[] = [];
    const off = mockEvents.on("doc-changed", (e) => events.push(e as DocChangedEvent));
    const after = await api.ocrApply({ docId: info.docId, pages: [{ page: 0, ocr, setRotation: 90 }], replaceExisting: false }, () => {});
    off();
    expect(after.pages[0].rotation).toBe(90);
    expect([after.pages[0].widthPt, after.pages[0].heightPt]).toEqual([geom.heightPt, geom.widthPt]);
    expect(after.docGeneration).toBe(info.docGeneration + 1);
    expect(events.at(-1)).toMatchObject({ reason: "ocr", structure: true });

    const undone = await api.undo({ docId: info.docId });
    expect(undone.pages[0].rotation).toBe(geom.rotation);
  });

  it("a result recognised at another rotation is refused", async () => {
    const info = await api.openDocument({ path: SAMPLE });
    const ocr: OcrPage = { page: 0, dpi: 300, widthPx: 100, heightPx: 100, rotation: 0, lines: [] };
    await expect(api.ocrApply({ docId: info.docId, pages: [{ page: 0, ocr, setRotation: 90 }], replaceExisting: false }, () => {}))
      .rejects.toMatchObject({ code: "invalidArgument" });
  });

  it("capabilities list languages per engine; detection answers upright", async () => {
    const caps = await api.ocrCapabilities();
    expect(caps.engineLanguages?.vision).toEqual(["kor", "eng", "jpn", "chi_sim"]);
    expect(caps.engineLanguages?.tesseract).toEqual(["kor", "eng"]);
    const info = await api.openDocument({ path: SAMPLE });
    const r = await api.ocrDetectOrientation({ docId: info.docId, page: 0, languages: ["ko-KR"] });
    expect(r.rotation).toBe(0);
    expect(r.scores).toHaveLength(4);
  });
});
