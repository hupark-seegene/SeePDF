/**
 * 여러 파일에서 검색 (P2): one query over a list of PDFs on disk, as **one** job.
 *
 * Per file: `open_document` beside the window's document (never through `docStore`; an encrypted
 * file asks 암호 입력 and dismissing it skips the file, like 여러 파일 OCR) → `search_start` from the
 * first page, collecting every hit with its page and ±40-character context → `close_document`, on
 * success, failure and cancel alike. The window's own document, when it is in the list, is searched
 * where it is (its unsaved edits included) and never closed.
 *
 * The run lives here, not in the dialog: the password prompt stacks on the dialog (the host renders
 * only the top entry), and the dialog may be closed while the job carries on in the status bar
 * (× cancels). 취소 cancels the running `search_start` and closes the file it was reading.
 *
 * A result opens its file through the ordinary open path (the unsaved-changes guard, the password
 * the search was given) and becomes the 검색 panel's state — the same highlights, the clicked hit
 * current and scrolled to.
 */
import { create } from "zustand";
import * as api from "../ipc/api";
import type { DocId, DocInfo, EngineError, JobId, SearchHit } from "../ipc/types";
import { askPassword, closeDialog, openDialog, useDialogStore } from "../dialogs/dialogState";
import { localJobId, registerCanceller, useJobStore } from "../store/jobStore";
import { useDocStore } from "../store/docStore";
import { toast } from "../app/toastStore";
import { mergeHits, useSearchStore } from "../viewer/search/SearchController";

export type FileStatus = "queued" | "opening" | "searching" | "done" | "skipped" | "failed" | "cancelled";
export type SearchPhase = "idle" | "running" | "finished";

export interface SearchFile {
  id: number;
  path: string;
  name: string;
  status: FileStatus;
  /** document order */
  hits: SearchHit[];
  /** i18n key: why the file was skipped or failed */
  reasonKey?: string;
  detail?: string;
  /** the password that opened it during the search — 열기 tries it first */
  password?: string;
}

export interface SearchOptions {
  matchCase: boolean;
  wholeWord: boolean;
}

export interface MultiSearchState {
  files: SearchFile[];
  query: string;
  options: SearchOptions;
  phase: SearchPhase;
  /** files finished / files in the current run */
  runDone: number;
  runTotal: number;
  cancelling: boolean;
  /** what the results in `files` answer (the field may already say something else) */
  searched: ({ query: string } & SearchOptions) | null;
  jobId: JobId | null;
}

function initial(): MultiSearchState {
  return {
    files: [], query: "", options: { matchCase: false, wholeWord: false }, phase: "idle",
    runDone: 0, runTotal: 0, cancelling: false, searched: null, jobId: null,
  };
}

export const useMultiSearch = create<MultiSearchState>(() => initial());

let nextId = 0;

/** What must be released however the run ends. */
const live: { controller: AbortController | null; docId: DocId | null; run: Promise<void> | null } = {
  controller: null,
  docId: null,
  run: null,
};

export function baseName(path: string): string {
  return path.split(/[\\/]/).pop() || path;
}

/** macOS and Windows paths compare without case; `/` and `\` are the same separator. */
export function pathKey(path: string): string {
  return path.replace(/\\/g, "/").toLowerCase();
}

function message(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

// ---------------------------------------------------------------------------
// The list (the dialog)
// ---------------------------------------------------------------------------

/** 파일 추가…: new paths join the end; a path already listed is not listed twice. */
export function addFiles(paths: string[]): void {
  const s = useMultiSearch.getState();
  const known = new Set(s.files.map((f) => pathKey(f.path)));
  const added: SearchFile[] = [];
  for (const path of paths) {
    const key = pathKey(path);
    if (known.has(key) || !/\.pdf$/i.test(path)) continue;
    known.add(key);
    added.push({ id: ++nextId, path, name: baseName(path), status: "queued", hits: [] });
  }
  if (added.length) useMultiSearch.setState({ files: [...s.files, ...added] });
}

/** 폴더 추가…: every PDF under the folder, sub-folders included. Resolves with how many joined. */
export async function addFolder(dir: string): Promise<number> {
  const before = useMultiSearch.getState().files.length;
  try {
    addFiles(await api.listPdfFiles({ dir, recursive: true }));
  } catch (e) {
    toast("multiSearch.folderFailed", undefined, { tone: "danger", detail: message(e) });
  }
  return useMultiSearch.getState().files.length - before;
}

export function removeFile(id: number): void {
  if (useMultiSearch.getState().phase === "running") return;
  useMultiSearch.setState({ files: useMultiSearch.getState().files.filter((f) => f.id !== id) });
}

export function clearFiles(): void {
  if (useMultiSearch.getState().phase === "running") return;
  useMultiSearch.setState({ files: [], phase: "idle", searched: null });
}

export function setQuery(query: string): void {
  useMultiSearch.setState({ query });
}

export function setOptions(patch: Partial<SearchOptions>): void {
  useMultiSearch.setState({ options: { ...useMultiSearch.getState().options, ...patch } });
}

/** Test seam, and a new window. Never mid-run. */
export function resetMultiSearch(): void {
  if (useMultiSearch.getState().phase === "running") return;
  useMultiSearch.setState(initial());
}

function patch(id: number, p: Partial<Omit<SearchFile, "id">>): void {
  useMultiSearch.setState({
    files: useMultiSearch.getState().files.map((f) => (f.id === id ? { ...f, ...p } : f)),
  });
}

/** Total hits and the files that have any — the dialog's summary line. */
export function summarize(files: SearchFile[]): { hits: number; withHits: number; skipped: number; failed: number } {
  return {
    hits: files.reduce((n, f) => n + f.hits.length, 0),
    withHits: files.filter((f) => f.hits.length > 0).length,
    skipped: files.filter((f) => f.status === "skipped").length,
    failed: files.filter((f) => f.status === "failed").length,
  };
}

// ---------------------------------------------------------------------------
// The run
// ---------------------------------------------------------------------------

type Outcome = Partial<Omit<SearchFile, "id">> & { status: FileStatus };

/** 검색: every listed file, in list order, as one status-bar job. Resolves when it is over. */
export function startSearch(): Promise<void> {
  const s = useMultiSearch.getState();
  if (s.phase === "running") return live.run ?? Promise.resolve();
  const query = s.query.trim();
  if (!query || s.files.length === 0) return Promise.resolve();
  const run = runQueue(query, s.options).finally(() => {
    if (live.run === run) live.run = null;
  });
  live.run = run;
  return run;
}

/** 취소 (the dialog or the status bar's ×): stop the file being read, close it, stop the run. */
export async function cancelSearch(): Promise<void> {
  const controller = live.controller;
  if (!controller) return;
  useMultiSearch.setState({ cancelling: true });
  controller.abort();
  await live.run?.catch(() => undefined);
}

export function isSearchRunning(): boolean {
  return live.controller !== null;
}

async function runQueue(query: string, options: SearchOptions): Promise<void> {
  const controller = new AbortController();
  live.controller = controller;
  const { signal } = controller;
  const ids = useMultiSearch.getState().files.map((f) => f.id);

  const jobId = localJobId();
  useJobStore.getState().start({ id: jobId, kind: "multiSearch", labelKey: "multiSearch.running", total: ids.length });
  const unregister = registerCanceller(jobId, () => void cancelSearch());
  useMultiSearch.setState({
    files: useMultiSearch.getState().files.map((f) => ({
      ...f, status: "queued" as FileStatus, hits: [], reasonKey: undefined, detail: undefined,
    })),
    phase: "running",
    runDone: 0,
    runTotal: ids.length,
    cancelling: false,
    searched: { query, ...options },
    jobId,
  });

  let finished = 0;
  try {
    for (const id of ids) {
      if (signal.aborted) break;
      const file = useMultiSearch.getState().files.find((f) => f.id === id);
      if (!file) continue;
      let outcome: Outcome;
      try {
        outcome = await searchFile(file, query, options, signal);
      } catch (e) {
        outcome = { status: "failed", reasonKey: "error.generic", detail: message(e) };
      }
      patch(id, outcome);
      if (outcome.status === "cancelled") break;
      finished += 1;
      useMultiSearch.setState({ runDone: finished });
      useJobStore.getState().update(jobId, { done: finished, note: file.name });
    }
  } finally {
    unregister();
    const cancelled = signal.aborted;
    // whatever the run did not reach stays 대기, a cancelled one says so
    if (cancelled) {
      useMultiSearch.setState({
        files: useMultiSearch.getState().files.map((f) =>
          f.status === "opening" || f.status === "searching" ? { ...f, status: "cancelled" } : f),
      });
    }
    useJobStore.getState().update(jobId, { state: cancelled ? "cancelled" : "done", done: finished });
    if (live.controller === controller) live.controller = null;
    useMultiSearch.setState({ phase: "finished", cancelling: false, jobId: null });
    announce(cancelled);
  }
}

async function searchFile(file: SearchFile, query: string, options: SearchOptions, signal: AbortSignal): Promise<Outcome> {
  // The window's own document is searched where it is: its unsaved edits count, and it stays open.
  const current = useDocStore.getState().info;
  let docId: DocId;
  let owned = false;
  let password: string | undefined;
  if (current?.path && pathKey(current.path) === pathKey(file.path)) {
    docId = current.docId;
  } else {
    patch(file.id, { status: "opening" });
    const opened = await openForSearch(file.path, signal);
    if ("status" in opened) return opened;
    docId = opened.info.docId;
    password = opened.password;
    owned = true;
    live.docId = docId;
  }
  try {
    if (signal.aborted) return { status: "cancelled" };
    patch(file.id, { status: "searching", hits: [] });
    const result = await searchDocument(docId, query, options, signal, (hits) => patch(file.id, { hits }));
    if (result === "cancelled" || signal.aborted) return { status: "cancelled" };
    if ("error" in result) {
      return { status: "failed", reasonKey: "multiSearch.reason.searchFailed", detail: result.error.message };
    }
    return { status: "done", hits: result.hits, password };
  } finally {
    if (owned) {
      if (live.docId === docId) live.docId = null;
      await api.closeDocument({ docId }).catch(() => undefined);
    }
  }
}

/**
 * One `search_start` pass from the first page, to the end. `onHits` sees the document-ordered list
 * grow as pages stream in; an abort cancels the engine job.
 */
function searchDocument(
  docId: DocId,
  query: string,
  options: SearchOptions,
  signal: AbortSignal,
  onHits: (hits: SearchHit[]) => void,
): Promise<{ hits: SearchHit[] } | { error: EngineError } | "cancelled"> {
  return new Promise((resolve) => {
    let hits: SearchHit[] = [];
    let jobId: JobId | null = null;
    let settled = false;
    const settle = (value: { hits: SearchHit[] } | { error: EngineError } | "cancelled") => {
      if (settled) return;
      settled = true;
      signal.removeEventListener("abort", onAbort);
      resolve(value);
    };
    const onAbort = () => {
      if (jobId !== null) void api.cancelJob({ jobId }).catch(() => false);
      settle("cancelled");
    };
    if (signal.aborted) return settle("cancelled");
    signal.addEventListener("abort", onAbort, { once: true });
    api
      .searchStart({ docId, query, matchCase: options.matchCase, wholeWord: options.wholeWord, fromPage: 0 }, (e) => {
        if (settled) return;
        if (e.type === "page") {
          hits = mergeHits(hits, e.page, e.hits);
          onHits(hits);
        } else if (e.type === "done") settle({ hits });
        else if (e.type === "cancelled") settle("cancelled");
        else settle({ error: e.error });
      })
      .then((id) => {
        jobId = id;
        // aborted while the command was on its way: the job exists now, so stop it
        if (settled && signal.aborted) void api.cancelJob({ jobId: id }).catch(() => false);
      })
      .catch((e) => settle({ error: { code: api.isSeePdfError(e) ? e.code : "pdfium", message: message(e) } }));
  });
}

/** `open_document` with the password retry of F-01; an `Outcome` when the file cannot be searched. */
async function openForSearch(
  path: string,
  signal: AbortSignal,
): Promise<{ info: DocInfo; password?: string } | Outcome> {
  let password: string | undefined;
  for (;;) {
    try {
      const info = await api.openDocument({ path, password });
      if (signal.aborted) {
        await api.closeDocument({ docId: info.docId }).catch(() => undefined);
        return { status: "cancelled" };
      }
      return { info, password };
    } catch (e) {
      if (signal.aborted) return { status: "cancelled" };
      if (api.isSeePdfError(e) && (e.code === "passwordRequired" || e.code === "passwordWrong")) {
        const entered = await askPasswordUnlessCancelled(path, e.code === "passwordWrong", signal);
        if (signal.aborted) return { status: "cancelled" };
        if (entered === null) return { status: "skipped", reasonKey: "multiSearch.reason.password" };
        password = entered;
        continue;
      }
      const missing = api.isSeePdfError(e) && e.code === "notFound";
      return { status: "failed", reasonKey: missing ? "error.fileMissing" : "error.openFailed", detail: message(e) };
    }
  }
}

/** 암호 입력 over the dialog; a 취소 of the run answers it "no password" and takes it down. */
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

/** A run that ends while its dialog is closed says so with a toast. */
function announce(cancelled: boolean): void {
  if (useDialogStore.getState().stack.some((e) => e.name === "multiSearch")) return;
  const s = summarize(useMultiSearch.getState().files);
  toast(
    cancelled ? "multiSearch.cancelled" : "multiSearch.finished",
    { hits: s.hits, files: s.withHits },
    {
      tone: cancelled ? "info" : "success",
      actions: [{ labelKey: "multiSearch.showResults", onSelect: () => openDialog("multiSearch") }],
    },
  );
}

// ---------------------------------------------------------------------------
// Opening a result
// ---------------------------------------------------------------------------

/**
 * A result was clicked: its file becomes the window's document (the ordinary open path — the
 * unsaved-changes guard first, the search's password tried before asking) and the hits become the
 * 검색 panel's, with hit `index` current and scrolled to. `false` when the open did not happen.
 */
export async function openResult(fileId: number, index: number): Promise<boolean> {
  const s = useMultiSearch.getState();
  const file = s.files.find((f) => f.id === fileId);
  const searched = s.searched;
  if (!file || !searched || !file.hits[index]) return false;
  let info = useDocStore.getState().info;
  if (!info?.path || pathKey(info.path) !== pathKey(file.path)) {
    const { openPath } = await import("../dialogs/flows");
    info = await openPath(file.path, { password: file.password });
    if (!info) return false;
  }
  closeDialog("multiSearch");
  useSearchStore.getState().show(
    info.docId,
    searched.query,
    { matchCase: searched.matchCase, wholeWord: searched.wholeWord },
    file.hits,
    index,
  );
  return true;
}
