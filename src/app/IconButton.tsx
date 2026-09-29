import type { ComponentType, ReactNode } from "react";
import { Tooltip } from "./Tooltip";

export interface IconProps {
  size?: number | string;
  strokeWidth?: number | string;
  className?: string;
  "aria-hidden"?: boolean;
}

export interface IconButtonProps {
  icon: ComponentType<IconProps>;
  /** localised label — also the accessible name */
  label: string;
  shortcut?: string;
  active?: boolean;
  disabled?: boolean;
  size?: number;
  tone?: "default" | "danger";
  onClick?: () => void;
  badge?: ReactNode;
  /** the tooltip's text when it differs from `label` (e.g. why the button is disabled) */
  tooltip?: string;
}

/**
 * 28×28 with a 20 px lucide glyph, stroke 1.75, `currentColor` (UI_SPEC §3, §14.6).
 * Active = `--accent-subtle` background + 1 px accent border.
 */
export function IconButton({
  icon: Icon,
  label,
  shortcut,
  active = false,
  disabled = false,
  size = 20,
  tone = "default",
  onClick,
  badge,
  tooltip,
}: IconButtonProps) {
  return (
    <Tooltip label={tooltip ?? label} shortcut={shortcut}>
      <button
        type="button"
        className="icon-btn"
        data-active={active || undefined}
        data-tone={tone === "danger" ? "danger" : undefined}
        aria-label={label}
        aria-pressed={active}
        disabled={disabled}
        onClick={onClick}
      >
        <Icon size={size} strokeWidth={1.75} aria-hidden />
        {badge}
      </button>
    </Tooltip>
  );
}
