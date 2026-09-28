/**
 * 자동 저장 / 복구 (P1-8): while a document is open, dirty and `settings.autosaveSec > 0`, a timer
 * writes a recovery copy (`write_recovery`) — only when `docGeneration` moved since the last write.
 * A successful 저장 / 다른 이름으로 저장 and a clean close drop the copy (`clear_recovery`). A failed
 * write is toasted at most once per document. The user's file is never touched.
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
  write(docId: DocId): Promise<unknown>;
  clear(docId: DocId): Promise<unknown>;
  onFail(info: DocInfo, error: unknown): void;
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

  constructor(private readonly deps: AutosaveDeps) {}

  /** (Re)arm the timer for the current document; call whenever the document, its dirty state or the interval changes. */
  sync(info: DocInfo | null, sec: number): void {
    const key = info && info.dirty && sec > 0 ? `${info.docId}:${sec}` : null;
    if (key === this.armed) return;
    this.stop();
    this.armed = key;
    if (key) this.timer = setInterval(() => void this.tick(), sec * 1000);
  }

  /** One timer beat; exposed for tests. */
  async tick(): Promise<void> {
    const info = this.deps.current();
    if (!info || !info.dirty || this.inFlight) return;
    if (this.covered.get(info.docId) === info.docGeneration) return;
    const { docId, docGeneration } = info;
    this.inFlight = (async () => {
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
    const info = this.deps.current();
    if (info?.docId === docId) this.covered.set(docId, info.docGeneration);
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
  }
}

export const autosave = new AutosaveController({
  current: () => useDocStore.getState().info,
  write: (docId) => api.writeRecovery({ docId }),
  clear: (docId) => api.clearRecovery({ docId }),
  onFail: (info, e) =>
    toast("autosave.failed", { name: info.name }, { tone: "danger", detail: e instanceof Error ? e.message : String(e) }),
});

/** Mounted once by the shell. */
export function useAutosave(): void {
  const docId = useDocStore((s) => s.info?.docId ?? null);
  const dirty = useDocStore((s) => s.info?.dirty ?? false);
  const sec = useAppStore((s) => autosaveSecOf(s.settings));
  useEffect(() => {
    autosave.sync(useDocStore.getState().info, sec);
  }, [docId, dirty, sec]);
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
