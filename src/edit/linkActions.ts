/**
 * 링크 (P2) — the 편집 mode 링크 tool's state and its three commands.
 *
 * The links themselves are annotations (`kind: 'link'`) and live in `annotStore` like every other
 * annotation, so the 읽기 layer, the 주석 list and this tool all see the same list; this module
 * only holds what the tool is pointing at (the selected link, the rectangle just drawn) and turns
 * a finished gesture into `create_link` / `update_link` / `delete_link` — one undo step each, a
 * toast with 실행 취소, and the page's list stored straight from the result.
 */
import { create } from "zustand";
import * as api from "../ipc/api";
import type { AnnotId, AnnotResult, LinkBorder, LinkTarget, PageIndex, Rect } from "../ipc/types";
import { useAnnotStore } from "../store/annotStore";
import { useDocStore } from "../store/docStore";
import { toast, type ToastAction } from "../app/toastStore";

export interface LinkRef {
  page: PageIndex;
  id: AnnotId;
}

interface LinkToolState {
  /** the link the inspector edits */
  selected: LinkRef | null;
  /** a rectangle just drawn with the 링크 tool, waiting for its target (the popover) */
  draft: { page: PageIndex; rect: Rect; quads?: Rect[] } | null;
  select(link: LinkRef | null): void;
  setDraft(draft: { page: PageIndex; rect: Rect; quads?: Rect[] } | null): void;
  reset(): void;
}

export const useLinkStore = create<LinkToolState>((set) => ({
  selected: null,
  draft: null,
  select(selected) {
    set({ selected, draft: null });
  },
  setDraft(draft) {
    set({ draft, ...(draft ? { selected: null } : null) });
  },
  reset() {
    set({ selected: null, draft: null });
  },
}));

/** A link rectangle must be at least this big (points) — a click is not a link. */
export const MIN_LINK_PT = 4;

const undoAction: ToastAction = {
  labelKey: "common.undo",
  onSelect: () => void import("../annot/sync").then((m) => m.undoWithAnnots()),
};

function docId(): string | null {
  return useDocStore.getState().info?.docId ?? null;
}

function store(result: AnnotResult): void {
  useAnnotStore.getState().setPage(result.list.page, result.list.annots, result.list.docGeneration);
}

function fail(e: unknown): void {
  const unsupported = api.isSeePdfError(e) && e.code === "unsupported";
  toast(unsupported ? "structure.encrypted" : "error.generic", undefined, {
    tone: "danger",
    detail: e instanceof Error ? e.message : String(e),
  });
}

/**
 * What a typed web address becomes: trimmed, `https://` added when no scheme is given (`mailto:`
 * for something that looks like an e-mail address), and non-ASCII percent-encoded — a PDF URI is
 * 7-bit, and the engine would encode it anyway, so the address reads back exactly as sent.
 */
export function normalizeUrl(input: string): string {
  const text = input.trim();
  if (!text) return "";
  let url = text;
  if (!/^[a-z][a-z0-9+.-]*:/i.test(text)) url = /^[^\s/@]+@[^\s/@]+\.[^\s/@]+$/.test(text) ? `mailto:${text}` : `https://${text}`;
  return url.replace(/[^\x21-\x7e]+/g, (run) => encodeURIComponent(run));
}

// v0.3 pkg2 (P5): `quads` — a link made from a text selection, one quad per line
export async function createLink(page: PageIndex, rect: Rect, target: LinkTarget, quads?: Rect[]): Promise<boolean> {
  const doc = docId();
  if (!doc) return false;
  try {
    const result = await api.createLink({ docId: doc, page, rect, target, ...(quads?.length ? { quads } : null) });
    store(result);
    useLinkStore.getState().select(result.annot ? { page, id: result.annot.id } : null);
    toast("link.created", undefined, { tone: "success", actions: [undoAction] });
    return true;
  } catch (e) {
    fail(e);
    return false;
  }
}

export async function updateLink(
  page: PageIndex,
  id: AnnotId,
  // v0.3 pkg2 (P5): `border` shows / hides the link box
  patch: { target?: LinkTarget; rect?: Rect; border?: LinkBorder },
): Promise<boolean> {
  const doc = docId();
  if (!doc) return false;
  try {
    const result = await api.updateLink({ docId: doc, page, id, ...patch });
    store(result);
    toast("link.updated", undefined, { tone: "success", actions: [undoAction] });
    return true;
  } catch (e) {
    fail(e);
    return false;
  }
}

export async function deleteLink(page: PageIndex, id: AnnotId): Promise<boolean> {
  const doc = docId();
  if (!doc) return false;
  try {
    const result = await api.deleteLink({ docId: doc, page, id });
    store(result);
    const sel = useLinkStore.getState().selected;
    if (sel?.id === id) useLinkStore.getState().select(null);
    useAnnotStore.getState().select(useAnnotStore.getState().selected.filter((x) => x !== id));
    toast("link.deleted", undefined, { actions: [undoAction] });
    return true;
  } catch (e) {
    fail(e);
    return false;
  }
}
