/**
 * The 주석 sidebar tab (UI_SPEC §4, F-14): every annotation of the document, grouped by page, with
 * its type icon, author, excerpt and date. Clicking a row scrolls to the page and selects the
 * annotation; ⌫ / the row's × deletes it.
 *
 * The list is filled by `scan_annotations` (one streamed pass per document, `src/annot/sync.ts`)
 * and kept current by the same per-page re-list every other surface uses, so it is never a second
 * source of truth.
 *
 * P2 threads: replies sit under their thread's top-level annotation, behind a count badge that
 * folds them; a row's context menu has 답글 (the thread popover, reply box focused) and 삭제
 * (which asks first when the annotation has replies — they go with it).
 */
import { useMemo, useState } from "react";
import {
  Circle, CornerDownRight, Highlighter, MessageSquare, MessageSquarePlus, Minus, MoveUpRight, PenLine,
  Signature, Square, Stamp, Sticker, Strikethrough, Trash2, Type, Underline, Waves,
} from "lucide-react";
import type { Annot, AnnotKind } from "../ipc/types";
import { formatRelativeDay } from "../i18n";
import { useT } from "../i18n/useT";
import { useAnnotStore } from "../store/annotStore";
import { useAppStore } from "../store/appStore";
import { useViewStore } from "../store/viewStore";
import { deleteAnnotations, openThread } from "../annot/actions";
import { isoOf, threaded } from "../annot/threads";
import { openContextMenu } from "../app/contextMenuStore";
import type { IconProps } from "../app/IconButton";
import type { ComponentType } from "react";

const ICONS: Partial<Record<AnnotKind, ComponentType<IconProps>>> = {
  highlight: Highlighter,
  underline: Underline,
  strikeout: Strikethrough,
  squiggly: Waves,
  note: MessageSquarePlus,
  ink: PenLine,
  square: Square,
  circle: Circle,
  line: Minus,
  arrow: MoveUpRight,
  textbox: Type,
  stamp: Stamp,
  signature: Signature,
};

/** The chips of UI_SPEC §4 — the kinds worth filtering by, in tool-strip order. */
const FILTERS: { kind: AnnotKind; labelKey: string }[] = [
  { kind: "highlight", labelKey: "tool.highlight" },
  { kind: "underline", labelKey: "tool.underline" },
  { kind: "strikeout", labelKey: "tool.strikeout" },
  { kind: "note", labelKey: "tool.note" },
  { kind: "ink", labelKey: "tool.pen" },
  { kind: "square", labelKey: "tool.rectangle" },
  { kind: "circle", labelKey: "tool.ellipse" },
  { kind: "textbox", labelKey: "tool.textbox" },
];

export function annotLabel(a: Annot, t: (key: string) => string): string {
  switch (a.kind) {
    case "highlight": return t("tool.highlight");
    case "underline": return t("tool.underline");
    case "strikeout": return t("tool.strikeout");
    case "squiggly": return t("tool.squiggly");
    case "note": return t("tool.note");
    case "ink": return t("tool.pen");
    case "square": return t("tool.rectangle");
    case "circle": return t("tool.ellipse");
    case "line": return t("annot.kind.line");
    case "arrow": return t("annot.kind.arrow");
    case "textbox": return t("annot.kind.textbox");
    case "stamp": return t("tool.stamp");
    case "signature": return t("tool.signature");
    default: return a.subtype;
  }
}

/**
 * Page → its annotations, page order. Ghosts are included so a new note appears immediately.
 * P2 threads: a reply is listed under its thread's top-level annotation (`Annot.replies`), not
 * on its own, and the filter chips pick threads by their top-level annotation's kind.
 */
export function groupByPage(byPage: Record<number, Annot[]>, ghosts: Annot[], filter: AnnotKind[] | null) {
  const merged = new Map<number, Annot[]>();
  for (const key of Object.keys(byPage)) {
    const page = Number(key);
    const list = threaded(byPage[page] ?? []).filter((a) => !filter || filter.includes(a.kind));
    if (list.length) merged.set(page, list);
  }
  for (const ghost of ghosts) {
    if (filter && !filter.includes(ghost.kind)) continue;
    const list = merged.get(ghost.page) ?? [];
    if (list.some((a) => a.id === ghost.id)) continue;
    merged.set(ghost.page, [...list, ghost]);
  }
  return [...merged.entries()].sort((a, b) => a[0] - b[0]);
}

function Icon({ annot }: { annot: Annot }) {
  const Glyph = ICONS[annot.kind] ?? Sticker;
  return <Glyph size={14} strokeWidth={1.75} color={`rgb(${annot.color.join(" ")})`} aria-hidden />;
}

export function AnnotationList() {
  const t = useT();
  const byPage = useAnnotStore((s) => s.byPage);
  const ghosts = useAnnotStore((s) => s.ghosts);
  const selected = useAnnotStore((s) => s.selected);
  const filter = useAnnotStore((s) => s.filter);
  const setFilter = useAnnotStore((s) => s.setFilter);
  const groups = useMemo(
    () => groupByPage(byPage, ghosts.map((g) => g.annot), filter),
    [byPage, ghosts, filter],
  );
  const total = groups.reduce((n, [, list]) => n + list.length, 0);
  // P2 threads are open by default; the badge folds one away
  const [collapsed, setCollapsed] = useState<ReadonlySet<string>>(() => new Set());
  const toggle = (id: string) =>
    setCollapsed((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });

  const open = (annot: Annot) => {
    useAnnotStore.getState().select([annot.id]);
    useViewStore.getState().goToPage(annot.page);
    if (useAppStore.getState().mode === "read") useAppStore.getState().setMode("annotate");
    if (annot.kind === "note" || annot.kind === "textbox") {
      useAnnotStore.getState().setEditing({ page: annot.page, id: annot.id });
    }
  };

  /** 답글 (UI_SPEC §12): the thread's popover with the reply box focused. */
  const reply = (annot: Annot) => {
    useViewStore.getState().goToPage(annot.page);
    if (useAppStore.getState().mode === "read") useAppStore.getState().setMode("annotate");
    openThread(annot.page, annot.id, true);
  };

  const menu = (e: React.MouseEvent, annot: Annot) => {
    e.preventDefault();
    e.stopPropagation();
    openContextMenu({
      x: e.clientX,
      y: e.clientY,
      labelKey: "sidebar.tab.annotations",
      items: [
        { id: "reply", labelKey: "annot.thread.reply", disabled: annot.id.startsWith("ghost-"), onSelect: () => reply(annot) },
        { id: "sep", separator: true },
        { id: "delete", labelKey: "common.delete", danger: true, onSelect: () => void deleteAnnotations(annot.page, [annot.id]) },
      ],
    });
  };

  return (
    <div className="annot-list">
      <div className="annot-list-filters" role="group" aria-label={t("sidebar.annotations.filter")}>
        {FILTERS.map((f) => {
          const on = !!filter?.includes(f.kind);
          return (
            <button
              key={f.kind}
              type="button"
              className="chip"
              data-active={on || undefined}
              aria-pressed={on}
              onClick={() => {
                const next = on ? (filter ?? []).filter((k) => k !== f.kind) : [...(filter ?? []), f.kind];
                setFilter(next.length ? next : null);
              }}
            >
              {t(f.labelKey)}
            </button>
          );
        })}
      </div>

      {total === 0 ? (
        <p className="empty">{t("sidebar.annotations.empty")}</p>
      ) : (
        groups.map(([page, list]) => (
          <section key={page}>
            <h3 className="annot-group-label">{t("a11y.page", { n: page + 1 })}</h3>
            {list.map((a) => {
              const replies = a.replies ?? [];
              const folded = collapsed.has(a.id);
              return (
                <div key={a.id} className="annot-thread-item" data-testid="annot-thread">
                  <div
                    className="annot-row"
                    data-selected={selected.includes(a.id) || undefined}
                    role="button"
                    tabIndex={0}
                    aria-label={t("a11y.annotation", { type: annotLabel(a, t), author: a.author ?? "" })}
                    onClick={() => open(a)}
                    onContextMenu={(e) => menu(e, a)}
                    onKeyDown={(e) => {
                      if (e.key === "Enter" || e.key === " ") {
                        e.preventDefault();
                        open(a);
                      }
                    }}
                  >
                    <Icon annot={a} />
                    <span>
                      <span className="excerpt">{(a.kind === "textbox" ? a.text || a.contents : a.contents) || annotLabel(a, t)}</span>
                      <span className="meta">
                        {annotLabel(a, t)}
                        {a.author ? ` · ${a.author}` : ""}
                        {a.modified && isoOf(a.modified) ? ` · ${formatRelativeDay(isoOf(a.modified))}` : ""}
                      </span>
                    </span>
                    {replies.length > 0 && (
                      <button
                        type="button"
                        className="annot-thread-badge"
                        aria-expanded={!folded}
                        aria-label={t(folded ? "annot.thread.expand" : "annot.thread.collapse", { count: replies.length })}
                        onClick={(e) => {
                          e.stopPropagation();
                          toggle(a.id);
                        }}
                      >
                        <MessageSquare size={12} strokeWidth={1.75} aria-hidden />
                        <span className="mono">{replies.length}</span>
                      </button>
                    )}
                    <button
                      type="button"
                      className="icon-btn"
                      aria-label={t("common.delete")}
                      onClick={(e) => {
                        e.stopPropagation();
                        void deleteAnnotations(a.page, [a.id]);
                      }}
                    >
                      <Trash2 size={14} strokeWidth={1.75} aria-hidden />
                    </button>
                  </div>
                  {replies.length > 0 && !folded && (
                    <ul className="annot-thread-replies" aria-label={t("annot.thread.label")}>
                      {replies.map((r) => (
                        <li key={r.id}>
                          <button
                            type="button"
                            className="annot-reply-row"
                            data-testid="annot-reply-row"
                            onClick={() => {
                              useViewStore.getState().goToPage(a.page);
                              openThread(a.page, a.id, false);
                            }}
                            onContextMenu={(e) => menu(e, r)}
                          >
                            <CornerDownRight size={12} strokeWidth={1.75} aria-hidden />
                            <span>
                              <span className="excerpt">{r.contents}</span>
                              <span className="meta">
                                {r.author || t("annot.thread.noAuthor")}
                                {isoOf(r.modified ?? r.created) ? ` · ${formatRelativeDay(isoOf(r.modified ?? r.created))}` : ""}
                              </span>
                            </span>
                          </button>
                        </li>
                      ))}
                    </ul>
                  )}
                </div>
              );
            })}
          </section>
        ))
      )}
    </div>
  );
}

export default AnnotationList;
