/**
 * 여러 파일 처리 (v0.3 pkg8, X1): the batch-OCR queue generalised to every per-file action the
 * engine already has — 워터마크·머리글·Bates (`add_stamp`), 압축 (`compress_estimate` →
 * `compress_apply`), 암호 설정 (`set_password`), 평면화 (`export_flattened`) and 이미지로 내보내기
 * (`export_images`).
 *
 * Per file, one at a time: `open_document` (beside the window's document, never through
 * `docStore`; an encrypted file asks for its password through the ordinary 암호 입력 prompt and
 * dismissing it skips the file) → the action → `save_document_as` to `<name>-<suffix>.pdf`
 * (beside the source or in the chosen folder; never over the source or anything that exists —
 * `(2)`, `(3)`…) when the action edits the document, or the action's own output file → always
 * `close_document`. Bates numbers continue from one file to the next.
 *
 * 취소 stops the queue after the current file: a running engine job (compress, flatten, images)
 * is cancelled and that file is closed without an output; nothing after it is opened.
 *
 * The queue model and naming rules are the batch-OCR ones (`src/ocr/batch/queue.ts`), with the
 * suffix as a parameter. The run lives here, not in the dialog, so it survives the dialog being
 * closed (the status bar shows it with its own ×).
 */
import { create } from "zustand";
import * as api from "../ipc/api";
import type { CompressPreset, DocId, DocInfo, JobEvent, JobId, StampSpec } from "../ipc/types";
import { askPassword, useDialogStore } from "../dialogs/dialogState";
import { localJobId, registerCanceller, useJobStore } from "../store/jobStore";
import { useAppStore } from "../store/appStore";
import { toast } from "../app/toastStore";
import {
  MAX_CANDIDATES, addPaths, baseName, isRunnable, patchItem, pathKey, removeItem, requeue, summarize,
  type BatchItem, type BatchStatus,
} from "../ocr/batch/queue";

export type { BatchItem, BatchStatus };

export type BatchAction = "stamp" | "compress" | "password" | "flatten" | "images";
export type BatchPhase = "idle" | "running" | "finished";

export interface BatchOptions {
  action: BatchAction;
  /** 워터마크 / 머리글·바닥글 / Bates: the 워터마크 dialog's spec (its `pages` is replaced by `'all'`). */
  stamp: StampSpec | null;
  compressDpi: CompressPreset;
  /** 암호 설정: the open password (it doubles as the owner password, every permission allowed). */
  password: string;
  imageFormat: "png" | "jpeg";
  imageDpi: number;
  /** `null` = beside each source file */
  outputDir: string | null;
}

export interface BatchState {
  items: BatchItem[];
  options: BatchOptions;
  phase: BatchPhase;
  runDone: number;
  runTotal: number;
  cancelling: boolean;
  jobId: number | null;
}

/** The `<name>-<suffix>.pdf` each action writes. */
export const SUFFIX: Record<Exclude<BatchAction, "images">, string> = {
  stamp: "-stamped",
  compress: "-compressed",
  password: "-protected",
  flatten: "-flat",
};

const DEFAULT_OPTIONS: BatchOptions = {
  action: "compress", stamp: null, compressDpi: 150, password: "", imageFormat: "png", imageDpi: 150, outputDir: null,
};

function initial(): BatchState {
  return { items: [], options: { ...DEFAULT_OPTIONS }, phase: "idle", runDone: 0, runTotal: 0, cancelling: false, jobId: null };
}

export const useBatch = create<BatchState>(() => initial());

let nextId = 0;
const newId = () => ++nextId;

const live: { controller: AbortController | null; run: Promise<void> | null; engineJob: JobId | null } = {
  controller: null,
  run: null,
  engineJob: null,
};

// ---------------------------------------------------------------------------
// Output names
// ---------------------------------------------------------------------------

function splitPath(path: string): { dir: string; sep: string; stem: string; ext: string } {
  const cut = Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\"));
  // A file at a root keeps the root's separator (as `flows.dirName`): `C:\scan.pdf` is in `C:\` —
  // `C:` alone is the drive's *current directory* on Windows — and `/x.pdf` is in `/`.
  const parent = cut >= 0 ? path.slice(0, cut) : "";
  const dir = cut >= 0 && (parent === "" || /^[A-Za-z]:$/.test(parent)) ? path.slice(0, cut + 1) : parent;
  const sep = cut >= 0 ? path[cut] : "/";
  const name = cut >= 0 ? path.slice(cut + 1) : path;
  const m = /\.pdf$/i.exec(name);
  return m ? { dir, sep, stem: name.slice(0, m.index), ext: m[0] } : { dir, sep, stem: name, ext: ".pdf" };
}

function joinPath(dir: string, sep: string, name: string): string {
  if (!dir) return name;
  return /[\\/]$/.test(dir) ? `${dir}${name}` : `${dir}${sep}${name}`;
}

function folderOf(folder: string): { dir: string; sep: string } {
  const sep = folder.includes("\\") && !folder.includes("/") ? "\\" : "/";
  let dir = folder;
  while (dir.length > 1 && /[\\/]$/.test(dir) && !/^[A-Za-z]:[\\/]$/.test(dir)) dir = dir.slice(0, -1);
  return { dir, sep };
}

/** `보고서.pdf` → `보고서-compressed.pdf`, then `보고서-compressed (2).pdf`, … (beside it or in `folder`). */
export function outputCandidate(source: string, folder: string | null, suffix: string, n = 1, ext?: string): string {
  const src = splitPath(source);
  const target = folder ? folderOf(folder) : { dir: src.dir, sep: src.sep };
  return joinPath(target.dir, target.sep, `${src.stem}${suffix}${n > 1 ? ` (${n})` : ""}${ext ?? src.ext}`);
}

/** The first free output: never the source, never a file that exists, never one this run claimed. */
export async function pickOutput(
  source: string,
  folder: string | null,
  suffix: string,
  exists: (path: string) => Promise<boolean>,
  reserved: ReadonlySet<string>,
  ext?: string,
): Promise<string> {
  const sourceKey = pathKey(source);
  for (let n = 1; n <= MAX_CANDIDATES; n++) {
    const candidate = outputCandidate(source, folder, suffix, n, ext);
    const key = pathKey(candidate);
    if (key === sourceKey || reserved.has(key)) continue;
    if (await exists(candidate)) continue;
    return candidate;
  }
  throw new Error(`no free output name for ${source}`);
}

// ---------------------------------------------------------------------------
// List editing (the dialog)
// ---------------------------------------------------------------------------

export function addFiles(paths: string[]): void {
  const s = useBatch.getState();
  useBatch.setState({ items: addPaths(s.items, paths, newId), phase: s.phase === "finished" ? "idle" : s.phase });
}

export function removeFile(id: number): void {
  useBatch.setState({ items: removeItem(useBatch.getState().items, id) });
}

export function clearFiles(): void {
  if (useBatch.getState().phase === "running") return;
  useBatch.setState({ items: [], phase: "idle" });
}

export function setOptions(patch: Partial<BatchOptions>): void {
  useBatch.setState({ options: { ...useBatch.getState().options, ...patch } });
}

export function resetBatch(): void {
  if (useBatch.getState().phase === "running") return;
  useBatch.setState(initial());
}

/** Why 시작 is disabled for the current options, or `null` when they are complete. */
export function optionsProblem(o: BatchOptions): string | null {
  if (o.action === "stamp" && !o.stamp) return "batch.need.stamp";
  if (o.action === "password" && !o.password) return "batch.need.password";
  return null;
}

function patch(id: number, p: Partial<Omit<BatchItem, "id">>): void {
  useBatch.setState({ items: patchItem(useBatch.getState().items, id, p) });
}

function itemOf(id: number): BatchItem | undefined {
  return useBatch.getState().items.find((i) => i.id === id);
}

function message(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

// ---------------------------------------------------------------------------
// The run
// ---------------------------------------------------------------------------

type Outcome = { status: BatchStatus } & Partial<Omit<BatchItem, "id" | "status">>;

export function startBatch(): Promise<void> {
  const s = useBatch.getState();
  if (s.phase === "running") return live.run ?? Promise.resolve();
  const queue = s.items.filter(isRunnable).map((i) => i.id);
  if (queue.length === 0 || optionsProblem(s.options)) return Promise.resolve();
  const run = runQueue(queue).finally(() => {
    if (live.run === run) live.run = null;
  });
  live.run = run;
  return run;
}

/** 취소: no further file is opened; a running engine job of the current file is cancelled. */
export async function cancelBatch(): Promise<void> {
  const controller = live.controller;
  if (!controller) return;
  useBatch.setState({ cancelling: true });
  controller.abort();
  if (live.engineJob !== null) await api.cancelJob({ jobId: live.engineJob }).catch(() => false);
  await live.run?.catch(() => undefined);
}

export function isBatchRunning(): boolean {
  return live.controller !== null;
}

async function runQueue(queue: number[]): Promise<void> {
  const controller = new AbortController();
  live.controller = controller;
  const { signal } = controller;
  const { options } = useBatch.getState();
  const reserved = new Set<string>();
  /** Bates numbering continues across files */
  const bates = { next: options.stamp?.batesStart ?? 1 };

  const jobId = localJobId();
  useJobStore.getState().start({ id: jobId, kind: "batch", labelKey: "batch.running", total: queue.length });
  const unregister = registerCanceller(jobId, () => void cancelBatch());
  useBatch.setState({
    items: requeue(useBatch.getState().items, new Set(queue)),
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
        outcome = await processFile(item, { options, reserved, signal, bates });
      } catch (e) {
        outcome = { status: "failed", reasonKey: "error.generic", detail: message(e) };
      }
      patch(id, outcome);
      if (outcome.status === "cancelled") break;
      finished += 1;
      useBatch.setState({ runDone: finished });
      useJobStore.getState().update(jobId, { done: finished, note: item.name });
    }
  } finally {
    unregister();
    const cancelled = signal.aborted;
    useJobStore.getState().update(jobId, { state: cancelled ? "cancelled" : "done", done: finished });
    if (live.controller === controller) live.controller = null;
    useBatch.setState({ phase: "finished", cancelling: false });
    announce(cancelled);
  }
}

interface FileContext {
  options: BatchOptions;
  reserved: Set<string>;
  signal: AbortSignal;
  bates: { next: number };
}

async function processFile(item: BatchItem, ctx: FileContext): Promise<Outcome> {
  const { signal, options } = ctx;
  patch(item.id, { status: "opening" });
  const opened = await openForBatch(item.path, signal);
  if ("status" in opened) return opened;
  const info = opened.info;
  try {
    if (signal.aborted) return { status: "cancelled" };
    patch(item.id, { status: "running", done: 0, total: info.pageCount });
    const exists = (path: string) => api.pathExists({ path });

    if (options.action === "images") {
      const ext = options.imageFormat === "png" ? "png" : "jpg";
      const src = splitPath(item.path);
      const folder = options.outputDir ? folderOf(options.outputDir) : { dir: src.dir, sep: src.sep };
      // `<stem>-001.png` must not overwrite anything: probe the first file of the set.
      let base = src.stem;
      for (let n = 2; n <= MAX_CANDIDATES; n++) {
        const first = joinPath(folder.dir, folder.sep, `${base}-001.${ext}`);
        if (!ctx.reserved.has(pathKey(first)) && !(await exists(first))) break;
        base = `${src.stem} (${n})`;
      }
      ctx.reserved.add(pathKey(joinPath(folder.dir, folder.sep, `${base}-001.${ext}`)));
      const end = await runEngineJob(
        (on) =>
          api.exportImages(
            {
              docId: info.docId, pages: [], format: options.imageFormat, dpi: options.imageDpi,
              quality: options.imageFormat === "jpeg" ? 85 : undefined, outDir: folder.dir || ".", baseName: base,
            },
            on,
          ),
        signal,
        (done) => patch(item.id, { done }),
      );
      if (end.type !== "done") return jobOutcome(end);
      return { status: "done", output: end.outputs?.[0] ?? joinPath(folder.dir, folder.sep, `${base}-001.${ext}`) };
    }

    const target = await pickOutput(item.path, options.outputDir, SUFFIX[options.action], exists, ctx.reserved).catch(
      () => null,
    );
    if (!target) return { status: "failed", reasonKey: "error.saveFailed" };
    ctx.reserved.add(pathKey(target));

    switch (options.action) {
      case "stamp": {
        const spec = options.stamp!;
        const batesAt = ctx.bates.next;
        const numbered = spec.batesStart !== undefined ? { ...spec, batesStart: batesAt } : spec;
        await api.addStamp({ docId: info.docId, spec: { ...numbered, pages: "all" } });
        if (spec.batesStart !== undefined) ctx.bates.next = batesAt + info.pageCount;
        return await saveAs(info.docId, target);
      }
      case "compress": {
        const end = await runEngineJob(
          (on) => api.compressEstimate({ docId: info.docId, options: { targetDpi: options.compressDpi } }, on),
          signal,
          (done) => patch(item.id, { done }),
        );
        if (end.type !== "done") return jobOutcome(end);
        const report = end.report;
        if (!report || report.imagesDownsampled === 0 || report.afterBytes >= report.beforeBytes) {
          if (report) await api.compressDiscard({ docId: info.docId, token: report.token }).catch(() => undefined);
          ctx.reserved.delete(pathKey(target));
          return { status: "skipped", reasonKey: "batch.reason.noGain" };
        }
        await api.compressApply({ docId: info.docId, token: report.token });
        return await saveAs(info.docId, target);
      }
      case "password": {
        patch(item.id, { status: "saving" });
        await api.setPassword({
          docId: info.docId, outPath: target, userPassword: options.password, ownerPassword: options.password,
          permissions: { print: true, extractText: true, modify: true, annotate: true },
        });
        return { status: "done", output: target };
      }
      case "flatten": {
        const end = await runEngineJob(
          (on) => api.exportFlattened({ docId: info.docId, outPath: target, annotations: true, forms: true }, on),
          signal,
        );
        if (end.type !== "done") return jobOutcome(end);
        return { status: "done", output: target };
      }
    }
  } catch (e) {
    if (signal.aborted) return { status: "cancelled" };
    return { status: "failed", reasonKey: api.isSeePdfError(e) ? api.errorKey(e) : "error.generic", detail: message(e) };
  } finally {
    await api.closeDocument({ docId: info.docId }).catch(() => undefined);
  }
  return { status: "failed", reasonKey: "error.generic" };

  async function saveAs(docId: DocId, target: string): Promise<Outcome> {
    patch(item.id, { status: "saving" });
    try {
      const saved = await api.saveDocumentAs({ docId, path: target }, () => undefined);
      return { status: "done", output: saved.path };
    } catch (e) {
      return { status: "failed", reasonKey: api.isSeePdfError(e) ? api.errorKey(e) : "error.saveFailed", detail: message(e) };
    }
  }
}

type JobEnd = Extract<JobEvent, { type: "done" | "cancelled" | "error" }>;

/** Runs one engine job to its terminal event; an abort cancels it (`cancel_job`). */
function runEngineJob(
  start: (on: (e: JobEvent) => void) => Promise<JobId>,
  signal: AbortSignal,
  onProgress?: (done: number) => void,
): Promise<JobEnd> {
  return new Promise<JobEnd>((resolve, reject) => {
    let jobId: JobId | null = null;
    let settled = false;
    const finish = (e: JobEnd) => {
      if (settled) return;
      settled = true;
      if (live.engineJob === jobId) live.engineJob = null;
      signal.removeEventListener("abort", onAbort);
      resolve(e);
    };
    const onAbort = () => {
      if (jobId !== null) void api.cancelJob({ jobId }).catch(() => false);
    };
    signal.addEventListener("abort", onAbort, { once: true });
    start((e) => {
      if (e.type === "progress") onProgress?.(e.done);
      else if (e.type === "done" || e.type === "cancelled" || e.type === "error") finish(e);
    })
      .then((id) => {
        jobId = id;
        if (!settled) live.engineJob = id;
        if (signal.aborted) onAbort();
      })
      .catch((e) => {
        settled = true;
        signal.removeEventListener("abort", onAbort);
        reject(e);
      });
  });
}

function jobOutcome(end: JobEnd): Outcome {
  if (end.type === "cancelled") return { status: "cancelled" };
  if (end.type === "error") return { status: "failed", reasonKey: api.errorKey(end.error), detail: end.error.message };
  return { status: "done" };
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

function askPasswordUnlessCancelled(path: string, wrong: boolean, signal: AbortSignal): Promise<string | null> {
  const answer = askPassword(baseName(path), wrong);
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

export function firstOutput(items: BatchItem[]): string | null {
  return items.find((i) => i.status === "done" && i.output)?.output ?? null;
}

export function revealOutputs(): void {
  const path = firstOutput(useBatch.getState().items);
  if (path) void api.revealInFileManager({ path }).catch(() => undefined);
}

function announce(cancelled: boolean): void {
  if (useDialogStore.getState().stack.some((e) => e.name === "batch")) return;
  const s = summarize(useBatch.getState().items);
  const windows = useAppStore.getState().os === "windows";
  const output = firstOutput(useBatch.getState().items);
  toast(
    cancelled ? "batch.cancelled" : "batch.finished",
    { done: s.done, skipped: s.skipped, failed: s.failed },
    {
      tone: s.failed > 0 ? "danger" : cancelled ? "info" : "success",
      actions: output
        ? [{ labelKey: windows ? "export.revealExplorer" : "export.revealFinder", onSelect: revealOutputs }]
        : undefined,
    },
  );
}
