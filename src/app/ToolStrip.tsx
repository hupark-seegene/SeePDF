import { Fragment } from "react";
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
      {tools.map((def, i) => (
        <Fragment key={def.id}>
        {/* v0.3 (pkg2): a divider where a strip section starts (양식 ▸ 필드 만들기, 양식 데이터) */}
        {def.group && def.group !== tools[i - 1]?.group && (
          <span className="toolstrip-sep" role="separator" aria-label={t(def.group)} />
        )}
        <IconButton
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
            // 모든 필드 지우기 — one `reset_form`, one undo step (IPC_CONTRACT §7.2); v0.3: the
            // form-data and 평면화 buttons run their `forms/formActions` action (a lazy chunk)
            if (def.kind === "action") {
              if (!def.action) return void resetFormFields();
              const action = def.action;
              return void import("../forms/formActions").then((m) => m.runFormAction(action));
            }
            setTool(def.id);
            toolController.arm(def.id);
          }}
        />
        </Fragment>
      ))}
    </div>
  );
}
