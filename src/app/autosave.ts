/**
 * 자동 저장 / 복구 (P1-8): while a document is open, dirty and `settings.autosaveSec > 0`, a timer
 * writes a recovery copy (`write_recovery`) — only when `docGeneration` moved since the last write.
 * v0.3 DR1: every dirty tab of the window gets its copy, not only the one on screen — a beat walks
 * the active document and then the background tabs (`tabStore.backgroundDocs`), one write at a time.
 * A successful 저장 / 다른 이름으로 저장 and a clean close drop the copy (`clear_recovery`), and so
 * does an undo that brings the document back to clean (Stage 8). A failed write is toasted at most
 * once per document. The user's file is never touched.
 *
 * While an annotation drag has something hidden in the engine (`annot/dragGate.ts`), a beat waits
 * for the drop to settle, so a copy never captures the transient `/F HIDDEN` bit; the write itself
 * is tracked so a drag that starts meanwhile hides only after it has landed.
 *
 * Entry chunk (the hook is mounted once by `App.tsx`), so it stays small and imports no dialog code.
 */
import { useEffect } from "react";
import * as api from "../ipc/api";
import { useDocStore } from "../store/docStore";
import { useAppStore } from "../store/appStore";
import { toast } from "./toastStore";
import { openDialog } from "../dialogs/dialogState";
import { windowLabel } from "../ipc/env";
import { trackWrite, whenEditsSettled } from "../annot/dragGate";
import { backgroundDocs, useTabStore } from "../store/tabStore";
import type { DocGeneration, DocId, DocInfo, RecoveryEntry, Settings } from "../ipc/types";

export const DEFAULT_AUTOSAVE_SEC = 60;
/** 끄기 / 30초 / 1분 / 5분 — the choices of the 설정 dialog. */
export const AUTOSAVE_CHOICES = [0, 30, 60, 300] as const;

/** `autosaveSec` with the default for a settings file written before Stage 5. */
export function autosaveSecOf(settings: Settings | null): number {
  const sec = settings?.autosaveSec;
  return typeof sec === "number" && Number.isFinite(sec) && sec >= 0 ? sec : DEFAULT_AUTOSAVE_SEC;
}

export interface AutosaveDeps {
  /** the window's document right now */
  current(): DocInfo | null;
  /** v0.3 DR1: the documents of the window's background tabs (default: none) */
  background?(): DocInfo[];
  write(docId: DocId): Promise<unknown>;
  clear(docId: DocId): Promise<unknown>;
  onFail(info: DocInfo, error: unknown): void;
  /** resolves when nothing is transiently hidden (an annotation drag); default: at once */
  idle?(): Promise<void>;
}

export class AutosaveController {
  private timer: ReturnType<typeof setInterval> | null = null;
  private armed: string | null = null;
  /** the generation each document's recovery copy describes (or that needs no copy) */
  private readonly covered = new Map<DocId, DocGeneration>();
  /** documents that have a copy on disk written by this window */
  private readonly written = new Set<DocId>();
  private readonly failed = new Set<DocId>();
  private inFlight: Promise<void> | null = null;
  /** a beat is waiting for a drag to settle */
  private waiting = false;

  constructor(private readonly deps: AutosaveDeps) {}

  /**
   * (Re)arm the timer for the current document; call whenever the document, its dirty state or the
   * interval changes — or a background tab's (v0.3 DR1: the timer runs while any tab is dirty).
   */
  sync(info: DocInfo | null, sec: number): void {
    const background = this.deps.background?.() ?? [];
    // Undo took the document back to clean: the copy describes changes that no longer exist.
    for (const doc of info ? [info, ...background] : background) {
      if (!doc.dirty && this.written.has(doc.docId)) void this.clear(doc.docId);
    }
    const dirty = (info?.dirty ?? false) || background.some((d) => d.dirty);
    // v0.3.0: not keyed by the document on screen — a beat walks every tab itself, and re-arming
    // on each tab switch meant a user switching more often than the interval never got a copy
    const key = dirty && sec > 0 ? String(sec) : null;
    if (key === this.armed) return;
    this.stop();
    this.armed = key;
    if (key) this.timer = setInterval(() => void this.tick(), sec * 1000);
  }

  /** One timer beat; exposed for tests. */
  async tick(): Promise<void> {
    if (this.waiting) return;
    if (this.deps.idle) {
      this.waiting = true;
      try {
        await this.deps.idle();
      } finally {
        this.waiting = false;
      }
    }
    if (this.inFlight) return;
    const current = this.deps.current();
    const due = [...(current ? [current] : []), ...(this.deps.background?.() ?? [])].filter(
      (d) => d.dirty && this.covered.get(d.docId) !== d.docGeneration,
    );
    if (!due.length) return;
    this.inFlight = (async () => {
      for (const info of due) {
        const { docId, docGeneration } = info;
        try {
          await this.deps.write(docId);
          this.covered.set(docId, docGeneration);
          this.written.add(docId);
          this.failed.delete(docId);
        } catch (e) {
          if (!this.failed.has(docId)) {
            this.failed.add(docId);
            this.deps.onFail(info, e);
          }
        }
      }
    })();
    try {
      await this.inFlight;
    } finally {
      this.inFlight = null;
    }
  }

  /**
   * Saved, or closed on purpose: drop the copy. Waits for a write in flight so it cannot land after
   * the clear. The current generation counts as covered, so a beat that races the `doc-changed`
   * of the save does not write the copy straight back.
   */
  async clear(docId: DocId): Promise<void> {
    if (this.inFlight) await this.inFlight.catch(() => undefined);
    const info = [this.deps.current(), ...(this.deps.background?.() ?? [])].find((d) => d?.docId === docId);
    if (info) this.covered.set(docId, info.docGeneration);
    this.failed.delete(docId);
    if (!this.written.has(docId)) return;
    this.written.delete(docId);
    await this.deps.clear(docId).catch(() => undefined);
  }

  stop(): void {
    if (this.timer !== null) clearInterval(this.timer);
    this.timer = null;
    this.armed = null;
  }

  /** Test helper. */
  reset(): void {
    this.stop();
    this.covered.clear();
    this.written.clear();
    this.failed.clear();
    this.inFlight = null;
    this.waiting = false;
  }
}

export const autosave = new AutosaveController({
  current: () => useDocStore.getState().info,
  background: backgroundDocs,
  write: (docId) => trackWrite(api.writeRecovery({ docId })),
  // the recovery copy includes the last nudge still waiting out its coalescing delay
  idle: () => whenEditsSettled().then(() => undefined),
  clear: (docId) => api.clearRecovery({ docId }),
  onFail: (info, e) =>
    toast("autosave.failed", { name: info.name }, { tone: "danger", detail: e instanceof Error ? e.message : String(e) }),
});

/** Mounted once by the shell. */
export function useAutosave(): void {
  const docId = useDocStore((s) => s.info?.docId ?? null);
  const dirty = useDocStore((s) => s.info?.dirty ?? false);
  const sec = useAppStore((s) => autosaveSecOf(s.settings));
  // v0.3 DR1: a background tab turning dirty or clean (a copy of a clean tab is dropped)
  const tabs = useTabStore((s) => s.tabs.map((t) => `${t.docId}${t.dirty ? "*" : ""}`).join());
  useEffect(() => {
    autosave.sync(useDocStore.getState().info, sec);
  }, [docId, dirty, sec, tabs]);
  useEffect(() => () => autosave.stop(), []);
}

// ---------------------------------------------------------------------------
// Documents opened from a recovery copy
// ---------------------------------------------------------------------------

/**
 * A document opened from the 복구 dialog lives at its recovery path, which is not the user's file:
 * 저장 goes to 다른 이름으로 저장 (suggesting the original name), the path stays out of 최근 항목,
 * and once it is saved elsewhere the recovery entry is discarded.
 */
const recovered = new Map<DocId, RecoveryEntry>();

export function markRecovered(docId: DocId, entry: RecoveryEntry): void {
  recovered.set(docId, entry);
}

// The window's document went away (closed, or replaced by another): forget its mark. A switch to
// another tab (v0.3 DR1) keeps it — the document is still open in its tab.
useDocStore.subscribe((s, prev) => {
  if (prev.docId && prev.docId !== s.docId && !useTabStore.getState().tabs.some((t) => t.docId === prev.docId)) {
    recovered.delete(prev.docId);
  }
});

export function recoveredEntry(docId: DocId | null | undefined): RecoveryEntry | undefined {
  return docId ? recovered.get(docId) : undefined;
}

/** Saved elsewhere: the copy has served its purpose. */
export async function settleRecovered(docId: DocId): Promise<void> {
  const entry = recovered.get(docId);
  if (!entry) return;
  recovered.delete(docId);
  await api.discardRecovery({ id: entry.id }).catch(() => undefined);
}

/**
 * At launch: recovery copies a crashed session left behind → the 복구 dialog. Only the main window
 * asks — a 새 창 must not offer the live copies of documents open in another window.
 */
export async function offerRecovery(): Promise<boolean> {
  if (windowLabel() !== "main") return false;
  const entries = await api.listRecovery().catch(() => [] as RecoveryEntry[]);
  if (entries.length) openDialog("recovery", { entries });
  return entries.length > 0;
}
