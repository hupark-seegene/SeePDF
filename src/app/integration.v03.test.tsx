/**
 * v0.3 integration: behaviour that only exists once two work packages meet.
 *
 * - pkg5 V7 (the 읽기 status-bar tool selector) × pkg6 V1 (스냅샷): 스냅샷 is a third latchable tool there.
 * - pkg5 U4 (the native window theme + `theme-changed`) × pkg6 U3 (기본값으로 되돌리기): resetting the
 *   settings also resets every window's chrome, not only this window's CSS.
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
import { toolAfterSnapshot } from "../tools/snapshot";
import { mockEvents } from "../ipc/mock";

const SAMPLE = "/Users/veri/Documents/SeePDF-샘플.pdf";

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
