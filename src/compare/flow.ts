/**
 * 문서 비교 job (P1-6): open document B **beside** the window's document (never through `docStore`),
 * run `compare_documents` with progress and 취소, then enter compare mode.
 *
 * The run lives in this module rather than in the dialog: the 암호 입력 prompt opens on top of the
 * 비교 dialog, and the dialog host only renders the top entry, so the dialog unmounts while the
 * password is asked. Its state (picked file, options, progress) is kept in `useCompareRun`.
 */
import { create } from "zustand";
import * as api from "../ipc/api";
import { useDocStore } from "../store/docStore";
import { useJobStore } from "../store/jobStore";
import { toast } from "../app/toastStore";
import { askPassword, closeDialog } from "../dialogs/dialogState";
import { baseName, message } from "../dialogs/flows";
import { useCompareStore } from "./state";
import type { DocId, DocInfo, JobEvent, JobId } from "../ipc/types";

export type ComparePhase = "idle" | "opening" | "comparing";

interface CompareRun {
  path: string | null;
  ignoreCase: boolean;
  phase: ComparePhase;
  done: number;
  total: number;
}

const INITIAL: CompareRun = { path: null, ignoreCase: false, phase: "idle", done: 0, total: 0 };

export const useCompareRun = create<CompareRun>(() => ({ ...INITIAL }));

/** What must be released however the run ends. */
const live: { run: number; jobId: JobId | null; docB: DocId | null; settle: (() => void) | null } = {
  run: 0,
  jobId: null,
  docB: null,
  settle: null,
};

function releaseB(): void {
  const docB = live.docB;
  live.docB = null;
  if (docB) void api.closeDocument({ docId: docB }).catch(() => undefined);
}

function idle(): void {
  useCompareRun.setState({ phase: "idle", done: 0, total: 0 });
}

/** `open_document` with the in-place password retry of F-01; `null` = cancelled or failed (toasted). */
async function openSecond(path: string, alive: () => boolean): Promise<DocInfo | null> {
  let password: string | undefined;
  for (;;) {
    try {
      return await api.openDocument({ path, password });
    } catch (e) {
      if (!alive()) return null;
      if (api.isSeePdfError(e) && (e.code === "passwordRequired" || e.code === "passwordWrong")) {
        const entered = await askPassword(baseName(path), e.code === "passwordWrong");
        if (entered === null || !alive()) return null;
        password = entered;
        continue;
      }
      const missing = api.isSeePdfError(e) && e.code === "notFound";
      toast(missing ? "error.fileMissing" : "error.openFailed", undefined, { tone: "danger", detail: message(e) });
      return null;
    }
  }
}

/** Resolves when the run is over; `true` = compare mode was entered. */
export async function startCompare(): Promise<boolean> {
  const { path, ignoreCase, phase } = useCompareRun.getState();
  const infoA = useDocStore.getState().info;
  if (!infoA || !path || phase !== "idle") return false;
  const run = ++live.run;
  const alive = () => run === live.run;
  useCompareRun.setState({ phase: "opening", done: 0, total: 0 });

  const infoB = await openSecond(path, alive);
  if (!infoB) {
    if (alive()) idle();
    return false;
  }
  if (!alive()) {
    void api.closeDocument({ docId: infoB.docId }).catch(() => undefined);
    return false;
  }
  live.docB = infoB.docId;
  useCompareRun.setState({ phase: "comparing" });

  return new Promise<boolean>((resolve) => {
    let finished = false;
    const settle = (entered: boolean) => {
      finished = true;
      if (live.settle === cancelSettle) live.settle = null;
      resolve(entered);
    };
    const cancelSettle = () => settle(false);
    live.settle = cancelSettle;

    const onEvent = (e: JobEvent) => {
      // the status bar follows the job even after a local 취소 (it shows the final state)
      useJobStore.getState().apply("compare", "compare.running", e);
      if (!alive()) return;
      switch (e.type) {
        case "started":
          live.jobId = e.jobId;
          useCompareRun.setState({ done: 0, total: e.total });
          break;
        case "progress":
          live.jobId = e.jobId;
          useCompareRun.setState({ done: e.done, total: e.total });
          break;
        case "done":
          live.jobId = null;
          if (e.compare) {
            live.docB = null; // owned by the compare session now
            useCompareStore.getState().enter({ report: e.compare, infoA, infoB });
            closeDialog("compare");
            useCompareRun.setState({ ...INITIAL });
            settle(true);
          } else {
            releaseB();
            idle();
            toast("compare.failed", undefined, { tone: "danger" });
            settle(false);
          }
          break;
        case "cancelled":
          // 취소, or (Stage 8) either document closed mid-job: a quiet end, no error toast
          live.jobId = null;
          releaseB();
          idle();
          settle(false);
          break;
        case "error":
          live.jobId = null;
          releaseB();
          idle();
          if (e.error.code !== "cancelled") toast("compare.failed", undefined, { tone: "danger", detail: e.error.message });
          settle(false);
          break;
      }
    };

    // Stage 8: pages are paired by text similarity, so an inserted / deleted page is its own row
    api
      .compareDocuments({ docA: infoA.docId, docB: infoB.docId, options: { ignoreCase, alignPages: true } }, onEvent)
      .then((jobId) => {
        // mock mode (and a fast engine) may have streamed every event before the id comes back
        if (finished) return;
        if (alive()) live.jobId = jobId;
        else void api.cancelJob({ jobId }).catch(() => false);
      })
      .catch((e) => {
        if (!alive()) return;
        releaseB();
        idle();
        if (!(api.isSeePdfError(e) && e.code === "cancelled")) {
          toast("compare.failed", undefined, { tone: "danger", detail: message(e) });
        }
        settle(false);
      });
  });
}

/** 취소 (or the dialog closing mid-run): stop the job and release document B. */
export function cancelCompare(): void {
  live.run += 1; // anything still in flight belongs to a dead run now
  const jobId = live.jobId;
  live.jobId = null;
  if (jobId !== null) void api.cancelJob({ jobId }).catch(() => false);
  releaseB();
  idle();
  live.settle?.();
}

/** The dialog was dismissed: forget the picked file as well. */
export function resetCompareRun(): void {
  if (useCompareRun.getState().phase !== "idle") cancelCompare();
  useCompareRun.setState({ ...INITIAL });
}
