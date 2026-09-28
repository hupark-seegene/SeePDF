/**
 * 영역 표시 (F-22) flows against the mock adapter: marks on two pages → debounced preview with
 * collateral → confirm → ONE `apply_redactions_batch` for both pages (one undo step) → toast; the
 * form-field refusal; `verifyFailed` (everything rolled back, every mark kept); cancel keeps the
 * marks; click-to-mark a text run, remove with ⌫; the leave guard; undo drops the marks; 영역
 * 표시로 표시 from a selection; Stage 8: a drag snaps to the runs it crosses and is clipped to the
 * page; closing / replacing the document with pending marks asks first.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { mock } from "../ipc/mock";
import type { RedactPreview } from "../ipc/types";
import { ensureTextLayer, getTextLayer, makePageLayerContext, useSelectionStore, type PageLayerContext } from "../viewer";
import { useAppStore } from "../store/appStore";
import { useDocStore } from "../store/docStore";
import { useToastStore } from "../app/toastStore";
import { useDialogStore } from "../dialogs/dialogState";
import { editLeaveGuard } from "../tools/commands";
import { RedactPanel } from "../app/Inspector/RedactPanel";
import { useEditStore } from "./editStore";
import { EditLayer } from "./EditLayer";
import { applyMarks, markTextSelection } from "./redact";
import { moveObjects } from "./actions";
import { editHost } from "./index";
import { closeDocumentFlow, openPath } from "../dialogs/flows";

let stop: (() => void) | null = null;

async function setup() {
  const info = await useDocStore.getState().open("/tmp/sample.pdf");
  if (!info) throw new Error("mock open failed");
  useAppStore.setState({ mode: "edit", tool: "redact" });
  stop = editHost.start();
  const ctxs = [0, 1].map((index) =>
    makePageLayerContext({
      docId: info.docId, docGeneration: info.docGeneration, page: info.pages[index], rotation: 0, zoomPercent: 100,
      width: info.pages[index].widthPt, height: info.pages[index].heightPt,
    }),
  );
  const surfaces = ctxs.map((ctx) => {
    const view = render(<EditLayer ctx={ctx} />);
    return view.container.querySelector(".edit-surface") as HTMLElement;
  });
  const panel = render(<RedactPanel />);
  await waitFor(() => expect(useEditStore.getState().pages[1]).toBeDefined());
  return { info, ctxs, surfaces, panel };
}

function at(ctx: PageLayerContext, x: number, y: number) {
  const [cx, cy] = ctx.toDevice(x, y);
  return { clientX: cx, clientY: cy, pointerId: 1, button: 0 };
}

function drag(surface: HTMLElement, ctx: PageLayerContext, from: [number, number], to: [number, number]) {
  fireEvent.pointerDown(surface, at(ctx, ...from));
  fireEvent.pointerMove(surface, at(ctx, ...to));
  fireEvent.pointerUp(surface, at(ctx, ...to));
}

const marks = () => useEditStore.getState().marks;
const toastKeys = () => useToastStore.getState().toasts.map((t) => t.messageKey);

function confirmTop() {
  const top = useDialogStore.getState().stack.at(-1);
  return top?.name === "confirm" ? top : null;
}

function answer(value: boolean) {
  act(() => (confirmTop()!.props.resolve as (v: boolean) => void)(value));
}

/** the mock preview + collateral on page 0 (PDFium cannot split a text object) */
function withCollateral(extra: Partial<RedactPreview> = {}) {
  const real = mock.redactPreview.bind(mock);
  return vi.spyOn(mock, "redactPreview").mockImplementation(async (a) => {
    const p = await real(a);
    if (a.page !== 0) return p;
    return {
      ...p,
      textObjects: p.textObjects.map((o, i) => ({ ...o, fullyInside: i > 0 })),
      collateral: ["Andreas Gal"],
      ...extra,
    };
  });
}

beforeEach(() => {
  useEditStore.getState().reset();
  useToastStore.setState({ toasts: [] });
  useDialogStore.getState().closeAll();
});

afterEach(() => {
  if (confirmTop()) answer(false);
  stop?.();
  stop = null;
  vi.restoreAllMocks();
});

describe("편집 · 영역 표시", () => {
  it("marks two pages, previews collateral, confirms, applies both pages in ONE batch call and toasts", async () => {
    const { info, ctxs, surfaces } = await setup();
    const preview = withCollateral();
    const apply = vi.spyOn(mock, "applyRedactionsBatch");
    const perPage = vi.spyOn(mock, "applyRedactions");

    drag(surfaces[0], ctxs[0], [60, 745], [400, 720]);
    drag(surfaces[1], ctxs[1], [60, 745], [300, 700]);
    expect(marks().map((m) => m.page)).toEqual([0, 1]);
    expect(screen.getByTestId("redact-count")).toHaveTextContent("표시된 영역 2개");
    expect(document.querySelectorAll(".redact-mark")).toHaveLength(2);

    // debounced preview: amber collateral in the panel and on the page
    await waitFor(() => expect(screen.getByText("표시한 영역 밖의 텍스트도 함께 제거됩니다")).toBeInTheDocument());
    expect(screen.getByText("Andreas Gal")).toBeInTheDocument();
    expect(await screen.findByTestId("redact-preview-1")).toBeInTheDocument();
    expect(document.querySelectorAll(".redact-collateral").length).toBeGreaterThan(0);
    expect(preview).toHaveBeenCalled();

    fireEvent.change(screen.getByLabelText("덮어쓸 문구"), { target: { value: "  비공개 " } });
    fireEvent.click(screen.getByRole("button", { name: "표시한 영역 적용" }));
    await waitFor(() => expect(confirmTop()).not.toBeNull());
    expect(confirmTop()!.props).toMatchObject({ danger: true, bodyKey: "redact.applyWarning" });
    expect(apply).not.toHaveBeenCalled();
    answer(true);

    await waitFor(() => expect(toastKeys()).toContain("redact.done"));
    expect(apply).toHaveBeenCalledTimes(1);
    expect(perPage).not.toHaveBeenCalled();
    const call = apply.mock.calls[0][0];
    expect(call.marks.map((m) => m.page)).toEqual([0, 1]);
    expect(call.marks.map((m) => m.rects.length)).toEqual([1, 1]);
    expect(call.options).toEqual({ fill: [0, 0, 0], overlayText: "비공개" });
    expect(useToastStore.getState().toasts.find((t) => t.messageKey === "redact.done")?.params).toEqual({ count: 2 });
    expect(marks()).toHaveLength(0);
    expect(useEditStore.getState().previews).toEqual({});
    // our own apply does not trip the "document changed" drop
    expect(toastKeys()).not.toContain("redact.marksDropped");

    // one undo step brings both pages back
    const before = await Promise.all([0, 1].map((page) => mock.listPageObjects({ docId: info.docId, page })));
    await act(async () => {
      await useDocStore.getState().undo();
    });
    const after = await Promise.all([0, 1].map((page) => mock.listPageObjects({ docId: info.docId, page })));
    expect(after[0].objects.length).toBeGreaterThan(before[0].objects.length);
    expect(after[1].objects.length).toBeGreaterThan(before[1].objects.length);
  });

  it("a mark over a form field disables 적용 with the reason and refuses", async () => {
    const { ctxs, surfaces } = await setup();
    withCollateral({ formFields: ["서명란"] });
    const apply = vi.spyOn(mock, "applyRedactionsBatch");
    drag(surfaces[0], ctxs[0], [60, 745], [400, 720]);
    await waitFor(() => expect(screen.getByRole("alert")).toHaveTextContent("양식 필드가 있는 영역은 적용할 수 없습니다: 서명란"));
    expect(screen.getByRole("button", { name: "표시한 영역 적용" })).toBeDisabled();
    await act(async () => {
      expect(await applyMarks()).toBe(false);
    });
    expect(confirmTop()).toBeNull();
    expect(apply).not.toHaveBeenCalled();
    expect(toastKeys()).toContain("redact.formFields");
    expect(marks()).toHaveLength(1);
  });

  it("verifyFailed rolls every page back and keeps every mark", async () => {
    const { ctxs, surfaces } = await setup();
    const apply = vi
      .spyOn(mock, "applyRedactionsBatch")
      .mockRejectedValueOnce({ code: "verifyFailed", message: "marked text still extractable" });
    drag(surfaces[0], ctxs[0], [60, 745], [400, 720]);
    drag(surfaces[1], ctxs[1], [60, 745], [300, 700]);
    const run = applyMarks();
    await waitFor(() => expect(confirmTop()).not.toBeNull());
    answer(true);
    await act(async () => {
      expect(await run).toBe(false);
    });
    expect(apply).toHaveBeenCalledTimes(1);
    const failed = useToastStore.getState().toasts.find((t) => t.messageKey === "redact.verifyFailed");
    expect(failed?.tone).toBe("danger");
    expect(toastKeys()).not.toContain("redact.done");
    expect(marks().map((m) => m.page)).toEqual([0, 1]);
  });

  it("cancel at the confirm keeps every mark and writes nothing", async () => {
    const { ctxs, surfaces } = await setup();
    const apply = vi.spyOn(mock, "applyRedactionsBatch");
    drag(surfaces[0], ctxs[0], [60, 745], [400, 720]);
    fireEvent.click(screen.getByRole("button", { name: "표시한 영역 적용" }));
    await waitFor(() => expect(confirmTop()).not.toBeNull());
    answer(false);
    await act(async () => undefined);
    expect(apply).not.toHaveBeenCalled();
    expect(marks()).toHaveLength(1);
  });

  it("a click on text marks the run; ⌫ removes the selected mark; × too", async () => {
    const { ctxs, surfaces } = await setup();
    const line = useEditStore.getState().pages[0]!.objects.find((o) => o.text?.startsWith("Lightweight"))!;
    fireEvent.pointerDown(surfaces[0], at(ctxs[0], 100, 730));
    fireEvent.pointerUp(surfaces[0], at(ctxs[0], 100, 730));
    expect(marks()).toHaveLength(1);
    expect(marks()[0].rect).toEqual(line.rect);
    expect(useEditStore.getState().markSel).toBe(marks()[0].id);
    fireEvent.keyDown(window, { key: "Backspace" });
    expect(marks()).toHaveLength(0);

    drag(surfaces[0], ctxs[0], [60, 500], [200, 400]);
    fireEvent.click(screen.getByRole("button", { name: "표시 지우기" }));
    expect(marks()).toHaveLength(0);
  });

  it("marks survive tool switches; leaving 편집 asks first; undo drops them", async () => {
    const { ctxs, surfaces } = await setup();
    drag(surfaces[0], ctxs[0], [60, 745], [400, 720]);
    act(() => useAppStore.getState().setTool("select"));
    expect(marks()).toHaveLength(1);

    const declined = editLeaveGuard();
    expect(declined).not.toBeNull();
    await waitFor(() => expect(confirmTop()).not.toBeNull());
    answer(false);
    expect(await declined).toBe(false);
    expect(marks()).toHaveLength(1);

    // our own 편집 edit keeps the marks (and re-previews) …
    const line = useEditStore.getState().pages[0]!.objects.find((o) => o.text?.startsWith("Lightweight"))!;
    await act(async () => {
      expect(await moveObjects(0, [line.objectId], 5, 0)).toBe(true);
    });
    expect(marks()).toHaveLength(1);
    expect(toastKeys()).not.toContain("redact.marksDropped");
    // … an undo moves the document under them → dropped with a toast
    await act(async () => {
      await useDocStore.getState().undo();
    });
    await waitFor(() => expect(marks()).toHaveLength(0));
    expect(toastKeys()).toContain("redact.marksDropped");
    expect(editLeaveGuard()).toBeNull();
  });

  it("영역 표시로 표시 turns the text selection's line rects into marks and arms 영역 표시", async () => {
    const info = await useDocStore.getState().open("/tmp/sample.pdf");
    if (!info) throw new Error("mock open failed");
    useAppStore.setState({ mode: "read", tool: "select" });
    ensureTextLayer(info.docId, info.docGeneration, 0);
    await waitFor(() => expect(getTextLayer(info.docId, info.docGeneration, 0)).not.toBeNull());
    const layer = getTextLayer(info.docId, info.docGeneration, 0)!;
    useSelectionStore.getState().setSelection({
      docId: info.docId,
      anchor: { page: 0, offset: 0 },
      focus: { page: 0, offset: Math.min(layer.charCount, 60) },
    });
    const added = markTextSelection();
    expect(added).toBeGreaterThan(0);
    expect(marks()).toHaveLength(added);
    expect(useAppStore.getState()).toMatchObject({ mode: "edit", tool: "redact" });
    expect(useSelectionStore.getState().selection).toBeNull();
  });
});

describe("편집 · 영역 표시 — Stage 8", () => {
  it("a drag snaps to the whole of every text run it crosses and keeps its own extent elsewhere", async () => {
    const { ctxs, surfaces } = await setup();
    const line = useEditStore.getState().pages[0]!.objects.find((o) => o.text?.startsWith("Lightweight"))!;
    const midY = (line.rect.b + line.rect.t) / 2;
    const midX = (line.rect.l + line.rect.r) / 2;
    // from the middle of the run to past its right end, a few points tall
    drag(surfaces[0], ctxs[0], [midX, midY + 2], [line.rect.r + 30, midY - 2]);
    expect(marks()).toHaveLength(1);
    const r = marks()[0].rect;
    expect(r.l).toBeCloseTo(line.rect.l);
    expect(r.b).toBeCloseTo(line.rect.b);
    expect(r.t).toBeCloseTo(line.rect.t);
    expect(r.r).toBeCloseTo(line.rect.r + 30); // the raw extent past the text stays
    // the neighbours whose boxes the drag only grazes are not pulled in
    const others = useEditStore.getState().pages[0]!.objects.filter((o) => o.type === "text" && o.objectId !== line.objectId);
    for (const o of others) expect(o.rect.t <= r.b + 0.01 || o.rect.b >= r.t - 0.01 || o.rect.r <= r.l || o.rect.l >= r.r).toBe(true);
  });

  it("a drag over empty paper keeps the raw rect; everything is clipped to the page box", async () => {
    const { info, ctxs, surfaces } = await setup();
    const crop = info.pages[0].crop;
    // bottom-right corner, well past the page edge (a drag that leaves the page)
    drag(surfaces[0], ctxs[0], [crop.r - 40, crop.b + 40], [crop.r + 80, crop.b - 80]);
    expect(marks()).toHaveLength(1);
    expect(marks()[0].rect).toEqual({ l: crop.r - 40, b: crop.b, r: crop.r, t: crop.b + 40 });
    // the draft shown while dragging is the snapped + clipped rect too
    fireEvent.pointerDown(surfaces[0], at(ctxs[0], crop.r - 20, crop.b + 20));
    expect(useEditStore.getState().markSel).toBe(marks()[0].id); // a press on a mark selects it
  });

  it("the draft mark shows the snapped rect while dragging", async () => {
    const { ctxs, surfaces } = await setup();
    const line = useEditStore.getState().pages[0]!.objects.find((o) => o.text?.startsWith("Lightweight"))!;
    const midY = (line.rect.b + line.rect.t) / 2;
    const midX = (line.rect.l + line.rect.r) / 2;
    fireEvent.pointerDown(surfaces[0], at(ctxs[0], midX, midY + 2));
    fireEvent.pointerMove(surfaces[0], at(ctxs[0], midX + 20, midY - 2));
    const draft = surfaces[0].querySelector(".redact-mark[data-draft]") as HTMLElement;
    const box = ctxs[0].rectToBox(line.rect);
    expect(parseFloat(draft.style.left)).toBeCloseTo(box.x);
    expect(parseFloat(draft.style.width)).toBeCloseTo(box.w);
    fireEvent.pointerUp(surfaces[0], at(ctxs[0], midX + 20, midY - 2));
  });

  it("closing the document with pending marks asks first: 취소 keeps both, 버리기 drops them and closes", async () => {
    const { ctxs, surfaces } = await setup();
    drag(surfaces[0], ctxs[0], [60, 745], [400, 720]);
    expect(marks()).toHaveLength(1);

    const declined = closeDocumentFlow();
    await waitFor(() => expect(confirmTop()).not.toBeNull());
    expect(confirmTop()!.props).toMatchObject({ titleKey: "redact.leaveTitle", danger: true });
    answer(false);
    expect(await declined).toBe(false);
    expect(useDocStore.getState().info).not.toBeNull();
    expect(marks()).toHaveLength(1);

    const accepted = closeDocumentFlow();
    await waitFor(() => expect(confirmTop()).not.toBeNull());
    answer(true);
    expect(await accepted).toBe(true);
    expect(marks()).toHaveLength(0);
    expect(useDocStore.getState().info).toBeNull();
  });

  it("opening another file with pending marks asks first; 취소 keeps the document", async () => {
    const { info, ctxs, surfaces } = await setup();
    drag(surfaces[1], ctxs[1], [60, 745], [300, 700]);
    const opening = openPath("/tmp/other.pdf");
    await waitFor(() => expect(confirmTop()).not.toBeNull());
    answer(false);
    expect(await opening).toBeNull();
    expect(useDocStore.getState().info?.docId).toBe(info.docId);
    expect(marks()).toHaveLength(1);
  });
});
