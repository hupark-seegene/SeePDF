/**
 * The 양식 overlay (F-20, IPC_CONTRACT §7.2): a value typed into the HTML input reaches
 * `set_form_field_value`, and the engine's answer — not the optimistic string — is what the
 * overlay ends up showing.
 *
 * The Korean case is the reason the inputs are uncontrolled: a `compositionstart` … `input` …
 * `compositionend` sequence must survive without React re-rendering the field mid-composition.
 */
import { beforeEach, describe, expect, it, vi } from "vitest";
import { act, fireEvent, render } from "@testing-library/react";
import * as api from "../ipc/api";
import { makePageLayerContext } from "../viewer";
import { useAppStore } from "../store/appStore";
import { useDocStore } from "../store/docStore";
import { FormLayer } from "./FormLayer";
import { commitField, fieldsOnPage, loadFields, useFormStore } from "./formStore";

async function openForm() {
  const info = await useDocStore.getState().open("/tmp/sample.pdf");
  if (!info) throw new Error("mock open failed");
  useAppStore.getState().setMode("form");
  await loadFields(info.docId, info.docGeneration);
  const page = 2;
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

describe("forms.store", () => {
  beforeEach(() => {
    useFormStore.getState().reset();
    useAppStore.getState().setMode("read");
  });

  it("lists the fields of the document and groups them by page", async () => {
    const { page } = await openForm();
    expect(useFormStore.getState().fields.length).toBeGreaterThan(0);
    expect(fieldsOnPage(page).length).toBeGreaterThan(0);
  });

  it("commits a text value and keeps what the engine answered", async () => {
    const { info, page } = await openForm();
    const spy = vi.spyOn(api, "setFormFieldValue");
    const field = await commitField(page, 0, { text: "박현우" });
    expect(spy).toHaveBeenCalledWith({ docId: info.docId, page, index: 0, value: { text: "박현우" } });
    expect(field?.value).toBe("박현우");
    expect(useFormStore.getState().fields.find((f) => f.page === page && f.index === 0)?.value).toBe("박현우");
    // a fill is a mutation: the generation moved, which invalidates the tiles (F-20)
    expect(useFormStore.getState().generation).toBeGreaterThan(info.docGeneration);
  });

  it("commits a checkbox and drops a stale reply", async () => {
    const { page } = await openForm();
    const field = await commitField(page, 2, { checked: true });
    expect(field?.checked).toBe(true);
    const generation = useFormStore.getState().generation;
    useFormStore.getState().setFields("d1", generation - 1, []);
    expect(useFormStore.getState().fields.length).toBeGreaterThan(0);
  });
});

describe("forms.overlay", () => {
  beforeEach(() => {
    useFormStore.getState().reset();
    useAppStore.getState().setMode("read");
  });

  it("renders one control per widget, placed with the page matrix", async () => {
    const { ctx, page } = await openForm();
    const { container } = render(<FormLayer ctx={ctx} />);
    const inputs = container.querySelectorAll(".form-field");
    expect(inputs).toHaveLength(fieldsOnPage(page).length);
    const first = container.querySelector<HTMLInputElement>("input.form-text");
    expect(first).not.toBeNull();
    // page 2's first field is at l=180, b=600, r=420, t=620 in user space
    expect(first?.style.left).toBe("180px");
    expect(first?.style.width).toBe("240px");
    expect(first?.style.height).toBe("20px");
  });

  it("commits on blur, not on every keystroke, and survives a Hangul composition", async () => {
    const { ctx, page } = await openForm();
    const spy = vi.spyOn(api, "setFormFieldValue");
    const { container } = render(<FormLayer ctx={ctx} />);
    const input = container.querySelector<HTMLInputElement>("input.form-text");
    if (!input) throw new Error("no text field");

    fireEvent.compositionStart(input);
    fireEvent.change(input, { target: { value: "ㅎ" } });
    fireEvent.change(input, { target: { value: "한" } });
    fireEvent.change(input, { target: { value: "한글" } });
    fireEvent.compositionEnd(input);
    // nothing has reached the engine yet: a command per jamo would re-render the widget mid-word
    expect(spy).not.toHaveBeenCalled();
    expect(input.value).toBe("한글");

    fireEvent.blur(input);
    expect(spy).toHaveBeenCalledTimes(1);
    expect(spy.mock.calls[0][0]).toMatchObject({ page, index: 0, value: { text: "한글" } });
  });

  it("shows the engine's value again after ⌘Z / 모든 필드 지우기 re-lists the field", async () => {
    const { ctx } = await openForm();
    const { container } = render(<FormLayer ctx={ctx} />);
    const input = container.querySelector<HTMLInputElement>("input.form-text");
    if (!input) throw new Error("no text field");
    const before = input.value;
    fireEvent.change(input, { target: { value: "박현우" } });
    await act(async () => {
      fireEvent.blur(input);
      await Promise.resolve();
    });
    await vi.waitFor(() => expect(useFormStore.getState().fields.some((f) => f.value === "박현우")).toBe(true));

    // 실행 취소: the engine goes back, and the re-list at the new generation must reach the DOM
    await act(async () => {
      await useDocStore.getState().undo();
      const info = useDocStore.getState().info!;
      await loadFields(info.docId, info.docGeneration);
    });
    expect(useFormStore.getState().fields.some((f) => f.value === "박현우")).toBe(false);
    const shown = container.querySelector<HTMLInputElement>("input.form-text")!;
    expect(shown.value).toBe(before);
  });

  it("an unrelated re-list keeps what is being typed", async () => {
    const { ctx, info } = await openForm();
    const { container } = render(<FormLayer ctx={ctx} />);
    const input = container.querySelector<HTMLInputElement>("input.form-text")!;
    input.focus();
    fireEvent.change(input, { target: { value: "입력 중" } });
    await act(async () => {
      await loadFields(info.docId, info.docGeneration + 1);
    });
    expect(container.querySelector<HTMLInputElement>("input.form-text")!.value).toBe("입력 중");
  });

  it("commits a checkbox immediately", async () => {
    const { ctx, page } = await openForm();
    const spy = vi.spyOn(api, "setFormFieldValue");
    const { container } = render(<FormLayer ctx={ctx} />);
    const box = container.querySelector<HTMLInputElement>("input.form-check");
    if (!box) throw new Error("no checkbox");
    fireEvent.click(box);
    expect(spy).toHaveBeenCalledTimes(1);
    expect(spy.mock.calls[0][0]).toMatchObject({ page, value: { checked: true } });
  });

  it("renders nothing outside 양식 mode", async () => {
    const { ctx } = await openForm();
    useAppStore.getState().setMode("read");
    const { container } = render(<FormLayer ctx={ctx} />);
    expect(container.querySelector(".form-layer")).toBeNull();
  });
});
