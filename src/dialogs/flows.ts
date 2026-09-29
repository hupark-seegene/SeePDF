/**
 * The file flows that are more than one command: open (with password retry and reading-position
 * restore), save / save as (with the read-only fallback), the unsaved-changes guard, multi-file
 * drop, and the page-operation helper every organizer action goes through.
 *
 * Everything here is dynamically imported (`import("../dialogs/flows")`) so none of it — and none of
 * the dialogs it opens — reaches the entry chunk.
 */
import * as api from "../ipc/api";
import { useDocStore } from "../store/docStore";
import { useViewStore } from "../store/viewStore";
import { useAppStore } from "../store/appStore";
import { useJobStore } from "../store/jobStore";
import { usePagesStore } from "../store/pagesStore";
import { toast } from "../app/toastStore";
import { askChoice, askConfirm, askMultipleFiles, askPassword, askUnsaved, closeDialog, openDialog } from "./dialogState";
import { windowLabel } from "../ipc/env";
import { autosave, markRecovered, recoveredEntry, settleRecovered } from "../app/autosave";
import { whenEditsSettled } from "../annot/dragGate";
import { editLeaveGuard } from "../tools/commands";
import type { DocId, DocInfo, PageIndex, PageOp, ProblemReport, RecentEntry, RecoveryEntry } from "../ipc/types";

// ---------------------------------------------------------------------------
// Opening
// ---------------------------------------------------------------------------

/** ⌘O and the welcome screen's 파일 열기… — one or many files. */
export async function openFileFlow(): Promise<void> {
  const picked = await api.openFileDialog({ multiple: true });
  if (picked?.length) await openPaths(picked);
}

/** A drop, an `open-file` event or a multi-selection in the picker. */
export async function openPaths(paths: string[]): Promise<void> {
  if (paths.length === 0) return;
  if (paths.length === 1) {
    await openPath(paths[0]);
    return;
  }
  const answer = await askMultipleFiles(paths);
  if (answer === "merge") await mergePaths(paths.map((path) => ({ path })));
  else if (answer === "separate") await openSeparately(paths);
}

async function openSeparately(paths: string[]): Promise<void> {
  await openPath(paths[0]);
  for (const path of paths.slice(1)) {
    // v0.3 H8: a file another window already shows is focused, not opened a second time
    if (await focusedElsewhere(path)) continue;
    await api.openInNewWindow({ path }).catch(() => undefined);
  }
}

/**
 * Open one file. Retries the password prompt in place (F-01) and restores the stored reading
 * position when 마지막으로 본 위치 기억 is on. `recovery`: the file is a 복구 copy (P1-8) — it is
 * marked so 저장 asks for a location, and it stays out of 최근 항목.
 */
export async function openPath(
  path: string,
  opts: { guard?: boolean; recovery?: RecoveryEntry; password?: string } = {},
): Promise<DocInfo | null> {
  // v0.3 H8: one window per file — a file another window shows is brought to the front there
  // (recents, the picker, drops and the OS's open-file events all come through here)
  if (!opts.recovery && (await focusedElsewhere(path))) return null;
  if (opts.guard !== false && !(await confirmLeaveDocument())) return null;
  const docs = useDocStore.getState();
  const previous = docs.info;
  // `password`: one the user already gave for this file (여러 파일에서 검색, P2) — tried first,
  // and a wrong one still falls back to the prompt
  let password: string | undefined = opts.password;
  for (;;) {
    // a 복구 copy opens under the original document's name, not `<uuid>.pdf` (Stage 8)
    const info = await docs.open(path, password, opts.recovery?.name);
    if (info) {
      if (previous && previous.docId !== info.docId) await releaseReplaced(previous);
      if (opts.recovery) markRecovered(info.docId, opts.recovery);
      await afterOpen(info);
      return info;
    }
    const error = useDocStore.getState().error;
    if (error?.code === "passwordRequired" || error?.code === "passwordWrong") {
      const entered = await askPassword(baseName(path), error.code === "passwordWrong");
      if (entered === null) {
        // 취소 on the password prompt: whatever was on screen stays, loaded and current
        useDocStore.setState({ status: previous ? "ready" : "empty", error: null });
        return null;
      }
      password = entered;
      continue;
    }
    // v0.3 pkg5 (H3): PDF 아님 / 손상된 PDF / 메모리 부족 when the engine could tell why
    toast(api.openErrorKey(error), undefined, {
      tone: "danger",
      detail: error?.message,
    });
    // A failed open replaces nothing: `docs.open` kept `docId` / `info`, so the previous document
    // is still displayed and still open in the engine — it must not be closed, and the store goes
    // back to `ready` (the toast carries the error; the welcome banner is only for an empty window).
    if (previous) useDocStore.setState({ status: "ready", error: null });
    return null;
  }
}

/**
 * The document `openPath` (or `mergePaths`) just replaced in this window. The unsaved gate already
 * dealt with it, so its 복구 copy goes (cleared first, while the engine can still name it), and then
 * the engine lets go of it — before this, every opened file stayed loaded until the app quit.
 *
 * Multi-window: `close_document` has no guard of its own (it closes whatever id it is given, even if
 * another window's `window_bind_document` still points at it). Closing here is safe because
 * `previous` is exactly the document this window is bound to (`docStore` binds on open / adopt /
 * close), and no document is shared between windows: every window — `open_in_new_window` included —
 * opens its own file with `open_document` (or builds one with `merge_documents`), which allocates a
 * fresh docId each time. If windows ever share a docId, this must first ask the backend whether
 * another window is still bound to it.
 */
async function releaseReplaced(previous: DocInfo): Promise<void> {
  await autosave.clear(previous.docId);
  await api.closeDocument({ docId: previous.docId }).catch(() => undefined);
}

async function afterOpen(info: DocInfo): Promise<void> {
  usePagesStore.getState().reset();
  const app = useAppStore.getState();
  const view = useViewStore.getState();
  // a new document always starts at its first page; the stored position then moves it (F-01)
  view.goToPage(0);
  const entry = app.recents.find((r) => r.path === info.path);
  if (entry && app.settings?.restorePosition !== false) {
    view.setLayout(entry.layout);
    if (entry.zoomPercent > 0) view.setZoom(entry.zoomPercent);
    if (entry.lastPage > 0 && entry.lastPage < info.pageCount) view.goToPage(entry.lastPage);
  } else if (app.settings) {
    view.setLayout(app.settings.defaultLayout);
    const zoom = app.settings.defaultZoom;
    if (typeof zoom === "number") view.setZoom(zoom);
    else view.setZoomMode(zoom === "actual" ? "actual" : zoom, zoom === "actual" ? 100 : undefined);
  }
  // 뒤로 must not lead back into the previous document's pages
  useViewStore.getState().resetHistory();
  await touchRecent(info);
}

/** Write the current reading position back into the recents list (IPC_CONTRACT §11). */
export async function touchRecent(info: DocInfo): Promise<void> {
  if (!info.path || recoveredEntry(info.docId)) return;
  const view = useViewStore.getState();
  const app = useAppStore.getState();
  const previous = app.recents.find((r) => r.path === info.path);
  let thumbId = previous?.thumbId ?? null;
  if (!thumbId) {
    thumbId = (await api.writeRecentThumbnail({ docId: info.docId }).catch(() => null))?.thumbId ?? null;
  }
  const entry: RecentEntry = {
    path: info.path,
    name: info.name,
    dir: dirName(info.path),
    pages: info.pageCount,
    bytes: info.bytes,
    lastOpened: new Date().toISOString(),
    lastPage: view.currentPage,
    zoomPercent: view.zoomPercent,
    layout: view.layout,
    pinned: previous?.pinned ?? false,
    thumbId,
  };
  await api.updateRecent({ entry }).catch(() => undefined);
  await useAppStore.getState().refreshRecents();
}

/**
 * 파일 합치기 — the merged document becomes the window's document (it has no path yet). It replaces
 * the current document exactly like `openPath` does: the leave guard (pending 편집 marks, then
 * 저장 / 저장 안 함 / 취소) runs first, and the replaced document is released only once the merge
 * has succeeded — a failed merge leaves it displayed and open.
 */
export async function mergePaths(
  inputs: { path: string; range?: string }[],
  opts: { guard?: boolean } = {},
): Promise<DocInfo | null> {
  if (opts.guard !== false && !(await confirmLeaveDocument())) return null;
  const previous = useDocStore.getState().info;
  let merged: Awaited<ReturnType<typeof api.mergeDocuments>>;
  try {
    merged = await api.mergeDocuments({ inputs });
  } catch (e) {
    toast("error.openFailed", undefined, { tone: "danger", detail: message(e) });
    return null;
  }
  const { info, warnings } = merged;
  usePagesStore.getState().reset();
  useDocStore.getState().adopt(info);
  // like `afterOpen`: a new document starts at its first page, and 뒤로 must not lead back into
  // the replaced document's pages (real-app QA: the merge kept the previous document's page)
  const view = useViewStore.getState();
  view.goToPage(0);
  view.resetHistory();
  if (previous && previous.docId !== info.docId) await releaseReplaced(previous);
  for (const w of warnings) {
    if (w === "formsDropped") toast("pages.merge.formWarning", undefined, { tone: "info" });
    if (w === "outlineDropped") toast("pages.merge.outlineWarning", undefined, { tone: "info" });
  }
  toast("pages.merge.done", { count: inputs.length }, { tone: "success" });
  return info;
}

// ---------------------------------------------------------------------------
// Saving
// ---------------------------------------------------------------------------

/**
 * ⌘S. Atomic on the backend; a read-only target falls through to Save As with an explanation.
 * v0.3: a signed document that would be rewritten asks first (S1), and a file changed on disk by
 * another program asks 덮어쓰기 / 다른 이름으로 저장 / 다시 불러오기 (H8; `force` is the first).
 */
export async function saveFlow(opts: { force?: boolean } = {}): Promise<boolean> {
  // ⌘S mid-drag is queued until the drop has settled: never save the transient `/F HIDDEN`. The
  // last nudge / slider patch still in its coalescing delay goes to the engine first.
  await whenEditsSettled();
  const info = useDocStore.getState().info;
  if (!info) return false;
  // a 복구 copy is not the user's file: never save over it in place
  if (!info.path || recoveredEntry(info.docId)) return saveAsFlow();
  if (!(await confirmSignatureRewrite(info))) return false;
  const jobs = useJobStore.getState();
  try {
    const args = opts.force ? { docId: info.docId, force: true } : { docId: info.docId };
    await api.saveDocument(args, (e) => jobs.apply("save", "status.saving", e));
    await useDocStore.getState().refresh();
    await autosave.clear(info.docId);
    const fresh = useDocStore.getState().info;
    if (fresh) await touchRecent(fresh);
    toast("status.saved", undefined, { tone: "success", timeoutMs: 2200 });
    return true;
  } catch (e) {
    if (api.isSeePdfError(e) && e.code === "readOnly") {
      toast("dialog.saveAs.readOnly", undefined, { tone: "info" });
      return saveAsFlow();
    }
    if (api.isSeePdfError(e) && e.code === "fileChangedOnDisk") return resolveChangedOnDisk(info);
    toast("error.saveFailed", undefined, { tone: "danger", detail: message(e) });
    return false;
  }
}

/** ⇧⌘S — the native save panel, then `save_document_as`. */
export async function saveAsFlow(): Promise<boolean> {
  await whenEditsSettled();
  const info = useDocStore.getState().info;
  if (!info) return false;
  const path = await api.saveFileDialog({ defaultPath: recoveredEntry(info.docId)?.name ?? info.name });
  if (!path) return false;
  if (!(await confirmSignatureRewrite(info))) return false;
  const jobs = useJobStore.getState();
  try {
    await api.saveDocumentAs({ docId: info.docId, path }, (e) => jobs.apply("save", "status.saving", e));
    await useDocStore.getState().refresh();
    await autosave.clear(info.docId);
    await settleRecovered(info.docId);
    const fresh = useDocStore.getState().info;
    if (fresh) await touchRecent(fresh);
    toast("status.saved", undefined, { tone: "success", timeoutMs: 2200 });
    return true;
  } catch (e) {
    toast("error.saveFailed", undefined, { tone: "danger", detail: message(e) });
    return false;
  }
}

/**
 * The 저장 / 저장 안 함 / 취소 gate in front of anything that discards the document
 * (close, open another file, quit, window close). `false` = the user cancelled.
 */
export async function confirmUnsaved(): Promise<boolean> {
  // an edit still in its coalescing delay is a change too: send it, and ask about it
  if (await whenEditsSettled()) await useDocStore.getState().refresh().catch(() => undefined);
  const info = useDocStore.getState().info;
  if (!info || !info.dirty) return true;
  const answer = await askUnsaved(info.name);
  if (answer === "cancel") return false;
  if (answer === "save") return saveFlow();
  await askKeepRecovery(info);
  return true;
}

/**
 * 저장 안 함 on a document opened from a 복구 copy (Stage 8): keep the copy for the next launch
 * (the default, 보관 / Esc) or delete it now.
 */
async function askKeepRecovery(info: DocInfo): Promise<void> {
  const entry = recoveredEntry(info.docId);
  if (!entry) return;
  const discard = await askConfirm({
    titleKey: "recovery.keep.title",
    bodyKey: "recovery.keep.body",
    bodyParams: { name: entry.name },
    confirmKey: "recovery.keep.discard",
    cancelKey: "recovery.keep.keep",
    danger: true,
  });
  if (discard) await settleRecovered(info.docId);
}

/**
 * The gate in front of closing or replacing the document: pending 편집 · 영역 표시 marks first
 * (F-22 — they are not in the document, so not even 저장 keeps them; 버리기 drops them, 취소 stops
 * the caller), then the 저장 / 저장 안 함 / 취소 gate.
 */
export async function confirmLeaveDocument(): Promise<boolean> {
  const pending = editLeaveGuard();
  if (pending && !(await pending)) return false;
  return confirmUnsaved();
}

/**
 * The window's close request (F-23) — the same gate as closing the document: pending 편집 · 영역
 * 표시 marks first (F-22; they do not make the document dirty, so a clean document with marks must
 * still ask), then 저장 / 저장 안 함 / 취소. `"close"`: nothing to ask, let the window close;
 * `"confirmed"`: the user was asked and agreed (the caller holds the close request and closes the
 * window itself); `"cancel"`: stay.
 */
export async function windowCloseGate(): Promise<"close" | "confirmed" | "cancel"> {
  const info = useDocStore.getState().info;
  const pending = info ? editLeaveGuard() : null;
  if (!info?.dirty && !pending) return "close";
  if (pending && !(await pending)) return "cancel";
  return (await confirmUnsaved()) ? "confirmed" : "cancel";
}

export async function closeDocumentFlow(): Promise<boolean> {
  if (!(await confirmLeaveDocument())) return false;
  const info = useDocStore.getState().info;
  if (info) {
    await touchRecent(info).catch(() => undefined);
    // 저장 or 저장 안 함 (or nothing to save): a clean close, so the recovery copy goes
    await autosave.clear(info.docId);
  }
  usePagesStore.getState().reset();
  await useDocStore.getState().close();
  return true;
}

// ---------------------------------------------------------------------------
// Pages
// ---------------------------------------------------------------------------

/**
 * Every organizer action goes through here: one `page_ops` call, one generation, one undo step
 * (F-16). `doc-changed` (structure) refreshes the page list, so nothing is applied locally except
 * the optimistic drag order, which is dropped as soon as this resolves.
 */
export async function runPageOps(ops: PageOp[]): Promise<boolean> {
  const info = useDocStore.getState().info;
  if (!info || ops.length === 0) return false;
  try {
    await api.pageOps({ docId: info.docId, ops });
    await useDocStore.getState().refresh();
    return true;
  } catch (e) {
    toast("error.generic", undefined, { tone: "danger", detail: message(e) });
    return false;
  } finally {
    usePagesStore.getState().setPendingOrder(null);
    usePagesStore.getState().setDropAt(null);
  }
}

/** 추출… — the dialog picks 새 파일 + 추출 후 원본에서 삭제, then `extract_pages`. */
export async function extractFlow(pages: PageIndex[]): Promise<void> {
  const info = useDocStore.getState().info;
  if (!info || pages.length === 0) return;
  openDialog("extract", { pages });
}

export async function runExtract(pages: PageIndex[], removeAfter: boolean): Promise<void> {
  const info = useDocStore.getState().info;
  if (!info) return;
  const outPath = await api.saveFileDialog({ defaultPath: suggestName(info.name, "pages") });
  if (!outPath) return;
  try {
    await api.extractPages({ docId: info.docId, pages, outPath, removeAfter });
    if (removeAfter) {
      usePagesStore.getState().clear();
      await useDocStore.getState().refresh();
    }
    toast("pages.extract.done", { count: pages.length }, {
      tone: "success",
      actions: [revealAction(outPath)],
    });
  } catch (e) {
    toast("error.generic", undefined, { tone: "danger", detail: message(e) });
  }
}

/** 파일에서 삽입… — a picker plus an optional 1-based range, inserted at `at`. */
export async function insertFromFileFlow(at: PageIndex): Promise<void> {
  const picked = await api.openFileDialog({ multiple: false });
  if (!picked?.length) return;
  openDialog("insertFrom", { at, path: picked[0] });
}

// ---------------------------------------------------------------------------
// Printing
// ---------------------------------------------------------------------------

/** How F-26 reaches paper. */
export type PrintMethod = "document" | "handler";

/**
 * 인쇄 (F-26), two ways, both of which put the **document** on paper — never the app chrome.
 *
 * * `"document"` (default) — mount the print-only DOM (`src/print/PrintRoot.tsx`): one
 *   full-width `<img>` per requested page off the `seepdf://page` route at 150 DPI, with
 *   `print.css` hiding the app shell, then `window.print()`. This is the path F-26 asks for:
 *   ⌘P opens the system print dialog, and its preview shows the pages.
 * * `"handler"` — `print_prepare` writes a flattened temp file (the chosen pages, annotations
 *   and filled form values baked in) and the OS handler (Preview / Edge / Acrobat) opens it.
 *   Kept as the alternative in the 인쇄 dialog: it is the only path that can print a document
 *   SeePDF cannot rasterise fast enough, and it is what Stage 2 verified end to end.
 *
 * Before the print-only DOM existed, `plugin:webview|print` printed the webview's DOM — the
 * SeePDF toolbar, sidebar and canvas element on one sheet ("Page 1 of 1", `s2-16.png`). It is
 * now only reached if the OS has no PDF handler at all.
 */
export async function runPrint(
  pages: PageIndex[] | undefined,
  method: PrintMethod = "document",
): Promise<void> {
  const info = useDocStore.getState().info;
  if (!info) return;
  // v0.3 S5: the document's print permission (the engine refuses `print_prepare` as well)
  if (!info.permissions.print) {
    toast("security.restricted.reason.print", undefined, { tone: "info" });
    return;
  }
  if (method === "document") return printDocumentDom(info, pages);
  toast("print.preparing", undefined, { timeoutMs: 1800 });
  try {
    const { tempPath } = await api.printPrepare({ docId: info.docId, pages });
    if (await openWithOs(tempPath)) return;
    // No OS handler for PDFs: the webview panel at least gets the user to a printer.
    if (!(await webviewPrint())) toast("print.title", undefined, { tone: "info", detail: tempPath });
  } catch (e) {
    toast("error.generic", undefined, { tone: "danger", detail: message(e) });
  }
}

/** Fill the print store; `PrintRoot` renders the pages and calls `window.print()` itself. */
async function printDocumentDom(info: DocInfo, pages: PageIndex[] | undefined): Promise<void> {
  const { PRINT_DPI, scaleKeyForDpi, usePrintStore } = await import("../print/printStore");
  const all = Array.from({ length: info.pageCount }, (_, i) => i);
  toast("print.rendering", undefined, { timeoutMs: 2400 });
  usePrintStore.getState().start({
    docId: info.docId,
    generation: info.docGeneration,
    pages: pages?.length ? pages : all,
    rotation: useViewStore.getState().rotation,
    scaleKey: scaleKeyForDpi(PRINT_DPI),
  });
}

/**
 * The primary print path: ask the webview to print itself, which opens the OS print panel.
 *
 * `@tauri-apps/api` 2.11 has no `Webview.print()` binding yet, but the Rust side does have the
 * `print` core command (`core:webview:allow-print`, in `capabilities/default.json`), so the raw
 * `invoke` is tried as well. Either way a failure is not an error — `runPrint` falls through to
 * the flattened temp file plus the OS handler, which is the path WORKPLAN §6 specifies.
 */
async function webviewPrint(): Promise<boolean> {
  try {
    const { getCurrentWebview } = await import("@tauri-apps/api/webview");
    const webview = getCurrentWebview() as unknown as { print?: () => Promise<void>; label?: string };
    if (typeof webview.print === "function") {
      await webview.print();
      return true;
    }
    const { invoke } = await import("@tauri-apps/api/core");
    await invoke("plugin:webview|print", { label: webview.label });
    return true;
  } catch {
    return false;
  }
}

/** Hand the flattened temp file to the OS PDF handler (Preview / Edge / Acrobat). */
async function openWithOs(path: string): Promise<boolean> {
  try {
    const { openPath: opener } = await import("@tauri-apps/plugin-opener");
    await opener(path);
    return true;
  } catch {
    return false;
  }
}

// ---------------------------------------------------------------------------
// Shared bits
// ---------------------------------------------------------------------------

export function revealAction(path: string): { labelKey: string; onSelect(): void } {
  const windows = useAppStore.getState().os === "windows";
  return {
    labelKey: windows ? "export.revealExplorer" : "export.revealFinder",
    onSelect() {
      void api.revealInFileManager({ path }).catch(() => undefined);
    },
  };
}

export function suggestName(name: string, suffix: string): string {
  const dot = name.lastIndexOf(".");
  const stem = dot > 0 ? name.slice(0, dot) : name;
  const ext = dot > 0 ? name.slice(dot) : ".pdf";
  return `${stem}-${suffix}${ext}`;
}

export function baseName(path: string): string {
  const parts = path.split(/[\\/]/);
  return parts[parts.length - 1] || path;
}

/**
 * The folder of `path`, in the path's own separator: `C:\Users\hw\a.pdf` → `C:\Users\hw` (Explorer's
 * `/select,` and `split_document`'s outDir need a real Windows path), and a drive root keeps its
 * separator — `C:\a.pdf` → `C:\`, because `C:` alone is the current directory on C:.
 */
export function dirName(path: string): string {
  const cut = Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\"));
  if (cut < 0) return ".";
  const dir = path.slice(0, cut);
  if (dir === "" || /^[A-Za-z]:$/.test(dir)) return dir + path[cut];
  return dir;
}

export function message(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

// ---------------------------------------------------------------------------
// v0.3 pkg5-app-shell-release-diagnostics
// ---------------------------------------------------------------------------

/**
 * H10 도움말 › 문제 보고…: the report (version, OS / arch, pdfium, the last 200 log lines) goes to the
 * clipboard, and the log folder opens with `problem-report.txt` — the same text — selected, so the
 * report survives a clipboard that refused it (a native-menu click carries no user activation).
 */
export async function reportProblem(): Promise<void> {
  let report: ProblemReport;
  try {
    report = await api.problemReport();
  } catch (e) {
    toast("help.report.failed", undefined, { tone: "danger", detail: message(e) });
    return;
  }
  const copied = await copyText(report.text);
  await api.revealInFileManager({ path: report.path }).catch(() => undefined);
  toast(copied ? "help.report.copied" : "help.report.copyFailed", undefined, {
    tone: copied ? "success" : "info",
    timeoutMs: 8000,
  });
}

async function copyText(text: string): Promise<boolean> {
  try {
    if (typeof navigator === "undefined" || !navigator.clipboard) return false;
    await navigator.clipboard.writeText(text);
    return true;
  } catch {
    return false;
  }
}

/** H10 도움말 › 로그 폴더 열기. */
export async function openLogFolder(): Promise<void> {
  try {
    await api.openLogFolder();
  } catch (e) {
    toast("help.logs.failed", undefined, { tone: "danger", detail: message(e) });
  }
}

let crashHandling: DocId | null = null;

/**
 * H4: a panicking engine command closed this window's document (`engine-crashed`). The engine no
 * longer knows it, so the window forgets it without `close_document` and opens it again — from its
 * newest 자동 저장 copy when it had unsaved changes and one exists (the copy is marked, so 저장 asks
 * for a location like any 복구), else from its file. A never-saved document without a copy is lost:
 * the window goes back to the welcome screen. Resolves with the reopened document, or null.
 */
export async function recoverFromEngineCrash(docIds: DocId[]): Promise<DocInfo | null> {
  const info = useDocStore.getState().info;
  if (!info || !docIds.includes(info.docId) || crashHandling === info.docId) return null;
  crashHandling = info.docId;
  try {
    toast("error.engineCrashed", undefined, { tone: "danger" });
    const copies = info.dirty ? await api.listRecovery().catch(() => [] as RecoveryEntry[]) : [];
    // newest first (list_recovery's order); a document that itself came from 복구 falls back to its copy
    const copy =
      copies.find((c) => (info.path ? c.originalPath === info.path : c.originalPath === null && c.name === info.name)) ??
      recoveredEntry(info.docId);
    autosave.stop();
    usePagesStore.getState().reset();
    useDocStore.setState({ docId: null, info: null, outline: [], status: "empty", error: null });
    if (copy) return await openPath(copy.recoveryPath, { guard: false, recovery: copy });
    if (info.path) return await openPath(info.path, { guard: false });
    return null;
  } finally {
    crashHandling = null;
  }
}

export { closeDialog, openDialog };

// ---------------------------------------------------------------------------
// v0.3 pkg3-security-save-integrity
// ---------------------------------------------------------------------------

/**
 * H8: `true` when another window already shows the file at `path` — the backend has brought it to
 * the front, and the caller opens nothing. This window's own file is not "elsewhere".
 */
export async function focusedElsewhere(path: string): Promise<boolean> {
  const label = await api.focusDocumentWindow({ path }).catch(() => null);
  return !!label && label !== windowLabel();
}

/** Documents whose "저장하면 서명이 무효화됩니다" prompt was answered 저장 (once per document). */
const rewriteAcknowledged = new Set<string>();

/**
 * S1: a signed document whose next save is a full rewrite (a lopdf edit happened, or an undo went
 * back to a re-serialised state) invalidates its signatures — ask once. An incremental save keeps
 * them and asks nothing.
 */
export async function confirmSignatureRewrite(info: DocInfo): Promise<boolean> {
  if (!info.signatures?.length || rewriteAcknowledged.has(info.docId)) return true;
  const fresh = await api.getDocument({ docId: info.docId }).catch(() => info);
  if (fresh.incrementalSave !== false) return true;
  const ok = await askConfirm({
    titleKey: "security.signed.saveTitle",
    bodyKey: "security.signed.saveBody",
    confirmKey: "common.save",
    danger: true,
  });
  if (ok) rewriteAcknowledged.add(info.docId);
  return ok;
}

export type ChangedOnDiskAnswer = "overwrite" | "saveAs" | "reload" | "cancel";

/**
 * H8: the file changed on disk since it was opened — 덮어쓰기 (save with `force`), 다른 이름으로
 * 저장, or 다시 불러오기 (reopen the file from disk, dropping this window's changes).
 */
export async function resolveChangedOnDisk(info: DocInfo): Promise<boolean> {
  const answer = await askChoice<ChangedOnDiskAnswer>({
    titleKey: "save.changed.title",
    bodyKey: "save.changed.body",
    bodyParams: { name: info.name },
    options: [
      { value: "overwrite", labelKey: "save.changed.overwrite", primary: true },
      { value: "saveAs", labelKey: "save.changed.saveAs" },
      { value: "reload", labelKey: "save.changed.reload" },
    ],
    cancel: { value: "cancel", labelKey: "common.cancel" },
  });
  if (answer === "overwrite") return saveFlow({ force: true });
  if (answer === "saveAs") return saveAsFlow();
  if (answer === "reload" && info.path) {
    await autosave.clear(info.docId);
    return (await openPath(info.path, { guard: false })) !== null;
  }
  return false;
}
