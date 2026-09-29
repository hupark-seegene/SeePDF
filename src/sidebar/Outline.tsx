/**
 * 목차 sidebar (F-05, UI_SPEC §4): the outline tree with disclosure triangles, the section
 * containing the current page highlighted, and a click that navigates — to the heading's own
 * position when the destination carries one (`OutlineNode.dest`), or opens a web node's address
 * after a confirm.
 *
 * P2: 편집 swaps the tree for `OutlineEditor` (its own lazy chunk) — add / rename / delete /
 * indent / reorder, then 완료 writes the whole tree with `set_outline` (one undo step). Page numbers
 * show the document's page labels when it has them, and a node written closed (`open: false`)
 * starts collapsed.
 *
 * v0.3 (pkg1, R6): an item's context menu (UI_SPEC §12 "Outline item") — 이동 · 하위 항목 모두 펼치기 ·
 * 하위 항목 모두 접기 (the item and everything under it).
 */
import { lazy, Suspense, useEffect, useMemo, useState } from "react";
import { ChevronDown, ChevronRight, Globe } from "lucide-react";
import type { OutlineNode, PageIndex } from "../ipc/types";
import { useT } from "../i18n/useT";
import { useDocStore } from "../store/docStore";
import { useViewStore } from "../store/viewStore";
import { displayLabel } from "../viewer/pageLabel";
import { openContextMenu } from "../app/contextMenuStore";
import "./sidebar.css";

const OutlineEditor = lazy(() => import("./OutlineEditor"));

interface Flat {
  key: string;
  node: OutlineNode;
  depth: number;
  hasChildren: boolean;
  parentKey: string | null;
}

function flatten(nodes: OutlineNode[], depth = 0, parentKey: string | null = null, out: Flat[] = []): Flat[] {
  nodes.forEach((node, i) => {
    const key = `${parentKey ?? "r"}.${i}`;
    out.push({ key, node, depth, hasChildren: node.children.length > 0, parentKey });
    if (node.children.length) flatten(node.children, depth + 1, key, out);
  });
  return out;
}

/** `key` and the keys of every node below it that has children (what 모두 펼치기 / 접기 touch). */
function branchKeys(flat: Flat[], key: string): string[] {
  return flat.filter((row) => row.hasChildren && (row.key === key || row.key.startsWith(`${key}.`))).map((row) => row.key);
}

/** The keys of the nodes the file says start closed (`/Count` < 0). */
function closedKeys(flat: Flat[]): Set<string> {
  return new Set(flat.filter((row) => row.hasChildren && row.node.open === false).map((row) => row.key));
}

export function Outline() {
  const t = useT();
  const outline = useDocStore((s) => s.outline);
  const info = useDocStore((s) => s.info);
  const currentPage = useViewStore((s) => s.currentPage);
  const goToPage = useViewStore((s) => s.goToPage);
  const [editing, setEditing] = useState(false);

  const flat = useMemo(() => flatten(outline), [outline]);
  const [collapsed, setCollapsed] = useState<Set<string>>(() => closedKeys(flat));
  // a new outline (another document, an edit, an undo) starts from the file's own open flags
  useEffect(() => setCollapsed(closedKeys(flat)), [flat]);
  // the editor belongs to one document
  useEffect(() => setEditing(false), [info?.docId]);

  const visible = useMemo(() => {
    const hidden = new Set<string>();
    return flat.filter((row) => {
      if (row.parentKey && (hidden.has(row.parentKey) || collapsed.has(row.parentKey))) {
        hidden.add(row.key);
        return false;
      }
      return true;
    });
  }, [flat, collapsed]);

  /** The deepest node whose page is at or before the current page — the section being read. */
  const activeKey = useMemo(() => {
    let best: { key: string; page: PageIndex } | null = null;
    for (const row of flat) {
      const page = row.node.page;
      if (page === null || page > currentPage) continue;
      if (!best || page >= best.page) best = { key: row.key, page };
    }
    return best?.key ?? null;
  }, [flat, currentPage]);

  if (editing && info) {
    return (
      <Suspense fallback={<p className="empty">{t("common.loading")}</p>}>
        <OutlineEditor onDone={() => setEditing(false)} />
      </Suspense>
    );
  }

  const head = info && (
    <div className="outline-head">
      <span className="text-xs dim">{t("sidebar.tab.outline")}</span>
      <button
        type="button"
        className="btn quiet outline-edit-btn text-sm"
        disabled={info.encrypted}
        title={info.encrypted ? t("structure.encrypted") : undefined}
        onClick={() => setEditing(true)}
      >
        {t("outline.edit")}
      </button>
    </div>
  );

  if (outline.length === 0) {
    return (
      <>
        {head}
        <p className="empty">{t("sidebar.outline.empty")}</p>
      </>
    );
  }

  const open = (node: OutlineNode) => {
    if (node.page !== null) goToPage(node.page, node.dest?.y);
    else if (node.url) void import("../annot/links").then((m) => m.openWebLink(node.url!));
  };

  const setBranch = (key: string, expanded: boolean) =>
    setCollapsed((prev) => {
      const next = new Set(prev);
      for (const k of branchKeys(flat, key)) {
        if (expanded) next.delete(k);
        else next.add(k);
      }
      return next;
    });

  const menu = (e: React.MouseEvent, row: Flat) => {
    e.preventDefault();
    e.stopPropagation();
    openContextMenu({
      x: e.clientX,
      y: e.clientY,
      labelKey: "sidebar.tab.outline",
      items: [
        { id: "go", labelKey: "outline.menu.go", disabled: row.node.page === null && !row.node.url, onSelect: () => open(row.node) },
        { id: "sep", separator: true },
        { id: "expandAll", labelKey: "outline.menu.expandAll", disabled: !row.hasChildren, onSelect: () => setBranch(row.key, true) },
        { id: "collapseAll", labelKey: "outline.menu.collapseAll", disabled: !row.hasChildren, onSelect: () => setBranch(row.key, false) },
      ],
    });
  };

  return (
    <>
      {head}
      <ul className="outline" role="tree" aria-label={t("sidebar.tab.outline")}>
        {visible.map((row) => {
          const isCollapsed = collapsed.has(row.key);
          return (
            <li
              key={row.key}
              role="treeitem"
              aria-expanded={row.hasChildren ? !isCollapsed : undefined}
              onContextMenu={(e) => menu(e, row)}
            >
              <div className="outline-row" style={{ paddingInlineStart: row.depth * 12 }}>
                {row.hasChildren ? (
                  <button
                    type="button"
                    className="outline-twisty"
                    aria-label={row.node.title}
                    aria-expanded={!isCollapsed}
                    onClick={() =>
                      setCollapsed((prev) => {
                        const next = new Set(prev);
                        if (next.has(row.key)) next.delete(row.key);
                        else next.add(row.key);
                        return next;
                      })
                    }
                  >
                    {isCollapsed ? <ChevronRight size={14} strokeWidth={1.75} /> : <ChevronDown size={14} strokeWidth={1.75} />}
                  </button>
                ) : (
                  <span className="outline-twisty" aria-hidden="true" />
                )}
                <button
                  type="button"
                  className="outline-item"
                  data-active={row.key === activeKey || undefined}
                  disabled={row.node.page === null && !row.node.url}
                  title={row.node.url}
                  onClick={() => open(row.node)}
                >
                  <span className="outline-title">{row.node.title}</span>
                  {row.node.page !== null && (
                    <span className="outline-page text-xs mono dim">{displayLabel(info?.pageLabels, row.node.page)}</span>
                  )}
                  {row.node.url && <Globe className="outline-page dim" size={12} strokeWidth={1.75} aria-hidden />}
                </button>
              </div>
            </li>
          );
        })}
      </ul>
    </>
  );
}
