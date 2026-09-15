import { IconButton } from "./IconButton";
import { TOOL_STRIP } from "./tools";
import { useT } from "../i18n/useT";
import { useAppStore } from "../store/appStore";
import { shortcutFor } from "../keys/keymap";
import { toolController } from "../tools/ToolController";
import { resetFormFields, useFormStore } from "../forms/formStore";

/**
 * The 40 px contextual tool strip — 주석 / 편집 / 양식 only (UI_SPEC §3).
 * Arming a tool goes through `toolController`, the seam Stage 1 (d) implements.
 */
export function ToolStrip() {
  const t = useT();
  const os = useAppStore((s) => s.os);
  const mode = useAppStore((s) => s.mode);
  const tool = useAppStore((s) => s.tool);
  const setTool = useAppStore((s) => s.setTool);
  // 양식 mode's two non-tools (UI_SPEC §3): a toggle and a one-shot action, both owned by (d).
  const fieldHighlight = useFormStore((s) => s.highlight);
  const tools = TOOL_STRIP[mode];

  if (tools.length === 0) return null;

  return (
    <div className="toolstrip" role="toolbar" aria-label={t("menu.tools")}>
      {tools.map((def) => (
        <IconButton
          key={def.id}
          icon={def.icon}
          label={t(def.labelKey)}
          shortcut={def.keyId ? shortcutFor(def.keyId, os) : undefined}
          active={
            def.kind === "toggle"
              ? fieldHighlight
              : def.kind === undefined || def.kind === "tool"
                ? tool === def.id
                : undefined
          }
          onClick={() => {
            if (def.kind === "toggle") return useFormStore.getState().toggleHighlight();
            // 모든 필드 지우기 — one `reset_form`, one undo step (IPC_CONTRACT §7.2)
            if (def.kind === "action") return void resetFormFields();
            setTool(def.id);
            toolController.arm(def.id);
          }}
        />
      ))}
    </div>
  );
}
