/**
 * 이미지로 PDF 만들기 (v0.3 D1) — the flows around `create_from_images`. Dynamically imported
 * (like `flows.ts`), so none of it reaches the entry chunk.
 *
 *  * 파일 ▸ 이미지로 PDF 만들기… (⋯ menu, native 파일 menu): an image picker, then the dialog;
 *  * images dropped on the window or the welcome screen: the dialog (not `open_document`, which
 *    would fail on a JPEG); PDFs dropped with them open in new windows;
 *  * 파일 ▸ 클립보드에서 새로 만들기: the system clipboard's image → a temp PNG → a one-page document;
 *  * 편집 ⌘V with an image on the system clipboard: see `edit/actions.ts` `pasteSystemImage`.
 *
 * A new document replaces the window's document exactly like 파일 합치기 does: the leave guard runs
 * first, and the replaced document is released only once the new one exists.
 */
import * as api from "../ipc/api";
import { useDocStore } from "../store/docStore";
import { useViewStore } from "../store/viewStore";
import { usePagesStore } from "../store/pagesStore";
import { useJobStore } from "../store/jobStore";
import { toast } from "../app/toastStore";
import { autosave } from "../app/autosave";
import { openDialog } from "./dialogState";
import { adoptIntoNewTab, confirmLeaveDocument, message, openTargetSetting } from "./flows";
import type { DocInfo, ImageFit, ImagePageSize } from "../ipc/types";

/** What `create_from_images` accepts (the engine checks the magic bytes; this is the routing guess). */
export const IMAGE_FILE = /\.(png|jpe?g)$/i;

export function isImagePath(path: string): boolean {
  return IMAGE_FILE.test(path);
}

const IMAGE_FILTER = [{ name: "Images", extensions: ["png", "jpg", "jpeg"] }];

/** 파일 ▸ 이미지로 PDF 만들기…: pick images, then order / page size / margin in the dialog. */
export async function imagesToPdfFlow(): Promise<void> {
  const picked = await api.openFileDialog({ multiple: true, filters: IMAGE_FILTER });
  if (!picked?.length) return;
  openDialog("imagesToPdf", { paths: picked.filter(isImagePath) });
}

/**
 * Dropped files: images go to the dialog, everything else opens as before. A drop that mixes
 * both opens the other files (the PDFs) each in a new window — this window's document is what
 * 만들기 replaces — and says so, rather than dropping them silently.
 */
export async function routeDroppedPaths(paths: string[]): Promise<void> {
  const images = paths.filter(isImagePath);
  const others = paths.filter((p) => !isImagePath(p));
  if (!images.length) {
    if (others.length) {
      const { openPaths } = await import("./flows");
      await openPaths(others);
    }
    return;
  }
  openDialog("imagesToPdf", { paths: images });
  if (!others.length) return;
  const { focusedElsewhere, openPath } = await import("./flows");
  let opened = 0;
  // v0.3 DR1: with tabs, 만들기 no longer replaces this window's document — the PDFs open in tabs
  if (openTargetSetting() === "tab") {
    for (const path of others) if (await openPath(path, { target: "tab" })) opened++;
    if (opened) toast("imagesToPdf.othersOpened", { count: opened }, { tone: "info" });
    return;
  }
  for (const path of others) {
    try {
      // v0.3 integration (D1 × H8): a file another window already shows is focused, not opened twice
      if (await focusedElsewhere(path)) continue;
      await api.openInNewWindow({ path });
      opened++;
    } catch {
      /* the new window reports its own open error */
    }
  }
  if (opened) toast("imagesToPdf.othersOpened", { count: opened }, { tone: "info" });
}

export interface ImagesOptions {
  pageSize: ImagePageSize;
  /** points on every side */
  margin: number;
  fit: ImageFit;
}

/** More images than this show a progress bar in the status bar. */
const PROGRESS_AFTER = 5;

/**
 * Builds the document and makes it the window's document. `null` when the user kept the current
 * one (leave guard) or the engine refused.
 */
export async function createFromImagesFlow(
  paths: string[],
  options: ImagesOptions,
  opts: { guard?: boolean; replace?: boolean } = {},
): Promise<DocInfo | null> {
  if (!paths.length) return null;
  // v0.3 DR1: the new document takes a new tab (설정 › 파일 열기 = 새 창, or `replace`: it replaces this one)
  const intoTab = !!useDocStore.getState().info && !opts.replace && openTargetSetting() === "tab";
  if (!intoTab && opts.guard !== false && !(await confirmLeaveDocument())) return null;
  const jobs = useJobStore.getState();
  let info: DocInfo;
  try {
    info = await api.createFromImages(
      { paths, pageSize: options.pageSize, margin: options.margin, fit: options.fit },
      (e) => {
        if (paths.length > PROGRESS_AFTER) jobs.apply("export", "imagesToPdf.progress", e);
      },
    );
  } catch (e) {
    toast(api.isSeePdfError(e) && e.code === "unsupported" ? "imagesToPdf.unsupported" : "error.generic", undefined, {
      tone: "danger",
      detail: message(e),
    });
    return null;
  }
  if (intoTab && !(await adoptIntoNewTab(info))) return null;
  const previous = intoTab ? null : useDocStore.getState().info;
  usePagesStore.getState().reset();
  useDocStore.getState().adopt(info);
  const view = useViewStore.getState();
  view.goToPage(0);
  view.resetHistory();
  if (previous && previous.docId !== info.docId) {
    await autosave.clear(previous.docId);
    await api.closeDocument({ docId: previous.docId }).catch(() => undefined);
  }
  toast("imagesToPdf.done", { count: paths.length }, { tone: "success" });
  return info;
}

/**
 * The system clipboard's image, as PNG / JPEG bytes — or `null` (nothing there, or the webview
 * refused: WebKit shows its own 붙여넣기 button and the user can dismiss it).
 */
export async function readClipboardImage(data?: DataTransfer | null): Promise<Uint8Array | null> {
  if (data) {
    for (const item of [...(data.items ?? [])]) {
      if (item.kind === "file" && /^image\/(png|jpeg)$/.test(item.type)) {
        const file = item.getAsFile();
        if (file) return new Uint8Array(await file.arrayBuffer());
      }
    }
    return null;
  }
  return (await readWebClipboard()).bytes ?? null;
}

/** `navigator.clipboard.read()`: the image, or `refused` when the webview would not let us read. */
async function readWebClipboard(): Promise<{ bytes?: Uint8Array; refused?: boolean }> {
  const clipboard = typeof navigator === "undefined" ? undefined : navigator.clipboard;
  if (!clipboard?.read) return { refused: true };
  try {
    for (const item of await clipboard.read()) {
      const type = item.types.find((t) => t === "image/png" || t === "image/jpeg");
      if (type) return { bytes: new Uint8Array(await (await item.getType(type)).arrayBuffer()) };
    }
    return {};
  } catch (e) {
    // WebKit outside a user gesture (a native menu item): NotAllowedError — not "no image"
    return (e as { name?: string } | null)?.name === "NotAllowedError" ? { refused: true } : {};
  }
}

/**
 * 파일 ▸ 클립보드에서 새로 만들기: a one-page document at the image's own size.
 *
 * v0.3 integration: the engine reads the clipboard (`clipboard_image_to_temp`: NSPasteboard / the
 * Windows clipboard, no user gesture needed) — the native menu item reaches the webview as an
 * event, and WebKit refuses `navigator.clipboard.read` there. The webview read is only the fallback
 * when that command is unavailable; when it is refused the toast says the clipboard could not be
 * read, not that it is empty.
 */
export async function newFromClipboardFlow(): Promise<DocInfo | null> {
  let path: string | null = null;
  let native = true;
  try {
    path = await api.clipboardImageToTemp();
  } catch (e) {
    const code = (e as { code?: string } | null)?.code;
    if (code === "unsupported") {
      toast("imagesToPdf.unsupported", undefined, { tone: "danger", detail: message(e) });
      return null;
    }
    native = false;
  }
  if (!path && !native) {
    const read = await readWebClipboard();
    if (read.refused) {
      toast("imagesToPdf.clipboardUnreadable", undefined, { tone: "danger" });
      return null;
    }
    if (read.bytes) {
      try {
        path = await api.writeTempImage(read.bytes);
      } catch (e) {
        toast("imagesToPdf.unsupported", undefined, { tone: "danger", detail: message(e) });
        return null;
      }
    }
  }
  if (!path) {
    toast("imagesToPdf.noClipboardImage", undefined, { tone: "info" });
    return null;
  }
  return createFromImagesFlow([path], { pageSize: "original", margin: 0, fit: "contain" });
}
