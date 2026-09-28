import { describe, expect, it, vi } from "vitest";
import * as api from "./api";
import { mockEvents } from "./mock";
import type { DocChangedEvent, JobEvent, SearchEvent } from "./types";

const SAMPLE = "/Users/veri/Documents/SeePDF-샘플.pdf";

describe("mock adapter — documents", () => {
  it("opens a fixture document with pages, outline and permissions", async () => {
    const info = await api.openDocument({ path: SAMPLE });
    expect(info.docId).toBe("d1");
    expect(info.name).toBe("SeePDF-샘플.pdf");
    expect(info.pageCount).toBe(3);
    expect(info.pages).toHaveLength(3);
    expect(info.pages[0].widthPt).toBeCloseTo(595.28, 2);
    expect(info.docGeneration).toBe(1);
    expect(info.dirty).toBe(false);

    const outline = await api.getOutline({ docId: info.docId });
    expect(outline[0].title).toBe("1. 시작하기");
    expect(outline[0].children).toHaveLength(2);
  });

  it("rejects an unknown document with a SeePdfError carrying a code", async () => {
    await expect(api.getDocument({ docId: "nope" })).rejects.toMatchObject({ code: "notFound" });
    try {
      await api.getDocument({ docId: "nope" });
    } catch (e) {
      expect(api.isSeePdfError(e)).toBe(true);
      expect(api.errorKey(e)).toBe("error.fileMissing");
    }
  });

  it("asks for a password when the path looks encrypted", async () => {
    await expect(api.openDocument({ path: "/tmp/encrypted.pdf" })).rejects.toMatchObject({ code: "passwordRequired" });
  });
});

describe("mock adapter — text layer and search", () => {
  it("returns a binary text layer with real word boxes", async () => {
    const info = await api.openDocument({ path: SAMPLE });
    const layer = await api.getTextLayer({ docId: info.docId, page: 0 });
    expect(layer.header.charCount).toBeGreaterThan(50);
    expect(layer.header.wordCount).toBeGreaterThan(5);
    expect(layer.text).toContain("SeePDF");
    const first = layer.char(0);
    expect(first.box.r).toBeGreaterThan(first.box.l);
    expect(first.box.t).toBeGreaterThan(first.box.b);
    expect(layer.line(0).box.l).toBeCloseTo(72, 1);
  });

  it("streams search hits page by page and can be cancelled", async () => {
    const info = await api.openDocument({ path: SAMPLE });
    const events: SearchEvent[] = [];
    await api.searchStart(
      { docId: info.docId, query: "PDF", matchCase: false, wholeWord: false, fromPage: 0 },
      (e) => events.push(e),
    );
    await vi.waitFor(() => expect(events.at(-1)?.type).toBe("done"), { timeout: 2000 });
    const pageEvents = events.filter((e) => e.type === "page");
    expect(pageEvents).toHaveLength(3);
    const done = events.at(-1) as Extract<SearchEvent, { type: "done" }>;
    expect(done.total).toBeGreaterThan(0);

    const cancelled: SearchEvent[] = [];
    const jobId = await api.searchStart(
      { docId: info.docId, query: "한국어", matchCase: false, wholeWord: false, fromPage: 0 },
      (e) => cancelled.push(e),
    );
    expect(await api.cancelJob({ jobId })).toBe(true);
    expect(cancelled.at(-1)?.type).toBe("cancelled");
  });
});

describe("mock adapter — mutations, events and history", () => {
  it("creates an annotation, bumps the generation and emits doc-changed", async () => {
    const info = await api.openDocument({ path: SAMPLE });
    const seen: DocChangedEvent[] = [];
    const off = mockEvents.on("doc-changed", (e) => seen.push(e as DocChangedEvent));

    const before = await api.listAnnotations({ docId: info.docId, page: 0 });
    expect(before.annots).toHaveLength(1);

    const result = await api.createAnnotation({
      docId: info.docId,
      page: 0,
      spec: { kind: "highlight", rects: [{ l: 72, b: 600, r: 300, t: 616 }], color: [255, 216, 77], opacity: 0.4 },
    });
    expect(result.annot?.kind).toBe("highlight");
    expect(result.list.annots).toHaveLength(2);
    expect(seen).toHaveLength(1);
    expect(seen[0].reason).toBe("edit");

    const after = await api.getDocument({ docId: info.docId });
    expect(after.docGeneration).toBe(info.docGeneration + 1);
    expect(after.dirty).toBe(true);
    expect(after.canUndo).toBe(true);

    const undone = await api.undo({ docId: info.docId });
    expect(undone.canRedo).toBe(true);
    // Like the engine, undo never hands a generation out twice (it only grows).
    expect(undone.docGeneration).toBe(after.docGeneration + 1);
    const list = await api.listAnnotations({ docId: info.docId, page: 0 });
    expect(list.annots).toHaveLength(1);
    const redone = await api.redo({ docId: info.docId });
    expect(redone.docGeneration).toBe(undone.docGeneration + 1);
    expect((await api.listAnnotations({ docId: info.docId, page: 0 })).annots).toHaveLength(2);
    off();
  });

  it("applies page operations and reindexes", async () => {
    const info = await api.openDocument({ path: SAMPLE });
    const rotated = await api.pageOps({ docId: info.docId, ops: [{ kind: "rotate", pages: [1], delta: 90 }] });
    expect(rotated.pages[1].rotation).toBe(90);

    const deleted = await api.pageOps({ docId: info.docId, ops: [{ kind: "delete", pages: [0] }] });
    expect(deleted.pageCount).toBe(2);
    expect(deleted.pages.map((p) => p.index)).toEqual([0, 1]);
  });

  it("reports save progress and clears the dirty flag", async () => {
    const info = await api.openDocument({ path: SAMPLE });
    await api.createAnnotation({
      docId: info.docId,
      page: 0,
      spec: { kind: "note", at: [100, 700], color: [255, 154, 61], contents: "메모" },
    });
    const events: JobEvent[] = [];
    const saved = await api.saveDocument({ docId: info.docId }, (e) => events.push(e));
    expect(saved.path).toBe(SAMPLE);
    expect(events[0].type).toBe("started");
    expect(events.at(-1)?.type).toBe("done");
    expect((await api.getDocument({ docId: info.docId })).dirty).toBe(false);
  });
});

describe("mock adapter — app state", () => {
  it("serves recents and settings, and records the newly opened file first", async () => {
    const recents = await api.getRecent();
    expect(recents.length).toBeGreaterThan(2);
    expect(recents.some((r) => r.pinned)).toBe(true);

    await api.openDocument({ path: "/Users/veri/Downloads/tracemonkey.pdf" });
    const after = await api.getRecent();
    expect(after[0].path).toBe("/Users/veri/Downloads/tracemonkey.pdf");

    const settings = await api.getSettings();
    expect(settings.locale).toBe("ko");
    const patched = await api.setSettings({ patch: { theme: "dark" } });
    expect(patched.theme).toBe("dark");
  });

  it("estimates exports and lists form fields", async () => {
    const info = await api.openDocument({ path: SAMPLE });
    const fields = await api.listFormFields({ docId: info.docId, page: 2 });
    expect(fields).toHaveLength(3);
    const set = await api.setFormFieldValue({ docId: info.docId, page: 2, index: 0, value: { text: "박현우" } });
    expect(set.field.value).toBe("박현우");

    const estimate = await api.estimateExport({ docId: info.docId, pages: [0, 1], format: "png", dpi: 150 });
    expect(estimate.bytes).toBeGreaterThan(0);
  });
});

describe("mock adapter — paragraph flow (Stage 9)", () => {
  const words = (n: number) => Array.from({ length: n }, (_, i) => `word${i}`).join(" ");

  it("a dry run changes nothing; push moves the content below; fit and overlap leave it; obstacles block", async () => {
    const info = await api.openDocument({ path: SAMPLE });
    const docId = info.docId;
    const page = 0;
    const before = await api.listPageObjects({ docId, page });
    const probe = (await api.probeParagraph({ docId, page, at: [100, 692] }))!;
    expect(probe.objectIds).toHaveLength(2);
    const lineB = () => api.listPageObjects({ docId, page }).then((r) => r.objects.find((o) => o.text?.startsWith("The mock"))!);
    const b0 = (await lineB()).rect;
    const call = async (text: string, flow: "push" | "overlap" | "fit", dryRun = false) => {
      const g = (await api.getDocument({ docId })).docGeneration;
      return api.editParagraph({
        docId, page, expectGeneration: g, allowFontSubstitution: false,
        edit: { objectIds: probe.objectIds, text, flow, ...(dryRun ? { dryRun } : null) },
      });
    };

    const events: DocChangedEvent[] = [];
    const off = mockEvents.on("doc-changed", (e) => events.push(e as DocChangedEvent));
    const dry = await call(words(18), "push", true);
    expect(dry).toMatchObject({ lines: 3, shiftedPt: 18, movedObjects: 2, overflowPt: 0 });
    expect(dry.blocked).toBeUndefined();
    expect(events).toHaveLength(0);
    const after = await api.getDocument({ docId });
    expect(after.docGeneration).toBe(info.docGeneration);
    expect(after.canUndo).toBe(false);
    expect((await api.listPageObjects({ docId, page })).objects).toEqual(before.objects);

    const fit = await call(words(18), "fit", true);
    expect(fit.fitScale).toBeCloseTo(0.92);
    expect(fit).toMatchObject({ shiftedPt: 0, movedObjects: 0, overflowPt: 0 });
    const overlap = await call(words(18), "overlap", true);
    expect(overlap.shiftedPt).toBe(0);

    expect(dry.objects.objects).toEqual([]); // a dry run does not list the page
    expect(dry.movedBand).toMatchObject({ l: 72, t: 672.11 });

    // a full-width figure under paragraph B: an obstacle, not content that moves. One more line
    // fits into the room (7.36) plus the 20.78 pt paragraph gap above B: not blocked.
    await api.addImageObject({ docId, page, rect: { l: 40, r: 560, t: 612, b: 500 }, path: "/tmp/f.png", keepAspect: false });
    const absorbed = await call(words(18), "push", true);
    expect(absorbed).toMatchObject({ roomPt: 7.36, shiftedPt: 7.36, overflowPt: 0 });
    expect(absorbed.blocked).toBeUndefined();
    // two more lines do not: 36 − 7.36 − 20.78 = 7.86 pt still overlap
    const blocked = await call(words(25), "push", true);
    expect(blocked).toMatchObject({ blocked: "obstacle", roomPt: 7.36, shiftedPt: 7.36, overflowPt: 7.86 });

    const written = await call(words(18), "push");
    off();
    expect(written.objects.docGeneration).toBe(info.docGeneration + 2);
    expect((await lineB()).rect.t).toBeCloseTo(b0.t - 7.36);
    expect((await api.getDocument({ docId })).undoLabel).toBe("undo.paragraphEdit");
    await api.undo({ docId });
    expect((await lineB()).rect).toEqual(b0);
  });
});
