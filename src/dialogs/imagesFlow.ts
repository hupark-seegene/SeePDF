/**
 * 이미지로 PDF 만들기 (v0.3 D1) — the flows around `create_from_images`. Dynamically imported
 * (like `flows.ts`), so none of it reaches the entry chunk.
 *
 *  * 파일 ▸ 이미지로 PDF 만들기… (⋯ menu, native 파일 menu): an image picker, then the dialog;
 *  * images dropped on the window or the welcome screen: the dialog (not `open_document`, which
 *    would fail on a JPEG);
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
import { confirmLeaveDocument, message } from "./flows";
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

/** Dropped files: images go to the dialog, everything else opens as before. */
export async function routeDroppedPaths(paths: string[]): Promise<void> {
  const images = paths.filter(isImagePath);
  const others = paths.filter((p) => !isImagePath(p));
  if (images.length) openDialog("imagesToPdf", { paths: images });
  if (others.length && !images.length) {
    const { openPaths } = await import("./flows");
    await openPaths(others);
  }
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
  opts: { guard?: boolean } = {},
): Promise<DocInfo | null> {
  if (!paths.length) return null;
  if (opts.guard !== false && !(await confirmLeaveDocument())) return null;
  const previous = useDocStore.getState().info;
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
  const clipboard = typeof navigator === "undefined" ? undefined : navigator.clipboard;
  if (!clipboard?.read) return null;
  try {
    for (const item of await clipboard.read()) {
      const type = item.types.find((t) => t === "image/png" || t === "image/jpeg");
      if (type) return new Uint8Array(await (await item.getType(type)).arrayBuffer());
    }
  } catch {
    /* permission refused or no image */
  }
  return null;
}

/** 파일 ▸ 클립보드에서 새로 만들기: a one-page document at the image's own size. */
export async function newFromClipboardFlow(): Promise<DocInfo | null> {
  const bytes = await readClipboardImage();
  if (!bytes) {
    toast("imagesToPdf.noClipboardImage", undefined, { tone: "info" });
    return null;
  }
  let path: string;
  try {
    path = await api.writeTempImage(bytes);
  } catch (e) {
    toast("imagesToPdf.unsupported", undefined, { tone: "danger", detail: message(e) });
    return null;
  }
  return createFromImagesFlow([path], { pageSize: "original", margin: 0, fit: "contain" });
}
