/**
 * v0.3 F1 / F2 against the mock: drawing with a field tool calls `create_form_field` with the
 * rectangle in PDF points and a default name; the new field is focused and 필드 속성 renames it;
 * 양식 평면화 asks first; the no-fields empty state; the field context menu's items.
 */
import { beforeEach, describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import * as api from "../ipc/api";
import { mock } from "../ipc/mock";
import { makePageLayerContext } from "../viewer";
import { useAppStore } from "../store/appStore";
import { useDocStore } from "../store/docStore";
import { useContextMenuStore } from "../app/contextMenuStore";
import { useDialogStore } from "../dialogs/dialogState";
import { useToastStore } from "../app/toastStore";
import { InspectorBody } from "../app/Inspector/InspectorBody";
import { FormLayer } from "./FormLayer";
import { fieldKey, loadFields, useFormStore } from "./formStore";
import { runFormAction } from "./formActions";

async function openForm(page = 0) {
  const info = await useDocStore.getState().open("/tmp/sample.pdf");
  if (!info) throw new Error("mock open failed");
  useAppStore.getState().setMode("form");
  await loadFields(info.docId, info.docGeneration);
  const ctx = makePageLayerContext({
    docId: info.docId,
    docGeneration: info.docGeneration,
    page: info.pages[page],
    rotation: 0,
    zoomPercent: 100,
    width: info.pages[page].widthPt,
    height: info.pages[page].heightPt,
  });
  return { info, ctx, page };
}

beforeEach(() => {
  useFormStore.getState().reset();
  useAppStore.getState().setMode("read");
  useContextMenuStore.setState({ menu: null });
  useDialogStore.setState({ stack: [] });
});

describe("필드 만들기", () => {
  it("drawing a text field calls create_form_field with the rect and a default name", async () => {
    const { info, ctx } = await openForm(0);
    act(() => useAppStore.getState().setTool("fieldText"));
    const spy = vi.spyOn(api, "createFormField");
    const { container } = render(<FormLayer ctx={ctx} />);
    const surface = container.querySelector(".form-author") as HTMLElement;
    expect(surface).not.toBeNull();
    // page box = page points at 100 %, y down: (100, 100) → (300, 124) is 200 × 24 pt
    fireEvent.pointerDown(surface, { button: 0, clientX: 100, clientY: 100 });
    fireEvent.pointerMove(window, { clientX: 300, clientY: 124 });
    fireEvent.pointerUp(window, { clientX: 300, clientY: 124 });
    await waitFor(() => expect(spy).toHaveBeenCalledTimes(1));
    const { docId, spec } = spy.mock.calls[0][0];
    expect(docId).toBe(info.docId);
    expect(spec.type).toBe("text");
    expect(spec.page).toBe(0);
    expect(spec.name).toBe("텍스트1");
    const h = info.pages[0].heightPt;
    expect(spec.rect.l).toBeCloseTo(100, 3);
    expect(spec.rect.r).toBeCloseTo(300, 3);
    expect(spec.rect.t).toBeCloseTo(h - 100, 3);
    expect(spec.rect.b).toBeCloseTo(h - 124, 3);
    // the new field is listed and selected for 필드 속성
    await waitFor(() => expect(useFormStore.getState().fields.some((f) => f.name === "텍스트1")).toBe(true));
    const made = useFormStore.getState().fields.find((f) => f.name === "텍스트1")!;
    expect(useFormStore.getState().selected).toBe(fieldKey(made.page, made.index));
    expect(useDocStore.getState().info?.undoLabel).toBe("undo.formFieldCreate");
  });

  it("필드 속성 renames the selected field in one update_form_field", async () => {
    await openForm(0);
    act(() => useAppStore.getState().setTool("fieldCheckbox"));
    const created = await (await import("./formActions")).createFieldAt(0, { l: 50, b: 50, r: 64, t: 64 }, "checkbox");
    expect(created?.name).toBe("확인란1");
    const spy = vi.spyOn(mock, "updateFormField");
    render(<InspectorBody />);
    const name = await screen.findByRole("textbox", { name: "필드 이름" });
    fireEvent.change(name, { target: { value: "동의함" } });
    fireEvent.keyDown(name, { key: "Enter" });
    await waitFor(() => expect(spy).toHaveBeenCalledTimes(1));
    expect(spy.mock.calls[0][0].patch).toEqual({ name: "동의함" });
    await waitFor(() => expect(useFormStore.getState().fields.some((f) => f.name === "동의함")).toBe(true));
  });
});

describe("라디오 단추 그룹 (verification round 1)", () => {
  it("radio buttons drawn one after another share one group; Alt starts a new group", async () => {
    await openForm(0);
    useToastStore.setState({ toasts: [] });
    const spy = vi.spyOn(api, "createFormField");
    const { createFieldAt } = await import("./formActions");
    await createFieldAt(0, { l: 50, b: 50, r: 64, t: 64 }, "radio");
    await createFieldAt(0, { l: 80, b: 50, r: 94, t: 64 }, "radio");
    await createFieldAt(0, { l: 110, b: 50, r: 124, t: 64 }, "radio");
    await createFieldAt(0, { l: 50, b: 90, r: 64, t: 104 }, "radio", { newGroup: true });
    await createFieldAt(0, { l: 80, b: 90, r: 94, t: 104 }, "radio");
    const specs = spy.mock.calls.map((c) => c[0].spec);
    expect(specs.map((s) => s.name)).toEqual(["라디오1", "라디오1", "라디오1", "라디오2", "라디오2"]);
    expect(specs.map((s) => s.options)).toEqual([["선택 1"], ["선택 2"], ["선택 3"], ["선택 1"], ["선택 2"]]);
    expect(useFormStore.getState().fields.filter((f) => f.name === "라디오1")).toHaveLength(3);
    expect(useToastStore.getState().toasts.at(-1)?.messageKey).toBe("form.author.radioJoined");
    spy.mockRestore();
  });

  it("a radio drawn while a non-radio field is selected starts a new group", async () => {
    await openForm(0);
    const { createFieldAt } = await import("./formActions");
    const first = await createFieldAt(0, { l: 50, b: 50, r: 64, t: 64 }, "radio");
    await createFieldAt(0, { l: 50, b: 150, r: 200, t: 170 }, "text");
    const second = await createFieldAt(0, { l: 80, b: 50, r: 94, t: 64 }, "radio");
    expect(second?.name).not.toBe(first?.name);
  });

  it("필드 속성 renaming a radio group after another joins it", async () => {
    await openForm(0);
    const { createFieldAt, updateField } = await import("./formActions");
    const a = await createFieldAt(0, { l: 50, b: 50, r: 64, t: 64 }, "radio");
    const b = await createFieldAt(0, { l: 80, b: 50, r: 94, t: 64 }, "radio", { newGroup: true });
    expect(await updateField(b!, { name: a!.name })).toBe(true);
    expect(useFormStore.getState().fields.filter((f) => f.name === a!.name)).toHaveLength(2);
  });
});

describe("양식 polish", () => {
  it("shows 입력 가능한 양식이 없습니다 when the document has no field", async () => {
    await openForm(0);
    act(() => useFormStore.getState().setFields(useDocStore.getState().info!.docId, 99, []));
    render(<InspectorBody />);
    expect(screen.getByText("이 문서에는 입력 가능한 양식이 없습니다")).toBeInTheDocument();
  });

  it("right-clicking a field offers 값 지우기 · 모든 필드 지우기 · 필드 강조 표시", async () => {
    const { ctx } = await openForm(2);
    const { container } = render(<FormLayer ctx={ctx} />);
    const input = container.querySelector("input.form-text") as HTMLElement;
    fireEvent.contextMenu(input, { clientX: 10, clientY: 10 });
    await waitFor(() => expect(useContextMenuStore.getState().menu).not.toBeNull());
    const ids = useContextMenuStore.getState().menu!.items.map((i) => i.id);
    expect(ids).toEqual(expect.arrayContaining(["clearField", "clearAll", "highlight"]));
    // 필드 강조 표시 toggles the overlay tint
    const before = useFormStore.getState().highlight;
    const item = useContextMenuStore.getState().menu!.items.find((i) => i.id === "highlight");
    if (item && "onSelect" in item) item.onSelect?.();
    expect(useFormStore.getState().highlight).toBe(!before);
  });

  it("양식 평면화 confirms first, then flattens in one step", async () => {
    await openForm(2);
    const spy = vi.spyOn(mock, "flattenForm");
    const running = runFormAction("flatten");
    await waitFor(() => expect(useDialogStore.getState().stack.at(-1)?.name).toBe("confirm"));
    const entry = useDialogStore.getState().stack.at(-1)!;
    expect(entry.props.bodyKey).toBe("form.flattenWarning");
    act(() => (entry.props.resolve as (v: boolean) => void)(true));
    await running;
    expect(spy).toHaveBeenCalledTimes(1);
    expect(useFormStore.getState().fields).toEqual([]);
    expect(useDocStore.getState().info?.hasForm).toBe(false);
  });
});
