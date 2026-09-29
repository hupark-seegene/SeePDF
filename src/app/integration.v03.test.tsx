/**
 * v0.3 integration: behaviour that only exists once two work packages meet.
 *
 * - pkg5 V7 (the 읽기 status-bar tool selector) × pkg6 V1 (스냅샷): 스냅샷 is a third latchable tool there.
 * - pkg5 U4 (the native window theme + `theme-changed`) × pkg6 U3 (기본값으로 되돌리기): resetting the
 *   settings also resets every window's chrome, not only this window's CSS.
 * - pkg3 S5 (permission flags) × pkg6 V1 / V2 / caret, × pkg1 R4: a document that forbids copying
 *   forbids 스냅샷 (every entry point); one that forbids comments disables the canvas menu's 주석
 *   entries and caret N / H / U / K; 그룹 해제 asks first on a signed document (S1's gate).
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import App from "../App";
import { useAppStore } from "../store/appStore";
import { useDocStore } from "../store/docStore";
import { useDialogStore } from "../dialogs/dialogState";
import { useToastStore } from "./toastStore";
import { useContextMenuStore } from "./contextMenuStore";
import { setWindowThemeBackend } from "./windowTheme";
import { resetToDefaults } from "../dialogs/SettingsDialog";
import { copySnapshot, toolAfterSnapshot } from "../tools/snapshot";
import { mutatingCall } from "../ipc/api";
import { permissionBlock } from "./permissions";
import { onCaretKey } from "../viewer/text/caretKeys";
import { useCaretStore } from "../viewer/text/caret";
import { mockEvents } from "../ipc/mock";

const SAMPLE = "/Users/veri/Documents/SeePDF-샘플.pdf";
/** pkg3's mock fixture: print, copy, modify and assemble are forbidden; comments are allowed. */
const RESTRICTED = "/Users/veri/Documents/restricted-보고서.pdf";

function toastKeys(): string[] {
  return useToastStore.getState().toasts.map((t) => t.messageKey);
}

beforeEach(() => {
  useDialogStore.getState().closeAll();
  useToastStore.setState({ toasts: [] });
  useContextMenuStore.setState({ menu: null });
  useDocStore.setState({ docId: null, info: null, outline: [], status: "empty", error: null });
  useAppStore.setState({ os: "macos", mode: "read", tool: "select", momentaryFrom: null, theme: "system", appInfo: null });
});

afterEach(() => {
  setWindowThemeBackend(null);
  vi.restoreAllMocks();
});

describe("V7 × V1: 스냅샷 in the 읽기 tool selector", () => {
  it("the status bar offers 선택 / 손 도구 / 스냅샷, and 스냅샷 arms the snapshot tool", async () => {
    render(<App />);
    await useDocStore.getState().open(SAMPLE);
    const group = await screen.findByRole("group", { name: "도구" });
    const snap = await screen.findByRole("button", { name: "스냅샷" });
    expect(group.contains(snap)).toBe(true);
    expect(group.contains(screen.getByRole("button", { name: "손 도구" }))).toBe(true);
    fireEvent.click(snap);
    expect(useAppStore.getState().tool).toBe("snapshot");
    expect(snap).toHaveAttribute("aria-pressed", "true");
    // one-shot: after the marquee the pointer goes back to 선택 (in 읽기), not to 손
    expect(toolAfterSnapshot()).toBe("select");
  });
});

describe("U4 × U3: 기본값으로 되돌리기 resets the window chrome too", () => {
  it("resetting from 어둡게 sets the native window theme back to the system and tells the other windows", async () => {
    const setTheme = vi.fn(async () => undefined);
    setWindowThemeBackend(setTheme);
    useAppStore.getState().setTheme("dark");
    await waitFor(() => expect(setTheme).toHaveBeenLastCalledWith("dark"));
    const broadcast: unknown[] = [];
    const off = mockEvents.on("theme-changed", (p) => broadcast.push(p));
    try {
      const done = resetToDefaults();
      const prompt = await waitFor(() => {
        const entry = useDialogStore.getState().stack.find((e) => e.name === "confirm");
        expect(entry).toBeDefined();
        return entry!;
      });
      await act(async () => (prompt.props.resolve as (v: boolean) => void)(true));
      expect(await done).toBe(true);
    } finally {
      off();
    }
    expect(useAppStore.getState().theme).toBe("system");
    await waitFor(() => expect(setTheme).toHaveBeenLastCalledWith(null));
    expect(broadcast).toContainEqual({ theme: "system" });
  });
});

describe("S5 × V1: a document that forbids copying forbids 스냅샷", () => {
  it("⌥⌘C, the status-bar segment, the canvas menu and the copy itself all refuse with the reason", async () => {
    render(<App />);
    const info = (await useDocStore.getState().open(RESTRICTED))!;
    const reason = "security.restricted.reason.extractText";
    expect(permissionBlock("tool.snapshot", info)).toBe(reason);

    // the status bar's 스냅샷 segment is disabled, 선택 / 손 도구 are not
    const snap = await screen.findByRole("button", { name: "스냅샷" });
    expect(snap).toBeDisabled();
    expect(screen.getByRole("button", { name: "손 도구" })).toBeEnabled();

    // the shortcut goes through the dispatcher: a toast, and the tool stays 선택
    const { runCommand } = await import("./useCommands");
    act(() => runCommand("tool.snapshot"));
    expect(useAppStore.getState().tool).toBe("select");
    expect(toastKeys()).toContain(reason);

    // the canvas menu's 스냅샷 is disabled with the reason
    const { openPageContextMenu } = await import("./pageMenus");
    openPageContextMenu(0, "canvas", 10, 10);
    const item = useContextMenuStore.getState().menu?.items.find((i) => i.id === "snapshot");
    expect(item).toMatchObject({ disabled: true, hintKey: reason });
    useContextMenuStore.getState().close();

    // and a marquee that was armed before (a tool kept from another document) copies nothing
    useToastStore.setState({ toasts: [] });
    const write = vi.fn();
    vi.stubGlobal("navigator", { ...navigator, clipboard: { write } });
    try {
      const ok = await copySnapshot({
        docId: info.docId, page: 0, rect: { l: 0, b: 0, r: 100, t: 100 }, zoomPercent: 100, rotation: 0, dpr: 1,
      });
      expect(ok).toBe(false);
      expect(write).not.toHaveBeenCalled();
      expect(toastKeys()).toContain(reason);
    } finally {
      vi.unstubAllGlobals();
    }
  });

  it("an unrestricted document keeps 스냅샷 everywhere", async () => {
    const info = (await useDocStore.getState().open(SAMPLE))!;
    expect(permissionBlock("tool.snapshot", info)).toBeNull();
    const { openPageContextMenu } = await import("./pageMenus");
    openPageContextMenu(0, "canvas", 10, 10);
    const item = useContextMenuStore.getState().menu?.items.find((i) => i.id === "snapshot");
    expect(item && "disabled" in item ? item.disabled : false).toBeFalsy();
    useContextMenuStore.getState().close();
  });
});

describe("S5 × caret browsing: N / H / U / K make comments", () => {
  it("on a document that forbids comments, N says why and adds no note", async () => {
    const opened = (await useDocStore.getState().open(SAMPLE))!;
    // the same document, with comments forbidden (pkg3's flags as DocInfo carries them)
    const info = { ...opened, encrypted: true, permissions: { ...opened.permissions, annotate: false } };
    useDocStore.setState({ info });
    useCaretStore.getState().set(true, { docId: info.docId, page: 0, offset: 0 });
    const create = vi.spyOn(await import("../annot/actions"), "createAnnotation");
    try {
      const e = new KeyboardEvent("keydown", { key: "n", code: "KeyN", bubbles: true, cancelable: true });
      document.body.dispatchEvent(e);
      onCaretKey(e);
      expect(e.defaultPrevented).toBe(true);
      expect(toastKeys()).toContain("security.restricted.reason.annotate");
      // addNote imports annot/actions lazily: give that a moment before checking nothing was made
      await new Promise((r) => setTimeout(r, 50));
      expect(create).not.toHaveBeenCalled();
      expect(useAppStore.getState().mode).toBe("read");
    } finally {
      useCaretStore.getState().set(false);
    }
  });
});

describe("S1 × R4: 그룹 해제 is an edit", () => {
  it("ungroup_object goes through the signed-document gate", () => {
    expect(mutatingCall("ungroup_object", { docId: "d1", page: 0, objectId: 3 })).toMatchObject({ docId: "d1" });
  });
});
