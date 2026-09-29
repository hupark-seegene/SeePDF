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
  | "confirm"
  // generic choice prompt (Stage 9: 문단이 들어갈 자리가 부족합니다)
  | "choice"
  // 업데이트 확인 (v0.2.0)
  | "update"
  // P2: 자르기 and 페이지 크기 변경 (페이지 mode)
  | "crop" | "resize"
  // 페이지 레이블 (P2)
  | "pageLabels"
  // 여러 파일에서 검색 (P2)
  | "multiSearch"
  // v0.3 pkg2-pages-structure-forms: 이미지로 PDF 만들기, 위치 이동…
  | "imagesToPdf" | "moveTo"
  // 도움말 › 단축키
  | "shortcuts"
  // v0.3 pkg5 (H1 / H11): SeePDF 정보 + 오픈 소스 라이선스
  | "about";

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

/**
 * A prompt's promise must settle however its entry leaves the stack. `ask` puts a `dismiss` in the
 * props that answers the prompt's cancel value (a no-op once it has been answered); an entry that
 * is replaced by another of the same name, or closed from outside, is dismissed.
 */
function dismiss(entries: DialogEntry[]): void {
  for (const entry of entries) (entry.props.dismiss as (() => void) | undefined)?.();
}

export const useDialogStore = create<DialogStore>((set, get) => ({
  stack: [],
  open(name, props = {}) {
    const stack = get().stack;
    set({ stack: [...stack.filter((e) => e.name !== name), { name, props, key: ++key }] });
    dismiss(stack.filter((e) => e.name === name));
  },
  close(name) {
    const stack = get().stack;
    const kept = name ? stack.filter((e) => e.name !== name) : stack.slice(0, -1);
    set({ stack: kept });
    dismiss(stack.filter((e) => !kept.includes(e)));
  },
  closeAll() {
    const stack = get().stack;
    set({ stack: [] });
    dismiss(stack);
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

/**
 * Open a prompt and wait for its answer. Settles exactly once: with the user's answer, or with
 * `cancelValue` when the prompt is replaced by another of its kind (a second password prompt, a
 * second unsaved prompt) or closed from outside — a caller never hangs on a prompt nobody sees.
 */
function ask<T>(name: DialogName, props: Record<string, unknown>, cancelValue: T): Promise<T> {
  return new Promise<T>((resolve) => {
    let settled = false;
    const settle = (value: T): boolean => {
      if (settled) return false;
      settled = true;
      resolve(value);
      return true;
    };
    openDialog(name, {
      ...props,
      resolve: (value: T) => {
        if (!settle(value)) return;
        // close this prompt only — never a newer one of the same name
        const store = useDialogStore.getState();
        const entry = store.stack.find((e) => e.props.dismiss === dismissThis);
        if (entry) useDialogStore.setState({ stack: store.stack.filter((e) => e !== entry) });
      },
      dismiss: dismissThis,
    });
    function dismissThis(): void {
      settle(cancelValue);
    }
  });
}

/** 저장 / 저장 안 함 / 취소 (F-23). */
export function askUnsaved(name: string): Promise<UnsavedAnswer> {
  return ask<UnsavedAnswer>("unsaved", { name }, "cancel");
}

/** 암호 입력, retried in place: `null` = the user cancelled (F-01). */
export function askPassword(fileName: string, wrong = false): Promise<string | null> {
  return ask<string | null>("password", { fileName, wrong }, null);
}

/** 여러 파일을 어떻게 열까요? — 각각 열기 / 하나로 합치기 (UI_SPEC §11). */
export function askMultipleFiles(paths: string[]): Promise<MultipleFilesAnswer> {
  return ask<MultipleFilesAnswer>("multipleFiles", { paths }, null);
}

/** 페이지 추출: resolves with the chosen path + 원본에서 삭제, or `null`. */
export function askExtract(pages: PageIndex[], info: DocInfo): Promise<{ removeAfter: boolean } | null> {
  return ask<{ removeAfter: boolean } | null>("extract", { pages, info }, null);
}

/** A generic yes / no prompt: resolves `true` on the primary button, `false` on cancel / Esc. */
export interface ConfirmRequest {
  titleKey: string;
  bodyKey: string;
  bodyParams?: Record<string, string | number>;
  confirmKey?: string;
  /** the secondary button's label (default 취소); it still resolves `false` */
  cancelKey?: string;
  /** a destructive primary button (영역 표시 적용) */
  danger?: boolean;
}

export function askConfirm(req: ConfirmRequest): Promise<boolean> {
  return ask<boolean>("confirm", { ...req }, false);
}

/** One option of `askChoice`: a full-width button in the prompt's body. */
export interface ChoiceOption<T extends string> {
  value: T;
  labelKey: string;
  labelParams?: Record<string, string | number>;
  /** the accent button (at most one) */
  primary?: boolean;
  disabled?: boolean;
}

/**
 * A prompt with several answers: the options stack in the body (long Korean labels fit a small
 * dialog), the footer's quiet button — and Esc, and a click on the backdrop — answer `cancel`.
 */
export interface ChoiceRequest<T extends string> {
  titleKey: string;
  bodyKey: string;
  bodyParams?: Record<string, string | number>;
  /** a secondary line under the body (e.g. why an option is disabled) */
  hintKey?: string;
  hintParams?: Record<string, string | number>;
  options: ChoiceOption<T>[];
  cancel: { value: T; labelKey: string };
}

export function askChoice<T extends string>(req: ChoiceRequest<T>): Promise<T> {
  return ask<T>("choice", { ...req }, req.cancel.value);
}
