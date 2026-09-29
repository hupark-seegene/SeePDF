import { useSyncExternalStore, type ComponentType } from "react";
import { BookOpen, Highlighter, LayoutGrid, SquarePen, TextCursorInput } from "lucide-react";
import { useT } from "../i18n/useT";
import { useAppStore, type Mode } from "../store/appStore";
import { useDocStore } from "../store/docStore";
import { shortcutFor } from "../keys/keymap";
import { MODES } from "./tools";
import { Tooltip } from "./Tooltip";
import type { IconProps } from "./IconButton";
import type { CommandId } from "./useCommands";

/** v0.3 pkg5 (H14): the glyph each mode keeps when its label no longer fits (UI_SPEC §1, §14.6). */
const MODE_ICON: Record<Mode, ComponentType<IconProps>> = {
  read: BookOpen,
  annotate: Highlighter,
  edit: SquarePen,
  pages: LayoutGrid,
  form: TextCursorInput,
};

/** UI_SPEC §1: below a 900 px window the mode labels become icon-only (`useCommands` NARROW_WINDOW_PX). */
const NARROW_QUERY = "(max-width: 899.98px)";

function subscribeNarrow(onChange: () => void): () => void {
  if (typeof window === "undefined" || typeof window.matchMedia !== "function") return () => undefined;
  const mq = window.matchMedia(NARROW_QUERY);
  mq.addEventListener("change", onChange);
  return () => mq.removeEventListener("change", onChange);
}

function narrowNow(): boolean {
  return typeof window !== "undefined" && typeof window.matchMedia === "function" && window.matchMedia(NARROW_QUERY).matches;
}

/** True while the window is narrower than 900 px. */
export function useNarrowWindow(): boolean {
  return useSyncExternalStore(subscribeNarrow, narrowNow, () => false);
}

/**
 * 읽기 · 주석 · 편집 · 페이지 · 양식 (⌘1–⌘5) — the primary navigation (UI_SPEC §2).
 * Changing mode changes the tool strip, the canvas cursor, the inspector and what a click does.
 * In a narrow window (`narrow`, by default the window width) each segment is its icon alone — the
 * tooltip and the accessible name keep the label — so the toolbar never wraps.
 */
export function ModeSwitcher({ run, narrow }: { run: (id: CommandId) => void; narrow?: boolean }) {
  const t = useT();
  const os = useAppStore((s) => s.os);
  const mode = useAppStore((s) => s.mode);
  const hasDoc = useDocStore((s) => s.info !== null);
  const narrowWindow = useNarrowWindow();
  const iconOnly = narrow ?? narrowWindow;

  return (
    <div className="segmented" role="tablist" aria-label={t("mode.read")} data-icon-only={iconOnly || undefined}>
      {MODES.map((m) => {
        const Icon = MODE_ICON[m.id];
        return (
          <Tooltip key={m.id} label={t(m.labelKey)} shortcut={shortcutFor(m.keyId, os)}>
            <button
              type="button"
              role="tab"
              className="segment"
              aria-selected={mode === m.id}
              aria-label={iconOnly ? t(m.labelKey) : undefined}
              data-active={mode === m.id || undefined}
              disabled={!hasDoc}
              onClick={() => run(m.keyId as CommandId)}
            >
              {iconOnly ? <Icon size={16} strokeWidth={1.75} aria-hidden /> : t(m.labelKey)}
            </button>
          </Tooltip>
        );
      })}
    </div>
  );
}
