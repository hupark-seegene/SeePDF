/**
 * 목차 sidebar (F-05, UI_SPEC §4): the outline tree with disclosure triangles, the section
 * containing the current page highlighted, and a click that navigates.
 *
 * `OutlineNode` carries only a page index (IPC_CONTRACT §4) — there is no destination rectangle in
 * v1, so "go to dest with position" is a page jump; the note in `docs/STAGE1C_NOTES.md` records
 * that gap.
 */
import { useMemo, useState } from "react";
import { ChevronDown, ChevronRight } from "lucide-react";
import type { OutlineNode, PageIndex } from "../ipc/types";
import { useT } from "../i18n/useT";
import { useDocStore } from "../store/docStore";
import { useViewStore } from "../store/viewStore";
import "./sidebar.css";

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

export function Outline() {
  const t = useT();
  const outline = useDocStore((s) => s.outline);
  const currentPage = useViewStore((s) => s.currentPage);
  const goToPage = useViewStore((s) => s.goToPage);
  const [collapsed, setCollapsed] = useState<Set<string>>(() => new Set());

  const flat = useMemo(() => flatten(outline), [outline]);

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

  if (outline.length === 0) return <p className="empty">{t("sidebar.outline.empty")}</p>;

  return (
    <ul className="outline" role="tree" aria-label={t("sidebar.tab.outline")}>
      {visible.map((row) => {
        const isCollapsed = collapsed.has(row.key);
        return (
          <li key={row.key} role="treeitem" aria-expanded={row.hasChildren ? !isCollapsed : undefined}>
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
                disabled={row.node.page === null}
                onClick={() => row.node.page !== null && goToPage(row.node.page)}
              >
                <span className="outline-title">{row.node.title}</span>
                {row.node.page !== null && <span className="outline-page text-xs mono dim">{row.node.page + 1}</span>}
              </button>
            </div>
          </li>
        );
      })}
    </ul>
  );
}
