/**
 * The seam between 업데이트 확인 and `@tauri-apps/plugin-updater` / `-process`.
 *
 * Everything the UI needs goes through [`UpdaterBackend`], so the store and the dialog never
 * import a Tauri package: the plugins are loaded with a dynamic `import()` on the first check
 * (keeping them out of the entry chunk), the mock adapter (`VITE_SEEPDF_MOCK=1`, plain browser)
 * answers "up to date", and tests install their own backend with [`setUpdaterBackend`].
 */
import { useMock } from "../ipc/env";

/** A release newer than the running app, as `latest.json` describes it. */
export interface AvailableUpdate {
  version: string;
  currentVersion: string;
  /** release notes from `latest.json`; may be empty */
  notes: string | null;
  /** ISO date of the release, when the manifest has one */
  date: string | null;
  /** Downloads the signed bundle. `total` is null when the server sent no Content-Length. */
  download(onProgress: (downloaded: number, total: number | null) => void): Promise<void>;
  /**
   * Installs what `download` fetched. On Windows this starts the installer and the app exits;
   * on macOS the `.app` is replaced in place and [`UpdaterBackend.relaunch`] finishes the job.
   */
  install(): Promise<void>;
}

export interface UpdaterBackend {
  /** `null` when the running version is the newest one. Rejects on a network or signature error. */
  check(): Promise<AvailableUpdate | null>;
  currentVersion(): Promise<string>;
  relaunch(): Promise<void>;
}

let override: UpdaterBackend | null = null;

/** Tests (and nothing else) swap the backend; `null` restores the real one. */
export function setUpdaterBackend(backend: UpdaterBackend | null): void {
  override = backend;
}

export function updaterBackend(): UpdaterBackend {
  if (override) return override;
  return useMock() ? mockBackend : tauriBackend;
}

/** Browser / mock mode: there is nothing to update. */
const mockBackend: UpdaterBackend = {
  async check() {
    return null;
  },
  async currentVersion() {
    return "0.0.0-mock";
  },
  async relaunch() {
    /* nothing to relaunch in a browser tab */
  },
};

const tauriBackend: UpdaterBackend = {
  async check() {
    const { check } = await import("@tauri-apps/plugin-updater");
    const update = await check();
    if (!update) return null;
    return {
      version: update.version,
      currentVersion: update.currentVersion,
      notes: update.body?.trim() || null,
      date: update.date ?? null,
      async download(onProgress) {
        let downloaded = 0;
        let total: number | null = null;
        await update.download((event) => {
          if (event.event === "Started") {
            total = event.data.contentLength ?? null;
            onProgress(0, total);
          } else if (event.event === "Progress") {
            downloaded += event.data.chunkLength;
            onProgress(downloaded, total);
          } else {
            onProgress(total ?? downloaded, total ?? downloaded);
          }
        });
      },
      async install() {
        await update.install();
      },
    };
  },
  async currentVersion() {
    const { getVersion } = await import("@tauri-apps/api/app");
    return getVersion();
  },
  async relaunch() {
    const { relaunch } = await import("@tauri-apps/plugin-process");
    await relaunch();
  },
};
