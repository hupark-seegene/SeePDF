/**
 * 도구별 기본 스타일 (P1-12): keeps `annotStore`'s per-tool styles and `Settings.toolDefaults`
 * in step while the annotation host is running.
 *
 * * On start: load the validated overrides from the settings and apply the armed tool's style.
 * * On every tool change: that tool's remembered style becomes the current one.
 * * On every change of the overrides (a panel edit with nothing selected, 이 스타일을 기본값으로,
 *   기본값으로 재설정): write them back, debounced, because a slider drag changes them per frame.
 */
import { useAnnotStore } from "../store/annotStore";
import { useAppStore } from "../store/appStore";
import { mergeToolDefaults, readToolDefaults } from "../store/toolStyles";

/** A slider drag is one write, not sixty. */
export const TOOL_DEFAULTS_SAVE_MS = 400;

/**
 * The settings are adopted **once per window session**: after that the in-memory copy is the
 * newest one (its writes may still be in flight when the host restarts for the next document).
 */
let adopted = false;

export function startToolDefaultsSync(): () => void {
  const adopt = () => {
    const settings = useAppStore.getState().settings;
    if (adopted || !settings) return false;
    adopted = true;
    useAnnotStore.getState().loadToolDefaults(readToolDefaults(settings.toolDefaults));
    return true;
  };
  adopt();
  useAnnotStore.getState().activateTool(useAppStore.getState().tool);

  let written = useAnnotStore.getState().toolDefaults;
  let timer: ReturnType<typeof setTimeout> | null = null;
  const flush = () => {
    timer = null;
    const overrides = useAnnotStore.getState().toolDefaults;
    written = overrides;
    const current = useAppStore.getState().settings?.toolDefaults;
    void useAppStore.getState().patchSettings({ toolDefaults: mergeToolDefaults(current, overrides) });
  };

  const offAnnot = useAnnotStore.subscribe((s) => {
    if (s.toolDefaults === written) return;
    if (timer) clearTimeout(timer);
    timer = setTimeout(flush, TOOL_DEFAULTS_SAVE_MS);
  });
  const offApp = useAppStore.subscribe((s, prev) => {
    if (s.tool !== prev.tool) useAnnotStore.getState().activateTool(s.tool);
    // Settings that arrive after the host started (a slow bootstrap).
    if (!prev.settings && s.settings && adopt()) written = useAnnotStore.getState().toolDefaults;
  });

  return () => {
    offAnnot();
    offApp();
    if (timer) {
      clearTimeout(timer);
      flush();
    }
  };
}
