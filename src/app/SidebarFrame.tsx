import { Suspense, lazy, useCallback, useEffect, useRef } from "react";
import { ListTree, MessageSquare, Paperclip, RectangleVertical, Search } from "lucide-react";
import { IconButton } from "./IconButton";
import { useT } from "../i18n/useT";
import { useAppStore, type SidebarTab } from "../store/appStore";
import { shortcutFor } from "../keys/keymap";
import { Thumbnails } from "../sidebar/Thumbnails";
import { Outline } from "../sidebar/Outline";
import { SearchPanel } from "../sidebar/SearchPanel";
import { useDocStore } from "../store/docStore";

// (d): the 주석 list pulls in the annotation actions, so it is code-split like every Stage 1 panel.
const AnnotationList = lazy(() => import("../sidebar/AnnotationList"));
// v0.3 pkg3 (S4): 첨부 파일, code-split like 주석.
const Attachments = lazy(() => import("../sidebar/Attachments"));
import type { IconProps } from "./IconButton";
import type { ComponentType } from "react";

const TABS: { id: SidebarTab; labelKey: string; icon: ComponentType<IconProps>; keyId: string }[] = [
  { id: "thumbnails", labelKey: "sidebar.tab.thumbnails", icon: RectangleVertical, keyId: "view.sidebar.thumbnails" },
  { id: "outline", labelKey: "sidebar.tab.outline", icon: ListTree, keyId: "view.sidebar.outline" },
  { id: "annotations", labelKey: "sidebar.tab.annotations", icon: MessageSquare, keyId: "view.sidebar.annotations" },
  { id: "search", labelKey: "sidebar.tab.search", icon: Search, keyId: "view.sidebar.search" },
  { id: "attachments", labelKey: "sidebar.tab.attachments", icon: Paperclip, keyId: "view.sidebar.attachments" },
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
  // v0.3 pkg3 (S4): 첨부 only when there is something to show, or in 편집 mode (to add some)
  const attachments = useDocStore((s) => s.info?.attachmentCount ?? 0);
  const editing = useAppStore((s) => s.mode === "edit");
  const showAttachments = attachments > 0 || editing;
  const tabs = showAttachments ? TABS : TABS.filter((d) => d.id !== "attachments");
  const shown = tab === "attachments" && !showAttachments ? "thumbnails" : tab;

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
        {tabs.map((tabDef) => (
          <IconButton
            key={tabDef.id}
            icon={tabDef.icon}
            label={t(tabDef.labelKey)}
            shortcut={shortcutFor(tabDef.keyId, os)}
            active={shown === tabDef.id}
            onClick={() => setTab(tabDef.id)}
          />
        ))}
      </div>

      <div className="sidebar-body" role="tabpanel" aria-label={t(`sidebar.tab.${shown}`)}>
        {/* 축소판 · 목차 · 검색 are (c)'s panels; 주석 is (d)'s `src/sidebar/AnnotationList.tsx`. */}
        {shown === "thumbnails" && <Thumbnails />}
        {shown === "outline" && <Outline />}
        {shown === "attachments" && (
          <Suspense fallback={<p className="empty">{t("common.loading")}</p>}>
            <Attachments />
          </Suspense>
        )}
        {shown === "annotations" && (
          <Suspense fallback={<p className="empty">{t("common.loading")}</p>}>
            <AnnotationList />
          </Suspense>
        )}
        {shown === "search" && <SearchPanel />}
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
