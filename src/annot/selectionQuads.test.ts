/**
 * The markup tools' quads come from the viewer's text layer, so this test drives the real thing:
 * the mock document's page 0 is loaded, a character range is selected, and the rectangles that
 * fall out are what `create_annotation` would receive (F-08, IPC_CONTRACT §7.1).
 */
import { beforeEach, describe, expect, it } from "vitest";
import { useDocStore } from "../store/docStore";
import { ensureTextLayer, getTextLayer, useSelectionStore } from "../viewer";
import { hasTextSelection, selectedPages, selectionRectsForPage } from "./selectionQuads";
import { makeMarkupTool } from "../tools/markup";
import type { ToolContext } from "../tools/ToolController";

async function openDoc() {
  const info = await useDocStore.getState().open("/tmp/sample.pdf");
  if (!info) throw new Error("mock open failed");
  ensureTextLayer(info.docId, info.docGeneration, 0);
  for (let i = 0; i < 40 && !getTextLayer(info.docId, info.docGeneration, 0); i++) {
    await new Promise((r) => setTimeout(r, 5));
  }
  return info;
}

describe("annot.selectionQuads", () => {
  beforeEach(() => {
    useSelectionStore.getState().clear();
  });

  it("produces one rectangle per line of the selection", async () => {
    const info = await openDoc();
    const layer = getTextLayer(info.docId, info.docGeneration, 0);
    expect(layer).not.toBeNull();
    if (!layer) return;

    // A range that certainly spans more than one line of the generated fixture page.
    const [firstLineStart] = layer.lineRange(0);
    const thirdLine = layer.lineRange(Math.min(layer.charCount - 1, firstLineStart + 200));
    useSelectionStore.getState().setSelection({
      docId: info.docId,
      anchor: { page: 0, offset: firstLineStart },
      focus: { page: 0, offset: thirdLine[1] },
    });

    const rects = selectionRectsForPage(0);
    expect(rects.length).toBeGreaterThanOrEqual(1);
    for (const r of rects) {
      expect(r.r).toBeGreaterThan(r.l);
      expect(r.t).toBeGreaterThan(r.b); // PDF user space, y-up (IPC_CONTRACT §3)
    }
    expect(hasTextSelection()).toBe(true);
    expect(selectedPages()).toEqual([0]);
  });

  it("feeds the markup tool, which commits exactly those rectangles as quads", async () => {
    const info = await openDoc();
    const layer = getTextLayer(info.docId, info.docGeneration, 0);
    if (!layer) throw new Error("no text layer");
    const line = layer.lineRange(0);
    useSelectionStore.getState().setSelection({
      docId: info.docId,
      anchor: { page: 0, offset: line[0] },
      focus: { page: 0, offset: line[1] },
    });
    const expected = selectionRectsForPage(0);

    const ctx: ToolContext = {
      docId: info.docId,
      docGeneration: info.docGeneration,
      style: { color: [255, 216, 77], opacity: 0.4, width: 2, fontSize: 12, fillColor: null, heads: [false, true], align: "left", eraserSize: 12 },
      modifiers: { shift: false, alt: false, meta: false, ctrl: false },
      scale: 1,
      selectionRects: selectionRectsForPage,
    };
    const tool = makeMarkupTool("highlight");
    let state = tool.init(ctx);
    state = tool.onDown(state, { page: 0, pt: [72, 700] }, ctx).state;
    const up = tool.onUp(state, { page: 0, pt: [300, 700] }, ctx);
    expect(up.commit?.page).toBe(0);
    const spec = up.commit?.spec;
    if (spec?.kind !== "highlight") throw new Error("expected a highlight");
    expect(spec.rects).toEqual(expected);
  });

  it("returns nothing for a page the selection does not touch", async () => {
    const info = await openDoc();
    useSelectionStore.getState().setSelection({
      docId: info.docId,
      anchor: { page: 0, offset: 0 },
      focus: { page: 0, offset: 20 },
    });
    expect(selectionRectsForPage(2)).toEqual([]);
  });
});
