/**
 * Long-running jobs (search, export, save, split, OCR, annotation scan) feeding the status-bar
 * progress slot (UI_SPEC §8): label + determinate bar + cancel ×. Owner from Stage 1: (f), shared.
 *
 * Every job id can be cancelled with `cancel_job` (IPC_CONTRACT §8).
 */
import { create } from "zustand";
import * as api from "../ipc/api";
import type { JobEvent, JobId } from "../ipc/types";

export type JobKind = "search" | "export" | "save" | "split" | "ocr" | "scan" | "print";
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
  cancel(id: JobId): Promise<void>;
  clearFinished(): void;
}

function pickActive(jobs: Job[]): Job | null {
  return [...jobs].reverse().find((j) => j.state === "running") ?? null;
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

  async cancel(id) {
    await api.cancelJob({ jobId: id }).catch(() => false);
    const jobs = get().jobs.map((j) => (j.id === id ? { ...j, state: "cancelled" as JobState } : j));
    set({ jobs, active: pickActive(jobs) });
  },

  clearFinished() {
    const jobs = get().jobs.filter((j) => j.state === "running");
    set({ jobs, active: pickActive(jobs) });
  },
}));
