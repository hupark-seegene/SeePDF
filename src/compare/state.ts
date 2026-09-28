/**
 * 문서 비교 (P1-6) session state. Lives in the entry chunk only because `App.tsx` must know whether
 * to mount the (lazy) full-window `CompareView`; everything else about comparing is code-split.
 *
 * Document B is opened with `open_document` **outside** `docStore`, so the window's document (A)
 * stays exactly as it was; closing compare mode closes B in the engine.
 */
import { create } from "zustand";
import * as api from "../ipc/api";
import type { CompareReport, DocInfo } from "../ipc/types";

export interface CompareSession {
  report: CompareReport;
  infoA: DocInfo;
  infoB: DocInfo;
}

interface CompareStore {
  session: CompareSession | null;
  enter(session: CompareSession): void;
  /** Leave compare mode and release document B. */
  exit(): Promise<void>;
}

export const useCompareStore = create<CompareStore>((set, get) => ({
  session: null,
  enter(session) {
    const previous = get().session;
    set({ session });
    if (previous && previous.infoB.docId !== session.infoB.docId) {
      void api.closeDocument({ docId: previous.infoB.docId }).catch(() => undefined);
    }
  },
  async exit() {
    const session = get().session;
    if (!session) return;
    set({ session: null });
    await api.closeDocument({ docId: session.infoB.docId }).catch(() => undefined);
  },
}));

export function isComparing(): boolean {
  return useCompareStore.getState().session !== null;
}
