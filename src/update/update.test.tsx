import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, renderHook, screen, waitFor } from "@testing-library/react";
import DialogHost from "../dialogs/DialogHost";
import { SettingsDialog } from "../dialogs/SettingsDialog";
import { useDialogStore } from "../dialogs/dialogState";
import { useToastStore } from "../app/toastStore";
import { useCommands } from "../app/useCommands";
import { useAppStore } from "../store/appStore";
import { MENU_IDS } from "../keys/keymap";
import { setUpdaterBackend, type AvailableUpdate, type UpdaterBackend } from "./updater";
import {
  checkForUpdates,
  downloadUpdate,
  installAndRestart,
  progressPercent,
  resetUpdateStore,
  useUpdateStore,
} from "./updateStore";
import { checkOnLaunch, wantsLaunchCheck } from "./launch";

/** A scripted updater: `release` is what the feed offers (null = up to date). */
function fakeBackend(release: { version: string; notes?: string } | null, opts: { failCheck?: string } = {}) {
  let finishDownload: () => void = () => undefined;
  let progress: (downloaded: number, total: number | null) => void = () => undefined;
  const install = vi.fn(async () => undefined);
  const relaunch = vi.fn(async () => undefined);
  const check = vi.fn(async (): Promise<AvailableUpdate | null> => {
    if (opts.failCheck) throw new Error(opts.failCheck);
    if (!release) return null;
    return {
      version: release.version,
      currentVersion: "0.2.0",
      notes: release.notes ?? null,
      date: "2026-10-01T00:00:00Z",
      download: (onProgress) =>
        new Promise<void>((resolve) => {
          progress = onProgress;
          finishDownload = resolve;
          onProgress(0, 1000);
        }),
      install,
    };
  });
  const backend: UpdaterBackend = { check, currentVersion: async () => "0.2.0", relaunch };
  setUpdaterBackend(backend);
  return {
    check,
    install,
    relaunch,
    progress: (n: number) => act(() => progress(n, 1000)),
    finish: () => act(() => finishDownload()),
  };
}

beforeEach(() => {
  resetUpdateStore();
  useDialogStore.getState().closeAll();
  useToastStore.setState({ toasts: [] });
});

afterEach(() => setUpdaterBackend(null));

describe("update.store", () => {
  it("check → available → download with progress → ready → install + relaunch", async () => {
    const f = fakeBackend({ version: "0.3.0", notes: "빠른 열기" });
    await checkForUpdates();
    expect(useUpdateStore.getState()).toMatchObject({ phase: "available", version: "0.3.0", currentVersion: "0.2.0", notes: "빠른 열기" });

    const done = downloadUpdate();
    expect(useUpdateStore.getState().phase).toBe("downloading");
    f.progress(400);
    expect(progressPercent(useUpdateStore.getState().downloaded, useUpdateStore.getState().total)).toBe(40);
    f.finish();
    await done;
    expect(useUpdateStore.getState().phase).toBe("ready");

    const confirm = vi.fn(async () => true);
    await expect(installAndRestart(confirm)).resolves.toBe(true);
    expect(confirm).toHaveBeenCalledOnce();
    expect(f.install).toHaveBeenCalledOnce();
    expect(f.relaunch).toHaveBeenCalledOnce();
  });

  it("취소 in the unsaved prompt installs nothing", async () => {
    const f = fakeBackend({ version: "0.3.0" });
    await checkForUpdates();
    const done = downloadUpdate();
    f.finish();
    await done;
    await expect(installAndRestart(async () => false)).resolves.toBe(false);
    expect(f.install).not.toHaveBeenCalled();
    expect(useUpdateStore.getState().phase).toBe("ready");
  });

  it("up to date, and a failed check is an error with its detail", async () => {
    fakeBackend(null);
    await checkForUpdates();
    expect(useUpdateStore.getState()).toMatchObject({ phase: "upToDate", currentVersion: "0.2.0" });

    resetUpdateStore();
    fakeBackend(null, { failCheck: "error sending request" });
    await checkForUpdates();
    expect(useUpdateStore.getState()).toMatchObject({ phase: "error", failedStep: "check", error: "error sending request" });
  });

  it("progress is unknown without a Content-Length", () => {
    expect(progressPercent(10, null)).toBeNull();
    expect(progressPercent(500, 1000)).toBe(50);
    expect(progressPercent(2000, 1000)).toBe(100);
  });
});

describe("update.dialog", () => {
  it("업데이트 확인…: version, notes, 다운로드, 진행률, 다시 시작", async () => {
    const f = fakeBackend({ version: "0.3.0", notes: "문서 비교가 빨라졌습니다" });
    render(<DialogHost />);
    const { result } = renderHook(() => useCommands());
    act(() => result.current("app.checkUpdates"));

    expect(await screen.findByRole("dialog", { name: "업데이트 확인" })).toBeInTheDocument();
    expect(await screen.findByText("새 버전 SeePDF 0.3.0을(를) 사용할 수 있습니다.")).toBeInTheDocument();
    expect(screen.getByText("현재 버전: 0.2.0")).toBeInTheDocument();
    expect(screen.getByText("문서 비교가 빨라졌습니다")).toBeInTheDocument();
    expect(f.check).toHaveBeenCalledOnce();

    fireEvent.click(screen.getByRole("button", { name: "다운로드" }));
    const bar = await screen.findByRole("progressbar", { name: "다운로드 진행률" });
    expect(bar).toHaveAttribute("aria-valuenow", "0");
    f.progress(250);
    expect(bar).toHaveAttribute("aria-valuenow", "25");
    expect(screen.getByRole("button", { name: "다운로드 중…" })).toBeDisabled();

    f.finish();
    const restart = await screen.findByRole("button", { name: "다시 시작" });
    expect(screen.getByText(/업데이트가 준비되었습니다/)).toBeInTheDocument();
    fireEvent.click(restart);
    await waitFor(() => expect(f.relaunch).toHaveBeenCalledOnce());
    expect(f.install).toHaveBeenCalledOnce();
  });

  it("up to date says so with the running version; an error offers 다시 시도", async () => {
    fakeBackend(null);
    render(<DialogHost />);
    const { result } = renderHook(() => useCommands());
    act(() => result.current("app.checkUpdates"));
    expect(await screen.findByText("최신 버전을 사용하고 있습니다 (SeePDF 0.2.0).")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "다운로드" })).toBeNull();

    const f = fakeBackend(null, { failCheck: "dns error" });
    act(() => useUpdateStore.setState({ phase: "idle" }));
    act(() => result.current("app.checkUpdates"));
    expect(await screen.findByText(/업데이트를 확인하지 못했습니다/)).toBeInTheDocument();
    expect(screen.getByText("dns error")).toBeInTheDocument();
    fakeBackend(null);
    fireEvent.click(screen.getByRole("button", { name: "다시 시도" }));
    expect(await screen.findByText(/최신 버전을 사용하고 있습니다/)).toBeInTheDocument();
    expect(f.check).toHaveBeenCalledOnce();
  });

  it("the native menu item and the ⋯ menu reach the same command", () => {
    expect(MENU_IDS).toContain("app.checkUpdates");
    // the app menu's 설정… item is `menu:settings`
    expect(MENU_IDS).toContain("settings");
    const { result } = renderHook(() => useCommands());
    act(() => result.current("settings"));
    expect(useDialogStore.getState().stack.at(-1)?.name).toBe("settings");
  });
});

describe("update.launch", () => {
  it("checks on launch unless 설정 turned it off, and only in the main window", () => {
    expect(wantsLaunchCheck({ checkUpdates: true }, "main")).toBe(true);
    expect(wantsLaunchCheck(null, "main")).toBe(true);
    expect(wantsLaunchCheck({ checkUpdates: false }, "main")).toBe(false);
    expect(wantsLaunchCheck({ checkUpdates: true }, "doc-2")).toBe(false);
  });

  it("a newer version is a toast with 업데이트…, which opens the dialog", async () => {
    fakeBackend({ version: "0.3.0" });
    await checkOnLaunch({ checkUpdates: true }, { force: true });
    const toast = useToastStore.getState().toasts.at(-1);
    expect(toast).toMatchObject({ messageKey: "update.toast", params: { version: "0.3.0" } });
    toast?.actions?.[0].onSelect();
    expect(useDialogStore.getState().stack.at(-1)?.name).toBe("update");
  });

  it("is silent when up to date or offline, and does not ask at all when turned off", async () => {
    const f = fakeBackend(null, { failCheck: "offline" });
    await checkOnLaunch({ checkUpdates: true }, { force: true });
    expect(useToastStore.getState().toasts).toHaveLength(0);
    expect(useDialogStore.getState().stack).toHaveLength(0);

    resetUpdateStore();
    await checkOnLaunch({ checkUpdates: false }, { force: true });
    expect(f.check).toHaveBeenCalledOnce();
  });

  it("설정 › 일반 has the switch, on by default", async () => {
    await useAppStore.getState().bootstrap();
    const patch = vi.spyOn(useAppStore.getState(), "patchSettings");
    render(<SettingsDialog onClose={() => undefined} />);
    const box = screen.getByRole("checkbox", { name: "시작할 때 업데이트 확인" });
    expect(box).toBeChecked();
    fireEvent.click(box);
    expect(patch).toHaveBeenCalledWith({ checkUpdates: false });
  });
});
