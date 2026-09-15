/**
 * The shared modal shell for every dialog of UI_SPEC §10: `--elevation-3`, `--radius-xl`, Esc
 * closes, the primary button is the accent one and 36 px tall.
 *
 * Rendered in the webview (never a native sheet) so it is themed and localised like everything else.
 */
import { useEffect, useId, useRef, type ReactNode } from "react";
import { useT } from "../i18n/useT";
import "./dialogs.css";

export interface DialogProps {
  titleKey: string;
  titleParams?: Record<string, string | number>;
  /** sm 380 · md 480 · lg 640 · xl 760 */
  size?: "sm" | "md" | "lg" | "xl";
  onClose(): void;
  /** the accent primary button; omitted for pure prompts that render their own footer */
  primary?: { labelKey: string; onSelect(): void; disabled?: boolean; danger?: boolean };
  secondary?: { labelKey: string; onSelect(): void };
  cancelKey?: string;
  footerExtra?: ReactNode;
  children: ReactNode;
}

export function Dialog({
  titleKey,
  titleParams,
  size = "md",
  onClose,
  primary,
  secondary,
  cancelKey = "common.cancel",
  footerExtra,
  children,
}: DialogProps) {
  const t = useT();
  const id = useId();
  const ref = useRef<HTMLDivElement>(null);

  // focus the first control so the keyboard works immediately, and restore it on close
  useEffect(() => {
    const previous = document.activeElement as HTMLElement | null;
    const first = ref.current?.querySelector<HTMLElement>(
      "input:not([type=hidden]), select, textarea, button.primary, button",
    );
    first?.focus();
    return () => previous?.focus?.();
  }, []);

  return (
    <div className="dlg-backdrop" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div
        className="dlg"
        data-size={size}
        role="dialog"
        aria-modal="true"
        aria-labelledby={id}
        ref={ref}
        onKeyDown={(e) => {
          if (e.key === "Escape") {
            e.stopPropagation();
            onClose();
          }
        }}
      >
        <header className="dlg-head">
          <h2 className="text-lg" id={id}>
            {t(titleKey, titleParams)}
          </h2>
        </header>
        <div className="dlg-body">{children}</div>
        <footer className="dlg-foot">
          {footerExtra}
          <span className="dlg-spacer" />
          <button type="button" className="btn quiet" onClick={onClose}>
            {t(cancelKey)}
          </button>
          {secondary && (
            <button type="button" className="btn" onClick={secondary.onSelect}>
              {t(secondary.labelKey)}
            </button>
          )}
          {primary && (
            <button
              type="button"
              className="btn primary dlg-primary"
              data-danger={primary.danger || undefined}
              disabled={primary.disabled}
              onClick={primary.onSelect}
            >
              {t(primary.labelKey)}
            </button>
          )}
        </footer>
      </div>
    </div>
  );
}

/** One labelled row inside a dialog body. */
export function Row({ labelKey, hintKey, children }: { labelKey?: string; hintKey?: string; children: ReactNode }) {
  const t = useT();
  return (
    <div className="dlg-row">
      {labelKey && <span className="dlg-label text-sm">{t(labelKey)}</span>}
      <div className="dlg-control">
        {children}
        {hintKey && <p className="dlg-hint text-xs">{t(hintKey)}</p>}
      </div>
    </div>
  );
}
