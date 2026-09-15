/**
 * The 주석 sidebar tab (UI_SPEC §4, F-14): every annotation of the document, grouped by page, with
 * its type icon, author, excerpt and date. Clicking a row scrolls to the page and selects the
 * annotation; ⌫ / the row's × deletes it.
 *
 * The list is filled by `scan_annotations` (one streamed pass per document, `src/annot/sync.ts`)
 * and kept current by the same per-page re-list every other surface uses, so it is never a second
 * source of truth.
 */
import { useMemo } from "react";
import {
  Circle, Highlighter, MessageSquarePlus, Minus, MoveUpRight, PenLine, Signature, Square,
  Stamp, Sticker, Strikethrough, Trash2, Type, Underline, Waves,
} from "lucide-react";
import type { Annot, AnnotKind } from "../ipc/types";
import { formatRelativeDay } from "../i18n";
import { useT } from "../i18n/useT";
import { useAnnotStore } from "../store/annotStore";
import { useAppStore } from "../store/appStore";
import { useViewStore } from "../store/viewStore";
import { deleteAnnotations } from "../annot/actions";
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

/** Page → its annotations, page order. Ghosts are included so a new note appears immediately. */
export function groupByPage(byPage: Record<number, Annot[]>, ghosts: Annot[], filter: AnnotKind[] | null) {
  const merged = new Map<number, Annot[]>();
  for (const key of Object.keys(byPage)) {
    const page = Number(key);
    const list = (byPage[page] ?? []).filter((a) => !filter || filter.includes(a.kind));
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

  const open = (annot: Annot) => {
    useAnnotStore.getState().select([annot.id]);
    useViewStore.getState().goToPage(annot.page);
    if (useAppStore.getState().mode === "read") useAppStore.getState().setMode("annotate");
    if (annot.kind === "note" || annot.kind === "textbox") {
      useAnnotStore.getState().setEditing({ page: annot.page, id: annot.id });
    }
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
            {list.map((a) => (
              <div
                key={a.id}
                className="annot-row"
                data-selected={selected.includes(a.id) || undefined}
                role="button"
                tabIndex={0}
                aria-label={t("a11y.annotation", { type: annotLabel(a, t), author: a.author ?? "" })}
                onClick={() => open(a)}
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
                    {a.modified ? ` · ${formatRelativeDay(a.modified)}` : ""}
                  </span>
                </span>
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
            ))}
          </section>
        ))
      )}
    </div>
  );
}

export default AnnotationList;
