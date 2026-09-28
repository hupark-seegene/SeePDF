/**
 * The shared modal shell for every dialog of UI_SPEC §10: `--elevation-3`, `--radius-xl`, Esc
 * closes, the primary button is the accent one and 36 px tall.
 *
 * Rendered in the webview (never a native sheet) so it is themed and localised like everything else.
 * It is modal for the keyboard too: Tab / Shift+Tab wrap inside it, focus that lands on the app
 * behind it is pulled back, and Esc closes it wherever focus is.
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

  const onCloseRef = useRef(onClose);
  onCloseRef.current = onClose;

  // Focus trap: whatever moves focus onto the app behind the backdrop (a click that slipped
  // through, a programmatic focus) is sent back into the dialog. Another modal surface (the OCR
  // sheet) or a menu opened from the dialog keeps its focus.
  useEffect(() => {
    const onFocusIn = (e: FocusEvent) => {
      const dlg = ref.current;
      const target = e.target as HTMLElement | null;
      if (!dlg || !target || dlg.contains(target) || typeof target.closest !== "function") return;
      if (target.closest('[role="dialog"], [role="menu"]')) return;
      focusables(dlg)[0]?.focus();
    };
    // Esc from wherever focus is (the `.dlg` handler below only sees keys from inside it).
    const onKeyDown = (e: KeyboardEvent) => {
      const dlg = ref.current;
      if (e.key !== "Escape" || !dlg) return;
      const target = e.target as Node | null;
      if (target && dlg.contains(target)) return;
      if (target instanceof HTMLElement && target.closest('[role="dialog"], [role="menu"]')) return;
      e.stopPropagation();
      onCloseRef.current();
    };
    document.addEventListener("focusin", onFocusIn);
    window.addEventListener("keydown", onKeyDown, true);
    return () => {
      document.removeEventListener("focusin", onFocusIn);
      window.removeEventListener("keydown", onKeyDown, true);
    };
  }, []);

  // Focus the first control so the keyboard works immediately, and restore it on close. Declared
  // after the trap, so the trap is already gone when focus goes back to the app.
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
            return;
          }
          if (e.key === "Tab" && ref.current) {
            // wrap at the edges instead of walking out onto the app behind the backdrop
            const list = focusables(ref.current);
            if (list.length === 0) return;
            const first = list[0];
            const last = list[list.length - 1];
            const active = document.activeElement;
            if (e.shiftKey && (active === first || !ref.current.contains(active))) {
              e.preventDefault();
              last.focus();
            } else if (!e.shiftKey && (active === last || !ref.current.contains(active))) {
              e.preventDefault();
              first.focus();
            }
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

const FOCUSABLE =
  'a[href], button:not([disabled]), input:not([disabled]):not([type=hidden]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';

/** The dialog's keyboard-reachable controls, in tab order (hidden ones excluded). */
function focusables(root: HTMLElement): HTMLElement[] {
  return Array.from(root.querySelectorAll<HTMLElement>(FOCUSABLE)).filter(
    (el) => !el.closest("[hidden], [inert]") && el.getAttribute("aria-hidden") !== "true",
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
