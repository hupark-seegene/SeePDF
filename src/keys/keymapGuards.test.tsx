/**
 * The keymap must stay out of the way (bug hunt 2026-09):
 *   - while a modal dialog is open, no shortcut acts on the document behind it;
 *   - a text field keeps its own ⌘Z / ⌘C / ⌘A / caret keys;
 *   - ⌘Q and Alt+F4 are the operating system's (the native Quit / window close carry the
 *     unsaved-changes guard), never swallowed;
 *   - Windows Ctrl+D duplicates pages in 페이지 mode (the more specific context wins);
 *   - a dialog traps focus, and Esc closes it even if focus got out.
 */
import { beforeEach, describe, expect, it } from "vitest";
import { act, render, waitFor, within } from "@testing-library/react";
import App from "../App";
import { useAppStore } from "../store/appStore";
import { useDocStore } from "../store/docStore";
import { useViewStore } from "../store/viewStore";
import { usePagesStore } from "../store/pagesStore";
import { useAnnotStore } from "../store/annotStore";
import { openDialog, useDialogStore } from "../dialogs/dialogState";
import { lookup } from "./keymap";
import { runPageOps } from "../dialogs/flows";

const SAMPLE = "/Users/veri/Documents/SeePDF-샘플.pdf";

function keydown(el: Element | Window, init: KeyboardEventInit): KeyboardEvent {
  const ev = new KeyboardEvent("keydown", { bubbles: true, cancelable: true, ...init });
  act(() => {
    el.dispatchEvent(ev);
  });
  return ev;
}

async function openApp(mode: "read" | "annotate" | "pages" = "annotate") {
  useAppStore.setState({ os: "macos", mode, tool: "select" });
  const view = render(<App />);
  await useDocStore.getState().open(SAMPLE);
  await waitFor(() => expect(useDocStore.getState().info).not.toBeNull());
  useAppStore.setState({ mode, tool: "select" });
  return view;
}

beforeEach(() => {
  useDialogStore.getState().closeAll();
  useViewStore.setState({ currentPage: 0, layout: "continuous" });
});

describe("keymap under a modal dialog", () => {
  it("tool letters and Space on a focused dialog button do nothing to the document", async () => {
    const { container } = await openApp("annotate");
    act(() => openDialog("settings"));
    const dlg = await within(container).findByRole("dialog");
    const close = within(dlg).getByRole("button", { name: "닫기" });
    close.focus();

    keydown(close, { key: "h", code: "KeyH" });
    expect(useAppStore.getState().tool).toBe("select");

    const space = keydown(close, { key: " ", code: "Space" });
    expect(space.defaultPrevented).toBe(false);
    expect(useViewStore.getState().currentPage).toBe(0);
  });

  it("⌫ under the 페이지 추출 dialog does not delete the pages it is about to extract", async () => {
    const { container } = await openApp("pages");
    act(() => usePagesStore.getState().setSelected([0, 1]));
    act(() => openDialog("extract", { pages: [0, 1] }));
    const dlg = await within(container).findByRole("dialog");
    const cancel = within(dlg).getByRole("button", { name: "취소" });
    cancel.focus();
    keydown(cancel, { key: "Backspace", code: "Backspace" });
    await new Promise((r) => setTimeout(r, 30));
    expect(useDocStore.getState().info?.pageCount).toBe(3);
  });
});

describe("annotation keys under a modal dialog", () => {
  it("⌫ does not delete the selected annotation behind the dialog, and Esc closes the dialog", async () => {
    const { container } = await openApp("annotate");
    const { createAnnotation } = await import("../annot/actions");
    const annot = await createAnnotation(0, {
      kind: "square", rect: { l: 10, b: 10, r: 60, t: 60 }, color: [245, 83, 61], fillColor: null, width: 2, opacity: 1,
    });
    expect(annot).not.toBeNull();
    // the annotation host is code-split: wait until it owns the canvas keys
    await waitFor(() => expect(document.querySelector(".page-annots")).not.toBeNull());
    act(() => useAnnotStore.getState().select([annot!.id]));

    act(() => openDialog("settings"));
    const dlg = await within(container).findByRole("dialog");
    const close = within(dlg).getByRole("button", { name: "닫기" });
    close.focus();
    keydown(close, { key: "Backspace", code: "Backspace" });
    await new Promise((r) => setTimeout(r, 30));
    expect((useAnnotStore.getState().byPage[0] ?? []).some((a) => a.id === annot!.id)).toBe(true);

    keydown(close, { key: "Escape", code: "Escape" });
    await waitFor(() => expect(useDialogStore.getState().stack).toHaveLength(0));
    expect(useAnnotStore.getState().selected).toEqual([annot!.id]);
  });
});

describe("keymap in a text field", () => {
  it("⌘Z in a field is the field's own undo, not the document's", async () => {
    await openApp("read");
    await runPageOps([{ kind: "rotate", pages: [0], delta: 90 }]);
    await waitFor(() => expect(useDocStore.getState().info?.canUndo).toBe(true));
    const field = document.querySelector<HTMLInputElement>(".page-input")!;
    field.focus();
    for (const init of [
      { key: "z", code: "KeyZ", metaKey: true },
      { key: "z", code: "KeyZ", metaKey: true, shiftKey: true },
      { key: "c", code: "KeyC", metaKey: true },
      { key: "v", code: "KeyV", metaKey: true },
      { key: "a", code: "KeyA", metaKey: true },
      { key: "ArrowUp", code: "ArrowUp", metaKey: true },
    ]) {
      const ev = keydown(field, init);
      expect(ev.defaultPrevented, JSON.stringify(init)).toBe(false);
    }
    await new Promise((r) => setTimeout(r, 30));
    expect(useDocStore.getState().info?.canUndo).toBe(true);
    // ⌘S still saves from a field
    expect(lookup({ key: "s", code: "KeyS", metaKey: true, ctrlKey: false, altKey: false, shiftKey: false }, "macos", ["always", "doc"])?.id).toBe("file.save");
  });
});

describe("keys the operating system owns", () => {
  it("⌘Q and Alt+F4 are never claimed", async () => {
    await openApp("read");
    const quit = keydown(window, { key: "q", code: "KeyQ", metaKey: true });
    expect(quit.defaultPrevented).toBe(false);
    act(() => useAppStore.setState({ os: "windows" }));
    const altF4 = keydown(window, { key: "F4", code: "F4", altKey: true });
    expect(altF4.defaultPrevented).toBe(false);
  });
});

describe("chord collisions", () => {
  it("Windows Ctrl+D is 복제 in 페이지 mode and 문서 정보 elsewhere", () => {
    const ctrlD = { key: "d", code: "KeyD", ctrlKey: true, metaKey: false, altKey: false, shiftKey: false };
    expect(lookup(ctrlD, "windows", ["always", "doc", "pages"])?.id).toBe("edit.duplicate");
    expect(lookup(ctrlD, "windows", ["always", "doc", "canvas"])?.id).toBe("file.docInfo");
  });
});

describe("dialog focus", () => {
  it("Tab and Shift+Tab wrap inside the dialog", async () => {
    const { container } = await openApp("annotate");
    act(() => openDialog("settings"));
    const dlg = await within(container).findByRole("dialog");
    const focusable = Array.from(
      dlg.querySelectorAll<HTMLElement>("button:not([disabled]), input:not([disabled]), select:not([disabled])"),
    );
    const first = focusable[0];
    const last = focusable[focusable.length - 1];
    last.focus();
    const tab = keydown(last, { key: "Tab", code: "Tab" });
    expect(tab.defaultPrevented).toBe(true);
    expect(document.activeElement).toBe(first);
    const back = keydown(first, { key: "Tab", code: "Tab", shiftKey: true });
    expect(back.defaultPrevented).toBe(true);
    expect(document.activeElement).toBe(last);
  });

  it("focus that lands outside is pulled back, and Esc still closes the dialog", async () => {
    const { container } = await openApp("annotate");
    act(() => openDialog("settings"));
    const dlg = await within(container).findByRole("dialog");
    const outside = Array.from(container.querySelectorAll<HTMLElement>("button")).find((b) => !dlg.contains(b))!;
    act(() => outside.focus());
    expect(dlg.contains(document.activeElement)).toBe(true);

    // even with focus on the body, Esc closes the dialog
    act(() => (document.activeElement as HTMLElement).blur());
    keydown(document.body, { key: "Escape", code: "Escape" });
    await waitFor(() => expect(useDialogStore.getState().stack).toHaveLength(0));
  });
});
