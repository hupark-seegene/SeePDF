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
import { askMultipleFiles, askPassword, askUnsaved, closeDialog, openDialog } from "./dialogState";
import type { DocInfo, PageIndex, PageOp, RecentEntry } from "../ipc/types";

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
    await api.openInNewWindow({ path }).catch(() => undefined);
  }
}

/**
 * Open one file. Retries the password prompt in place (F-01) and restores the stored reading
 * position when 마지막으로 본 위치 기억 is on.
 */
export async function openPath(path: string, opts: { guard?: boolean } = {}): Promise<DocInfo | null> {
  if (opts.guard !== false && !(await confirmUnsaved())) return null;
  const docs = useDocStore.getState();
  let password: string | undefined;
  for (;;) {
    const info = await docs.open(path, password);
    if (info) {
      await afterOpen(info);
      return info;
    }
    const error = useDocStore.getState().error;
    if (error?.code === "passwordRequired" || error?.code === "passwordWrong") {
      const entered = await askPassword(baseName(path), error.code === "passwordWrong");
      if (entered === null) {
        useDocStore.setState({ status: "empty", error: null });
        return null;
      }
      password = entered;
      continue;
    }
    toast(error?.code === "notFound" ? "error.fileMissing" : "error.openFailed", undefined, {
      tone: "danger",
      detail: error?.message,
    });
    return null;
  }
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
  await touchRecent(info);
}

/** Write the current reading position back into the recents list (IPC_CONTRACT §11). */
export async function touchRecent(info: DocInfo): Promise<void> {
  if (!info.path) return;
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

/** 파일 합치기 — the merged document becomes the window's document (it has no path yet). */
export async function mergePaths(inputs: { path: string; range?: string }[]): Promise<DocInfo | null> {
  try {
    const { info, warnings } = await api.mergeDocuments({ inputs });
    usePagesStore.getState().reset();
    useDocStore.getState().adopt(info);
    for (const w of warnings) {
      if (w === "formsDropped") toast("pages.merge.formWarning", undefined, { tone: "info" });
      if (w === "outlineDropped") toast("pages.merge.outlineWarning", undefined, { tone: "info" });
    }
    toast("pages.merge.done", { count: inputs.length }, { tone: "success" });
    return info;
  } catch (e) {
    toast("error.openFailed", undefined, { tone: "danger", detail: message(e) });
    return null;
  }
}

// ---------------------------------------------------------------------------
// Saving
// ---------------------------------------------------------------------------

/** ⌘S. Atomic on the backend; a read-only target falls through to Save As with an explanation. */
export async function saveFlow(): Promise<boolean> {
  const info = useDocStore.getState().info;
  if (!info) return false;
  if (!info.path) return saveAsFlow();
  const jobs = useJobStore.getState();
  try {
    await api.saveDocument({ docId: info.docId }, (e) => jobs.apply("save", "status.saving", e));
    await useDocStore.getState().refresh();
    const fresh = useDocStore.getState().info;
    if (fresh) await touchRecent(fresh);
    toast("status.saved", undefined, { tone: "success", timeoutMs: 2200 });
    return true;
  } catch (e) {
    if (api.isSeePdfError(e) && e.code === "readOnly") {
      toast("dialog.saveAs.readOnly", undefined, { tone: "info" });
      return saveAsFlow();
    }
    toast("error.saveFailed", undefined, { tone: "danger", detail: message(e) });
    return false;
  }
}

/** ⇧⌘S — the native save panel, then `save_document_as`. */
export async function saveAsFlow(): Promise<boolean> {
  const info = useDocStore.getState().info;
  if (!info) return false;
  const path = await api.saveFileDialog({ defaultPath: info.name });
  if (!path) return false;
  const jobs = useJobStore.getState();
  try {
    await api.saveDocumentAs({ docId: info.docId, path }, (e) => jobs.apply("save", "status.saving", e));
    await useDocStore.getState().refresh();
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
  const info = useDocStore.getState().info;
  if (!info || !info.dirty) return true;
  const answer = await askUnsaved(info.name);
  if (answer === "cancel") return false;
  if (answer === "save") return saveFlow();
  return true;
}

export async function closeDocumentFlow(): Promise<boolean> {
  if (!(await confirmUnsaved())) return false;
  const info = useDocStore.getState().info;
  if (info) await touchRecent(info).catch(() => undefined);
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

/**
 * `print_prepare` writes a flattened temp file (the chosen pages, annotations and filled form
 * values baked in) and the OS handler opens it, which is what actually reaches paper.
 *
 * Webview printing is **deliberately the fallback**, not the primary path: it is available —
 * `core:webview:allow-print` is in `capabilities/default.json` and the `plugin:webview|print`
 * command opens the macOS print panel — but what it prints is the webview's **DOM**, i.e. the
 * SeePDF toolbar, sidebar and canvas element, on one sheet. F-26 wants the rendered pages, so
 * that path only makes sense once there is a print-only DOM (STAGE1E_NOTES §7.4, P1). Verified
 * in the Stage 2 smoke: the panel's preview showed the app chrome, "Page 1 of 1"
 * (`docs/STAGE2_INTEGRATION.md`).
 */
export async function runPrint(pages: PageIndex[] | undefined): Promise<void> {
  const info = useDocStore.getState().info;
  if (!info) return;
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

export function dirName(path: string): string {
  const parts = path.split(/[\\/]/);
  parts.pop();
  return parts.join("/") || "/";
}

export function message(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

export { closeDialog, openDialog };
