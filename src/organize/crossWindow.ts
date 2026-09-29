/**
 * Pages dragged between windows (v0.3 P3).
 *
 * A pointer drag started on a 축소판 or a 페이지-mode cell and **released outside its own window**
 * is published to every window (`pages-drop`: source document, pages, the release point in screen
 * coordinates). The window whose client area holds that point — and that shows a document with a
 * 축소판 list or the page grid under it — calls `import_pages_from_doc` with the caret under the
 * point: one undo step on the target, the source untouched.
 *
 * Every window runs {@link startCrossWindowDrops} once (App.tsx). The resolution is injectable so
 * the flow is testable without a layout engine.
 */
import * as api from "../ipc/api";
import { emitPagesDrop, onPagesDrop, type PagesDropEvent, type Unsubscribe } from "../ipc/events";
import { useMock, windowLabel } from "../ipc/env";
import { useDocStore } from "../store/docStore";
import { toast } from "../app/toastStore";
import { caretFor, organizerMounted } from "./fileDrop";

/** Did the pointer leave this window's client area? */
export function releasedOutside(clientX: number, clientY: number): boolean {
  if (typeof window === "undefined") return false;
  return clientX < 0 || clientY < 0 || clientX > window.innerWidth || clientY > window.innerHeight;
}

/** Publish a release outside this window (the source side). */
export function publishPagesDrop(srcDocId: string, pages: number[], screenX: number, screenY: number): void {
  if (!pages.length) return;
  void emitPagesDrop({ srcDocId, pages, from: windowLabel(), screen: { x: screenX, y: screenY } }).catch(() => undefined);
}

/** This window's client-area origin on screen, in CSS pixels. */
async function clientOrigin(): Promise<{ x: number; y: number }> {
  if (useMock()) return { x: window.screenX || 0, y: window.screenY || 0 };
  const { getCurrentWindow } = await import("@tauri-apps/api/window");
  const win = getCurrentWindow();
  const [pos, scale] = await Promise.all([win.innerPosition(), win.scaleFactor()]);
  return { x: pos.x / scale, y: pos.y / scale };
}

export interface DropResolver {
  origin(): Promise<{ x: number; y: number }>;
  elementAt(x: number, y: number): Element | null;
}

const defaultResolver: DropResolver = {
  origin: clientOrigin,
  elementAt: (x, y) => (typeof document.elementFromPoint === "function" ? document.elementFromPoint(x, y) : null),
};

/** The caret (0…pageCount) for a client point over this window's 축소판 or grid, or `null`. */
export function caretAtPoint(el: Element | null, x: number, y: number, pageCount: number): number | null {
  if (!el) return null;
  if (el.closest(".org-grid") && organizerMounted()) return caretFor({ x, y });
  const thumb = el.closest<HTMLElement>(".thumb[data-page]");
  if (thumb) {
    const page = Number(thumb.dataset.page);
    const r = thumb.getBoundingClientRect();
    return y < r.top + r.height / 2 ? page : page + 1;
  }
  if (el.closest(".thumb-list")) return pageCount;
  return null;
}

/** The target side: import the pages when the release point is over this window's pages. */
export async function handlePagesDrop(e: PagesDropEvent, resolver: DropResolver = defaultResolver): Promise<boolean> {
  if (e.from === windowLabel()) return false;
  const info = useDocStore.getState().info;
  if (!info || info.docId === e.srcDocId) return false;
  const origin = await resolver.origin();
  const x = e.screen.x - origin.x;
  const y = e.screen.y - origin.y;
  if (x < 0 || y < 0 || x > window.innerWidth || y > window.innerHeight) return false;
  const at = caretAtPoint(resolver.elementAt(x, y), x, y, info.pageCount);
  if (at === null) return false;
  try {
    const next = await api.importPagesFromDoc({ srcDocId: e.srcDocId, pages: e.pages, dstDocId: info.docId, at });
    useDocStore.getState().adopt(next);
    toast("pages.importedFromWindow", { count: e.pages.length }, { tone: "success" });
    return true;
  } catch (err) {
    toast("error.generic", undefined, { tone: "danger", detail: err instanceof Error ? err.message : String(err) });
    return false;
  }
}

export function startCrossWindowDrops(): Unsubscribe {
  return onPagesDrop((e) => void handlePagesDrop(e));
}
