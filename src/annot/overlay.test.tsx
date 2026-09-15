/**
 * The overlay's two React-side contracts:
 *
 * 1. the optimistic ghost disappears on the **page bitmap's `onload`** for the generation that
 *    contains it (ARCHITECTURE §10: "the ghost is removed on the new tile's onload — no flash");
 * 2. an annotation the engine has already painted is *not* painted again — only outlined when it
 *    is hovered or selected — or its opacity would double.
 */
import { describe, expect, it, beforeEach } from "vitest";
import { fireEvent, render, screen } from "@testing-library/react";
import type { AnnotSpec } from "../ipc/types";
import { makePageLayerContext } from "../viewer";
import { useAnnotStore } from "../store/annotStore";
import { useDocStore } from "../store/docStore";
import { createAnnotation, resetPatchQueue } from "./actions";
import { PageShell } from "../viewer/PageShell";
import { AnnotOverlay } from "./AnnotOverlay";

const SQUARE: AnnotSpec = { kind: "square", rect: { l: 10, b: 10, r: 60, t: 60 }, color: [245, 83, 61], fillColor: null, width: 2, opacity: 1 };

async function openAndContext() {
  const info = await useDocStore.getState().open("/tmp/sample.pdf");
  if (!info) throw new Error("mock open failed");
  const ctx = makePageLayerContext({
    docId: info.docId,
    docGeneration: info.docGeneration,
    page: info.pages[0],
    rotation: 0,
    zoomPercent: 100,
    width: info.pages[0].widthPt,
    height: info.pages[0].heightPt,
  });
  return { info, ctx };
}

describe("annot overlay", () => {
  beforeEach(() => {
    useAnnotStore.getState().reset();
    resetPatchQueue();
  });

  it("drops the ghost when the page bitmap of the new generation loads", async () => {
    const { ctx } = await openAndContext();
    const annot = await createAnnotation(0, SQUARE);
    expect(annot).not.toBeNull();
    expect(useAnnotStore.getState().ghosts).toHaveLength(1);
    const ghostGeneration = useAnnotStore.getState().ghosts[0].generation;
    expect(ghostGeneration).not.toBeNull();

    // Stage 2: the ghost settles on `PageShell`'s own bitmap `onload`, at the generation the
    // shell is rendering (ARCHITECTURE §10) — no probe `<img>` of its own any more.
    const { container } = render(
      <PageShell
        ctx={{ ...ctx, docGeneration: ghostGeneration as number }}
        left={0}
        top={0}
        bitmapScale={1}
        bitmapWidth={ctx.width}
        bitmapHeight={ctx.height}
        placeholderUrl="data:image/png;base64,iVBORw0KGgo="
        bitmapUrl={null}
        tiles={[]}
        night="off"
        current
        label="page 1"
        onTileLoad={() => undefined}
        onTileError={() => undefined}
        onPageRendered={(page, generation) => useAnnotStore.getState().pageRendered(page, generation)}
      />,
    );
    const img = container.querySelector("img.ph");
    expect(img).not.toBeNull();
    fireEvent.load(img as Element);
    expect(useAnnotStore.getState().ghosts).toHaveLength(0);
  });

  it("a bitmap for an older generation leaves the ghost alone", async () => {
    const { ctx } = await openAndContext();
    await createAnnotation(0, SQUARE);
    expect(useAnnotStore.getState().ghosts).toHaveLength(1);
    const { container } = render(
      <PageShell
        ctx={{ ...ctx, docGeneration: 1 }}
        left={0}
        top={0}
        bitmapScale={1}
        bitmapWidth={ctx.width}
        bitmapHeight={ctx.height}
        placeholderUrl="data:image/png;base64,iVBORw0KGgo="
        bitmapUrl={null}
        tiles={[]}
        night="off"
        current
        label="page 1"
        onTileLoad={() => undefined}
        onTileError={() => undefined}
        onPageRendered={(page, generation) => useAnnotStore.getState().pageRendered(page, generation)}
      />,
    );
    fireEvent.load(container.querySelector("img.ph") as Element);
    expect(useAnnotStore.getState().ghosts).toHaveLength(1);
  });

  it("outlines a listed annotation instead of repainting it, and paints a ghost in full", async () => {
    const { ctx, info } = await openAndContext();
    const fixture = (await import("../ipc/api")).listAnnotations({ docId: info.docId, page: 0 });
    const list = await fixture;
    useAnnotStore.getState().setPage(0, list.annots, list.docGeneration);

    const { container, rerender } = render(
      <svg>
        <AnnotOverlay ctx={ctx} />
      </svg>,
    );
    // nothing selected: no outline, no paint
    expect(container.querySelector(".annot-outline")).toBeNull();
    expect(container.querySelector(".annot-ghost")).toBeNull();

    useAnnotStore.getState().select([list.annots[0].id]);
    rerender(
      <svg>
        <AnnotOverlay ctx={ctx} />
      </svg>,
    );
    expect(container.querySelector('.annot-outline[data-state="selected"]')).not.toBeNull();

    await createAnnotation(0, SQUARE);
    rerender(
      <svg>
        <AnnotOverlay ctx={ctx} />
      </svg>,
    );
    expect(container.querySelector(".annot-ghost")).not.toBeNull();
  });

  it("keeps the handles of a selected shape at a constant pixel size", async () => {
    const { info } = await openAndContext();
    const annot = await createAnnotation(0, SQUARE);
    if (!annot) throw new Error("create failed");
    useAnnotStore.getState().select([annot.id]);
    const zoomed = makePageLayerContext({
      docId: info.docId,
      docGeneration: info.docGeneration,
      page: info.pages[0],
      rotation: 0,
      zoomPercent: 400,
      width: info.pages[0].widthPt * 4,
      height: info.pages[0].heightPt * 4,
    });
    const { container } = render(
      <svg>
        <AnnotOverlay ctx={zoomed} />
      </svg>,
    );
    const handle = container.querySelector(".annot-handles rect");
    expect(handle).not.toBeNull();
    // 14 CSS px at 400 % = 3.5 pt in user space
    expect(Number(handle?.getAttribute("width"))).toBeCloseTo(3.5, 5);
    expect(screen).toBeDefined();
  });
});
