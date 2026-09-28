/**
 * Which dialog is on screen. A tiny store rather than context: `useCommands.ts` is not a component
 * and the open flows run outside React.
 *
 * It is a **stack** so a prompt can open over a sheet (the unsaved prompt over 파일 열기, the
 * password prompt over the welcome screen). Only the top entry renders.
 *
 * This module stays in the entry chunk — it must be importable from the dispatcher — so it holds no
 * component and no CSS. `DialogHost` (and every dialog behind it) is dynamically imported.
 */
import { create } from "zustand";
import type { DocInfo, PageIndex } from "../ipc/types";

export type DialogName =
  | "export" | "merge" | "split" | "print" | "settings" | "docInfo"
  | "password" | "unsaved" | "extract" | "insertFrom" | "multipleFiles" | "signature" | "security"
  | "stamp" | "compress" | "compare" | "recovery"
  // 여러 파일 OCR (P1-7)
  | "batchOcr"
  // 도장 선택 (P1-12)
  | "stampPicker"
  // generic 확인 prompt (Stage 7: 글꼴 바꾸기)
  | "confirm";

/** Answers the modal prompts resolve with. */
export type UnsavedAnswer = "save" | "dontSave" | "cancel";
export type MultipleFilesAnswer = "separate" | "merge" | null;

export interface DialogEntry {
  name: DialogName;
  /** per-dialog props; prompts carry their `resolve` here */
  props: Record<string, unknown>;
  key: number;
}

interface DialogStore {
  stack: DialogEntry[];
  open(name: DialogName, props?: Record<string, unknown>): void;
  close(name?: DialogName): void;
  closeAll(): void;
}

let key = 0;

export const useDialogStore = create<DialogStore>((set, get) => ({
  stack: [],
  open(name, props = {}) {
    set({ stack: [...get().stack.filter((e) => e.name !== name), { name, props, key: ++key }] });
  },
  close(name) {
    const stack = get().stack;
    set({ stack: name ? stack.filter((e) => e.name !== name) : stack.slice(0, -1) });
  },
  closeAll() {
    set({ stack: [] });
  },
}));

/** Open a dialog from anywhere (the command dispatcher, a context menu, a toast action). */
export function openDialog(name: DialogName, props: Record<string, unknown> = {}): void {
  useDialogStore.getState().open(name, props);
}

export function closeDialog(name?: DialogName): void {
  useDialogStore.getState().close(name);
}

export function isDialogOpen(): boolean {
  return useDialogStore.getState().stack.length > 0;
}

function ask<T>(name: DialogName, props: Record<string, unknown>): Promise<T> {
  return new Promise<T>((resolve) => {
    openDialog(name, {
      ...props,
      resolve: (value: T) => {
        closeDialog(name);
        resolve(value);
      },
    });
  });
}

/** 저장 / 저장 안 함 / 취소 (F-23). */
export function askUnsaved(name: string): Promise<UnsavedAnswer> {
  return ask<UnsavedAnswer>("unsaved", { name });
}

/** 암호 입력, retried in place: `null` = the user cancelled (F-01). */
export function askPassword(fileName: string, wrong = false): Promise<string | null> {
  return ask<string | null>("password", { fileName, wrong });
}

/** 여러 파일을 어떻게 열까요? — 각각 열기 / 하나로 합치기 (UI_SPEC §11). */
export function askMultipleFiles(paths: string[]): Promise<MultipleFilesAnswer> {
  return ask<MultipleFilesAnswer>("multipleFiles", { paths });
}

/** 페이지 추출: resolves with the chosen path + 원본에서 삭제, or `null`. */
export function askExtract(pages: PageIndex[], info: DocInfo): Promise<{ removeAfter: boolean } | null> {
  return ask<{ removeAfter: boolean } | null>("extract", { pages, info });
}

/** A generic yes / no prompt: resolves `true` on the primary button, `false` on cancel / Esc. */
export interface ConfirmRequest {
  titleKey: string;
  bodyKey: string;
  bodyParams?: Record<string, string | number>;
  confirmKey?: string;
  /** a destructive primary button (영역 표시 적용) */
  danger?: boolean;
}

export function askConfirm(req: ConfirmRequest): Promise<boolean> {
  return ask<boolean>("confirm", { ...req });
}
