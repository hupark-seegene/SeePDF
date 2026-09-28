/**
 * The 편집 mode surface of one page (Stage 7) — mounted in `PageShell`'s `surface` slot while
 * 편집 is the mode, so it sees every pointer event on the page before the viewer's text
 * selection does.
 *
 *   선택        hover outlines · click selects (⇧ adds) · drag moves · corner handles scale ·
 *               double-click on text opens 문단 편집
 *   텍스트 수정  click → `probe_paragraph` → the editing box
 *   텍스트 추가  click → an empty editing box
 *   이미지 추가  click or drag a box → file picker → `add_image_object`
 *
 * The page's objects are listed on mount and again whenever the document generation moves past
 * the one we hold (object ids are per-generation, IPC_CONTRACT §7.4).
 */
import { useEffect, useRef, useState } from "react";
import type { ObjectId, PageObject, Point, Rect } from "../ipc/types";
import type { PageLayerContext } from "../viewer";
import { useT } from "../i18n/useT";
import { useAppStore } from "../store/appStore";
import { useEditStore } from "./editStore";
import {
  addImageFlow, beginAddText, beginParagraphEdit, loadPage, moveObjects, reasonKey, resizeObject,
} from "./actions";
import {
  CORNERS, cornerPoint, deltaToPage, hitObject, imageRectAt, rectFromPoints, resizeRect, type Corner,
} from "./geometry";
import { TextEditor } from "./TextEditor";

const EMPTY: ObjectId[] = [];
const NO_OBJECTS: PageObject[] = [];
/** CSS px a press must travel before it is a drag rather than a click */
const DRAG_PX = 3;

type Drag =
  | { kind: "move"; ids: ObjectId[]; x0: number; y0: number; dx: number; dy: number; moved: boolean }
  | { kind: "resize"; id: ObjectId; corner: Corner; from: Rect; to: Rect; x0: number; y0: number }
  | { kind: "image"; start: Point; end: Point; x0: number; y0: number; moved: boolean };

export function EditLayer({ ctx }: { ctx: PageLayerContext }) {
  const t = useT();
  const tool = useAppStore((s) => s.tool);
  const entry = useEditStore((s) => s.pages[ctx.index]);
  const boundDoc = useEditStore((s) => s.docId);
  const selected = useEditStore((s) => (s.selection?.page === ctx.index ? s.selection.ids : EMPTY));
  const session = useEditStore((s) => (s.session?.page === ctx.index ? s.session : null));
  const anySession = useEditStore((s) => s.session !== null);
  const [hover, setHover] = useState<ObjectId | null>(null);
  const [drag, setDrag] = useState<Drag | null>(null);
  const ref = useRef<HTMLDivElement>(null);

  const objects = entry?.objects ?? NO_OBJECTS;
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
    if (tool === "addImage") {
      e.currentTarget.setPointerCapture?.(e.pointerId);
      setDrag({ kind: "image", start: at, end: at, x0: e.clientX, y0: e.clientY, moved: false });
      return;
    }
    if (tool !== "select") return;

    const hit = hitObject(objects, at[0], at[1]);
    if (!hit) {
      store.clearSelection();
      return;
    }
    if (e.detail >= 2 && hit.type === "text") {
      void beginParagraphEdit(ctx.index, at);
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
      if (tool === "select" || tool === "editText") {
        const [x, y] = toPage(e);
        const hit = hitObject(objects, x, y);
        const next = hit && (tool === "select" || hit.type === "text") ? hit.objectId : null;
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
      {tool === "editText" && hovered?.type === "text" && <Outline ctx={ctx} rect={hovered.rect} state="hover" />}
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
              title={t(reasonKey(o.reason))}
              style={{ left: box.x, top: Math.max(0, box.y - 20) }}
            >
              {t(o.editable === "readOnly" ? "edit.badge.readOnly" : "edit.badge.moveOnly")}
              {o.reason && <span className="edit-badge-reason"> · {t(reasonKey(o.reason))}</span>}
            </span>
          );
        })}
      {drag?.kind === "image" && drag.moved && <Outline ctx={ctx} rect={rectFromPoints(drag.start, drag.end)} state="draft" />}
      {session && <TextEditor key={sessionKey(session)} ctx={ctx} session={session} />}
    </div>
  );
}

function sessionKey(s: NonNullable<ReturnType<typeof useEditStore.getState>["session"]>): string {
  return s.kind === "paragraph" ? `p:${s.probe.objectIds.join(",")}` : `a:${s.at.join(",")}`;
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
