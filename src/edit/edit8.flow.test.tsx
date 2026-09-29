/**
 * 편집 mode, Stage 8, against the mock adapter: the 선택 marquee; ⌘C / ⌘V / ⌘D through
 * `duplicate_objects` (same page, cascading pastes, another page via `targetPage`, `unsupported`,
 * a clipboard whose objects are gone, the macOS clipboard events and the command bus); the
 * Inspector's X / Y / 너비 / 높이; a multi-object move that fails part-way; and the 문단 편집 frame
 * turning with the page / view rotation.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { mock } from "../ipc/mock";
import type { PageGeom, Rotation } from "../ipc/types";
import { makePageLayerContext, type PageLayerContext } from "../viewer";
import { useAppStore, type ToolId } from "../store/appStore";
import { useDocStore } from "../store/docStore";
import { useViewStore } from "../store/viewStore";
import { useToastStore } from "../app/toastStore";
import { useDialogStore } from "../dialogs/dialogState";
import { runAnnotCommand } from "../tools/commands";
import { EditPanel } from "../app/Inspector/EditPanel";
import { useEditStore } from "./editStore";
import { EditLayer } from "./EditLayer";
import { clearObjectClipboard, moveObjects, pasteObjects } from "./actions";
import { hitObject } from "./geometry";
import { editHost } from "./index";

let stop: (() => void) | null = null;

async function setup(tool: ToolId = "select", rotation: { page?: Rotation; view?: Rotation } = {}) {
  const info = await useDocStore.getState().open("/tmp/sample.pdf");
  if (!info) throw new Error("mock open failed");
  useAppStore.setState({ mode: "edit", tool });
  useViewStore.setState({ currentPage: 0 });
  stop = editHost.start();
  const page: PageGeom = { ...info.pages[0], rotation: rotation.page ?? info.pages[0].rotation };
  const view = rotation.view ?? 0;
  const quarter = ((page.rotation + view) % 180) !== 0;
  const ctx = makePageLayerContext({
    docId: info.docId, docGeneration: info.docGeneration, page, rotation: view, zoomPercent: 100,
    width: quarter ? page.heightPt : page.widthPt, height: quarter ? page.widthPt : page.heightPt,
  });
  const rendered = render(<EditLayer ctx={ctx} />);
  await waitFor(() => expect(useEditStore.getState().pages[0]).toBeDefined());
  const surface = rendered.container.querySelector(".edit-surface") as HTMLElement;
  return { info, ctx, surface, ...rendered };
}

function at(ctx: PageLayerContext, x: number, y: number, extra: Record<string, unknown> = {}) {
  const [cx, cy] = ctx.toDevice(x, y);
  return { clientX: cx, clientY: cy, pointerId: 1, button: 0, ...extra };
}

const objects = (page = 0) => useEditStore.getState().pages[page]?.objects ?? [];
const toastKeys = () => useToastStore.getState().toasts.map((t) => t.messageKey);
const toastOf = (key: string) => useToastStore.getState().toasts.find((t) => t.messageKey === key);

/** Text lines of page 0, top first; line 0 is the read-only title. */
function lines() {
  return objects().filter((o) => o.type === "text").sort((a, b) => b.rect.t - a.rect.t);
}

function select(ids: number[], page = 0) {
  act(() => useEditStore.getState().select(page, ids));
}

beforeEach(() => {
  useEditStore.getState().reset();
  clearObjectClipboard();
  useToastStore.setState({ toasts: [] });
  useDialogStore.getState().closeAll();
});

afterEach(() => {
  useEditStore.getState().closeSession();
  stop?.();
  stop = null;
  vi.restoreAllMocks();
});

describe("편집 · 선택 marquee", () => {
  it("a drag on empty space selects the objects wholly inside; ⇧ adds; a plain click deselects", async () => {
    const { ctx, surface, container } = await setup();
    const [, first, second, third] = lines();
    const maxR = Math.max(...objects().map((o) => o.rect.r));
    const start: [number, number] = [maxR + 10, first.rect.t + 1];
    const end: [number, number] = [Math.min(first.rect.l, second.rect.l) - 4, second.rect.b - 1];
    expect(hitObject(objects(), ...start)).toBeNull();

    fireEvent.pointerDown(surface, at(ctx, ...start));
    fireEvent.pointerMove(surface, at(ctx, ...end));
    expect(container.querySelector("[data-testid='edit-marquee']")).not.toBeNull();
    fireEvent.pointerUp(surface, at(ctx, ...end));
    expect(container.querySelector("[data-testid='edit-marquee']")).toBeNull();
    expect(new Set(useEditStore.getState().selection?.ids)).toEqual(new Set([first.objectId, second.objectId]));

    // ⇧ + marquee around the third line adds it
    const s3: [number, number] = [maxR + 10, third.rect.t + 0.5];
    const e3: [number, number] = [third.rect.l - 4, third.rect.b - 0.5];
    fireEvent.pointerDown(surface, at(ctx, ...s3, { shiftKey: true }));
    fireEvent.pointerMove(surface, at(ctx, ...e3, { shiftKey: true }));
    fireEvent.pointerUp(surface, at(ctx, ...e3, { shiftKey: true }));
    expect(new Set(useEditStore.getState().selection?.ids)).toEqual(new Set([first.objectId, second.objectId, third.objectId]));

    // a click (no drag) on empty space deselects
    fireEvent.pointerDown(surface, at(ctx, ...start));
    fireEvent.pointerUp(surface, at(ctx, ...start));
    expect(useEditStore.getState().selection).toBeNull();
  });
});

describe("편집 · ⌘C / ⌘V / ⌘D", () => {
  it("⌘D duplicates in place one offset step away and selects the copy", async () => {
    await setup();
    const dup = vi.spyOn(mock, "duplicateObjects");
    const line = lines()[1];
    const count = objects().length;
    select([line.objectId]);
    fireEvent.keyDown(window, { key: "d", metaKey: true });
    await waitFor(() => expect(objects()).toHaveLength(count + 1));
    expect(dup.mock.calls[0][0]).toMatchObject({ page: 0, objectIds: [line.objectId], offset: [10, -10] });
    expect(dup.mock.calls[0][0].targetPage).toBeUndefined();
    const copy = objects()[count];
    expect(copy.rect.l).toBeCloseTo(line.rect.l + 10);
    expect(copy.rect.t).toBeCloseTo(line.rect.t - 10);
    expect(useEditStore.getState().selection).toEqual({ page: 0, ids: [copy.objectId] });

    // with the Korean input source on, ⌘D arrives as the jamo: the physical key still counts
    fireEvent.keyDown(window, { key: "ㅇ", code: "KeyD", metaKey: true });
    await waitFor(() => expect(objects()).toHaveLength(count + 2));

    // the menu's 복제 (command bus) does the same
    expect(runAnnotCommand("edit.duplicate")).toBe(true);
    await waitFor(() => expect(dup).toHaveBeenCalledTimes(3));
  });

  it("⌘C then ⌘V pastes onto the page in view, cascading; another page goes through targetPage", async () => {
    await setup();
    const dup = vi.spyOn(mock, "duplicateObjects");
    const line = lines()[2];
    select([line.objectId]);
    const copyKey = new KeyboardEvent("keydown", { key: "c", metaKey: true, bubbles: true, cancelable: true });
    window.dispatchEvent(copyKey);
    expect(copyKey.defaultPrevented).toBe(true);
    expect(dup).not.toHaveBeenCalled();

    fireEvent.keyDown(window, { key: "v", metaKey: true });
    await waitFor(() => expect(dup).toHaveBeenCalledTimes(1));
    expect(dup.mock.calls[0][0]).toMatchObject({ page: 0, objectIds: [line.objectId], offset: [10, -10] });
    fireEvent.keyDown(window, { key: "v", metaKey: true });
    await waitFor(() => expect(dup).toHaveBeenCalledTimes(2));
    expect(dup.mock.calls[1][0]).toMatchObject({ page: 0, objectIds: [line.objectId], offset: [20, -20] });

    // the page in view is now 2 (index 1): the copy lands there
    act(() => useViewStore.setState({ currentPage: 1 }));
    fireEvent.keyDown(window, { key: "v", metaKey: true });
    await waitFor(() => expect(dup).toHaveBeenCalledTimes(3));
    expect(dup.mock.calls[2][0]).toMatchObject({ page: 0, objectIds: [line.objectId], offset: [10, -10], targetPage: 1 });
    await waitFor(() => expect(useEditStore.getState().selection?.page).toBe(1));
    const pasted = objects(1).find((o) => o.objectId === useEditStore.getState().selection!.ids[0]);
    expect(pasted?.text).toBe(line.text);
  });

  it("붙여넣기 at a point (the canvas menu) puts the copy's top-left corner on that point", async () => {
    await setup();
    const dup = vi.spyOn(mock, "duplicateObjects");
    const line = lines()[2];
    select([line.objectId]);
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "c", metaKey: true, bubbles: true, cancelable: true }));
    await act(async () => {
      await pasteObjects(1, [100, 300]);
    });
    expect(dup).toHaveBeenCalledTimes(1);
    const call = dup.mock.calls[0][0];
    expect(call).toMatchObject({ page: 0, objectIds: [line.objectId], targetPage: 1 });
    expect(call.offset![0]).toBeCloseTo(100 - line.rect.l, 6);
    expect(call.offset![1]).toBeCloseTo(300 - line.rect.t, 6);
    // a point paste does not advance the ⌘V cascade
    fireEvent.keyDown(window, { key: "v", metaKey: true });
    await waitFor(() => expect(dup).toHaveBeenCalledTimes(2));
    expect(dup.mock.calls[1][0]).toMatchObject({ offset: [10, -10] });
  });

  it("an object the engine cannot carry to another page is a toast, not an error", async () => {
    await setup();
    const line = lines()[1];
    select([line.objectId]);
    fireEvent.keyDown(window, { key: "c", ctrlKey: true });
    act(() => useViewStore.setState({ currentPage: 1 }));
    vi.spyOn(mock, "duplicateObjects").mockRejectedValueOnce({ code: "unsupported", message: "form object" });
    fireEvent.keyDown(window, { key: "v", ctrlKey: true });
    await waitFor(() => expect(toastKeys()).toContain("edit.duplicate.crossPage"));
    expect(toastOf("edit.duplicate.crossPage")?.tone).toBe("danger");
  });

  it("the macOS route: DOM copy / paste events; a clipboard whose objects are gone refuses", async () => {
    await setup();
    const dup = vi.spyOn(mock, "duplicateObjects");
    const line = lines()[3];
    select([line.objectId]);
    fireEvent.copy(document.body);
    fireEvent.paste(document.body);
    await waitFor(() => expect(dup).toHaveBeenCalledTimes(1));
    expect(dup.mock.calls[0][0].objectIds).toEqual([line.objectId]);

    // copy, then delete the original: the paste cannot find it any more
    select([line.objectId]);
    fireEvent.copy(document.body);
    fireEvent.keyDown(window, { key: "Backspace" });
    await waitFor(() => expect(objects().some((o) => o.objectId === line.objectId && o.text === line.text)).toBe(false));
    fireEvent.paste(document.body);
    await waitFor(() => expect(toastKeys()).toContain("edit.clipboard.gone"));
    expect(dup).toHaveBeenCalledTimes(1);
  });

  it("a read-only selection is not copied: its reason is shown", async () => {
    await setup();
    const dup = vi.spyOn(mock, "duplicateObjects");
    select([lines()[0].objectId]);
    fireEvent.keyDown(window, { key: "d", metaKey: true });
    await act(async () => undefined);
    expect(dup).not.toHaveBeenCalled();
    expect(toastKeys()).toContain("edit.readOnly.xobject");
  });
});

describe("편집 · Inspector X / Y / 너비 / 높이 and multi-object moves", () => {
  it("X / Y move, 너비 / 높이 scale one object; with several selected X / Y move them all", async () => {
    await setup();
    render(<EditPanel />);
    const transform = vi.spyOn(mock, "transformObject");
    const [, a, b] = lines();
    select([a.objectId]);

    const x = screen.getByLabelText("X") as HTMLInputElement;
    expect(Number(x.value)).toBeCloseTo(a.rect.l, 0);
    fireEvent.change(x, { target: { value: String(a.rect.l + 25) } });
    fireEvent.keyDown(x, { key: "Enter" });
    await waitFor(() => expect(transform).toHaveBeenCalledTimes(1));
    expect(transform.mock.calls[0][0]).toMatchObject({ objectId: a.objectId });
    expect(transform.mock.calls[0][0].translate![0]).toBeCloseTo(25);
    expect(transform.mock.calls[0][0].translate![1]).toBeCloseTo(0);

    const w0 = a.rect.r - a.rect.l;
    const width = screen.getByLabelText("너비") as HTMLInputElement;
    fireEvent.change(width, { target: { value: String(Math.round(w0 * 2)) } });
    fireEvent.blur(width);
    await waitFor(() => expect(transform).toHaveBeenCalledTimes(2));
    expect(transform.mock.calls[1][0].scale![0]).toBeCloseTo(Math.round(w0 * 2) / w0, 2);
    expect(transform.mock.calls[1][0].scale![1]).toBeCloseTo(1);

    // Esc restores a half-typed value and sends nothing
    const y = screen.getByLabelText("Y") as HTMLInputElement;
    fireEvent.change(y, { target: { value: "1" } });
    fireEvent.keyDown(y, { key: "Escape" });
    fireEvent.blur(y);
    await act(async () => undefined);
    expect(transform).toHaveBeenCalledTimes(2);

    // two objects: 너비 / 높이 are off, Y moves both by the same delta
    select([a.objectId, b.objectId]);
    expect(screen.getByLabelText("너비")).toBeDisabled();
    const yy = screen.getByLabelText("Y") as HTMLInputElement;
    const bottom = Number(yy.value);
    fireEvent.change(yy, { target: { value: String(bottom - 40) } });
    fireEvent.keyDown(yy, { key: "Enter" });
    await waitFor(() => expect(transform).toHaveBeenCalledTimes(4));
    expect(transform.mock.calls.slice(2).map((c) => c[0].objectId).sort()).toEqual([a.objectId, b.objectId].sort());
    for (const c of transform.mock.calls.slice(2)) expect(c[0].translate![1]).toBeCloseTo(-40, 0);
  });

  it("a move of several objects that fails part-way says how many moved", async () => {
    await setup();
    const [, a, b, c] = lines();
    const real = mock.transformObject.bind(mock);
    let n = 0;
    const transform = vi.spyOn(mock, "transformObject").mockImplementation(async (args) => {
      if (++n === 2) throw { code: "notFound", message: "object moved on" };
      return real(args);
    });
    let ok = true;
    await act(async () => {
      ok = await moveObjects(0, [a.objectId, b.objectId, c.objectId], 5, 0);
    });
    expect(ok).toBe(false);
    // not stale: the third is still sent, the gesture reports 2 of 3
    expect(transform).toHaveBeenCalledTimes(3);
    expect(toastOf("edit.move.partial")?.params).toEqual({ done: 2, total: 3 });
  });

  it("a stale document stops the move at once", async () => {
    await setup();
    const [, a, b] = lines();
    const transform = vi.spyOn(mock, "transformObject").mockRejectedValue({ code: "stale", message: "generation moved on" });
    await act(async () => {
      await moveObjects(0, [a.objectId, b.objectId], 5, 0);
    });
    expect(transform).toHaveBeenCalledTimes(1);
    expect(toastKeys()).not.toContain("edit.move.partial");
    expect(toastKeys().length).toBe(1);
  });
});

describe("편집 · 문단 편집 under rotation", () => {
  async function openEditor(rotation: { page?: Rotation; view?: Rotation }) {
    const env = await setup("editText", rotation);
    const line = lines()[3];
    const p: [number, number] = [line.rect.l + 10, (line.rect.b + line.rect.t) / 2];
    fireEvent.pointerDown(env.surface, at(env.ctx, ...p));
    await screen.findByLabelText("문단 편집");
    const frame = env.container.querySelector(".edit-frame") as HTMLElement;
    return { ...env, frame };
  }

  it("the frame turns with a view rotation and is anchored at the text's top-left", async () => {
    const { ctx, frame } = await openEditor({ view: 90 });
    expect(frame.dataset.rotation).toBe("90");
    expect(frame.style.transform).toBe("rotate(90deg)");
    const s = useEditStore.getState().session!;
    const rect = s.kind === "paragraph" ? s.probe.rect : null;
    const [x, y] = ctx.toDevice(rect!.l, rect!.t);
    expect(parseFloat(frame.style.left)).toBeCloseTo(x);
    expect(parseFloat(frame.style.top)).toBeCloseTo(y);
  });

  it("a page /Rotate turns it too; page 90 + view 270 is upright", async () => {
    const turned = await openEditor({ page: 90 });
    expect(turned.frame.style.transform).toBe("rotate(90deg)");
    turned.unmount();
    useEditStore.getState().closeSession();
    stop?.();
    stop = null;
    useEditStore.getState().reset();

    const upright = await openEditor({ page: 90, view: 270 });
    expect(upright.frame.dataset.rotation).toBe("0");
    expect(upright.frame.style.transform).toBe("");
  });

  it("the width handle follows the page's +x on screen (down, at 90°)", async () => {
    const { frame } = await openEditor({ view: 90 });
    const handle = frame.querySelector(".edit-width-handle") as HTMLElement;
    const before = useEditStore.getState().session!.width;
    fireEvent.pointerDown(handle, { clientX: 100, clientY: 100, pointerId: 1, button: 0 });
    fireEvent.pointerMove(handle, { clientX: 100, clientY: 130, pointerId: 1 });
    fireEvent.pointerUp(handle, { clientX: 100, clientY: 130, pointerId: 1 });
    expect(useEditStore.getState().session!.width).toBeCloseTo(before + 30);
  });
});
