/**
 * Mock ↔ engine drift found by the bug hunt: each case pins the mock to what the engine really
 * does (the engine side is covered by `src-tauri/tests/*.rs`), so a flow that passes here also
 * works against PDFium.
 */
import { describe, expect, it } from "vitest";
import * as api from "./api";
import { mock, mockEvents, resetMock } from "./mock";
import type { DocChangedEvent, JobEvent } from "./types";

const SAMPLE = "/Users/veri/Documents/SeePDF-샘플.pdf";

describe("mock ↔ engine contract", () => {
  it("duplicate puts each copy right after its source (engine: import at index + 1)", async () => {
    const info = await api.openDocument({ path: SAMPLE });
    await api.pageOps({ docId: info.docId, ops: [{ kind: "rotate", pages: [0], delta: 90 }] });
    await api.pageOps({ docId: info.docId, ops: [{ kind: "rotate", pages: [2], delta: 180 }] });
    const after = await api.pageOps({ docId: info.docId, ops: [{ kind: "duplicate", pages: [2, 0, 0] }] });
    // [p0 90, p1 0, p2 180] → [p0, p0', p1, p2, p2'] (indices deduplicated, like check_pages)
    expect(after.pages.map((p) => p.rotation)).toEqual([90, 90, 0, 180, 180]);
    expect(after.pages.map((p) => p.index)).toEqual([0, 1, 2, 3, 4]);
    expect(after.pageCount).toBe(5);
  });

  it("save keeps the generation and emits doc-saved only (no doc-changed)", async () => {
    const info = await api.openDocument({ path: SAMPLE });
    await api.pageOps({ docId: info.docId, ops: [{ kind: "rotate", pages: [0], delta: 90 }] });
    const edited = await api.getDocument({ docId: info.docId });
    expect(edited.dirty).toBe(true);
    const changed: DocChangedEvent[] = [];
    const saved: unknown[] = [];
    const off1 = mockEvents.on("doc-changed", (e) => changed.push(e as DocChangedEvent));
    const off2 = mockEvents.on("doc-saved", (e) => saved.push(e));
    const events: JobEvent[] = [];
    const result = await api.saveDocument({ docId: info.docId }, (e) => events.push(e));
    off1();
    off2();
    expect(result.docGeneration).toBe(edited.docGeneration);
    const now = await api.getDocument({ docId: info.docId });
    expect(now.docGeneration).toBe(edited.docGeneration);
    expect(now.dirty).toBe(false);
    expect(changed).toHaveLength(0);
    expect(saved).toEqual([{ docId: info.docId, path: SAMPLE, docGeneration: edited.docGeneration }]);
  });

  it("update_annotation maps the patch like the engine: rects → quads + rect, no stray keys", async () => {
    const info = await api.openDocument({ path: SAMPLE });
    const created = await api.createAnnotation({
      docId: info.docId,
      page: 0,
      spec: { kind: "highlight", rects: [{ l: 10, b: 10, r: 100, t: 20 }], color: [255, 230, 0], opacity: 0.5 },
    });
    const id = created.annot!.id;
    const moved = await api.updateAnnotation({
      docId: info.docId, page: 0, id,
      patch: { rects: [{ l: 210, b: 210, r: 300, t: 220 }, { l: 210, b: 196, r: 280, t: 206 }] },
    });
    const annot = moved.annot!;
    expect(annot.quads).toEqual([{ l: 210, b: 210, r: 300, t: 220 }, { l: 210, b: 196, r: 280, t: 206 }]);
    expect(annot.rect).toEqual({ l: 210, b: 196, r: 300, t: 220 });
    expect("rects" in annot).toBe(false);

    const ink = await api.createAnnotation({
      docId: info.docId, page: 0,
      spec: { kind: "ink", paths: [[0, 0, 10, 10]], color: [0, 0, 0], width: 2, opacity: 1 },
    });
    const redrawn = (await api.updateAnnotation({
      docId: info.docId, page: 0, id: ink.annot!.id, patch: { paths: [[100, 100, 150, 180]] },
    })).annot!;
    expect(redrawn.inkPaths).toEqual([[100, 100, 150, 180]]);
    expect(redrawn.rect).toEqual({ l: 99, b: 99, r: 151, t: 181 });
    expect("paths" in redrawn).toBe(false);

    const line = await api.createAnnotation({
      docId: info.docId, page: 0,
      spec: { kind: "line", p1: [0, 0], p2: [10, 0], color: [0, 0, 0], width: 2, opacity: 1 },
    });
    const stretched = (await api.updateAnnotation({
      docId: info.docId, page: 0, id: line.annot!.id, patch: { p2: [50, 40] },
    })).annot!;
    expect(stretched.linePoints).toEqual([0, 0, 50, 40]);
    expect(stretched.inkPaths?.[0]).toEqual([0, 0, 50, 40]);
    expect("p2" in stretched).toBe(false);

    const box = await api.createAnnotation({
      docId: info.docId, page: 0,
      spec: { kind: "textbox", rect: { l: 0, b: 0, r: 100, t: 30 }, text: "가", fontSize: 12, color: [0, 0, 0], align: "left", fillColor: null },
    });
    const retyped = (await api.updateAnnotation({
      docId: info.docId, page: 0, id: box.annot!.id, patch: { text: "나다", contents: "나다", borderWidth: 3 },
    })).annot!;
    expect(retyped.text).toBe("나다");
    expect(retyped.contents).toBe("나다");
    expect(retyped.borderWidth).toBe(3);
  });

  it("compress_apply answers `stale` once the document changed after the estimate", async () => {
    const info = await api.openDocument({ path: SAMPLE });
    const report = await new Promise<NonNullable<Extract<JobEvent, { type: "done" }>["report"]>>((resolve) => {
      void api.compressEstimate({ docId: info.docId, options: { targetDpi: 150 } }, (e) => {
        if (e.type === "done" && e.report) resolve(e.report);
      });
    });
    await api.pageOps({ docId: info.docId, ops: [{ kind: "rotate", pages: [0], delta: 90 }] });
    await expect(api.compressApply({ docId: info.docId, token: report.token })).rejects.toMatchObject({ code: "stale" });
    // the token was spent, like `take_pending`
    await expect(api.compressApply({ docId: info.docId, token: report.token })).rejects.toMatchObject({ code: "notFound" });
  });

  it("delete_annotations with an id that is not on the page is notFound (with the page), and changes nothing", async () => {
    const info = await api.openDocument({ path: SAMPLE });
    const before = await api.listAnnotations({ docId: info.docId, page: 0 });
    await expect(api.deleteAnnotations({ docId: info.docId, page: 0, ids: ["nope"] })).rejects.toMatchObject({
      code: "notFound",
      page: 0,
    });
    const after = await api.getDocument({ docId: info.docId });
    expect(after.docGeneration).toBe(info.docGeneration);
    expect(after.canUndo).toBe(false);
    expect((await api.listAnnotations({ docId: info.docId, page: 0 })).annots).toEqual(before.annots);
  });

  it("settings keep the engine's defaults across resetMock, and a bad patch is refused", async () => {
    resetMock();
    const settings = await mock.getSettings();
    expect(settings.night).toBe("off");
    expect(settings.checkUpdates).toBe(true);
    expect(settings.signatures).toEqual([]);
    await expect(mock.setSettings({ patch: { theme: "purple" } as never })).rejects.toMatchObject({ code: "invalidArgument" });
    await expect(mock.setSettings({ patch: { tileCacheMb: "64" } as never })).rejects.toMatchObject({ code: "invalidArgument" });
    expect((await mock.getSettings()).theme).toBe(settings.theme);
    // an unknown 야간 모드 reads as off, like the engine's lenient `night`
    expect((await mock.setSettings({ patch: { night: "amber" } as never })).night).toBe("off");
    expect((await mock.setSettings({ patch: { night: "sepia" } })).night).toBe("sepia");
  });
});
