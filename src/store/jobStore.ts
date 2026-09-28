/**
 * Long-running jobs (search, export, save, split, OCR, annotation scan) feeding the status-bar
 * progress slot (UI_SPEC §8): label + determinate bar + cancel ×. Owner from Stage 1: (f), shared.
 *
 * Every backend job id can be cancelled with `cancel_job` (IPC_CONTRACT §8). OCR is different: the
 * loop lives in the frontend (a tesseract.js worker pool, one `ocr_apply` per page), so it takes a
 * **local** job id from `localJobId()` and registers a canceller with `registerCanceller()`. The
 * status bar calls `cancel(id)` either way and does not need to know which kind it is.
 */
import { create } from "zustand";
import * as api from "../ipc/api";
import type { JobEvent, JobId } from "../ipc/types";

export type JobKind = "search" | "export" | "save" | "split" | "ocr" | "scan" | "print" | "compare";
export type JobState = "running" | "done" | "cancelled" | "error";

export interface Job {
  id: JobId;
  kind: JobKind;
  /** i18n key rendered in the status bar */
  labelKey: string;
  done: number;
  total: number;
  note?: string;
  state: JobState;
  startedAt: number;
  error?: string;
}

export interface JobStore {
  jobs: Job[];
  /** the job the status bar shows (the newest running one) */
  active: Job | null;
  start(job: Pick<Job, "id" | "kind" | "labelKey"> & Partial<Job>): void;
  apply(kind: JobKind, labelKey: string, e: JobEvent): void;
  /** Patch a job the frontend drives itself (OCR): progress, note, final state. */
  update(id: JobId, patch: Partial<Omit<Job, "id">>): void;
  cancel(id: JobId): Promise<void>;
  clearFinished(): void;
}

function pickActive(jobs: Job[]): Job | null {
  return [...jobs].reverse().find((j) => j.state === "running") ?? null;
}

/**
 * Ids for jobs that never reach `cancel_job`. Negative so they can never collide with an engine
 * `JobId` (u32 counter starting at 1).
 */
let localCounter = 0;
export function localJobId(): JobId {
  localCounter -= 1;
  return localCounter;
}

export function isLocalJobId(id: JobId): boolean {
  return id < 0;
}

/** Frontend-side cancellers, keyed by job id (OCR aborts its worker pool through one of these). */
const cancellers = new Map<JobId, () => void>();

/** Register `fn` as the cancel action for `id`; returns an unregister function. */
export function registerCanceller(id: JobId, fn: () => void): () => void {
  cancellers.set(id, fn);
  return () => {
    if (cancellers.get(id) === fn) cancellers.delete(id);
  };
}

/**
 * Remaining time for the status bar / OCR dialog (`ocr.remaining`), from the mean time per unit so
 * far. `null` until at least one unit is done, so the UI can show "…" instead of a wild number.
 */
export function etaMs(job: Pick<Job, "done" | "total" | "startedAt" | "state">): number | null {
  if (job.state !== "running" || job.done <= 0 || job.total <= 0 || job.done >= job.total) return null;
  const elapsed = Date.now() - job.startedAt;
  if (elapsed <= 0) return null;
  return Math.round((elapsed / job.done) * (job.total - job.done));
}

export const useJobStore = create<JobStore>((set, get) => ({
  jobs: [],
  active: null,

  start(job) {
    const full: Job = { done: 0, total: 0, state: "running", startedAt: Date.now(), ...job };
    const jobs = [...get().jobs.filter((j) => j.id !== full.id), full];
    set({ jobs, active: pickActive(jobs) });
  },

  apply(kind, labelKey, e) {
    const jobs = [...get().jobs];
    const idx = jobs.findIndex((j) => j.id === e.jobId);
    const base: Job = idx >= 0
      ? jobs[idx]
      : { id: e.jobId, kind, labelKey, done: 0, total: 0, state: "running", startedAt: Date.now() };
    let next: Job = base;
    switch (e.type) {
      case "started":
        next = { ...base, total: e.total, state: "running" };
        break;
      case "progress":
        next = { ...base, done: e.done, total: e.total, note: e.note, state: "running" };
        break;
      case "done":
        next = { ...base, done: base.total, state: "done" };
        break;
      case "cancelled":
        next = { ...base, done: e.done, state: "cancelled" };
        break;
      case "error":
        next = { ...base, state: "error", error: e.error.code };
        break;
    }
    if (idx >= 0) jobs[idx] = next;
    else jobs.push(next);
    set({ jobs, active: pickActive(jobs) });
  },

  update(id, patch) {
    const jobs = get().jobs.map((j) => (j.id === id ? { ...j, ...patch } : j));
    set({ jobs, active: pickActive(jobs) });
  },

  async cancel(id) {
    // Frontend-driven job (OCR): stop the worker pool first, so the in-flight page dies immediately.
    cancellers.get(id)?.();
    if (!isLocalJobId(id)) await api.cancelJob({ jobId: id }).catch(() => false);
    const jobs = get().jobs.map((j) => (j.id === id ? { ...j, state: "cancelled" as JobState } : j));
    set({ jobs, active: pickActive(jobs) });
  },

  clearFinished() {
    const jobs = get().jobs.filter((j) => j.state === "running");
    set({ jobs, active: pickActive(jobs) });
  },
}));

// ---------------------------------------------------------------------------
// WORKPLAN §6 probe hook (module f) — the only place in the app that reaches src/ocr/probe.ts.
// The comparison folds to `false` in a normal build, so rolldown drops the block *and* the chunk:
// `npx vite build` emits no probe code at all. Enable it with `VITE_SEEPDF_OCR_PROBE=1 vite build`
// (or `… npm run dev`). Keep it a direct `import.meta.env.X === "1"` — anything cleverer defeats the
// constant folding and ships a dead 27 kB chunk.
// ---------------------------------------------------------------------------
if (import.meta.env.VITE_SEEPDF_OCR_PROBE === "1" && typeof window !== "undefined") {
  void import("../ocr/probe").then((m) => m.mountOcrProbe()).catch(() => {});
}
