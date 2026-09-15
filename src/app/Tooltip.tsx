import { useEffect, useRef, useState, type ReactNode } from "react";

/**
 * Icon-only buttons always get a tooltip after 500 ms, with the shortcut appended in a dimmer
 * weight (UI_SPEC §14.7 rule 5). Rendered inline (fixed position) — no portal, no library.
 */
export interface TooltipProps {
  label: string;
  shortcut?: string;
  placement?: "bottom" | "top";
  children: ReactNode;
}

const DELAY_MS = 500;

export function Tooltip({ label, shortcut, placement = "bottom", children }: TooltipProps) {
  const hostRef = useRef<HTMLSpanElement>(null);
  const timer = useRef<number | undefined>(undefined);
  const [pos, setPos] = useState<{ x: number; y: number } | null>(null);

  useEffect(() => () => window.clearTimeout(timer.current), []);

  const show = () => {
    window.clearTimeout(timer.current);
    timer.current = window.setTimeout(() => {
      const rect = hostRef.current?.getBoundingClientRect();
      if (!rect) return;
      setPos({
        x: rect.left + rect.width / 2,
        y: placement === "bottom" ? rect.bottom + 6 : rect.top - 6,
      });
    }, DELAY_MS);
  };

  const hide = () => {
    window.clearTimeout(timer.current);
    setPos(null);
  };

  return (
    <span
      ref={hostRef}
      className="tooltip-host"
      onPointerEnter={show}
      onPointerLeave={hide}
      onPointerDown={hide}
      onBlur={hide}
    >
      {children}
      {pos && (
        <span
          role="tooltip"
          className="tooltip"
          data-placement={placement}
          style={{ left: pos.x, top: pos.y }}
        >
          {label}
          {shortcut && <span className="tooltip-key">{shortcut}</span>}
        </span>
      )}
    </span>
  );
}
