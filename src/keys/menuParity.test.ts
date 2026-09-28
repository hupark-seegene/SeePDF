/**
 * The native menu (`src-tauri/src/app/menu.rs`) and the frontend dispatcher must agree: every id
 * the menu can emit is listened for (`MENU_IDS`) and has a `case` in `useCommands` — otherwise the
 * item is silently dead (bug hunt 2026-09: 설정…, Finder에서 보기, 메뉴 지우기, 선택 해제, 영역 표시,
 * 단축키, 뒤로 / 앞으로).
 */
import { describe, expect, it, vi } from "vitest";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { renderHook } from "@testing-library/react";
import { MENU_IDS } from "./keymap";
import { useCommands } from "../app/useCommands";
import { useAppStore } from "../store/appStore";
import { useDocStore } from "../store/docStore";
import { useDialogStore } from "../dialogs/dialogState";
import { useViewStore } from "../store/viewStore";
import * as api from "../ipc/api";

const menuRs = readFileSync(join(process.cwd(), "src-tauri/src/app/menu.rs"), "utf8");
const dispatcher = readFileSync(join(process.cwd(), "src/app/useCommands.ts"), "utf8");

function rustMenuIds(): string[] {
  const block = /pub const MENU_IDS: &\[&str\] = &\[([\s\S]*?)\];/.exec(menuRs);
  expect(block, "MENU_IDS in menu.rs").not.toBeNull();
  return [...block![1].matchAll(/"([^"]+)"/g)].map((m) => m[1]);
}

const SAMPLE = "/Users/veri/Documents/SeePDF-샘플.pdf";

describe("native menu ↔ dispatcher parity", () => {
  it("every id menu.rs emits is subscribed by the frontend", () => {
    const missing = rustMenuIds().filter((id) => !MENU_IDS.includes(id));
    expect(missing).toEqual([]);
  });

  it("every id menu.rs emits has a case in useCommands", () => {
    const handled = (id: string) =>
      dispatcher.includes(`case "${id}":`) || id.startsWith("tool.") || id.startsWith("mode.");
    const unhandled = rustMenuIds().filter((id) => !handled(id));
    expect(unhandled).toEqual([]);
  });
});

describe("the formerly dead menu items", () => {
  it("메뉴 지우기 clears the recents list", async () => {
    const clear = vi.spyOn(api, "clearRecent");
    const { result } = renderHook(() => useCommands());
    result.current("file.clearRecent");
    await vi.waitFor(() => expect(clear).toHaveBeenCalled());
  });

  it("Finder에서 보기 reveals the open file", async () => {
    await useDocStore.getState().open(SAMPLE);
    const reveal = vi.spyOn(api, "revealInFileManager");
    const { result } = renderHook(() => useCommands());
    result.current("file.revealInFinder");
    expect(reveal).toHaveBeenCalledWith({ path: SAMPLE });
  });

  it("영역 표시 switches to 편집 mode with the redaction tool", async () => {
    await useDocStore.getState().open(SAMPLE);
    useAppStore.setState({ mode: "read", tool: "select" });
    const { result } = renderHook(() => useCommands());
    result.current("tools.redact");
    await vi.waitFor(() => expect(useAppStore.getState().mode).toBe("edit"));
    expect(useAppStore.getState().tool).toBe("redact");
  });

  it("단축키 opens the shortcut sheet", () => {
    useDialogStore.getState().closeAll();
    const { result } = renderHook(() => useCommands());
    result.current("help.shortcuts");
    expect(useDialogStore.getState().stack.map((e) => e.name)).toEqual(["shortcuts"]);
    useDialogStore.getState().closeAll();
  });

  it("뒤로 / 앞으로 retrace the pages 이동 jumped between", async () => {
    await useDocStore.getState().open(SAMPLE);
    useViewStore.getState().resetHistory();
    useViewStore.setState({ currentPage: 0 });
    const { result } = renderHook(() => useCommands());
    useViewStore.getState().goToPage(2);
    expect(useViewStore.getState().currentPage).toBe(2);
    result.current("go.back");
    expect(useViewStore.getState().currentPage).toBe(0);
    result.current("go.forward");
    expect(useViewStore.getState().currentPage).toBe(2);
    // nothing further forward: stays put
    result.current("go.forward");
    expect(useViewStore.getState().currentPage).toBe(2);
  });
});
