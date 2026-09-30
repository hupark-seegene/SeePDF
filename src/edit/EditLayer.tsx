/**
 * The 편집 mode surface of one page (Stage 7) — mounted in `PageShell`'s `surface` slot while
 * 편집 is the mode, so it sees every pointer event on the page before the viewer's text
 * selection does.
 *
 *   선택        hover outlines · click selects (⇧ adds) · drag moves · corner handles scale ·
 *               drag on empty space draws a marquee that selects the objects wholly inside it
 *               (⇧ adds) · double-click on text opens 문단 편집 — on a group too (v0.3.1: through
 *               그룹 해제 후 편집할까요?)
 *   텍스트 수정  click → `probe_paragraph` → the editing box
 *   텍스트 추가  click → an empty editing box
 *   이미지 추가  click or drag a box → file picker → `add_image_object`
 *   영역 표시    drag marks an area — snapped to the text runs it crosses, clipped to the page, and
 *               drawn that way while dragging · click on text marks that run · click a mark
 *               selects it (× / ⌫ removes it). Marks show hatched in every 편집 tool; the text the
 *               preview says will go although it reaches outside the marks is outlined amber.
 *
 * The page's objects are listed on mount and again whenever the document generation moves past
 * the one we hold (object ids are per-generation, IPC_CONTRACT §7.4).
 */
import { useEffect, useRef, useState } from "react";
import { X } from "lucide-react";
import type { ObjectId, PageObject, Point, Rect } from "../ipc/types";
import type { PageLayerContext } from "../viewer";
import { useT } from "../i18n/useT";
import { useAppStore } from "../store/appStore";
import { useEditStore } from "./editStore";
import {
  addImageFlow, badgeReason, beginAddText, beginParagraphEdit, loadPage, moveObjects, resizeObject,
} from "./actions";
import {
  CORNERS, cornerPoint, deltaToPage, hitObject, imageRectAt, objectsInside, rectFromPoints, resizeRect, type Corner,
} from "./geometry";
import { TextEditor } from "./TextEditor";
import { markArea, markAt, markRectFor, markTextRunAt } from "./redact";
import type { RedactMark } from "./editStore";
import type { Annot } from "../ipc/types";
import { useAnnotStore } from "../store/annotStore";
import { createLink, MIN_LINK_PT, useLinkStore } from "./linkActions";
import { LinkTargetForm } from "./LinkTargetForm";
import { selectionRectsForPage } from "../annot/selectionQuads";

const EMPTY: ObjectId[] = [];
const NO_OBJECTS: PageObject[] = [];
const NO_MARKS: RedactMark[] = [];
const NO_ANNOTS: Annot[] = [];
/** CSS px a press must travel before it is a drag rather than a click */
const DRAG_PX = 3;

type Drag =
  | { kind: "move"; ids: ObjectId[]; x0: number; y0: number; dx: number; dy: number; moved: boolean }
  | { kind: "resize"; id: ObjectId; corner: Corner; from: Rect; to: Rect; x0: number; y0: number }
  | { kind: "image" | "mark" | "link"; start: Point; end: Point; x0: number; y0: number; moved: boolean }
  | { kind: "marquee"; start: Point; end: Point; x0: number; y0: number; moved: boolean; additive: boolean };

export function EditLayer({ ctx }: { ctx: PageLayerContext }) {
  const t = useT();
  const tool = useAppStore((s) => s.tool);
  const entry = useEditStore((s) => s.pages[ctx.index]);
  const boundDoc = useEditStore((s) => s.docId);
  const selected = useEditStore((s) => (s.selection?.page === ctx.index ? s.selection.ids : EMPTY));
  const session = useEditStore((s) => (s.session?.page === ctx.index ? s.session : null));
  const anySession = useEditStore((s) => s.session !== null);
  const allMarks = useEditStore((s) => s.marks);
  const markSel = useEditStore((s) => s.markSel);
  const preview = useEditStore((s) => s.previews[ctx.index]);
  // 링크 (P2): the page's links (annotations) and what the tool points at
  const pageAnnots = useAnnotStore((s) => (tool === "link" ? s.byPage[ctx.index] : undefined)) ?? NO_ANNOTS;
  const links = tool === "link" ? pageAnnots.filter((a) => a.kind === "link") : NO_ANNOTS;
  const linkSel = useLinkStore((s) => (s.selected?.page === ctx.index ? s.selected.id : null));
  const linkDraft = useLinkStore((s) => (s.draft?.page === ctx.index ? s.draft.rect : null));
  const [hover, setHover] = useState<ObjectId | null>(null);
  const [drag, setDrag] = useState<Drag | null>(null);
  const ref = useRef<HTMLDivElement>(null);

  const objects = entry?.objects ?? NO_OBJECTS;
  const marks = allMarks.length ? allMarks.filter((m) => m.page === ctx.index) : NO_MARKS;
  const listedAt = entry?.docGeneration ?? -1;

  useEffect(() => {
    if (boundDoc !== ctx.docId) return;
    if (listedAt < ctx.docGeneration) void loadPage(ctx.docId, ctx.index, ctx.docGeneration);
  }, [boundDoc, ctx.docId, ctx.index, ctx.docGeneration, listedAt]);

  const local = (e: { clientX: number; clientY: number }): [number, number] => {
    const r = ref.current?.getBoundingClientRect();
    return [e.clientX - (r?.left ?? 0), e.clientY - (r?.top ?? 0)];
  };
  const toPage = (e: { clientX: number; clientY: number }): Point => {
    const [x, y] = local(e);
    return ctx.toPage(x, y);
  };

  const onPointerDown = (e: React.PointerEvent<HTMLDivElement>) => {
    if (e.button !== 0) return;
    e.stopPropagation();
    e.preventDefault();
    (e.currentTarget.closest(".canvas") as HTMLElement | null)?.focus({ preventScroll: true });
    // A click outside an open editor only commits it (the editor's own document listener).
    if (anySession) return;
    const at = toPage(e);
    const store = useEditStore.getState();

    if (tool === "editText") {
      void beginParagraphEdit(ctx.index, at);
      return;
    }
    if (tool === "addText") {
      beginAddText(ctx.index, at);
      return;
    }
    if (tool === "link") {
      // a click on a link selects it (the inspector edits it); anywhere else a drag draws a new one
      const hit = [...links].reverse().find((a) => inside(a.rect, at[0], at[1]));
      if (hit) {
        useLinkStore.getState().select({ page: ctx.index, id: hit.id });
        return;
      }
      e.currentTarget.setPointerCapture?.(e.pointerId);
      setDrag({ kind: "link", start: at, end: at, x0: e.clientX, y0: e.clientY, moved: false });
      return;
    }
    if (tool === "addImage" || tool === "redact") {
      if (tool === "redact") {
        const mark = markAt(marks, at[0], at[1]);
        if (mark) {
          store.selectMark(mark.id);
          return;
        }
      }
      e.currentTarget.setPointerCapture?.(e.pointerId);
      setDrag({ kind: tool === "redact" ? "mark" : "image", start: at, end: at, x0: e.clientX, y0: e.clientY, moved: false });
      return;
    }
    if (tool !== "select") return;

    const hit = hitObject(objects, at[0], at[1]);
    if (!hit) {
      // empty space: a marquee (a plain click deselects on release)
      e.currentTarget.setPointerCapture?.(e.pointerId);
      setDrag({ kind: "marquee", start: at, end: at, x0: e.clientX, y0: e.clientY, moved: false, additive: e.shiftKey });
      return;
    }
    // v0.3.1: a group (as the engine lists it: one `form` object) opens its text too, through
    // 그룹 해제 후 편집할까요?
    if (e.detail >= 2 && (hit.type === "text" || hit.type === "form")) {
      void beginParagraphEdit(ctx.index, at, { onGroup: hit.type === "form" });
      return;
    }
    let ids = selected;
    if (e.shiftKey) {
      ids = selected.includes(hit.objectId) ? selected.filter((id) => id !== hit.objectId) : [...selected, hit.objectId];
      store.select(ctx.index, ids);
      return;
    }
    if (!selected.includes(hit.objectId)) {
      ids = [hit.objectId];
      store.select(ctx.index, ids);
    }
    if (hit.editable === "readOnly") return; // the badge says why; no drag
    e.currentTarget.setPointerCapture?.(e.pointerId);
    setDrag({ kind: "move", ids, x0: e.clientX, y0: e.clientY, dx: 0, dy: 0, moved: false });
  };

  const onPointerMove = (e: React.PointerEvent<HTMLDivElement>) => {
    if (!drag) {
      if (tool === "select" || tool === "editText" || tool === "redact") {
        const [x, y] = toPage(e);
        const hit = tool === "redact" && markAt(marks, x, y) ? null : hitObject(objects, x, y);
        const next = hit && (tool === "select" || hit.type === "text" || (hit.type === "form" && hit.text)) ? hit.objectId : null;
        if (next !== hover) setHover(next);
      }
      return;
    }
    const dxCss = e.clientX - drag.x0;
    const dyCss = e.clientY - drag.y0;
    const far = Math.hypot(dxCss, dyCss) >= DRAG_PX;
    if (drag.kind === "move") {
      setDrag({ ...drag, dx: dxCss, dy: dyCss, moved: drag.moved || far });
    } else if (drag.kind === "resize") {
      const [dx, dy] = deltaToPage(ctx.inverse, dxCss, dyCss);
      const object = objects.find((o) => o.objectId === drag.id);
      setDrag({ ...drag, to: resizeRect(drag.from, drag.corner, dx, dy, object?.type === "image" || e.shiftKey) });
    } else {
      setDrag({ ...drag, end: toPage(e), moved: drag.moved || far });
    }
  };

  const onPointerUp = (e: React.PointerEvent<HTMLDivElement>) => {
    if (!drag) return;
    if (e.currentTarget.hasPointerCapture?.(e.pointerId)) e.currentTarget.releasePointerCapture(e.pointerId);
    setDrag(null);
    if (drag.kind === "move") {
      if (!drag.moved) return;
      const [dx, dy] = deltaToPage(ctx.inverse, drag.dx, drag.dy);
      void moveObjects(ctx.index, drag.ids, round2(dx), round2(dy));
    } else if (drag.kind === "resize") {
      void resizeObject(ctx.index, drag.id, drag.to);
    } else if (drag.kind === "mark") {
      if (drag.moved) markArea(ctx.index, rectFromPoints(drag.start, drag.end));
      else markTextRunAt(ctx.index, drag.start);
    } else if (drag.kind === "link") {
      const rect = rectFromPoints(drag.start, drag.end);
      const big = rect.r - rect.l >= MIN_LINK_PT && rect.t - rect.b >= MIN_LINK_PT;
      if (drag.moved && big) useLinkStore.getState().setDraft({ page: ctx.index, rect });
      // v0.3 pkg2 (P5): a click with text selected on this page links the selection, one quad per line
      else if (!drag.moved && selectionRectsForPage(ctx.index).length) {
        const quads = selectionRectsForPage(ctx.index);
        const union = quads.reduce((a, q) => ({ l: Math.min(a.l, q.l), b: Math.min(a.b, q.b), r: Math.max(a.r, q.r), t: Math.max(a.t, q.t) }));
        useLinkStore.getState().setDraft({ page: ctx.index, rect: union, quads });
      }
      else useLinkStore.getState().select(null);
    } else if (drag.kind === "marquee") {
      const store = useEditStore.getState();
      const inside = drag.moved ? objectsInside(objects, rectFromPoints(drag.start, drag.end)).map((o) => o.objectId) : [];
      const ids = drag.additive ? [...selected, ...inside.filter((id) => !selected.includes(id))] : inside;
      if (ids.length) store.select(ctx.index, ids);
      else if (!drag.additive) store.clearSelection();
    } else {
      const rect = drag.moved ? rectFromPoints(drag.start, drag.end) : imageRectAt(drag.start, ctx.page);
      void addImageFlow(ctx.index, rect);
    }
  };

  const startResize = (e: React.PointerEvent<HTMLDivElement>, object: PageObject, corner: Corner) => {
    if (e.button !== 0) return;
    e.stopPropagation();
    e.preventDefault();
    ref.current?.setPointerCapture?.(e.pointerId);
    setDrag({ kind: "resize", id: object.objectId, corner, from: object.rect, to: object.rect, x0: e.clientX, y0: e.clientY });
  };

  // ------------------------------------------------------------------ render
  const selectedObjects = objects.filter((o) => selected.includes(o.objectId));
  const hovered = hover !== null && !selected.includes(hover) ? objects.find((o) => o.objectId === hover) : undefined;
  const moveOffset = drag?.kind === "move" && drag.moved ? { x: drag.dx, y: drag.dy } : { x: 0, y: 0 };
  const single = selectedObjects.length === 1 && !drag ? selectedObjects[0] : null;
  const marquee = drag?.kind === "marquee" && drag.moved ? rectFromPoints(drag.start, drag.end) : null;
  const draftMark = drag?.kind === "mark" && drag.moved ? rectFromPoints(drag.start, drag.end) : null;

  return (
    <div
      ref={ref}
      className="edit-surface"
      data-tool={tool}
      data-hover={hovered ? hovered.type : undefined}
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={onPointerUp}
      onPointerCancel={() => setDrag(null)}
      onPointerLeave={() => !drag && setHover(null)}
    >
      {tool === "select" && hovered && <Outline ctx={ctx} rect={hovered.rect} state="hover" />}
      {selectedObjects.map((o) => {
        const rect = drag?.kind === "resize" && drag.id === o.objectId ? drag.to : o.rect;
        return (
          <Outline
            key={o.objectId}
            ctx={ctx}
            rect={rect}
            state="selected"
            dx={o.editable === "readOnly" ? 0 : moveOffset.x}
            dy={o.editable === "readOnly" ? 0 : moveOffset.y}
          />
        );
      })}
      {(tool === "editText" || tool === "redact") && (hovered?.type === "text" || (hovered?.type === "form" && hovered.text)) && (
        <Outline ctx={ctx} rect={hovered.rect} state="hover" />
      )}
      {preview?.status === "ready" &&
        preview.result.textObjects
          .filter((o) => !o.fullyInside && !o.split) // v0.3 (R2): a split run loses nothing outside the marks
          .map((o) => {
            const box = ctx.rectToBox(o.rect);
            return (
              <div
                key={`c-${o.objectId}`}
                className="redact-collateral"
                title={t("redact.collateral")}
                style={{ left: box.x - 1, top: box.y - 1, width: box.w + 2, height: box.h + 2 }}
              />
            );
          })}
      {marks.map((m) => {
        const box = ctx.rectToBox(m.rect);
        const on = m.id === markSel && tool === "redact";
        return (
          <div
            key={m.id}
            className="redact-mark"
            data-selected={on || undefined}
            style={{ left: box.x, top: box.y, width: box.w, height: box.h }}
          >
            {on && (
              <button
                type="button"
                className="redact-mark-remove"
                aria-label={t("redact.removeMark")}
                onPointerDown={(e) => e.stopPropagation()}
                onClick={() => useEditStore.getState().removeMark(m.id)}
              >
                <X size={12} strokeWidth={2.25} />
              </button>
            )}
          </div>
        );
      })}
      {single?.editable === "full" &&
        CORNERS.map((corner) => {
          const [x, y] = ctx.toDevice(...cornerPoint(single.rect, corner));
          return (
            <div
              key={corner}
              className="edit-handle"
              data-corner={corner}
              style={{ left: x - 4, top: y - 4 }}
              onPointerDown={(e) => startResize(e, single, corner)}
            />
          );
        })}
      {[...selectedObjects, ...(hovered ? [hovered] : [])]
        .filter((o) => o.editable !== "full")
        .map((o) => {
          const box = ctx.rectToBox(o.rect);
          return (
            <span
              key={`badge-${o.objectId}`}
              className="edit-badge text-xs"
              data-editable={o.editable}
              title={t(badgeReason(o))}
              style={{ left: box.x, top: Math.max(0, box.y - 20) }}
            >
              {t(o.type === "form" ? "edit.badge.group" : o.editable === "readOnly" ? "edit.badge.readOnly" : "edit.badge.moveOnly")}
              {(o.reason || o.type === "form") && <span className="edit-badge-reason"> · {t(badgeReason(o))}</span>}
            </span>
          );
        })}
      {drag?.kind === "image" && drag.moved && <Outline ctx={ctx} rect={rectFromPoints(drag.start, drag.end)} state="draft" />}
      {draftMark && (
        <div className="redact-mark" data-draft style={boxStyle(ctx.rectToBox(markRectFor(ctx.index, draftMark) ?? draftMark))} />
      )}
      {marquee &&
        objectsInside(objects, marquee)
          .filter((o) => !selected.includes(o.objectId))
          .map((o) => <Outline key={`m-${o.objectId}`} ctx={ctx} rect={o.rect} state="hover" />)}
      {marquee && <div className="edit-marquee" data-testid="edit-marquee" style={boxStyle(ctx.rectToBox(marquee))} />}
      {links.map((a) => (
        <div
          key={a.id}
          className="edit-link"
          data-selected={a.id === linkSel || undefined}
          data-kind={a.dest ? "page" : "url"}
          style={boxStyle(ctx.rectToBox(a.rect))}
        />
      ))}
      {drag?.kind === "link" && drag.moved && <div className="edit-link" data-draft style={boxStyle(ctx.rectToBox(rectFromPoints(drag.start, drag.end)))} />}
      {linkDraft && tool === "link" && <LinkPopover ctx={ctx} rect={linkDraft} />}
      {session && <TextEditor key={sessionKey(session)} ctx={ctx} session={session} />}
    </div>
  );
}

/**
 * One editor per session: a patch — even a probe found again elsewhere on the page, with new object
 * ids and a new box — keeps the same textarea and what was typed in it. Only a new session (a new
 * serial) mounts a new editor.
 */
function sessionKey(s: NonNullable<ReturnType<typeof useEditStore.getState>["session"]>): string {
  return `${s.kind === "paragraph" ? "p" : "a"}:${s.serial ?? 0}`;
}

function boxStyle(box: { x: number; y: number; w: number; h: number }): React.CSSProperties {
  return { left: box.x, top: box.y, width: box.w, height: box.h };
}

function inside(r: Rect, x: number, y: number): boolean {
  return x >= Math.min(r.l, r.r) && x <= Math.max(r.l, r.r) && y >= Math.min(r.b, r.t) && y <= Math.max(r.b, r.t);
}

/**
 * The 링크 popover (P2): right under the rectangle just drawn (above it near the page's bottom
 * edge) — 페이지로 이동 | 웹 주소, 만들기 / 취소. The rectangle stays outlined until it is answered.
 */
function LinkPopover({ ctx, rect }: { ctx: PageLayerContext; rect: Rect }) {
  const t = useT();
  const box = ctx.rectToBox(rect);
  const below = box.y + box.h + 6;
  const top = below + 190 > ctx.height ? Math.max(0, box.y - 196) : below;
  const left = Math.max(0, Math.min(box.x, ctx.width - 280));
  const close = () => useLinkStore.getState().setDraft(null);
  return (
    <>
      <div className="edit-link" data-draft style={boxStyle(box)} />
      <div
        className="link-popover"
        role="dialog"
        aria-label={t("link.new")}
        style={{ left, top }}
        onPointerDown={(e) => e.stopPropagation()}
      >
        <LinkTargetForm
          submitKey="link.create"
          autoFocus
          onCancel={close}
          onSubmit={(target) => {
            const quads = useLinkStore.getState().draft?.quads;
            close();
            void createLink(ctx.index, rect, target, quads);
          }}
        />
      </div>
    </>
  );
}

function round2(v: number): number {
  return Math.round(v * 100) / 100;
}

function Outline({ ctx, rect, state, dx = 0, dy = 0 }: {
  ctx: PageLayerContext;
  rect: Rect;
  state: "hover" | "selected" | "draft";
  dx?: number;
  dy?: number;
}) {
  const box = ctx.rectToBox(rect);
  return (
    <div
      className="edit-outline"
      data-state={state}
      style={{ left: box.x + dx - 1, top: box.y + dy - 1, width: box.w + 2, height: box.h + 2 }}
    />
  );
}

export default EditLayer;
