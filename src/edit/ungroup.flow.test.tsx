/**
 * v0.3.1 그룹 해제 → 편집 (the v0.3.0 user report "그룹 해제가 안돼"; the engine side is
 * `src-tauri/tests/ungroup_corpus.rs`). Every way in reaches 그룹 해제 and ends in the paragraph
 * editor on the clicked text: a click with 텍스트 수정, a double-click with 선택 on the group (listed,
 * like the engine lists it, as one `form` object), and the Inspector's 그룹 해제 button. A second
 * click or the second press of a double-click never asks again or sends a 그룹 해제 pinned to an old
 * generation (the screenshot's "expectGeneration 3 but the document is at 4"), a `stale` answer is
 * recovered, and every failure has its own Korean toast.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import * as api from "../ipc/api";
import { mock } from "../ipc/mock";
import type { ParagraphProbe } from "../ipc/types";
import { makePageLayerContext, type PageLayerContext } from "../viewer";
import { useAppStore, type ToolId } from "../store/appStore";
import { useDocStore } from "../store/docStore";
import { useToastStore } from "../app/toastStore";
import { useDialogStore } from "../dialogs/dialogState";
import { useEditStore } from "./editStore";
import { EditLayer } from "./EditLayer";
import { EditPanel } from "../app/Inspector/EditPanel";
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

function confirms() {
  return useDialogStore.getState().stack.filter((d) => d.name === "confirm").length;
}

function deferred() {
  let open!: () => void;
  const gate = new Promise<void>((r) => (open = r));
  return { gate, open };
}

function toasts() {
  return useToastStore.getState().toasts;
}

function objects() {
  return useEditStore.getState().pages[0]?.objects ?? [];
}

/** The mock's group: its title line, listed as one read-only `form` object. */
const GROUPED: [number, number] = [100, 765];

async function answerUngroup() {
  await waitFor(() => expect(confirmTop()).not.toBeNull());
  expect(useDialogStore.getState().stack.at(-1)!.props).toMatchObject({ titleKey: "edit.ungroup.title" });
  act(() => confirmTop()!(true));
}

async function editorOpen() {
  await waitFor(() => expect(useEditStore.getState().session?.kind).toBe("paragraph"));
  return useEditStore.getState().session!;
}

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

describe("그룹 해제 → 편집: every way in", () => {
  it("텍스트 수정: click → 그룹 해제 후 편집할까요? → 그룹 해제 (with the click point) → the editor on that paragraph", async () => {
    const { ctx, surface, info } = await setup("editText");
    const ungroup = vi.spyOn(mock, "ungroupObject");
    fireEvent.pointerDown(surface, at(ctx, ...GROUPED));
    await answerUngroup();
    const session = await editorOpen();
    expect(ungroup).toHaveBeenCalledTimes(1);
    expect(ungroup.mock.calls[0][0]).toMatchObject({ page: 0, objectId: 0, expectGeneration: info.docGeneration, at: GROUPED });
    expect(session.kind === "paragraph" && session.probe.objectIds).toEqual([0]);
    expect(toasts()).toHaveLength(0);
    expect((await api.getDocument({ docId: info.docId })).undoLabel).toBe("undo.ungroup");
  });

  it("선택: a double-click on the group (type form, as the engine lists it) asks and opens the editor", async () => {
    const { ctx, surface } = await setup("select");
    expect(objects()[0]).toMatchObject({ type: "form", editable: "readOnly", reason: "insideXObject" });
    const probe = vi.spyOn(mock, "probeParagraph");
    fireEvent.pointerDown(surface, at(ctx, ...GROUPED, 1));
    fireEvent.pointerUp(surface, at(ctx, ...GROUPED, 1));
    fireEvent.pointerDown(surface, at(ctx, ...GROUPED, 2));
    fireEvent.pointerUp(surface, at(ctx, ...GROUPED, 2));
    await answerUngroup();
    await editorOpen();
    expect(probe).toHaveBeenCalledTimes(2); // the ask, and the paragraph after 그룹 해제
    expect(objects()[0]).toMatchObject({ type: "text", editable: "full" });
  });

  it("선택: the same double-click in WebView2 (Windows), whose pointerdown reports no click count, asks and opens the editor", async () => {
    const { ctx, surface } = await setup("select");
    const probe = vi.spyOn(mock, "probeParagraph");
    for (let i = 0; i < 2; i++) {
      fireEvent.pointerDown(surface, at(ctx, ...GROUPED, 0));
      fireEvent.pointerUp(surface, at(ctx, ...GROUPED, 0));
    }
    await answerUngroup();
    await editorOpen();
    expect(probe).toHaveBeenCalledTimes(2);
    expect(confirms()).toBe(0);
    expect(toasts()).toHaveLength(0);
  });

  it("선택: a double-click on a group where it holds no text says where 그룹 해제 is", async () => {
    const { ctx, surface } = await setup("select");
    vi.spyOn(mock, "probeParagraph").mockResolvedValue(null);
    fireEvent.pointerDown(surface, at(ctx, ...GROUPED, 2));
    await waitFor(() => expect(toasts().map((t) => t.messageKey)).toEqual(["edit.ungroup.noText"]));
    expect(confirmTop()).toBeNull();
  });

  it("Inspector: 그룹 해제 on the selected group ungroups it (no question) and selects what came out", async () => {
    const { ctx, surface } = await setup("select");
    render(<EditPanel />);
    const ungroup = vi.spyOn(mock, "ungroupObject");
    fireEvent.pointerDown(surface, at(ctx, ...GROUPED));
    fireEvent.pointerUp(surface, at(ctx, ...GROUPED));
    expect(screen.getAllByText(/더블클릭하거나 텍스트 수정으로 클릭하면 그룹을 해제하고 편집합니다/).length).toBeGreaterThan(0);
    fireEvent.click(screen.getByRole("button", { name: "그룹 해제" }));
    await waitFor(() => expect(ungroup).toHaveBeenCalledTimes(1));
    expect(ungroup.mock.calls[0][0].at).toBeUndefined();
    expect(confirmTop()).toBeNull();
    await waitFor(() => expect(objects()[0]?.type).toBe("text"));
    expect(useEditStore.getState().selection).toEqual({ page: 0, ids: [0] });
    expect(toasts().map((t) => t.messageKey)).toEqual(["edit.ungroup.done"]);
  });
});

describe("그룹 해제 → 편집: one at a time (the screenshot)", () => {
  it("a click while 그룹 해제 is in flight gets the running answer: no second question, no second 그룹 해제, no toast", async () => {
    const { ctx, surface, info } = await setup("editText");
    const original = mock.ungroupObject.bind(mock);
    const slow = deferred();
    const ungroup = vi.spyOn(mock, "ungroupObject").mockImplementation(async (a) => {
      await slow.gate; // the first 그룹 해제 is still queued
      return original(a);
    });
    const probe = vi.spyOn(mock, "probeParagraph");

    fireEvent.pointerDown(surface, at(ctx, ...GROUPED));
    await answerUngroup();
    await waitFor(() => expect(ungroup).toHaveBeenCalledTimes(1));

    // nothing visible happened yet → the user clicks the text again (and again)
    fireEvent.pointerDown(surface, at(ctx, ...GROUPED));
    fireEvent.pointerDown(surface, at(ctx, ...GROUPED));
    await act(async () => undefined);
    expect(confirmTop()).toBeNull();
    expect(probe).toHaveBeenCalledTimes(1);

    slow.open();
    await editorOpen();
    expect(ungroup).toHaveBeenCalledTimes(1);
    expect((await api.getDocument({ docId: info.docId })).docGeneration).toBe(info.docGeneration + 1);
    expect(toasts()).toHaveLength(0);
  });

  it("a double-click with 텍스트 수정 probes once and asks once", async () => {
    const { ctx, surface } = await setup("editText");
    const probe = vi.spyOn(mock, "probeParagraph");
    const ungroup = vi.spyOn(mock, "ungroupObject");
    fireEvent.pointerDown(surface, at(ctx, ...GROUPED, 1));
    fireEvent.pointerDown(surface, at(ctx, ...GROUPED, 2));
    await answerUngroup();
    await editorOpen();
    await act(async () => undefined);
    expect(confirms()).toBe(0);
    expect(ungroup).toHaveBeenCalledTimes(1);
    expect(probe).toHaveBeenCalledTimes(2); // the click's probe, and the one after 그룹 해제
    expect(toasts()).toHaveLength(0);
  });

  it("`stale` (the document moved on under the question): probed again and ungrouped without asking again", async () => {
    const { ctx, surface, info } = await setup("editText");
    const original = mock.ungroupObject.bind(mock);
    const ungroup = vi.spyOn(mock, "ungroupObject").mockImplementationOnce(async () => {
      throw new api.SeePdfError({
        code: "stale",
        message: `expectGeneration ${info.docGeneration} but the document is at ${info.docGeneration + 1}; re-list the page objects`,
      });
    }).mockImplementation(original);
    const probe = vi.spyOn(mock, "probeParagraph");
    fireEvent.pointerDown(surface, at(ctx, ...GROUPED));
    await answerUngroup();
    await editorOpen();
    expect(ungroup).toHaveBeenCalledTimes(2);
    expect(probe).toHaveBeenCalledTimes(3); // click, the re-probe after `stale`, the paragraph
    expect(confirms()).toBe(0);
    expect(toasts()).toHaveLength(0);
  });

  it("`stale` because another 그룹 해제 already went through: the editor simply opens", async () => {
    const { ctx, surface, info } = await setup("editText");
    const original = mock.ungroupObject.bind(mock);
    const ungroup = vi.spyOn(mock, "ungroupObject").mockImplementationOnce(async (a) => {
      await original({ ...a }); // someone else's 그룹 해제 lands first…
      throw new api.SeePdfError({ code: "stale", message: "generation moved on" }); // …so ours is stale
    });
    fireEvent.pointerDown(surface, at(ctx, ...GROUPED));
    await answerUngroup();
    await editorOpen();
    expect(ungroup).toHaveBeenCalledTimes(1);
    expect((await api.getDocument({ docId: info.docId })).docGeneration).toBe(info.docGeneration + 1);
    expect(toasts()).toHaveLength(0);
  });
});

describe("Inspector 그룹 해제: one at a time (verification round 1)", () => {
  async function selectGroup() {
    const view = await setup("select");
    render(<EditPanel />);
    fireEvent.pointerDown(view.surface, at(view.ctx, ...GROUPED));
    fireEvent.pointerUp(view.surface, at(view.ctx, ...GROUPED));
    await waitFor(() => expect(useEditStore.getState().selection).toEqual({ page: 0, ids: [0] }));
    return view;
  }

  it("a double-click on the button sends one 그룹 해제 — no second call pinned to the same generation, no stale toast", async () => {
    const { info } = await selectGroup();
    const ungroup = vi.spyOn(mock, "ungroupObject");
    const button = screen.getByRole("button", { name: "그룹 해제" });
    fireEvent.click(button, { detail: 1 });
    fireEvent.click(button, { detail: 2 });
    await waitFor(() => expect(toasts().map((t) => t.messageKey)).toEqual(["edit.ungroup.done"]));
    await act(async () => undefined);
    expect(ungroup.mock.calls.map((c) => c[0].expectGeneration)).toEqual([info.docGeneration]);
    expect((await api.getDocument({ docId: info.docId })).docGeneration).toBe(info.docGeneration + 1);
  });

  it("a second press while the first 그룹 해제 is still running gets the running answer", async () => {
    const { info } = await selectGroup();
    const original = mock.ungroupObject.bind(mock);
    const slow = deferred();
    const ungroup = vi.spyOn(mock, "ungroupObject").mockImplementation(async (a) => {
      await slow.gate;
      return original(a);
    });
    const { ungroupSelection } = await import("./ungroup");
    const first = ungroupSelection();
    const second = ungroupSelection();
    await waitFor(() => expect(ungroup).toHaveBeenCalledTimes(1));
    slow.open();
    expect(await first).toBe(true);
    expect(await second).toBe(true);
    expect(ungroup).toHaveBeenCalledTimes(1);
    expect(toasts().map((t) => t.messageKey)).toEqual(["edit.ungroup.done"]);
    expect((await api.getDocument({ docId: info.docId })).docGeneration).toBe(info.docGeneration + 1);
  });

  it("`stale` (the document moved on elsewhere): the page is listed again and the same group is ungrouped at the new generation — no toast to press again", async () => {
    const { info } = await selectGroup();
    const ungroup = vi.spyOn(mock, "ungroupObject").mockRejectedValueOnce(
      new api.SeePdfError({ code: "stale", message: "expectGeneration 1 but the document is at 2; re-list the page objects" }),
    );
    const list = vi.spyOn(mock, "listPageObjects");
    fireEvent.click(screen.getByRole("button", { name: "그룹 해제" }));
    await waitFor(() => expect(toasts().map((t) => t.messageKey)).toEqual(["edit.ungroup.done"]));
    expect(list).toHaveBeenCalled();
    expect(ungroup).toHaveBeenCalledTimes(2);
    expect(objects()[0]?.type).toBe("text");
    expect((await api.getDocument({ docId: info.docId })).undoLabel).toBe("undo.ungroup");
  });

  it("`stale` twice: the group stays selected and the user is asked to press again — and pressing again really works", async () => {
    await selectGroup();
    const stale = new api.SeePdfError({ code: "stale", message: "expectGeneration 1 but the document is at 2; re-list the page objects" });
    vi.spyOn(mock, "ungroupObject").mockRejectedValueOnce(stale).mockRejectedValueOnce(stale);
    fireEvent.click(screen.getByRole("button", { name: "그룹 해제" }));
    await waitFor(() => expect(toasts().map((t) => t.messageKey)).toEqual(["error.stale"]));
    expect(toasts()[0].detail).toBeUndefined();
    expect(useEditStore.getState().selection).toEqual({ page: 0, ids: [0] });
    await waitFor(() => expect(screen.getByRole("button", { name: "그룹 해제" })).not.toBeDisabled());
    fireEvent.click(screen.getByRole("button", { name: "그룹 해제" }));
    await waitFor(() => expect(objects()[0]?.type).toBe("text"));
  });

  it("an edit on another page (the page-0 listing older than the document): 그룹 해제 is pinned to the newest generation and goes through (round 2)", async () => {
    const { info } = await selectGroup();
    const added = await api.addTextObject({
      docId: info.docId, page: 1, rect: { l: 72, b: 700, r: 300, t: 720 }, text: "elsewhere", fontSizePt: 12,
      color: [0, 0, 0], align: "left",
    });
    const now = await api.getDocument({ docId: info.docId });
    expect(now.docGeneration).toBe(info.docGeneration + 1);
    act(() => useDocStore.getState().applyDocChanged({
      docId: info.docId, docGeneration: now.docGeneration, changedPages: [1], structure: false, dirty: true,
      reason: "edit", canUndo: true, canRedo: false,
    }));
    expect(added).toBeTruthy();
    expect(useEditStore.getState().pages[0]?.docGeneration).toBe(info.docGeneration); // the listing is behind
    const ungroup = vi.spyOn(mock, "ungroupObject");
    fireEvent.click(screen.getByRole("button", { name: "그룹 해제" }));
    await waitFor(() => expect(toasts().map((t) => t.messageKey)).toEqual(["edit.ungroup.done"]));
    expect(ungroup.mock.calls.map((c) => c[0].expectGeneration)).toEqual([now.docGeneration]);
    expect(objects()[0]?.type).toBe("text");
  });

  it("`stale` because the group was already ungrouped: listed again, nothing to say", async () => {
    await selectGroup();
    const original = mock.ungroupObject.bind(mock);
    vi.spyOn(mock, "ungroupObject").mockImplementationOnce(async (a) => {
      await original({ ...a }); // another 그룹 해제 landed first
      throw new api.SeePdfError({ code: "stale", message: "generation moved on" });
    });
    fireEvent.click(screen.getByRole("button", { name: "그룹 해제" }));
    await waitFor(() => expect(objects()[0]?.type).toBe("text"));
    await act(async () => undefined);
    expect(toasts()).toHaveLength(0);
  });
});

describe("그룹 해제 → 편집: what the user is told", () => {
  const failures: [string, api.SeePdfError, string][] = [
    ["the page would look different (transparency)", new api.SeePdfError({ code: "unsupported", message: "ungrouping this group would change how the page looks; the group was left as it was", detail: "lookChanged" }), "edit.ungroup.failed.lookChanged"],
    ["not a group", new api.SeePdfError({ code: "invalidArgument", message: "object 0 is not a group (Form XObject)", detail: "notAGroup" }), "edit.ungroup.failed.notAGroup"],
    ["the group is gone", new api.SeePdfError({ code: "notFound", message: "object 0 of 0" }), "edit.ungroup.failed.notFound"],
    ["the document's permissions", new api.SeePdfError({ code: "permissionDenied", message: "the document's permissions forbid 'modify'" }), "error.permissionDenied"],
  ];
  for (const [what, error, key] of failures) {
    it(`${what} → ${key}, in words, not the engine's sentence`, async () => {
      const { ctx, surface } = await setup("editText");
      vi.spyOn(mock, "ungroupObject").mockRejectedValue(error);
      fireEvent.pointerDown(surface, at(ctx, ...GROUPED));
      await answerUngroup();
      await waitFor(() => expect(toasts()).toHaveLength(1));
      expect(toasts()[0]).toMatchObject({ messageKey: key, tone: "danger" });
      expect(toasts()[0].detail).toBeUndefined();
      expect(useEditStore.getState().session).toBeNull();
    });
  }

  it("an unexplained failure says the document is unchanged and keeps the engine's words as detail", async () => {
    const { ctx, surface } = await setup("editText");
    vi.spyOn(mock, "ungroupObject").mockRejectedValue(new api.SeePdfError({ code: "pdfium", message: "FPDFFormObj_RemoveObject failed" }));
    fireEvent.pointerDown(surface, at(ctx, ...GROUPED));
    await answerUngroup();
    await waitFor(() => expect(toasts()).toHaveLength(1));
    expect(toasts()[0]).toMatchObject({ messageKey: "edit.ungroup.failed.generic", detail: "FPDFFormObj_RemoveObject failed" });
  });

  it("only the text came out (`partial`): the editor opens and the toast says the graphics stayed grouped", async () => {
    const { ctx, surface } = await setup("editText");
    const original = mock.ungroupObject.bind(mock);
    vi.spyOn(mock, "ungroupObject").mockImplementation(async (a) => ({ ...(await original(a)), partial: true }));
    fireEvent.pointerDown(surface, at(ctx, ...GROUPED));
    await answerUngroup();
    await editorOpen();
    expect(toasts().map((t) => t.messageKey)).toEqual(["edit.ungroup.partial"]);
  });

  it("grouped text that could never be edited (no /ToUnicode) is refused with its reason — nothing is ungrouped", async () => {
    const { ctx, surface, info } = await setup("editText");
    const original = mock.probeParagraph.bind(mock);
    vi.spyOn(mock, "probeParagraph").mockImplementation(async (a) => {
      const p = await original(a);
      // the engine classifies the run inside the group first (TAMReview.pdf)
      return p && p.reason === "insideXObject"
        ? ({ ...p, reason: "noUnicode", groupObjectId: undefined, groupDepth: undefined } as ParagraphProbe)
        : p;
    });
    const ungroup = vi.spyOn(mock, "ungroupObject");
    fireEvent.pointerDown(surface, at(ctx, ...GROUPED));
    await waitFor(() => expect(toasts()).toHaveLength(1));
    expect(toasts()[0].messageKey).toBe("edit.readOnly.noUnicode");
    expect(confirmTop()).toBeNull();
    expect(ungroup).not.toHaveBeenCalled();
    expect((await api.getDocument({ docId: info.docId })).dirty).toBe(false);
  });

  it("if the paragraph still cannot be edited after 그룹 해제, the 그룹 해제 is taken back and the reason shown", async () => {
    const { ctx, surface, info } = await setup("editText");
    const original = mock.probeParagraph.bind(mock);
    vi.spyOn(mock, "probeParagraph").mockImplementation(async (a) => {
      const p = await original(a);
      return p && p.strategy !== "refused" ? { ...p, strategy: "refused", reason: "unwritableContent" } : p;
    });
    fireEvent.pointerDown(surface, at(ctx, ...GROUPED));
    await answerUngroup();
    await waitFor(() => expect(toasts()).toHaveLength(1));
    expect(toasts()[0].messageKey).toBe("edit.reason.unwritableContent");
    expect(useEditStore.getState().session).toBeNull();
    const now = await api.getDocument({ docId: info.docId });
    expect(now.undoLabel).toBeNull();
    expect(now.redoLabel).toBe("undo.ungroup");
    expect(now.dirty).toBe(false);
  });

  it("`stale` from any other edit is told in words (error.stale), not 문제가 발생했습니다 + English", () => {
    expect(api.errorKey(new api.SeePdfError({ code: "stale", message: "expectGeneration 3 but the document is at 4; re-list the page objects" }))).toBe("error.stale");
  });
});

describe("영역 표시 over a group (verification round 2)", () => {
  it("hovering grouped text shows no outline and no 텍스트 수정 badge — a click there marks nothing; an ordinary line still outlines and marks", async () => {
    const { ctx, surface, container } = await setup("redact");
    const hoverOutlines = () => container.querySelectorAll(".edit-outline[data-state=hover]").length;
    fireEvent.pointerMove(surface, at(ctx, ...GROUPED));
    expect(hoverOutlines()).toBe(0);
    expect(container.querySelector(".edit-badge")).toBeNull();
    fireEvent.pointerDown(surface, at(ctx, ...GROUPED));
    fireEvent.pointerUp(surface, at(ctx, ...GROUPED));
    expect(useEditStore.getState().marks).toHaveLength(0);
    // the control: an ordinary text line
    fireEvent.pointerMove(surface, at(ctx, 100, 730));
    expect(hoverOutlines()).toBe(1);
    fireEvent.pointerDown(surface, at(ctx, 100, 730));
    fireEvent.pointerUp(surface, at(ctx, 100, 730));
    expect(useEditStore.getState().marks).toHaveLength(1);
  });

  it("텍스트 수정 keeps the group's hover outline (a click there ungroups it)", async () => {
    const { ctx, surface, container } = await setup("editText");
    fireEvent.pointerMove(surface, at(ctx, ...GROUPED));
    expect(container.querySelectorAll(".edit-outline[data-state=hover]").length).toBe(1);
  });
});
