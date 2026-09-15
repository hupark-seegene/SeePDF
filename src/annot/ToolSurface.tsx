/**
 * The `surface` slot of one `PageShell` — the topmost layer of the page, and the only one in (d)
 * that sees a pointer event.
 *
 * It is mounted **only** while a 주석 tool that draws is armed. That is deliberate:
 *
 * * the four markup tools do not get a surface, because the viewer's own pointer handler must keep
 *   producing the text selection they read on pointer-up (`Scroller.selectsText`);
 * * 양식 mode does not get one either, or it would sit on top of the form inputs.
 *
 * Coordinates: the surface covers the page box exactly, so `client − boundingRect` is CSS px
 * inside the box and `ctx.toPage` turns it into PDF user space (STAGE1C_NOTES §1.1).
 */
import { useCallback, useRef } from "react";
import type { PageLayerContext } from "../viewer";
import { toolController, type ToolContext } from "../tools/ToolController";
import { GRAB_PX } from "../tools/select";
import { annotAt } from "../tools/hit";
import { useAnnotStore } from "../store/annotStore";
import { annotsOnPage } from "./actions";
import { selectionRectsForPage } from "./selectionQuads";
import { stampImage } from "../tools/stamp";

export interface ToolSurfaceProps {
  ctx: PageLayerContext;
  /** the armed tool, so the cursor and the hover behaviour follow it without a store read */
  tool: string;
}

interface Mods {
  shiftKey: boolean;
  altKey: boolean;
  metaKey: boolean;
  ctrlKey: boolean;
}

export function toolContext(ctx: PageLayerContext, e: Mods): ToolContext {
  const store = useAnnotStore.getState();
  const tool = toolController.current();
  return {
    docId: ctx.docId,
    docGeneration: ctx.docGeneration,
    style: store.style,
    modifiers: { shift: e.shiftKey, alt: e.altKey, meta: e.metaKey, ctrl: e.ctrlKey },
    scale: ctx.scale,
    annots: annotsOnPage,
    selected: store.selected,
    selectionRects: selectionRectsForPage,
    image: tool === "stamp" || tool === "signature" ? stampImage(tool) : null,
  };
}

export function ToolSurface({ ctx, tool }: ToolSurfaceProps) {
  const ref = useRef<HTMLDivElement>(null);

  const pointAt = useCallback(
    (clientX: number, clientY: number) => {
      const el = ref.current;
      if (!el) return null;
      const rect = el.getBoundingClientRect();
      const [x, y] = ctx.toPage(clientX - rect.left, clientY - rect.top);
      return { page: ctx.index, pt: [x, y] as [number, number] };
    },
    [ctx],
  );

  const onPointerDown = useCallback(
    (e: React.PointerEvent<HTMLDivElement>) => {
      if (e.button !== 0) return;
      const at = pointAt(e.clientX, e.clientY);
      if (!at) return;
      e.stopPropagation();
      e.preventDefault();
      // Keep the canvas focused so the tool keys and ⌫ stay in the `canvas` keymap context.
      (e.currentTarget.closest(".canvas") as HTMLElement | null)?.focus({ preventScroll: true });
      e.currentTarget.setPointerCapture(e.pointerId);
      toolController.pointerDown(at, toolContext(ctx, e));
    },
    [ctx, pointAt],
  );

  const onPointerMove = useCallback(
    (e: React.PointerEvent<HTMLDivElement>) => {
      const at = pointAt(e.clientX, e.clientY);
      if (!at) return;
      if (toolController.active()) e.stopPropagation();
      toolController.pointerMove(at, toolContext(ctx, e));
      if (tool === "select" && !toolController.active()) {
        const hit = annotAt(annotsOnPage(ctx.index), at.pt[0], at.pt[1], GRAB_PX / ctx.scale);
        useAnnotStore.getState().hover(hit?.id ?? null);
      }
    },
    [ctx, pointAt, tool],
  );

  const onPointerUp = useCallback(
    (e: React.PointerEvent<HTMLDivElement>) => {
      const at = pointAt(e.clientX, e.clientY);
      if (!at) return;
      e.stopPropagation();
      if (e.currentTarget.hasPointerCapture(e.pointerId)) e.currentTarget.releasePointerCapture(e.pointerId);
      toolController.pointerUp(at, toolContext(ctx, e));
    },
    [ctx, pointAt],
  );

  const onPointerCancel = useCallback(() => {
    toolController.cancel();
  }, []);

  const onPointerLeave = useCallback(() => {
    if (!toolController.active()) {
      toolController.cancel();
      useAnnotStore.getState().hover(null);
    }
  }, []);

  return (
    <div
      ref={ref}
      className="annot-surface"
      data-tool={tool}
      style={{ cursor: toolController.cursor() }}
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={onPointerUp}
      onPointerCancel={onPointerCancel}
      onPointerLeave={onPointerLeave}
      /* No `stopPropagation` on contextmenu: swallowing it hid the canvas menu from every
         bubble-phase listener (STAGE1E_NOTES §5.2). A layer that wants to own the menu marks
         itself `data-context-menu` instead — both `App.tsx` and `pageMenus.ts` skip those
         subtrees — and `App.tsx` still calls `preventDefault()` over a page, so the webview's
         own menu never appears. */
    />
  );
}
