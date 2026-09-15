/**
 * Toasts (UI_SPEC §10: "a completion toast with Finder에서 보기 / 폴더 열기").
 *
 * Bottom-trailing stack above the status bar. Errors keep `message` behind 자세히 and never show it
 * directly (IPC_CONTRACT §2). `toast()` in `toastStore.ts` is the entry point; this renderer is
 * mounted lazily while something is queued.
 */
import { useEffect, useState } from "react";
import { X } from "lucide-react";
import { useT } from "../i18n/useT";
import { useToastStore, type Toast } from "./toastStore";
import "./overlays.css";

export default function Toasts() {
  const toasts = useToastStore((s) => s.toasts);
  if (toasts.length === 0) return null;
  return (
    <div className="toast-layer" role="region" aria-live="polite">
      {toasts.map((toast) => (
        <ToastRow key={toast.id} toast={toast} />
      ))}
    </div>
  );
}

function ToastRow({ toast }: { toast: Toast }) {
  const t = useT();
  const dismiss = useToastStore((s) => s.dismiss);
  const [details, setDetails] = useState(false);

  useEffect(() => {
    if (toast.timeoutMs <= 0) return;
    const timer = window.setTimeout(() => dismiss(toast.id), toast.timeoutMs);
    return () => window.clearTimeout(timer);
  }, [toast.id, toast.timeoutMs, dismiss]);

  return (
    <div className="toast" data-tone={toast.tone}>
      <span className="toast-text text-sm">{t(toast.messageKey, toast.params)}</span>
      {toast.actions?.map((action) => (
        <button
          key={action.labelKey}
          type="button"
          className="btn quiet toast-action"
          onClick={() => {
            action.onSelect();
            dismiss(toast.id);
          }}
        >
          {t(action.labelKey)}
        </button>
      ))}
      {toast.detail && (
        <button type="button" className="btn quiet toast-action" onClick={() => setDetails((v) => !v)}>
          {t("error.details")}
        </button>
      )}
      <button type="button" className="icon-btn toast-close" aria-label={t("common.close")} onClick={() => dismiss(toast.id)}>
        <X size={14} strokeWidth={1.75} aria-hidden />
      </button>
      {details && toast.detail && <p className="toast-detail text-xs mono">{toast.detail}</p>}
    </div>
  );
}
