import { useT } from "../i18n/useT";
import { useAppStore } from "../store/appStore";
import { useDocStore } from "../store/docStore";
import { shortcutFor } from "../keys/keymap";
import { MODES } from "./tools";
import { Tooltip } from "./Tooltip";
import type { CommandId } from "./useCommands";

/**
 * 읽기 · 주석 · 편집 · 페이지 · 양식 (⌘1–⌘5) — the primary navigation (UI_SPEC §2).
 * Changing mode changes the tool strip, the canvas cursor, the inspector and what a click does.
 */
export function ModeSwitcher({ run }: { run: (id: CommandId) => void }) {
  const t = useT();
  const os = useAppStore((s) => s.os);
  const mode = useAppStore((s) => s.mode);
  const hasDoc = useDocStore((s) => s.info !== null);

  return (
    <div className="segmented" role="tablist" aria-label={t("mode.read")}>
      {MODES.map((m) => (
        <Tooltip key={m.id} label={t(m.labelKey)} shortcut={shortcutFor(m.keyId, os)}>
          <button
            type="button"
            role="tab"
            className="segment"
            aria-selected={mode === m.id}
            data-active={mode === m.id || undefined}
            disabled={!hasDoc}
            onClick={() => run(m.keyId as CommandId)}
          >
            {t(m.labelKey)}
          </button>
        </Tooltip>
      ))}
    </div>
  );
}
