import { useCallback, useEffect, useRef } from "react";
import { ListTree, MessageSquare, RectangleVertical, Search } from "lucide-react";
import { IconButton } from "./IconButton";
import { useT } from "../i18n/useT";
import { useAppStore, type SidebarTab } from "../store/appStore";
import { useDocStore } from "../store/docStore";
import { useViewStore } from "../store/viewStore";
import { shortcutFor } from "../keys/keymap";
import { thumbUrl } from "../ipc/protocol";
import type { OutlineNode } from "../ipc/types";
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
  const outline = useDocStore((s) => s.outline);
  const info = useDocStore((s) => s.info);
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
        {tab === "thumbnails" && (
          <ol className="thumb-rail">
            {(info?.pages ?? []).map((page) => (
              <li key={page.index}>
                <ThumbPlaceholder index={page.index} ratio={page.heightPt / page.widthPt} />
              </li>
            ))}
          </ol>
        )}

        {tab === "outline" && (
          outline.length === 0
            ? <p className="empty">{t("sidebar.outline.empty")}</p>
            : <OutlineTree nodes={outline} />
        )}

        {tab === "annotations" && <p className="empty">{t("sidebar.annotations.empty")}</p>}

        {tab === "search" && (
          <div className="search-panel">
            <input className="field" type="search" placeholder={t("sidebar.search.placeholder")} />
            <p className="empty">{t("sidebar.search.empty")}</p>
          </div>
        )}
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

function ThumbPlaceholder({ index, ratio }: { index: number; ratio: number }) {
  const t = useT();
  const info = useDocStore((s) => s.info);
  const currentPage = useViewStore((s) => s.currentPage);
  const goToPage = useViewStore((s) => s.goToPage);
  const src = info ? thumbUrl({ doc: info.docId, gen: info.docGeneration, page: index, w: 120 }) : undefined;
  return (
    <button
      type="button"
      className="thumb"
      data-current={index === currentPage || undefined}
      aria-label={t("a11y.pageThumbnail", { n: index + 1 })}
      aria-current={index === currentPage ? "page" : undefined}
      onClick={() => goToPage(index)}
    >
      <span className="thumb-page" style={{ aspectRatio: `1 / ${ratio}` }}>
        {src && <img src={src} alt="" draggable={false} />}
      </span>
      <span className="thumb-num text-xs">{index + 1}</span>
    </button>
  );
}

function OutlineTree({ nodes, depth = 0 }: { nodes: OutlineNode[]; depth?: number }) {
  const goToPage = useViewStore((s) => s.goToPage);
  return (
    <ul className="outline" style={{ paddingInlineStart: depth === 0 ? 0 : 12 }}>
      {nodes.map((node, i) => (
        <li key={`${depth}-${i}-${node.title}`}>
          <button
            type="button"
            className="outline-item"
            onClick={() => node.page !== null && goToPage(node.page)}
          >
            {node.title}
          </button>
          {node.children.length > 0 && <OutlineTree nodes={node.children} depth={depth + 1} />}
        </li>
      ))}
    </ul>
  );
}
