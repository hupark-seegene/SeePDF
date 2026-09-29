/**
 * v0.3 pkg5-app-shell-release-diagnostics — the frontend half of H1, H3, H4, H10, H11, H14, U4, V7,
 * against the mock adapter.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import App from "../App";
import Welcome from "../welcome/Welcome";
import AboutDialog from "../dialogs/AboutDialog";
import UpdateDialog from "../update/UpdateDialog";
import { ModeSwitcher } from "./ModeSwitcher";
import { StatusBar, progressValueText } from "./StatusBar";
import { runCommand } from "./useCommands";
import { useAppStore } from "../store/appStore";
import { useDocStore } from "../store/docStore";
import { useDialogStore } from "../dialogs/dialogState";
import { useContextMenuStore, isSeparator, type MenuItem } from "./contextMenuStore";
import { useToastStore } from "./toastStore";
import { setWindowThemeBackend } from "./windowTheme";
import { setUpdaterBackend } from "../update/updater";
import { checkForUpdates, resetUpdateStore, useUpdateStore } from "../update/updateStore";
import { lookup } from "../keys/keymap";
import { mockEvents, seedRecovery } from "../ipc/mock";
import * as api from "../ipc/api";
import { t } from "../i18n";

const SAMPLE = "/Users/veri/Documents/SeePDF-샘플.pdf";

function topDialog(): string | undefined {
  const stack = useDialogStore.getState().stack;
  return stack[stack.length - 1]?.name;
}

function toastKeys(): string[] {
  return useToastStore.getState().toasts.map((x) => x.messageKey);
}

function overflowItems(): MenuItem[] {
  runCommand("app.overflow");
  const menu = useContextMenuStore.getState().menu;
  expect(menu).not.toBeNull();
  return menu!.items.filter((e): e is MenuItem => !isSeparator(e));
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
  setUpdaterBackend(null);
  resetUpdateStore();
});

// ---------------------------------------------------------------------------
// H1 — version, 도움말, 단축키 on Windows, SeePDF 정보, update 404
// ---------------------------------------------------------------------------

describe("H1: version and help reachable everywhere", () => {
  it("Welcome shows the running version from app_info, not a hard-coded one", async () => {
    await useAppStore.getState().bootstrap();
    expect(useAppStore.getState().appInfo?.version).toBe("0.2.0");
    render(<Welcome />);
    expect(screen.getByText("버전 0.2.0")).toBeInTheDocument();
    expect(screen.queryByText("버전 0.1.0")).toBeNull();
  });

  it("Welcome's quiet 도움말 opens the shortcuts sheet", async () => {
    render(<Welcome />);
    fireEvent.click(screen.getByRole("button", { name: /도움말/ }));
    expect(topDialog()).toBe("shortcuts");
  });

  it("the ⋯ overflow menu has 단축키 and SeePDF 정보, and they open their dialogs", () => {
    const items = overflowItems();
    const shortcuts = items.find((i) => i.id === "shortcuts");
    const about = items.find((i) => i.id === "about");
    expect(shortcuts?.labelKey).toBe("menu.help.shortcuts");
    expect(about?.labelKey).toBe("menu.help.about");
    shortcuts!.onSelect?.();
    expect(topDialog()).toBe("shortcuts");
    about!.onSelect?.();
    expect(topDialog()).toBe("about");
  });

  it("F1 on Windows and ⌘/ on macOS are keymap rows for 단축키", () => {
    const key = (k: string, code: string, mods: Partial<KeyboardEvent> = {}) => ({
      key: k, code, metaKey: false, ctrlKey: false, altKey: false, shiftKey: false, ...mods,
    });
    expect(lookup(key("F1", "F1"), "windows", ["always"])?.id).toBe("help.shortcuts");
    expect(lookup(key("/", "Slash", { metaKey: true }), "macos", ["always"])?.id).toBe("help.shortcuts");
  });

  it("F1 opens the shortcuts sheet in the running app (Windows)", async () => {
    useAppStore.setState({ os: "windows" });
    render(<App />);
    await waitFor(() => expect(useAppStore.getState().ready).toBe(true));
    fireEvent.keyDown(window, { key: "F1", code: "F1" });
    await waitFor(() => expect(topDialog()).toBe("shortcuts"));
  });

  it("업데이트 확인 with no latest.json on the feed (404) is a neutral 'no update information'", async () => {
    setUpdaterBackend({
      check: async () => {
        throw new Error("Could not fetch a valid release JSON from the remote");
      },
      currentVersion: async () => "0.2.0",
      relaunch: async () => undefined,
    });
    await checkForUpdates();
    expect(useUpdateStore.getState().phase).toBe("noInfo");
    render(<UpdateDialog onClose={() => undefined} />);
    expect(screen.getByText(t("update.noInfo"))).toBeInTheDocument();
    expect(screen.queryByText(t("update.error.check"))).toBeNull();
    expect(screen.queryByRole("button", { name: "다시 시도" })).toBeNull();
  });

  it("a real network failure is still an error with 다시 시도", async () => {
    setUpdaterBackend({
      check: async () => {
        throw new Error("error sending request for url (https://github.com/…): dns error");
      },
      currentVersion: async () => "0.2.0",
      relaunch: async () => undefined,
    });
    await checkForUpdates();
    expect(useUpdateStore.getState().phase).toBe("error");
  });
});

// ---------------------------------------------------------------------------
// H11 — SeePDF 정보 ▸ 오픈 소스 라이선스
// ---------------------------------------------------------------------------

describe("H11: open-source licences", () => {
  it("the About dialog shows the build and opens the notices in a scrollable pane", async () => {
    render(<AboutDialog onClose={() => undefined} />);
    expect(await screen.findByText("버전 0.2.0")).toBeInTheDocument();
    expect(screen.getByText("PDFium 155.0.8057.0")).toBeInTheDocument();
    expect(screen.getByText("macos / aarch64")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "오픈 소스 라이선스" }));
    const pane = await screen.findByLabelText("오픈 소스 라이선스", { selector: "pre" });
    expect(pane.textContent).toContain("pdfium-render");
    expect(pane.className).toContain("about-notices");
    fireEvent.click(screen.getByRole("button", { name: "뒤로" }));
    expect(await screen.findByText("PDFium 155.0.8057.0")).toBeInTheDocument();
  });
});

// ---------------------------------------------------------------------------
// H10 — 문제 보고 / 로그 폴더 열기
// ---------------------------------------------------------------------------

describe("H10: 문제 보고 and 로그 폴더 열기", () => {
  it("문제 보고 copies the report to the clipboard and reveals it in the log folder", async () => {
    const writeText = vi.fn(async () => undefined);
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    const reveal = vi.spyOn(api, "revealInFileManager");
    runCommand("help.reportProblem");
    await waitFor(() => expect(reveal).toHaveBeenCalledWith({ path: "/mock/logs/problem-report.txt" }));
    expect(writeText).toHaveBeenCalledWith(expect.stringContaining("PDFium: 155.0.8057.0"));
    expect(toastKeys()).toContain("help.report.copied");
  });

  it("a clipboard that refuses still leaves the report file, and says so", async () => {
    Object.defineProperty(navigator, "clipboard", {
      value: { writeText: vi.fn(async () => Promise.reject(new Error("NotAllowedError"))) },
      configurable: true,
    });
    const reveal = vi.spyOn(api, "revealInFileManager");
    runCommand("help.reportProblem");
    await waitFor(() => expect(toastKeys()).toContain("help.report.copyFailed"));
    expect(reveal).toHaveBeenCalled();
  });

  it("로그 폴더 열기 opens the log folder; both live in ⋯ too", async () => {
    const open = vi.spyOn(api, "openLogFolder");
    runCommand("help.openLogs");
    await waitFor(() => expect(open).toHaveBeenCalled());
    const ids = overflowItems().map((i) => i.id);
    expect(ids).toEqual(expect.arrayContaining(["reportProblem", "openLogs"]));
  });
});

// ---------------------------------------------------------------------------
// H3 — why a file did not open, badges, progress a11y
// ---------------------------------------------------------------------------

describe("H3: clear messages", () => {
  it.each([
    ["/x/회의록-not-a-pdf.pdf", "error.notPdf"],
    ["/x/corrupt.pdf", "error.corrupted"],
    ["/x/out-of-memory.pdf", "error.outOfMemory"],
    ["/x/damaged.pdf", "error.openFailed"],
  ])("opening %s toasts %s", async (path, key) => {
    const { openPath } = await import("../dialogs/flows");
    expect(await openPath(path, { guard: false })).toBeNull();
    expect(toastKeys()).toContain(key);
  });

  it("the welcome banner says why too", async () => {
    await useDocStore.getState().open("/x/not-a-pdf.pdf");
    render(<Welcome />);
    expect(screen.getByText(t("error.notPdf"))).toBeInTheDocument();
  });

  it("the status bar shows 읽기 전용 and 암호화됨 from DocInfo", async () => {
    await useDocStore.getState().open(SAMPLE);
    const info = useDocStore.getState().info!;
    const { rerender } = render(<StatusBar />);
    expect(screen.queryByText("읽기 전용")).toBeNull();
    expect(screen.queryByText("암호화됨")).toBeNull();
    act(() =>
      useDocStore.setState({ info: { ...info, encrypted: true, permissions: { ...info.permissions, modify: false } } }),
    );
    rerender(<StatusBar />);
    expect(screen.getByText("읽기 전용")).toBeInTheDocument();
    expect(screen.getByText("암호화됨")).toBeInTheDocument();
  });

  it("a determinate progress bar reads as 진행률 N퍼센트", () => {
    expect(progressValueText(3, 12, t)).toBe("진행률 25퍼센트");
    expect(progressValueText(0, 0, t)).toBeUndefined();
  });
});

// ---------------------------------------------------------------------------
// H4 — engine crash → reopen
// ---------------------------------------------------------------------------

describe("H4: engine-thread panic recovery", () => {
  it("an engineCrashed error maps to error.engineCrashed", () => {
    expect(api.errorKey(new api.SeePdfError({ code: "engineCrashed", message: "x panicked" }))).toBe("error.engineCrashed");
  });

  it("engine-crashed reopens a clean document from its file and toasts", async () => {
    render(<App />);
    await waitFor(() => expect(useAppStore.getState().ready).toBe(true));
    const { openPath } = await import("../dialogs/flows");
    const before = (await openPath(SAMPLE, { guard: false }))!;
    act(() => mockEvents.emit("engine-crashed", { docIds: [before.docId], label: "render" }));
    await waitFor(() => {
      const now = useDocStore.getState().info;
      expect(now?.path).toBe(SAMPLE);
      expect(now?.docId).not.toBe(before.docId);
    });
    expect(toastKeys()).toContain("error.engineCrashed");
  });

  it("a dirty document comes back from its newest autosave copy", async () => {
    const { openPath, recoverFromEngineCrash } = await import("../dialogs/flows");
    const before = (await openPath(SAMPLE, { guard: false }))!;
    seedRecovery([
      {
        id: "3f7c2a4e-0000-4000-8000-000000000001", originalPath: SAMPLE, name: "SeePDF-샘플.pdf",
        savedAt: "2026-09-29T10:00:00.000Z", bytes: 1234, pages: before.pageCount,
        recoveryPath: "/mock/app-data/recovery/3f7c2a4e-0000-4000-8000-000000000001.pdf",
      },
    ]);
    useDocStore.setState({ info: { ...before, dirty: true } });
    const reopened = await recoverFromEngineCrash([before.docId]);
    expect(reopened?.path).toBe("/mock/app-data/recovery/3f7c2a4e-0000-4000-8000-000000000001.pdf");
    // another document's crash is not this window's business
    expect(await recoverFromEngineCrash(["d999"])).toBeNull();
  });
});

// ---------------------------------------------------------------------------
// V7 — 손 도구 latched
// ---------------------------------------------------------------------------

describe("V7: the hand tool can be latched", () => {
  it("clicking 손 in the status bar latches it past pointerup; Esc returns to 선택", async () => {
    render(<App />);
    await useDocStore.getState().open(SAMPLE);
    const hand = await screen.findByRole("button", { name: "손 도구" });
    fireEvent.click(hand);
    expect(useAppStore.getState().tool).toBe("hand");
    fireEvent.pointerDown(document.body);
    fireEvent.pointerUp(document.body);
    fireEvent.pointerUp(window);
    expect(useAppStore.getState().tool).toBe("hand");
    expect(hand).toHaveAttribute("aria-pressed", "true");
    (document.activeElement as HTMLElement | null)?.blur();
    fireEvent.keyDown(window, { key: "Escape", code: "Escape" });
    await waitFor(() => expect(useAppStore.getState().tool).toBe("select"));
  });

  it("보기 › 손 도구 (menu, ⋯, ⇧H) latches it too", async () => {
    await useDocStore.getState().open(SAMPLE);
    runCommand("view.handTool");
    expect(useAppStore.getState().tool).toBe("hand");
    expect(useAppStore.getState().momentaryFrom).toBeNull();
    expect(overflowItems().some((i) => i.id === "handTool")).toBe(true);
    const shiftH = { key: "H", code: "KeyH", metaKey: false, ctrlKey: false, altKey: false, shiftKey: true };
    expect(lookup(shiftH, "macos", ["always", "doc", "canvas"])?.id).toBe("view.handTool");
  });
});

// ---------------------------------------------------------------------------
// U4 — native window theme
// ---------------------------------------------------------------------------

describe("U4: the window chrome follows the app theme", () => {
  it("dark calls the window's setTheme with 'dark', system with null", async () => {
    const setTheme = vi.fn(async () => undefined);
    setWindowThemeBackend(setTheme);
    useAppStore.getState().setTheme("dark");
    await waitFor(() => expect(setTheme).toHaveBeenLastCalledWith("dark"));
    useAppStore.getState().setTheme("system");
    await waitFor(() => expect(setTheme).toHaveBeenLastCalledWith(null));
  });

  it("another window's change is followed without writing settings again", async () => {
    const setTheme = vi.fn(async () => undefined);
    setWindowThemeBackend(setTheme);
    render(<App />);
    await waitFor(() => expect(useAppStore.getState().ready).toBe(true));
    const patch = vi.spyOn(api, "setSettings");
    act(() => mockEvents.emit("theme-changed", { theme: "light" }));
    expect(useAppStore.getState().theme).toBe("light");
    expect(document.documentElement.getAttribute("data-theme")).toBe("light");
    await waitFor(() => expect(setTheme).toHaveBeenLastCalledWith("light"));
    expect(patch).not.toHaveBeenCalled();
  });
});

// ---------------------------------------------------------------------------
// H14 — narrow windows
// ---------------------------------------------------------------------------

describe("H14: narrow windows", () => {
  it("ModeSwitcher renders icon-only under the narrow flag, keeping the names", async () => {
    await useDocStore.getState().open(SAMPLE);
    const { rerender } = render(<ModeSwitcher run={() => undefined} narrow />);
    const tab = screen.getByRole("tab", { name: "주석" });
    expect(tab.textContent).toBe("");
    expect(tab.querySelector("svg")).not.toBeNull();
    expect(screen.getByRole("tablist")).toHaveAttribute("data-icon-only");
    rerender(<ModeSwitcher run={() => undefined} narrow={false} />);
    expect(screen.getByRole("tab", { name: "주석" }).textContent).toBe("주석");
  });

  it("⋯ carries 찾기 and 내보내기 when the window is narrower than 900 px", async () => {
    await useDocStore.getState().open(SAMPLE);
    const width = window.innerWidth;
    try {
      Object.defineProperty(window, "innerWidth", { value: 820, configurable: true });
      expect(overflowItems().slice(0, 2).map((i) => i.id)).toEqual(["find", "export"]);
      Object.defineProperty(window, "innerWidth", { value: 1400, configurable: true });
      expect(overflowItems().some((i) => i.id === "find")).toBe(false);
    } finally {
      Object.defineProperty(window, "innerWidth", { value: width, configurable: true });
    }
  });
});
