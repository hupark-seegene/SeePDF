/**
 * The launch check (설정 › 일반 › 시작할 때 업데이트 확인, on by default). Main window only — a
 * per-document window runs the same `App` and must not check again. Silent: a newer version is a
 * toast with 업데이트…, anything else (up to date, offline, a firewall) says nothing.
 */
import { useMock, windowLabel } from "../ipc/env";
import { toast } from "../app/toastStore";
import { openDialog } from "../dialogs/dialogState";
import type { Settings } from "../ipc/types";
import { checkForUpdates, useUpdateStore } from "./updateStore";

export function wantsLaunchCheck(settings: Pick<Settings, "checkUpdates"> | null, label = windowLabel()): boolean {
  return label === "main" && settings?.checkUpdates !== false;
}

export async function checkOnLaunch(settings: Pick<Settings, "checkUpdates"> | null, opts: { force?: boolean } = {}): Promise<void> {
  if (!wantsLaunchCheck(settings)) return;
  if (useMock() && !opts.force) return;
  await checkForUpdates();
  const { phase, version } = useUpdateStore.getState();
  if (phase !== "available" || !version) return;
  toast("update.toast", { version }, {
    timeoutMs: 15000,
    actions: [{ labelKey: "update.toast.action", onSelect: () => openDialog("update") }],
  });
}
