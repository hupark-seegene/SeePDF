import { Suspense, lazy, useCallback, useEffect, useRef } from "react";
import { ListTree, MessageSquare, RectangleVertical, Search } from "lucide-react";
import { IconButton } from "./IconButton";
import { useT } from "../i18n/useT";
import { useAppStore, type SidebarTab } from "../store/appStore";
import { shortcutFor } from "../keys/keymap";
import { Thumbnails } from "../sidebar/Thumbnails";
import { Outline } from "../sidebar/Outline";
import { SearchPanel } from "../sidebar/SearchPanel";

// (d): the 주석 list pulls in the annotation actions, so it is code-split like every Stage 1 panel.
const AnnotationList = lazy(() => import("../sidebar/AnnotationList"));
import type { IconProps } from "./IconButton";
import type { ComponentType } from "react";

const TABS: { id: SidebarTab; labelKey: string; icon: ComponentType<IconProps>; keyId: string }[] = [
  { id: "thumbnails", labelKey: "sidebar.tab.thumbnails", icon: RectangleVertical, keyId: "view.sidebar.thumbnails" },
  { id: "outline", labelKey: "sidebar.tab.outline", icon: ListTree, keyId: "view.sidebar.outline" },
  { id: "annotations", labelKey: "sidebar.tab.annotations", icon: MessageSquare, keyId: "view.sidebar.annotations" },
  { id: "search", labelKey: "sidebar.tab.search", icon: Search, keyId: "view.sidebar.search" },
];

/**
 * Sidebar chrome (UI_SPEC §4): 4 tabs, drag-resize 180–420 px, state persists per app.
 * The panel bodies are Stage 1's: (c) owns 축소판/목차/검색, (d) owns 주석.
 */
export function SidebarFrame() {
  const t = useT();
  const os = useAppStore((s) => s.os);
  const tab = useAppStore((s) => s.sidebarTab);
  const width = useAppStore((s) => s.sidebarWidth);
  const setTab = useAppStore((s) => s.setSidebarTab);
  const setWidth = useAppStore((s) => s.setSidebarWidth);
  const dragging = useRef(false);

  const onPointerDown = useCallback((e: React.PointerEvent) => {
    dragging.current = true;
    (e.target as HTMLElement).setPointerCapture(e.pointerId);
  }, []);

  useEffect(() => {
    const move = (e: PointerEvent) => {
      if (dragging.current) setWidth(e.clientX);
    };
    const up = () => {
      dragging.current = false;
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
    return () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
    };
  }, [setWidth]);

  return (
    <aside className="sidebar" style={{ width }} aria-label={t("sidebar.toggle")}>
      <div className="sidebar-tabs" role="tablist">
        {TABS.map((tabDef) => (
          <IconButton
            key={tabDef.id}
            icon={tabDef.icon}
            label={t(tabDef.labelKey)}
            shortcut={shortcutFor(tabDef.keyId, os)}
            active={tab === tabDef.id}
            onClick={() => setTab(tabDef.id)}
          />
        ))}
      </div>

      <div className="sidebar-body" role="tabpanel" aria-label={t(`sidebar.tab.${tab}`)}>
        {/* 축소판 · 목차 · 검색 are (c)'s panels; 주석 is (d)'s `src/sidebar/AnnotationList.tsx`. */}
        {tab === "thumbnails" && <Thumbnails />}
        {tab === "outline" && <Outline />}
        {tab === "annotations" && (
          <Suspense fallback={<p className="empty">{t("common.loading")}</p>}>
            <AnnotationList />
          </Suspense>
        )}
        {tab === "search" && <SearchPanel />}
      </div>

      <div
        className="sidebar-resize"
        role="separator"
        aria-orientation="vertical"
        onPointerDown={onPointerDown}
      />
    </aside>
  );
}
