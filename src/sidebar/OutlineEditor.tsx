/**
 * 목차 편집 (P2) — the 목차 tab's edit mode, its own lazy chunk.
 *
 * The outline is edited as a local draft (`outlineEdit.ts`) and written in one go: 완료 sends the
 * whole tree with `set_outline` (one undo step `undo.outlineEdit`, 실행 취소 in the toast), 취소
 * throws the draft away. Nothing touches the document until 완료.
 *
 *   현재 페이지 추가   a node for the current view (page + the y of the viewport's top), right after
 *                    the selection, and straight into renaming
 *   이름 바꾸기        Enter or double-click; Enter / blur commits, Esc restores
 *   목적지 = 현재 보기  re-points the selection at the current view
 *   들여쓰기 / 내어쓰기 Tab / ⇧Tab — into the previous sibling, out after the parent
 *   위로 / 아래로      ⌥↑ / ⌥↓ among the siblings; or drag a row onto another (top quarter = before,
 *                    bottom quarter = after, the middle = inside; dragged 32 px right = inside,
 *                    32 px left = out after the target's parent). v0.3 H7: pointer events, not HTML5
 *                    drag and drop, which never fires in the Windows webview.
 *   삭제              Delete / ⌫, the node and everything under it
 *   제목에서 목차 만들기 (v0.3 P4) — `headings.ts` over the text layer; the proposals are reviewed
 *                    (tick / untick) and 목차에 추가 appends them to the draft, nothing is written yet
 */
import { useMemo, useRef, useState, type KeyboardEvent } from "react";
import {
  ArrowDown, ArrowUp, ChevronDown, ChevronRight, Globe, IndentDecrease, IndentIncrease, ListTree, LocateFixed, Pencil, Plus,
  Trash2,
} from "lucide-react";
import { startPointerDrag, type DragMove } from "../dialogs/pointerDrag";
import { detectHeadings, linesFromLayer, toOutlineNodes, type HeadingLine, type HeadingProposal } from "./headings";
import * as api from "../ipc/api";
import { useT } from "../i18n/useT";
import { useDocStore } from "../store/docStore";
import { currentViewTarget } from "../store/viewStore";
import { toast } from "../app/toastStore";
import { IconButton } from "../app/IconButton";
import { displayLabel } from "../viewer/pageLabel";
import {
  findNode, flattenEdit, indent, insertAfter, moveBy, moveNode, newId, outdent, removeNode, sameOutline, toEditTree,
  toOutline, updateNode, type DropPosition, type EditNode,
} from "./outlineEdit";
import "./outlineEditor.css";

/** A pointer dragged this far sideways nests (→) or un-nests (←) the dragged node. */
const NEST_DX = 32;
/** 제목에서 목차 만들기 reads at most this many pages. */
const MAX_SCAN_PAGES = 500;

export default function OutlineEditor({ onDone }: { onDone(): void }) {
  const t = useT();
  const info = useDocStore((s) => s.info);
  const original = useDocStore((s) => s.outline);
  const initial = useMemo(() => toEditTree(original), [original]);
  const [tree, setTree] = useState<EditNode[]>(initial);
  const [selected, setSelected] = useState<number | null>(initial[0]?.id ?? null);
  const [renaming, setRenaming] = useState<number | null>(null);
  const [collapsed, setCollapsed] = useState<Set<number>>(() => new Set());
  const [drag, setDrag] = useState<number | null>(null);
  const [drop, setDrop] = useState<{ id: number; position: DropPosition } | null>(null);
  const [busy, setBusy] = useState(false);
  // v0.3 P4: 제목에서 목차 만들기 — the scan, then the proposals under review
  const [scanning, setScanning] = useState(false);
  const [proposals, setProposals] = useState<(HeadingProposal & { keep: boolean })[] | null>(null);
  const listRef = useRef<HTMLUListElement>(null);

  const rows = useMemo(() => flattenEdit(tree, collapsed), [tree, collapsed]);
  const current = selected !== null ? findNode(tree, selected) : null;
  const changed = !sameOutline(tree, initial);

  if (!info) return null;
  const labels = info.pageLabels;

  const focusList = () => requestAnimationFrame(() => listRef.current?.focus({ preventScroll: true }));

  function add() {
    const view = currentViewTarget();
    const node: EditNode = {
      id: newId(),
      title: t("outline.untitled"),
      page: view.page,
      open: true,
      children: [],
      ...(view.y !== undefined ? { dest: { y: view.y } } : null),
    };
    setTree((prev) => insertAfter(prev, selected, node));
    setSelected(node.id);
    setRenaming(node.id);
  }

  function setDestination() {
    if (selected === null) return;
    const view = currentViewTarget();
    setTree((prev) => updateNode(prev, selected, { page: view.page, url: undefined, dest: view.y !== undefined ? { y: view.y } : undefined }));
  }

  function remove(id: number | null = selected) {
    if (id === null) return;
    const index = rows.findIndex((r) => r.node.id === id);
    const next = rows[index + 1]?.node.id ?? rows[index - 1]?.node.id ?? null;
    setTree((prev) => removeNode(prev, id));
    setSelected(next === id ? null : next);
    focusList();
  }

  function apply(op: (nodes: EditNode[], id: number) => EditNode[]) {
    if (selected === null) return;
    setTree((prev) => op(prev, selected));
    focusList();
  }

  async function save() {
    if (!info) return;
    if (!changed) return onDone();
    setBusy(true);
    try {
      const next = await api.setOutline({ docId: info.docId, nodes: toOutline(tree) });
      useDocStore.getState().adopt(next);
      await useDocStore.getState().reloadOutline();
      toast("outline.saved", undefined, {
        tone: "success",
        actions: [{ labelKey: "common.undo", onSelect: () => void import("../annot/sync").then((m) => m.undoWithAnnots()) }],
      });
      onDone();
    } catch (e) {
      setBusy(false);
      const unsupported = api.isSeePdfError(e) && e.code === "unsupported";
      toast(unsupported ? "structure.encrypted" : "error.generic", undefined, {
        tone: "danger",
        detail: e instanceof Error ? e.message : String(e),
      });
    }
  }

  function onKeyDown(e: KeyboardEvent<HTMLUListElement>) {
    if (renaming !== null || busy) return;
    const index = rows.findIndex((r) => r.node.id === selected);
    const claim = () => {
      e.preventDefault();
      e.stopPropagation();
    };
    if (e.key === "ArrowDown" && e.altKey) return claim(), apply((n, id) => moveBy(n, id, 1));
    if (e.key === "ArrowUp" && e.altKey) return claim(), apply((n, id) => moveBy(n, id, -1));
    if (e.key === "ArrowDown") return claim(), setSelected(rows[Math.min(rows.length - 1, index + 1)]?.node.id ?? selected);
    if (e.key === "ArrowUp") return claim(), setSelected(rows[Math.max(0, index - 1)]?.node.id ?? selected);
    if (e.key === "Enter" && selected !== null) return claim(), setRenaming(selected);
    if ((e.key === "Delete" || e.key === "Backspace") && selected !== null) return claim(), remove();
    if (e.key === "Tab" && selected !== null) return claim(), apply(e.shiftKey ? outdent : indent);
  }

  /** The row under the pointer and where the dragged node goes relative to it. */
  function dropTarget(m: DragMove, dragged: number): { id: number; position: DropPosition } | null {
    const items = [...(listRef.current?.querySelectorAll<HTMLElement>("li[data-node-id]") ?? [])];
    let hit: HTMLElement | null = null;
    for (const li of items) {
      const r = li.getBoundingClientRect();
      if (m.clientY >= r.top && m.clientY < r.bottom) {
        hit = li;
        break;
      }
    }
    // above the first row / below the last one
    if (!hit && items.length) {
      const first = items[0].getBoundingClientRect();
      hit = m.clientY < first.top ? items[0] : items[items.length - 1];
    }
    if (!hit) return null;
    const id = Number(hit.dataset.nodeId);
    const r = hit.getBoundingClientRect();
    const y = m.clientY - r.top;
    let position: DropPosition = !r.height ? "after" : y < r.height / 4 ? "before" : y > (r.height * 3) / 4 ? "after" : "inside";
    let target = id;
    if (m.dx >= NEST_DX && id !== dragged) position = "inside";
    else if (m.dx <= -NEST_DX) {
      const row = rows.find((x) => x.node.id === id);
      if (row?.parentId != null) {
        target = row.parentId;
        position = "after";
      }
    }
    if (target === dragged) return null;
    return { id: target, position };
  }

  function beginDrag(e: React.PointerEvent, id: number) {
    if (renaming !== null || busy) return;
    startPointerDrag(e, {
      onStart: () => {
        setDrag(id);
        setSelected(id);
      },
      onMove: (m) => {
        const t = dropTarget(m, id);
        setDrop((prev) => (prev?.id === t?.id && prev?.position === t?.position ? prev : t));
      },
      onDrop: (m) => {
        const t = dropTarget(m, id);
        if (t) setTree((prev) => moveNode(prev, id, t.id, t.position));
        setDrag(null);
        setDrop(null);
        focusList();
      },
      onCancel: () => {
        setDrag(null);
        setDrop(null);
      },
    });
  }

  /** 제목에서 목차 만들기: read the pages' text, propose headings for review. */
  async function proposeHeadings() {
    if (!info) return;
    setScanning(true);
    const lines: HeadingLine[] = [];
    const last = Math.min(info.pageCount, MAX_SCAN_PAGES);
    try {
      for (let page = 0; page < last; page++) {
        const view = await api.getTextLayer({ docId: info.docId, page });
        lines.push(...linesFromLayer(view, page));
      }
      const found = detectHeadings(lines);
      setProposals(found.map((p) => ({ ...p, keep: true })));
      if (!found.length) toast("outline.headings.none", undefined, { tone: "info" });
    } catch (e) {
      toast("error.generic", undefined, { tone: "danger", detail: e instanceof Error ? e.message : String(e) });
    } finally {
      setScanning(false);
    }
  }

  function acceptHeadings() {
    if (!proposals) return;
    const nodes = toOutlineNodes(proposals.filter((p) => p.keep));
    const added = toEditTree(nodes);
    setTree((prev) => [...prev, ...added]);
    setProposals(null);
    if (added[0]) setSelected(added[0].id);
  }

  return (
    <div className="outline-editor">
      <div className="outline-editor-tools" role="toolbar" aria-label={t("outline.edit")}>
        <IconButton icon={Plus} label={t("outline.add")} size={16} disabled={busy} onClick={add} />
        <IconButton icon={Pencil} label={t("outline.rename")} size={16} disabled={!current || busy} onClick={() => selected !== null && setRenaming(selected)} />
        <IconButton icon={LocateFixed} label={t("outline.setDest")} size={16} disabled={!current || busy} onClick={setDestination} />
        <IconButton icon={IndentIncrease} label={t("outline.indent")} size={16} disabled={!current || busy} onClick={() => apply(indent)} />
        <IconButton icon={IndentDecrease} label={t("outline.outdent")} size={16} disabled={!current || busy} onClick={() => apply(outdent)} />
        <IconButton icon={ArrowUp} label={t("outline.moveUp")} size={16} disabled={!current || busy} onClick={() => apply((n, id) => moveBy(n, id, -1))} />
        <IconButton icon={ArrowDown} label={t("outline.moveDown")} size={16} disabled={!current || busy} onClick={() => apply((n, id) => moveBy(n, id, 1))} />
        <IconButton icon={Trash2} label={t("outline.delete")} size={16} tone="danger" disabled={!current || busy} onClick={() => remove()} />
        {/* v0.3 P4 */}
        <IconButton
          icon={ListTree}
          label={t("outline.headings")}
          size={16}
          disabled={busy || scanning || proposals !== null}
          onClick={() => void proposeHeadings()}
        />
      </div>

      {scanning && <p className="outline-editor-hint text-xs dim" role="status">{t("outline.headings.scanning")}</p>}
      {proposals && (
        <section className="outline-proposals" aria-label={t("outline.headings")}>
          <p className="text-xs dim">{t("outline.headings.review", { count: proposals.length })}</p>
          <ul className="outline-proposal-list">
            {proposals.map((p, i) => (
              <li key={`${p.page}:${i}`} style={{ paddingInlineStart: (p.level - 1) * 12 }}>
                <label className="dlg-check text-sm">
                  <input
                    type="checkbox"
                    checked={p.keep}
                    onChange={(e) => {
                      const keep = e.currentTarget.checked;
                      setProposals((prev) => prev && prev.map((q, j) => (j === i ? { ...q, keep } : q)));
                    }}
                  />
                  <span className="outline-edit-title">{p.title}</span>
                  <span className="outline-page text-xs mono dim">{displayLabel(labels, p.page)}</span>
                </label>
              </li>
            ))}
          </ul>
          <div className="outline-editor-foot">
            <button type="button" className="btn quiet" onClick={() => setProposals(null)}>
              {t("common.cancel")}
            </button>
            <button
              type="button"
              className="btn"
              disabled={!proposals.some((p) => p.keep)}
              onClick={acceptHeadings}
            >
              {t("outline.headings.add")}
            </button>
          </div>
        </section>
      )}

      {rows.length === 0 ? (
        <p className="empty">{t("outline.emptyEdit")}</p>
      ) : (
        <ul
          ref={listRef}
          className="outline outline-edit-list"
          role="tree"
          aria-label={t("outline.edit")}
          tabIndex={0}
          data-own-keys=""
          onKeyDown={onKeyDown}
        >
          {rows.map(({ node, depth }) => {
            const isCollapsed = collapsed.has(node.id);
            const target = drop?.id === node.id ? drop.position : undefined;
            return (
              <li
                key={node.id}
                role="treeitem"
                aria-selected={node.id === selected}
                aria-expanded={node.children.length ? !isCollapsed : undefined}
                data-node-id={node.id}
                data-drop={target}
                data-dragging={drag === node.id || undefined}
                onPointerDown={(e) => {
                  if (renaming !== node.id) beginDrag(e, node.id);
                }}
              >
                <div
                  className="outline-row outline-edit-row"
                  data-selected={node.id === selected || undefined}
                  style={{ paddingInlineStart: depth * 12 }}
                  onClick={() => setSelected(node.id)}
                  onDoubleClick={() => setRenaming(node.id)}
                >
                  {node.children.length ? (
                    <button
                      type="button"
                      className="outline-twisty"
                      aria-label={node.title}
                      aria-expanded={!isCollapsed}
                      tabIndex={-1}
                      onClick={(e) => {
                        e.stopPropagation();
                        setCollapsed((prev) => {
                          const next = new Set(prev);
                          if (next.has(node.id)) next.delete(node.id);
                          else next.add(node.id);
                          return next;
                        });
                      }}
                    >
                      {isCollapsed ? <ChevronRight size={14} strokeWidth={1.75} /> : <ChevronDown size={14} strokeWidth={1.75} />}
                    </button>
                  ) : (
                    <span className="outline-twisty" aria-hidden="true" />
                  )}
                  {renaming === node.id ? (
                    <RenameField
                      value={node.title}
                      label={t("outline.titleField")}
                      onCommit={(title) => {
                        setTree((prev) => updateNode(prev, node.id, { title: title.trim() || node.title }));
                        setRenaming(null);
                        focusList();
                      }}
                      onCancel={() => {
                        setRenaming(null);
                        focusList();
                      }}
                    />
                  ) : (
                    <span className="outline-edit-title text-sm">{node.title}</span>
                  )}
                  <span className="outline-page text-xs mono dim">
                    {node.url ? (
                      <Globe size={12} strokeWidth={1.75} aria-label={t("outline.webLink")} />
                    ) : node.page !== null ? (
                      displayLabel(labels, node.page)
                    ) : (
                      "—"
                    )}
                  </span>
                </div>
              </li>
            );
          })}
        </ul>
      )}

      <p className="outline-editor-hint text-xs dim">{t("outline.editHint")}</p>
      <div className="outline-editor-foot">
        <button type="button" className="btn quiet" disabled={busy} onClick={onDone}>
          {t("common.cancel")}
        </button>
        <button type="button" className="btn primary" disabled={busy} onClick={() => void save()}>
          {t("outline.done")}
        </button>
      </div>
    </div>
  );
}

/** The inline title input: Enter or blur commits, Esc restores. */
function RenameField({ value, label, onCommit, onCancel }: {
  value: string;
  label: string;
  onCommit(title: string): void;
  onCancel(): void;
}) {
  const [text, setText] = useState(value);
  const done = useRef(false);
  return (
    <input
      className="field outline-rename"
      aria-label={label}
      value={text}
      autoFocus
      onFocus={(e) => e.currentTarget.select()}
      onChange={(e) => setText(e.target.value)}
      onClick={(e) => e.stopPropagation()}
      onKeyDown={(e) => {
        e.stopPropagation();
        if (e.key === "Enter" && !e.nativeEvent.isComposing) {
          e.preventDefault();
          done.current = true;
          onCommit(text);
        } else if (e.key === "Escape") {
          e.preventDefault();
          done.current = true;
          onCancel();
        }
      }}
      onBlur={() => {
        if (!done.current) onCommit(text);
      }}
    />
  );
}
