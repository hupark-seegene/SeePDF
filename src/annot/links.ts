/**
 * Following a link (P2): a go-to-page link scrolls to its page (and the destination's y); a web
 * address opens in the default browser through the opener plugin, **after a confirm** that shows
 * the address — a PDF can carry any URI, and a click in 읽기 mode must not launch one silently.
 * Only http(s) and mailto are handed to the OS; anything else (file:, javascript:, …) is refused.
 *
 * Shared by the 읽기-mode link layer, the 목차 tab's web nodes and the link inspector.
 */
import * as api from "../ipc/api";
import type { Annot } from "../ipc/types";
import { askConfirm } from "../dialogs/dialogState";
import { toast } from "../app/toastStore";
import { useViewStore } from "../store/viewStore";

/** The schemes a click may hand to the OS. */
export function isOpenableUrl(url: string): boolean {
  return /^(https?:\/\/|mailto:)/i.test(url.trim());
}

export async function openWebLink(url: string): Promise<boolean> {
  if (!isOpenableUrl(url)) {
    toast("link.blocked", { url }, { tone: "danger" });
    return false;
  }
  const ok = await askConfirm({ titleKey: "link.openTitle", bodyKey: "link.openBody", bodyParams: { url }, confirmKey: "link.open" });
  if (!ok) return false;
  try {
    await api.openUrl(url);
    return true;
  } catch (e) {
    toast("error.generic", undefined, { tone: "danger", detail: e instanceof Error ? e.message : String(e) });
    return false;
  }
}

/** Where a click on this link goes. */
export function followLink(annot: Pick<Annot, "dest" | "uri">): void {
  if (annot.dest) useViewStore.getState().goToPage(annot.dest.page, annot.dest.y);
  else if (annot.uri) void openWebLink(annot.uri);
}
