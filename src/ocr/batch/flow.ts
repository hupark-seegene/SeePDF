/**
 * 여러 파일 OCR (P1-7): a queue of PDFs on disk, one file at a time, the pages of each file in
 * parallel on **one** tesseract worker pool shared by the whole batch.
 *
 * Per file: `open_document` (beside the window's document, never through `docStore`; an encrypted
 * file asks for its password through the ordinary 암호 입력 prompt, and dismissing it skips the
 * file) → `runOcrJob` over every page → `save_document_as` to `<name>-ocr.pdf` (beside the source
 * or in the chosen folder; never over the source, never over anything that exists — `(2)`, `(3)`…)
 * → `close_document`, on success, failure and cancel alike.
 *
 * The run lives in this module, not in the dialog: the password prompt stacks on top of the dialog
 * and the host renders only the top entry, and the user may close the dialog and keep working while
 * the queue runs (the status bar shows it, with its own ×).
 */
import { create } from "zustand";
import * as api from "../../ipc/api";
import type { DocId, DocInfo } from "../../ipc/types";
import { askPassword, useDialogStore } from "../../dialogs/dialogState";
import { localJobId, registerCanceller, useJobStore } from "../../store/jobStore";
import { useAppStore } from "../../store/appStore";
import { toast } from "../../app/toastStore";
import { resolveDpi, runOcrJob, type OcrDpi } from "../ocrJob";
import { DEFAULT_LAYOUT, TesseractPool, defaultWorkerCount } from "../tesseractPool";
import {
  addPaths, isRunnable, patchItem, pathKey, pickOutputPath, removeItem, requeue, summarize,
  type BatchItem, type BatchStatus,
} from "./queue";

export type BatchPhase = "idle" | "running" | "finished";

export interface BatchOptions {
  /** 한국어 — always recognised together with English (`kor` alone garbles Latin) */
  ko: boolean;
  en: boolean;
  dpi: OcrDpi;
  /** UI_SPEC `ocr.option.skipText`, on by default */
  skipPagesWithText: boolean;
  /** `null` = beside each source file */
  outputDir: string | null;
}

export interface BatchState {
  items: BatchItem[];
  options: BatchOptions;
  phase: BatchPhase;
  /** files finished in the current run / files in the current run */
  runDone: number;
  runTotal: number;
  cancelling: boolean;
  /** the status-bar job of the current run */
  jobId: number | null;
}

const DEFAULT_OPTIONS: BatchOptions = { ko: true, en: true, dpi: "auto", skipPagesWithText: true, outputDir: null };

function initial(): BatchState {
  return {
    items: [], options: { ...DEFAULT_OPTIONS }, phase: "idle",
    runDone: 0, runTotal: 0, cancelling: false, jobId: null,
  };
}

export const useBatchOcr = create<BatchState>(() => initial());

let nextId = 0;
const newId = () => ++nextId;

/** What must be released however the run ends. */
const live: { controller: AbortController | null; docId: DocId | null; run: Promise<void> | null } = {
  controller: null,
  docId: null,
  run: null,
};

/** The OCR dialog's language rule: `kor` never goes alone (spike §4.2). */
export function langsFor(o: Pick<BatchOptions, "ko" | "en">): string {
  return o.ko ? "kor+eng" : "eng";
}

// ---------------------------------------------------------------------------
// List editing (the dialog)
// ---------------------------------------------------------------------------

export function addFiles(paths: string[]): void {
  const s = useBatchOcr.getState();
  // A new file after a finished run starts a new session for the summary, not a new list.
  useBatchOcr.setState({ items: addPaths(s.items, paths, newId), phase: s.phase === "finished" ? "idle" : s.phase });
}

export function removeFile(id: number): void {
  useBatchOcr.setState({ items: removeItem(useBatchOcr.getState().items, id) });
}

export function clearFiles(): void {
  if (useBatchOcr.getState().phase === "running") return;
  useBatchOcr.setState({ items: [], phase: "idle" });
}

export function setOptions(patch: Partial<BatchOptions>): void {
  useBatchOcr.setState({ options: { ...useBatchOcr.getState().options, ...patch } });
}

/** The dialog closed after a run: the next one starts from an empty list. Never mid-run. */
export function resetBatch(): void {
  if (useBatchOcr.getState().phase === "running") return;
  useBatchOcr.setState(initial());
}

function patch(id: number, p: Partial<Omit<BatchItem, "id">>): void {
  useBatchOcr.setState({ items: patchItem(useBatchOcr.getState().items, id, p) });
}

function itemOf(id: number): BatchItem | undefined {
  return useBatchOcr.getState().items.find((i) => i.id === id);
}

function message(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

// ---------------------------------------------------------------------------
// The run
// ---------------------------------------------------------------------------

type Outcome = { status: BatchStatus } & Partial<Omit<BatchItem, "id" | "status">>;

/** 시작: process every runnable file in list order. Resolves when the run is over. */
export function startBatch(): Promise<void> {
  const s = useBatchOcr.getState();
  if (s.phase === "running") return live.run ?? Promise.resolve();
  const queue = s.items.filter(isRunnable).map((i) => i.id);
  if (queue.length === 0 || (!s.options.ko && !s.options.en)) return Promise.resolve();
  const run = runQueue(queue).finally(() => {
    if (live.run === run) live.run = null;
  });
  live.run = run;
  return run;
}

/** 취소 (the dialog or the status bar's ×): stop the current file, close it, stop the queue. */
export async function cancelBatch(): Promise<void> {
  const controller = live.controller;
  if (!controller) return;
  useBatchOcr.setState({ cancelling: true });
  controller.abort();
  await live.run?.catch(() => undefined);
}

export function isBatchRunning(): boolean {
  return live.controller !== null;
}

async function runQueue(queue: number[]): Promise<void> {
  const controller = new AbortController();
  live.controller = controller;
  const { signal } = controller;
  const { options } = useBatchOcr.getState();
  const langs = langsFor(options);
  const dpi = resolveDpi(options.dpi);
  const workers = defaultWorkerCount();
  // One pool for the whole batch: the workers load their language data once, not once per file.
  const pool = new TesseractPool({ langs, layout: DEFAULT_LAYOUT, dpi, workers });
  /** outputs this run has claimed, so two `보고서.pdf` never race for one name */
  const reserved = new Set<string>();

  const jobId = localJobId();
  const jobs = useJobStore.getState();
  jobs.start({ id: jobId, kind: "batchOcr", labelKey: "batchOcr.running", total: queue.length });
  const unregister = registerCanceller(jobId, () => void cancelBatch());

  useBatchOcr.setState({
    items: requeue(useBatchOcr.getState().items, new Set(queue)),
    phase: "running", runDone: 0, runTotal: queue.length, cancelling: false, jobId,
  });

  let finished = 0;
  try {
    for (const id of queue) {
      if (signal.aborted) break;
      const item = itemOf(id);
      if (!item) continue;
      let outcome: Outcome;
      try {
        outcome = await processFile(item, { options, langs, workers, pool, reserved, signal });
      } catch (e) {
        outcome = { status: "failed", reasonKey: "error.generic", detail: message(e) };
      }
      patch(id, outcome);
      if (outcome.status === "cancelled") break;
      finished += 1;
      useBatchOcr.setState({ runDone: finished });
      useJobStore.getState().update(jobId, { done: finished, note: item.name });
    }
  } finally {
    unregister();
    await pool.terminate().catch(() => undefined);
    const cancelled = signal.aborted;
    useJobStore.getState().update(jobId, { state: cancelled ? "cancelled" : "done", done: finished });
    if (live.controller === controller) live.controller = null;
    useBatchOcr.setState({ phase: "finished", cancelling: false });
    announce(cancelled);
  }
}

interface FileContext {
  options: BatchOptions;
  langs: string;
  workers: number;
  pool: TesseractPool;
  reserved: Set<string>;
  signal: AbortSignal;
}

async function processFile(item: BatchItem, ctx: FileContext): Promise<Outcome> {
  const { signal } = ctx;
  patch(item.id, { status: "opening" });
  const opened = await openForBatch(item.path, signal);
  if ("status" in opened) return opened;
  const info = opened.info;
  live.docId = info.docId;
  try {
    if (signal.aborted) return { status: "cancelled" };
    const pages = Array.from({ length: info.pageCount }, (_, i) => i);
    patch(item.id, { status: "running", done: 0, total: pages.length });
    const result = await runOcrJob({
      docId: info.docId,
      docGeneration: info.docGeneration,
      pages,
      pageGeom: info.pages,
      langs: ctx.langs,
      dpi: ctx.options.dpi,
      skipPagesWithText: ctx.options.skipPagesWithText,
      workers: ctx.workers,
      pool: ctx.pool,
      track: false,
      signal,
      onPlan: (todo) => patch(item.id, { total: todo.length }),
      onPageDone: () => patch(item.id, { done: (itemOf(item.id)?.done ?? 0) + 1 }),
    });
    if (result.status === "cancelled" || signal.aborted) return { status: "cancelled" };
    if (result.status === "error") {
      return {
        status: "failed",
        reasonKey: result.detailKey ?? "ocr.failed",
        detail: result.error === undefined ? undefined : message(result.error),
      };
    }
    // Every page already had text: nothing to add, so no copy is written.
    if (result.applied.length === 0) return { status: "skipped", reasonKey: "ocr.noImagePages" };

    patch(item.id, { status: "saving" });
    let target: string;
    try {
      target = await pickOutputPath(item.path, ctx.options.outputDir, (path) => api.pathExists({ path }), ctx.reserved);
    } catch (e) {
      return { status: "failed", reasonKey: "error.saveFailed", detail: message(e) };
    }
    ctx.reserved.add(pathKey(target));
    try {
      const saved = await api.saveDocumentAs({ docId: info.docId, path: target }, () => undefined);
      return { status: "done", output: saved.path };
    } catch (e) {
      return { status: "failed", reasonKey: api.isSeePdfError(e) ? api.errorKey(e) : "error.saveFailed", detail: message(e) };
    }
  } finally {
    if (live.docId === info.docId) live.docId = null;
    await api.closeDocument({ docId: info.docId }).catch(() => undefined);
  }
}

/** `open_document` with the password retry of F-01; an `Outcome` when the file cannot be processed. */
async function openForBatch(path: string, signal: AbortSignal): Promise<{ info: DocInfo } | Outcome> {
  let password: string | undefined;
  for (;;) {
    try {
      const info = await api.openDocument({ path, password });
      if (signal.aborted) {
        await api.closeDocument({ docId: info.docId }).catch(() => undefined);
        return { status: "cancelled" };
      }
      return { info };
    } catch (e) {
      if (signal.aborted) return { status: "cancelled" };
      if (api.isSeePdfError(e) && (e.code === "passwordRequired" || e.code === "passwordWrong")) {
        const entered = await askPasswordUnlessCancelled(path, e.code === "passwordWrong", signal);
        if (signal.aborted) return { status: "cancelled" };
        if (entered === null) return { status: "skipped", reasonKey: "batchOcr.reason.password" };
        password = entered;
        continue;
      }
      const missing = api.isSeePdfError(e) && e.code === "notFound";
      return { status: "failed", reasonKey: missing ? "error.fileMissing" : "error.openFailed", detail: message(e) };
    }
  }
}

/**
 * 암호 입력 over the batch dialog. A 취소 while it is up answers it with "no password" and takes it
 * down, so the queue never waits on a prompt that belongs to a cancelled run.
 */
function askPasswordUnlessCancelled(path: string, wrong: boolean, signal: AbortSignal): Promise<string | null> {
  const answer = askPassword(path.split(/[\\/]/).pop() || path, wrong);
  // `askPassword` pushed the prompt synchronously: the top of the stack is ours.
  const prompt = useDialogStore.getState().stack.at(-1);
  return new Promise((resolve) => {
    const onAbort = () => {
      const dismiss = prompt?.name === "password" ? (prompt.props.resolve as ((v: string | null) => void) | undefined) : undefined;
      dismiss?.(null);
      resolve(null);
    };
    if (signal.aborted) return onAbort();
    signal.addEventListener("abort", onAbort, { once: true });
    void answer.then((value) => {
      signal.removeEventListener("abort", onAbort);
      resolve(value);
    });
  });
}

/** The first searchable copy of the run — what 'Finder에서 보기' shows. */
export function firstOutput(items: BatchItem[]): string | null {
  return items.find((i) => i.status === "done" && i.output)?.output ?? null;
}

export function revealOutputs(): void {
  const path = firstOutput(useBatchOcr.getState().items);
  if (path) void api.revealInFileManager({ path }).catch(() => undefined);
}

/** A run that ends while its dialog is closed says so with a toast (the dialog shows it itself). */
function announce(cancelled: boolean): void {
  const visible = useDialogStore.getState().stack.some((e) => e.name === "batchOcr");
  if (visible) return;
  const s = summarize(useBatchOcr.getState().items);
  const windows = useAppStore.getState().os === "windows";
  const output = firstOutput(useBatchOcr.getState().items);
  toast(
    cancelled ? "batchOcr.cancelled" : "batchOcr.finished",
    { done: s.done, skipped: s.skipped, failed: s.failed },
    {
      tone: s.failed > 0 ? "danger" : cancelled ? "info" : "success",
      actions: output
        ? [{ labelKey: windows ? "export.revealExplorer" : "export.revealFinder", onSelect: revealOutputs }]
        : undefined,
    },
  );
}
