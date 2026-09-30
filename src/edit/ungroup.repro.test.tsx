/**
 * REPRODUCTIONS (v0.3.0 user report "그룹 해제가 안돼"): these tests pin the CURRENT, faulty behaviour
 * of the 그룹 해제 → 문단 편집 path so each failure is demonstrable against the mock adapter. They are
 * expected to be rewritten (inverted) by the fix. The engine side lives in
 * `src-tauri/tests/ungroup_corpus.rs`.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, waitFor } from "@testing-library/react";
import * as api from "../ipc/api";
import { mock } from "../ipc/mock";
import type { ParagraphProbe, PageObject } from "../ipc/types";
import { makePageLayerContext, type PageLayerContext } from "../viewer";
import { useAppStore, type ToolId } from "../store/appStore";
import { useDocStore } from "../store/docStore";
import { useToastStore } from "../app/toastStore";
import { useDialogStore } from "../dialogs/dialogState";
import { useEditStore } from "./editStore";
import { EditLayer } from "./EditLayer";
import { editHost } from "./index";

let stop: (() => void) | null = null;

async function setup(tool: ToolId) {
  const info = await useDocStore.getState().open("/tmp/sample.pdf");
  if (!info) throw new Error("mock open failed");
  useAppStore.setState({ mode: "edit", tool });
  stop = editHost.start();
  const page = info.pages[0];
  const ctx = makePageLayerContext({
    docId: info.docId, docGeneration: info.docGeneration, page, rotation: 0, zoomPercent: 100,
    width: page.widthPt, height: page.heightPt,
  });
  const view = render(<EditLayer ctx={ctx} />);
  await waitFor(() => expect(useEditStore.getState().pages[0]).toBeDefined());
  const surface = view.container.querySelector(".edit-surface") as HTMLElement;
  return { info, ctx, surface, ...view };
}

function at(ctx: PageLayerContext, x: number, y: number, detail = 1) {
  const [cx, cy] = ctx.toDevice(x, y);
  return { clientX: cx, clientY: cy, pointerId: 1, button: 0, detail };
}

function confirmTop() {
  const top = useDialogStore.getState().stack.at(-1);
  return top?.name === "confirm" ? (top.props.resolve as (v: boolean) => void) : null;
}

function deferred() {
  let open!: () => void;
  const gate = new Promise<void>((r) => (open = r));
  return { gate, open };
}

/** The grouped run of the mock (line 0, the title). */
const GROUPED: [number, number] = [100, 765];

beforeEach(() => {
  useEditStore.getState().reset();
  useToastStore.setState({ toasts: [] });
  useDialogStore.getState().closeAll();
});

afterEach(() => {
  useEditStore.getState().closeSession();
  confirmTop()?.(false);
  stop?.();
  stop = null;
  vi.restoreAllMocks();
});

describe("repro: 그룹 해제 → 편집", () => {
  // The screenshot: 2× "문제가 발생했습니다 · expectGeneration 3 but the document is at 4; re-list the page
  // objects" with the document dirty. The engine runs Lane::Interactive (probe_paragraph) before
  // Lane::Edit (ungroup_object) — proven in ungroup_corpus.rs `lane_overtake_makes_ungroup_stale` —
  // so a click while the first 그룹 해제 is still pending is probed at the OLD generation, refused
  // insideXObject again, asks again, and its 그룹 해제 is pinned to the old generation. Nothing in the
  // UI blocks a second beginParagraphEdit while one is running, and confirmUngroup shows the raw
  // engine message instead of recovering.
  it("a click while 그룹 해제 is in flight asks again; that 그룹 해제 fails `stale` with the raw English message", async () => {
    const { ctx, surface, info } = await setup("editText");
    const g0 = info.docGeneration;
    const original = mock.ungroupObject.bind(mock);
    const slow = deferred();
    const ungroup = vi.spyOn(mock, "ungroupObject").mockImplementation(async (a) => {
      if (ungroup.mock.calls.length === 1) await slow.gate; // the first 그룹 해제 is still queued
      return original(a);
    });

    fireEvent.pointerDown(surface, at(ctx, ...GROUPED));
    await waitFor(() => expect(confirmTop()).not.toBeNull());
    act(() => confirmTop()!(true)); // 그룹 해제
    await waitFor(() => expect(ungroup).toHaveBeenCalledTimes(1));

    // nothing visible happened yet → the user clicks the text again
    fireEvent.pointerDown(surface, at(ctx, ...GROUPED));
    await waitFor(() => expect(confirmTop()).not.toBeNull()); // asked AGAIN (probe at the old generation)

    slow.open(); // the first ungroup lands: gen g0 → g0 + 1, the editor opens on the paragraph
    await waitFor(() => expect(useEditStore.getState().session?.kind).toBe("paragraph"));
    expect((await api.getDocument({ docId: info.docId })).docGeneration).toBe(g0 + 1);

    act(() => confirmTop()!(true)); // the user answers the second prompt too
    await waitFor(() => expect(ungroup).toHaveBeenCalledTimes(2));
    expect(ungroup.mock.calls[1][0].expectGeneration).toBe(g0); // pinned to the stale probe
    await waitFor(() => expect(useToastStore.getState().toasts.length).toBeGreaterThan(0));
    const t = useToastStore.getState().toasts.at(-1)!;
    expect(t.messageKey).toBe("error.generic"); // 문제가 발생했습니다
    expect(t.tone).toBe("danger");
    expect(t.detail).toBe("generation moved on"); // engine: "expectGeneration 3 but the document is at 4; re-list the page objects"
  });

  it("a double-click with 텍스트 수정 sends two probes; a slow second probe re-asks after 그룹 해제 and fails `stale`", async () => {
    const { ctx, surface, info } = await setup("editText");
    const g0 = info.docGeneration;
    const probeOriginal = mock.probeParagraph.bind(mock);
    const secondProbe = deferred();
    let stale: ParagraphProbe | null = null;
    const probe = vi.spyOn(mock, "probeParagraph").mockImplementation(async (a) => {
      const n = probe.mock.calls.length;
      if (n === 2) {
        // evaluated at the old generation (before the ungroup) but delivered late
        stale = await probeOriginal(a);
        await secondProbe.gate;
        return stale;
      }
      return probeOriginal(a);
    });
    const ungroup = vi.spyOn(mock, "ungroupObject");

    fireEvent.pointerDown(surface, at(ctx, ...GROUPED, 1));
    fireEvent.pointerDown(surface, at(ctx, ...GROUPED, 2));
    await waitFor(() => expect(confirmTop()).not.toBeNull());
    act(() => confirmTop()!(true));
    await waitFor(() => expect(useEditStore.getState().session?.kind).toBe("paragraph"));
    expect(ungroup).toHaveBeenCalledTimes(1);

    secondProbe.open();
    await waitFor(() => expect(confirmTop()).not.toBeNull()); // a second 그룹 해제 후 편집할까요? over the open editor
    act(() => confirmTop()!(true));
    await waitFor(() => expect(ungroup).toHaveBeenCalledTimes(2));
    expect(ungroup.mock.calls[1][0].expectGeneration).toBe(g0);
    await waitFor(() => expect(useToastStore.getState().toasts.some((t) => t.messageKey === "error.generic")).toBe(true));
  });

  // The engine lists a group as ONE top-level object of type "form" (objects/mod.rs `describe`);
  // the mock lists the grouped run as type "text", which is why the existing double-click test passes.
  it("선택 double-click on a group as the engine lists it (type form) never reaches confirmUngroup", async () => {
    const { ctx, surface } = await setup("select");
    const listed = await api.listPageObjects({ docId: useDocStore.getState().info!.docId, page: 0 });
    const asEngine = {
      ...listed,
      objects: listed.objects.map((o): PageObject => (o.reason === "insideXObject" ? { ...o, type: "form" } : o)),
    };
    act(() => useEditStore.getState().setPage(0, asEngine));
    const probe = vi.spyOn(mock, "probeParagraph");

    fireEvent.pointerDown(surface, at(ctx, ...GROUPED, 1));
    fireEvent.pointerUp(surface, at(ctx, ...GROUPED, 1));
    fireEvent.pointerDown(surface, at(ctx, ...GROUPED, 2));
    fireEvent.pointerUp(surface, at(ctx, ...GROUPED, 2));
    await act(async () => undefined);

    expect(useEditStore.getState().selection?.ids.length).toBe(1); // the badge shows 읽기 전용 · 그룹(XObject)…
    expect(probe).not.toHaveBeenCalled();
    expect(confirmTop()).toBeNull();
    expect(useToastStore.getState().toasts).toHaveLength(0); // no hint at all
  });

  it("an Office-style transparency group: 그룹 해제 answers 문제가 발생했습니다 with the English engine message", async () => {
    const { ctx, surface } = await setup("editText");
    vi.spyOn(mock, "ungroupObject").mockRejectedValue(
      new api.SeePdfError({
        code: "unsupported",
        message: "this group is drawn with transparency, which its objects would lose",
        detail: "groupTransparency",
      }),
    );
    fireEvent.pointerDown(surface, at(ctx, ...GROUPED));
    await waitFor(() => expect(confirmTop()).not.toBeNull());
    act(() => confirmTop()!(true));
    await waitFor(() => expect(useToastStore.getState().toasts.length).toBe(1));
    const t = useToastStore.getState().toasts[0];
    expect(t.messageKey).toBe("error.generic");
    expect(t.detail).toBe("this group is drawn with transparency, which its objects would lose");
    expect(useEditStore.getState().session).toBeNull();
  });

  it("text in a nested group (the outer form has no text of its own): the probe answers null and the click does nothing", async () => {
    const { ctx, surface } = await setup("editText");
    vi.spyOn(mock, "probeParagraph").mockResolvedValue(null); // engine: classify() skips a form without DIRECT text children
    fireEvent.pointerDown(surface, at(ctx, ...GROUPED));
    await act(async () => undefined);
    expect(confirmTop()).toBeNull();
    expect(useToastStore.getState().toasts).toHaveLength(0);
    expect(useEditStore.getState().session).toBeNull();
  });

  it("after 그룹 해제 the re-probe can refuse for another reason (noUnicode): the document stays ungrouped and dirty, no editor", async () => {
    const { ctx, surface, info } = await setup("editText");
    const probeOriginal = mock.probeParagraph.bind(mock);
    vi.spyOn(mock, "probeParagraph").mockImplementation(async (a) => {
      const p = await probeOriginal(a);
      // TAMReview.pdf p.2: the ungrouped runs have no ToUnicode
      return p && p.strategy !== "refused" ? { ...p, strategy: "refused", reason: "noUnicode" } : p;
    });
    fireEvent.pointerDown(surface, at(ctx, ...GROUPED));
    await waitFor(() => expect(confirmTop()).not.toBeNull());
    act(() => confirmTop()!(true));
    await waitFor(() => expect(useToastStore.getState().toasts.length).toBe(1));
    expect(useToastStore.getState().toasts[0].messageKey).toBe("edit.readOnly.noUnicode");
    expect(useEditStore.getState().session).toBeNull();
    const now = await api.getDocument({ docId: info.docId });
    expect(now.docGeneration).toBe(info.docGeneration + 1);
    expect(now.dirty).toBe(true);
    expect(now.undoLabel).toBe("undo.ungroup");
  });
});
