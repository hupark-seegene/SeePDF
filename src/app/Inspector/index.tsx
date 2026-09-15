/**
 * The 260 px properties panel frame (UI_SPEC §7). The frame is on the critical path because
 * `App.tsx` imports it directly; everything with logic in it — the per-context sections, the
 * swatches, the sliders and the `update_annotation` plumbing — lives in the lazily-imported body,
 * so an opened document that never leaves 읽기 never downloads it.
 */
import { Suspense, lazy } from "react";
import { PanelRight } from "lucide-react";
import { IconButton } from "../IconButton";
import { useT } from "../../i18n/useT";
import { useAppStore } from "../../store/appStore";

const InspectorBody = lazy(() => import("./InspectorBody"));

export function Inspector() {
  const t = useT();
  const toggle = useAppStore((s) => s.toggleInspector);

  return (
    <aside className="inspector" aria-label={t("prop.title")}>
      <div className="inspector-head">
        <h2 className="text-md">{t("prop.title")}</h2>
        <IconButton icon={PanelRight} label={t("a11y.closePanel")} onClick={() => toggle(false)} />
      </div>

      <div className="inspector-body">
        <Suspense fallback={<p className="empty">{t("common.loading")}</p>}>
          <InspectorBody />
        </Suspense>
      </div>
    </aside>
  );
}

export default Inspector;
