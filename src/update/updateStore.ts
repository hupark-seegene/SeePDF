/**
 * 업데이트 확인 (v0.2.0): one small state machine shared by the launch check, the menu item and
 * the dialog.
 *
 *   idle → checking → upToDate
 *                   → noInfo (v0.3 pkg5, H1: the feed has no latest.json — a release published
 *                     without signed updater bundles answers 404; neutral, not an error)
 *                   → available → downloading → ready → installing → (the app restarts)
 *                   → error (retry = check again; a failed download also lands here)
 *
 * The launch check is **silent**: it never opens anything, it only raises a toast with
 * "업데이트…" when there is a newer version, and a network error stays in the log (a machine
 * behind a firewall should not be nagged at every start). 업데이트 확인… is explicit: it opens
 * the dialog, which renders whatever phase the store is in.
 */
import { create } from "zustand";
import { updaterBackend, type AvailableUpdate } from "./updater";

export type UpdatePhase =
  | "idle"
  | "checking"
  | "upToDate"
  | "noInfo"
  | "available"
  | "downloading"
  | "ready"
  | "installing"
  | "error";

interface UpdateState {
  phase: UpdatePhase;
  currentVersion: string | null;
  /** the newer version, while one is known */
  version: string | null;
  notes: string | null;
  date: string | null;
  downloaded: number;
  total: number | null;
  /** developer detail of the last failure (shown small, under the localised message) */
  error: string | null;
  /** which step failed, so 다시 시도 repeats the right one */
  failedStep: "check" | "download" | "install" | null;
}

const INITIAL: UpdateState = {
  phase: "idle",
  currentVersion: null,
  version: null,
  notes: null,
  date: null,
  downloaded: 0,
  total: null,
  error: null,
  failedStep: null,
};

export const useUpdateStore = create<UpdateState>(() => ({ ...INITIAL }));

/** The update `check` returned; kept outside the store because it is not plain data. */
let pending: AvailableUpdate | null = null;

function message(e: unknown): string {
  if (e instanceof Error) return e.message;
  return typeof e === "string" ? e : JSON.stringify(e);
}

/**
 * v0.3 pkg5 (H1): the endpoint answered, but with no release manifest — tauri-plugin-updater's
 * `ReleaseNotFound` ("Could not fetch a valid release JSON from the remote"), which is what a 404 on
 * `releases/latest/download/latest.json` becomes. Nothing is wrong with this copy of SeePDF.
 */
export function isNoUpdateInfo(error: string): boolean {
  return /valid release JSON|release ?not ?found|\b404\b/i.test(error);
}

/**
 * Asks the release feed for a newer version. A check while another one runs, or once an update
 * is downloading / downloaded, is a no-op: the dialog already shows that state.
 */
export async function checkForUpdates(): Promise<void> {
  const { phase } = useUpdateStore.getState();
  if (phase === "checking" || phase === "downloading" || phase === "ready" || phase === "installing") return;
  useUpdateStore.setState({ ...INITIAL, phase: "checking", currentVersion: useUpdateStore.getState().currentVersion });
  const backend = updaterBackend();
  try {
    const currentVersion = await backend.currentVersion().catch(() => null);
    useUpdateStore.setState({ currentVersion });
    const update = await backend.check();
    pending = update;
    if (!update) {
      useUpdateStore.setState({ phase: "upToDate" });
      return;
    }
    useUpdateStore.setState({
      phase: "available",
      version: update.version,
      currentVersion: update.currentVersion || currentVersion,
      notes: update.notes,
      date: update.date,
    });
  } catch (e) {
    pending = null;
    const detail = message(e);
    if (isNoUpdateInfo(detail)) {
      useUpdateStore.setState({ phase: "noInfo", error: detail });
      return;
    }
    useUpdateStore.setState({ phase: "error", error: detail, failedStep: "check" });
  }
}

/** Downloads the bundle `check` found, reporting progress into the store. */
export async function downloadUpdate(): Promise<void> {
  const update = pending;
  if (!update || useUpdateStore.getState().phase !== "available") return;
  useUpdateStore.setState({ phase: "downloading", downloaded: 0, total: null, error: null, failedStep: null });
  try {
    await update.download((downloaded, total) => useUpdateStore.setState({ downloaded, total }));
    useUpdateStore.setState({ phase: "ready" });
  } catch (e) {
    useUpdateStore.setState({ phase: "error", error: message(e), failedStep: "download" });
  }
}

/**
 * 다시 시작: installs the downloaded update and relaunches. `confirmLeave` is the unsaved-changes
 * gate (저장 / 저장 안 함 / 취소) — on Windows `install` ends the process, so a dirty document must
 * be settled first. Resolves `false` when the user cancelled.
 */
export async function installAndRestart(confirmLeave: () => Promise<boolean>): Promise<boolean> {
  const update = pending;
  if (!update || useUpdateStore.getState().phase !== "ready") return false;
  if (!(await confirmLeave())) return false;
  useUpdateStore.setState({ phase: "installing" });
  try {
    await update.install();
    await updaterBackend().relaunch();
    return true;
  } catch (e) {
    useUpdateStore.setState({ phase: "error", error: message(e), failedStep: "install" });
    return false;
  }
}

/** 다시 시도 after an error: a failed download or install starts over from the check. */
export function retryUpdate(): Promise<void> {
  useUpdateStore.setState({ phase: "idle" });
  return checkForUpdates();
}

/** Download progress as a whole percentage, or null while the size is unknown. */
export function progressPercent(downloaded: number, total: number | null): number | null {
  if (!total || total <= 0) return null;
  return Math.max(0, Math.min(100, Math.round((downloaded / total) * 100)));
}

/** Tests only. */
export function resetUpdateStore(): void {
  pending = null;
  useUpdateStore.setState({ ...INITIAL });
}
