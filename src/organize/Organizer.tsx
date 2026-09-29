/**
 * 페이지 organizer (UI_SPEC §9, F-16). The canvas is replaced by a virtualised grid of `/thumb`
 * images; the sidebar auto-collapses.
 *
 * Design notes:
 * - The layout comes from the page list, not from pixels, so cells never reflow (spikes/pages.md §5).
 *   Only the rows inside ±1 screen are mounted, which is what keeps 500 pages at 60 fps.
 * - Every mutation goes through `page_ops` (`runPageOps`) — one call, one generation, one undo step —
 *   and the page list comes back through `doc-changed` (structure) → `docStore.refresh()`.
 * - A drag paints the new order optimistically (`pagesStore.pendingOrder`) and emits exactly one
 *   `move` op (`moveOpFor`); the response reconciles it away.
 */
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import {
  ArrowLeftRight, Copy, Crop, FilePlus2, MoveRight, Plus, RotateCcw, RotateCw, Scaling, Scissors, SplitSquareHorizontal,
  Trash2, Tags,
} from "lucide-react";
import { useT } from "../i18n/useT";
import { thumbUrl } from "../ipc/protocol";
import { useDocStore } from "../store/docStore";
import { useViewStore } from "../store/viewStore";
import { useAppStore } from "../store/appStore";
import { usePagesStore, THUMB_SIZES, type ThumbSize } from "../store/pagesStore";
import { openContextMenu } from "../app/contextMenuStore";
import { openDialog } from "../dialogs/dialogState";
import { displayLabel } from "../viewer/pageLabel";
import { boxFromPoints, boxesIntersect, marqueeSelect, pressSelect, stepFocus, type Box } from "./selection";
import { applyMove, identityOrder, moveOpFor } from "./moveOp";
import type { PageGeom, PageIndex, PageOp } from "../ipc/types";
// v0.3 pkg2-pages-structure-forms: OS file drops at the caret (P2), pages to other windows (P3)
import { setOrganizerCaret } from "./fileDrop";
import { publishPagesDrop, releasedOutside } from "./crossWindow";
import "./organize.css";

const GAP = 12;
const LABEL_H = 26;
const DRAG_SLOP = 5;
/** v0.3 P2: a marquee this close to the grid's top / bottom edge scrolls it. */
const AUTO_SCROLL_EDGE = 32;

export function Organizer() {
  const t = useT();
  const info = useDocStore((s) => s.info);
  const changeNonce = useDocStore((s) => s.changeNonce);
  const setMode = useAppStore((s) => s.setMode);
  const goToPage = useViewStore((s) => s.goToPage);
  const pages = usePagesStore();

  const scrollRef = useRef<HTMLDivElement>(null);
  const contentRef = useRef<HTMLDivElement>(null);
  const [box, setBox] = useState({ w: 0, h: 0 });
  const [scrollTop, setScrollTop] = useState(0);
  const [marquee, setMarquee] = useState<Box | null>(null);
  const [busy, setBusy] = useState(false);
  const drag = useRef<{ page: PageIndex; x: number; y: number; moved: boolean } | null>(null);

  const count = info?.pageCount ?? 0;
  const order = pages.pendingOrder ?? identityOrder(count);
  const thumb = pages.thumbSize;
  const cellW = thumb;
  const cellImgH = Math.round(thumb * 1.32);
  const cellH = cellImgH + LABEL_H;
  const rowH = cellH + GAP;
  const cols = Math.max(1, Math.floor((box.w - GAP) / (cellW + GAP)) || 1);
  const rows = Math.ceil(order.length / cols);
  const contentH = GAP + rows * rowH;

  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    const update = () => {
      const w = el.clientWidth || 800;
      const h = el.clientHeight || 600;
      setBox((prev) => (prev.w === w && prev.h === h ? prev : { w, h }));
    };
    update();
    if (typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver(update);
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  // a structural change (or a new document) invalidates the optimistic order
  useEffect(() => {
    usePagesStore.getState().setPendingOrder(null);
  }, [changeNonce]);

  const first = Math.max(0, Math.floor((scrollTop - box.h * 0.5) / rowH));
  const last = Math.min(rows - 1, Math.ceil((scrollTop + box.h * 1.5) / rowH));

  const cellBox = useCallback(
    (slot: number): Box => ({
      x: GAP + (slot % cols) * (cellW + GAP),
      y: GAP + Math.floor(slot / cols) * rowH,
      w: cellW,
      h: cellH,
    }),
    [cols, cellW, cellH, rowH],
  );

  /** Pointer position inside the scrolled content. */
  const localPoint = (e: { clientX: number; clientY: number }) => {
    const rect = contentRef.current?.getBoundingClientRect();
    return { x: e.clientX - (rect?.left ?? 0), y: e.clientY - (rect?.top ?? 0) };
  };

  /** The insertion caret (0…count) nearest to a point — UI_SPEC §9's 2 px vertical bar. */
  const caretAt = useCallback(
    (x: number, y: number): number => {
      const row = Math.max(0, Math.min(rows - 1, Math.floor((y - GAP) / rowH)));
      const rawCol = (x - GAP) / (cellW + GAP);
      const col = Math.max(0, Math.min(cols, Math.round(rawCol)));
      return Math.max(0, Math.min(order.length, row * cols + col));
    },
    [rows, rowH, cols, cellW, order.length],
  );

  // v0.3 P2 / P3: a file or a page from another window dropped on the grid lands at this caret
  useEffect(() => {
    setOrganizerCaret((clientX, clientY) => {
      const rect = contentRef.current?.getBoundingClientRect();
      return caretAt(clientX - (rect?.left ?? 0), clientY - (rect?.top ?? 0));
    });
    return () => setOrganizerCaret(null);
  }, [caretAt]);

  const commitMove = useCallback(
    async (caret: number) => {
      const selected = usePagesStore.getState().selected;
      const moving = selected.length ? selected : drag.current ? [drag.current.page] : [];
      const op = moveOpFor(identityOrder(count), moving, caret);
      usePagesStore.getState().setDropAt(null);
      if (!op) return;
      usePagesStore.getState().setPendingOrder(applyMove(identityOrder(count), moving, caret));
      setBusy(true);
      const { runPageOps } = await import("../dialogs/flows");
      const ok = await runPageOps([op]);
      // the block keeps its selection at its new home, the way every page organizer behaves
      if (ok) usePagesStore.getState().setSelected(op.pages.map((_, i) => op.to + i));
      setBusy(false);
    },
    [count],
  );

  const onPointerDown = (e: React.PointerEvent, page: PageIndex | null) => {
    if (e.button !== 0) return;
    const point = localPoint(e);
    const mods = { shift: e.shiftKey, meta: e.metaKey || e.ctrlKey };
    if (page === null) {
      // marquee over empty space
      const base = { selected: pages.selected, anchor: pages.lastAnchor };
      if (!mods.meta && !mods.shift) pages.clear();
      setMarquee({ x: point.x, y: point.y, w: 0, h: 0 });
      const startBase = base;
      let last = { clientX: e.clientX, clientY: e.clientY, additive: false };
      const select = () => {
        const p = localPoint(last);
        const rect = boxFromPoints(point.x, point.y, p.x, p.y);
        setMarquee(rect);
        const inside = order.filter((_, slot) => boxesIntersect(rect, cellBox(slot)));
        const next = marqueeSelect(startBase, inside, last.additive);
        usePagesStore.getState().setSelected(next.selected);
      };
      // v0.3 P2: a marquee held near the top / bottom edge scrolls the grid, faster the closer
      let raf = 0;
      const edgeSpeed = (): number => {
        const el = scrollRef.current;
        if (!el) return 0;
        const r = el.getBoundingClientRect();
        if (!r.height) return 0;
        if (last.clientY < r.top + AUTO_SCROLL_EDGE) return -Math.ceil((r.top + AUTO_SCROLL_EDGE - last.clientY) / 3);
        if (last.clientY > r.bottom - AUTO_SCROLL_EDGE) return Math.ceil((last.clientY - (r.bottom - AUTO_SCROLL_EDGE)) / 3);
        return 0;
      };
      const tick = () => {
        const el = scrollRef.current;
        const speed = edgeSpeed();
        if (!el || !speed) {
          raf = 0;
          return;
        }
        el.scrollTop += speed;
        setScrollTop(el.scrollTop);
        select();
        raf = requestAnimationFrame(tick);
      };
      const move = (ev: PointerEvent) => {
        last = { clientX: ev.clientX, clientY: ev.clientY, additive: ev.metaKey || ev.ctrlKey || ev.shiftKey };
        select();
        if (!raf && edgeSpeed()) raf = requestAnimationFrame(tick);
      };
      const up = () => {
        if (raf) cancelAnimationFrame(raf);
        raf = 0;
        setMarquee(null);
        window.removeEventListener("pointermove", move);
        window.removeEventListener("pointerup", up);
      };
      window.addEventListener("pointermove", move);
      window.addEventListener("pointerup", up);
      return;
    }
    const next = pressSelect({ selected: pages.selected, anchor: pages.lastAnchor }, page, mods);
    usePagesStore.setState({ selected: next.selected, lastAnchor: next.anchor, focus: page });
    drag.current = { page, x: e.clientX, y: e.clientY, moved: false };

    const move = (ev: PointerEvent) => {
      const d = drag.current;
      if (!d) return;
      if (!d.moved && Math.hypot(ev.clientX - d.x, ev.clientY - d.y) < DRAG_SLOP) return;
      d.moved = true;
      const p = localPoint(ev);
      usePagesStore.getState().setDropAt(caretAt(p.x, p.y));
    };
    const up = (ev: PointerEvent) => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      const d = drag.current;
      drag.current = null;
      if (!d) return;
      if (d.moved && releasedOutside(ev.clientX, ev.clientY)) {
        // v0.3 P3: released outside this window — another window may take the pages
        usePagesStore.getState().setDropAt(null);
        const selected = usePagesStore.getState().selected;
        if (info) publishPagesDrop(info.docId, selected.length ? selected : [d.page], ev.screenX, ev.screenY);
      } else if (d.moved) {
        const p = localPoint(ev);
        void commitMove(caretAt(p.x, p.y));
      } else if (!mods.meta && !mods.shift) {
        usePagesStore.getState().click(page, {});
      }
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  };

  const onKeyDown = (e: React.KeyboardEvent) => {
    if (!info) return;
    const step = { ArrowLeft: -1, ArrowRight: 1, ArrowUp: -cols, ArrowDown: cols }[e.key];
    if (step !== undefined) {
      e.preventDefault();
      const next = stepFocus(pages.focus, step, count);
      pages.setFocus(next);
      if (e.shiftKey) pages.selectRange(next);
      else usePagesStore.getState().click(next, {});
      scrollSlotIntoView(next);
      return;
    }
    if (e.key === "Home" || e.key === "End") {
      e.preventDefault();
      const next = e.key === "Home" ? 0 : count - 1;
      pages.setFocus(next);
      usePagesStore.getState().click(next, {});
      scrollSlotIntoView(next);
      return;
    }
    if (e.key === "Enter" && pages.focus !== null) {
      e.preventDefault();
      openInReader(pages.focus);
    }
    if (e.key === " " && pages.focus !== null) {
      e.preventDefault();
      usePagesStore.getState().click(pages.focus, { meta: true });
    }
  };

  const scrollSlotIntoView = (page: PageIndex) => {
    const el = scrollRef.current;
    if (!el) return;
    const slot = order.indexOf(page);
    if (slot < 0) return;
    const b = cellBox(slot);
    if (b.y < el.scrollTop) el.scrollTop = Math.max(0, b.y - GAP);
    else if (b.y + b.h > el.scrollTop + el.clientHeight) el.scrollTop = b.y + b.h - el.clientHeight + GAP;
  };

  const openInReader = (page: PageIndex) => {
    goToPage(page);
    setMode("read");
  };

  const run = useCallback(async (ops: PageOp[]) => {
    setBusy(true);
    const { runPageOps } = await import("../dialogs/flows");
    await runPageOps(ops);
    setBusy(false);
  }, []);

  const selected = pages.selected;
  const target = selected.length ? selected : pages.focus !== null ? [pages.focus] : [];

  const actions = useMemo(
    () => ({
      rotateLeft: () => target.length && void run([{ kind: "rotate", pages: target, delta: 270 }]),
      rotateRight: () => target.length && void run([{ kind: "rotate", pages: target, delta: 90 }]),
      remove: () => {
        if (!target.length) return;
        usePagesStore.getState().clear();
        void run([{ kind: "delete", pages: target }]);
      },
      duplicate: () => target.length && void run([{ kind: "duplicate", pages: target }]),
      insertBlank: (at: number) => void run([{ kind: "insertBlank", at, size: "sameAs" }]),
      reverse: () => void run([{ kind: "reverse" }]),
      extract: () => target.length && openDialog("extract", { pages: target }),
      insertFrom: async (at: number) => {
        const { insertFromFileFlow } = await import("../dialogs/flows");
        await insertFromFileFlow(at);
      },
      split: () => openDialog("split"),
      // v0.3 P2: 위치 이동… — the selection (or the focused page) as one block
      moveTo: () => target.length && openDialog("moveTo", { pages: target }),
      // P2: the crop tool and 페이지 크기 변경 act on the selection (or the focused page)
      crop: () => openDialog("crop", { pages: selected.length ? selected : undefined }),
      resize: () => openDialog("resize", { pages: selected.length ? selected : undefined }),
    }),
    [target, selected, run],
  );

  const cellMenu = (page: PageIndex, x: number, y: number) => {
    if (!selected.includes(page)) usePagesStore.getState().click(page, {});
    openContextMenu({
      x,
      y,
      labelKey: "pages.title",
      items: [
        { id: "goTo", labelKey: "pages.goTo", onSelect: () => openInReader(page) },
        { id: "sep1", separator: true },
        { id: "rotateLeft", labelKey: "pages.rotateLeft", onSelect: actions.rotateLeft },
        { id: "rotateRight", labelKey: "pages.rotateRight", onSelect: actions.rotateRight },
        { id: "duplicate", labelKey: "pages.duplicate", onSelect: actions.duplicate },
        { id: "extract", labelKey: "pages.extract", onSelect: actions.extract },
        { id: "insertAfter", labelKey: "pages.insertAfter", onSelect: () => void actions.insertFrom(page + 1) },
        { id: "moveTo", labelKey: "pages.moveTo", onSelect: actions.moveTo },
        { id: "crop", labelKey: "pages.crop", onSelect: actions.crop },
        { id: "resize", labelKey: "pages.resize", onSelect: actions.resize },
        { id: "sep2", separator: true },
        { id: "exportImage", labelKey: "pages.exportImage", onSelect: () => openDialog("export") },
        { id: "delete", labelKey: "pages.delete", danger: true, onSelect: actions.remove },
      ],
    });
  };

  useLayoutEffect(() => {
    if (pages.focus === null && count > 0) usePagesStore.getState().setFocus(0);
  }, [pages.focus, count]);

  if (!info) return null;

  const dpr = typeof window === "undefined" ? 1 : Math.min(2, window.devicePixelRatio || 1);
  const slots: number[] = [];
  for (let row = first; row <= last; row++) {
    for (let col = 0; col < cols; col++) {
      const slot = row * cols + col;
      if (slot < order.length) slots.push(slot);
    }
  }

  return (
    <div className="organizer" data-busy={busy || undefined}>
      <div
        className="org-grid"
        ref={scrollRef}
        role="listbox"
        aria-multiselectable="true"
        aria-label={t("pages.title")}
        tabIndex={0}
        onScroll={(e) => setScrollTop(e.currentTarget.scrollTop)}
        onKeyDown={onKeyDown}
        onPointerDown={(e) => {
          if ((e.target as HTMLElement).closest(".org-cell")) return;
          onPointerDown(e, null);
        }}
        onContextMenu={(e) => {
          if ((e.target as HTMLElement).closest(".org-cell")) return;
          e.preventDefault();
          openContextMenu({
            x: e.clientX,
            y: e.clientY,
            labelKey: "pages.title",
            items: [
              { id: "selectAll", labelKey: "menu.edit.selectAll", onSelect: () => pages.selectAll(count) },
              { id: "insertBlank", labelKey: "pages.insertBlank", onSelect: () => actions.insertBlank(count) },
              { id: "insertFrom", labelKey: "pages.insertFromFile", onSelect: () => void actions.insertFrom(count) },
              { id: "reverse", labelKey: "pages.reverse", onSelect: actions.reverse },
            ],
          });
        }}
      >
        <div className="org-content" ref={contentRef} style={{ height: contentH }}>
          {slots.map((slot) => {
            const page = order[slot];
            const geom = info.pages[page];
            if (!geom) return null;
            const b = cellBox(slot);
            const isSelected = selected.includes(page);
            return (
              <div
                key={page}
                className="org-cell"
                role="option"
                aria-selected={isSelected}
                aria-label={t("a11y.pageThumbnail", { n: page + 1 })}
                data-page={page}
                data-selected={isSelected || undefined}
                data-focused={pages.focus === page || undefined}
                style={{ left: b.x, top: b.y, width: b.w, height: b.h }}
                onPointerDown={(e) => onPointerDown(e, page)}
                onDoubleClick={() => openInReader(page)}
                onContextMenu={(e) => {
                  e.preventDefault();
                  cellMenu(page, e.clientX, e.clientY);
                }}
              >
                <span className="org-thumb" style={{ height: cellImgH }}>
                  <img
                    src={thumbUrl({
                      doc: info.docId,
                      gen: info.docGeneration,
                      page,
                      w: Math.round(cellW * dpr),
                    })}
                    alt=""
                    draggable={false}
                    style={aspect(geom, cellW, cellImgH)}
                  />
                  {geom.rotation !== 0 && <span className="org-badge text-xs mono">{geom.rotation}°</span>}
                  <span className="org-hover">
                    <button
                      type="button"
                      className="icon-btn"
                      aria-label={t("pages.rotateLeft")}
                      onPointerDown={(e) => e.stopPropagation()}
                      onClick={() => void run([{ kind: "rotate", pages: [page], delta: 270 }])}
                    >
                      <RotateCcw size={16} strokeWidth={1.75} aria-hidden />
                    </button>
                    <button
                      type="button"
                      className="icon-btn"
                      aria-label={t("pages.rotateRight")}
                      onPointerDown={(e) => e.stopPropagation()}
                      onClick={() => void run([{ kind: "rotate", pages: [page], delta: 90 }])}
                    >
                      <RotateCw size={16} strokeWidth={1.75} aria-hidden />
                    </button>
                    <button
                      type="button"
                      className="icon-btn"
                      data-tone="danger"
                      aria-label={t("pages.delete")}
                      onPointerDown={(e) => e.stopPropagation()}
                      onClick={() => void run([{ kind: "delete", pages: [page] }])}
                    >
                      <Trash2 size={16} strokeWidth={1.75} aria-hidden />
                    </button>
                  </span>
                </span>
                <span className="org-num text-xs mono" title={info.pageLabels?.[page] ? String(page + 1) : undefined}>
                  {displayLabel(info.pageLabels, page)}
                </span>
                <button
                  type="button"
                  className="org-insert"
                  aria-label={t("pages.insertHere")}
                  onPointerDown={(e) => e.stopPropagation()}
                  onClick={() => actions.insertBlank(slot + 1)}
                >
                  <Plus size={14} strokeWidth={2} aria-hidden />
                </button>
              </div>
            );
          })}

          {pages.dropAt !== null && <span className="org-caret" style={caretStyle(pages.dropAt, cellBox, cols, cellH)} />}
          {marquee && (
            <span className="org-marquee" style={{ left: marquee.x, top: marquee.y, width: marquee.w, height: marquee.h }} />
          )}
        </div>
      </div>

      <aside className="org-rail" aria-label={t("pages.title")}>
        <p className="text-sm dim">{t("pages.selected", { count: selected.length })}</p>
        <RailButton icon={RotateCcw} labelKey="pages.rotateLeft" disabled={!target.length} onSelect={actions.rotateLeft} />
        <RailButton icon={RotateCw} labelKey="pages.rotateRight" disabled={!target.length} onSelect={actions.rotateRight} />
        <RailButton icon={Trash2} labelKey="pages.delete" danger disabled={!target.length} onSelect={actions.remove} />
        <RailButton icon={Scissors} labelKey="pages.extract" disabled={!target.length} onSelect={actions.extract} />
        <RailButton icon={Copy} labelKey="pages.duplicate" disabled={!target.length} onSelect={actions.duplicate} />
        <RailButton icon={Plus} labelKey="pages.insertBlank" onSelect={() => actions.insertBlank(nextInsertAt(selected, count))} />
        <RailButton icon={FilePlus2} labelKey="pages.insertFromFile" onSelect={() => void actions.insertFrom(nextInsertAt(selected, count))} />
        <RailButton icon={ArrowLeftRight} labelKey="pages.reverse" onSelect={actions.reverse} />
        <RailButton icon={SplitSquareHorizontal} labelKey="pages.split" onSelect={actions.split} />
        <RailButton icon={Crop} labelKey="pages.crop" onSelect={actions.crop} />
        <RailButton icon={Scaling} labelKey="pages.resize" onSelect={actions.resize} />
        {/* P2 */}
        <RailButton icon={Tags} labelKey="pageLabels.open" onSelect={() => openDialog("pageLabels")} />
        {/* v0.3 P2 */}
        <RailButton icon={MoveRight} labelKey="pages.moveTo" disabled={!target.length} onSelect={actions.moveTo} />

        <span className="rail-spacer" />
        <label className="org-size text-xs dim">
          {t("pages.thumbnailSize")}
          <input
            type="range"
            className="slider"
            min={0}
            max={THUMB_SIZES.length - 1}
            step={1}
            value={THUMB_SIZES.indexOf(thumb)}
            aria-label={t("pages.thumbnailSize")}
            onChange={(e) => pages.setThumbSize(THUMB_SIZES[Number(e.target.value)] as ThumbSize)}
          />
        </label>
      </aside>
    </div>
  );
}

function RailButton({
  icon: Icon,
  labelKey,
  onSelect,
  disabled,
  danger,
}: {
  icon: typeof RotateCcw;
  labelKey: string;
  onSelect(): void;
  disabled?: boolean;
  danger?: boolean;
}) {
  const t = useT();
  return (
    <button type="button" className="btn quiet rail-btn" data-tone={danger ? "danger" : undefined} disabled={disabled} onClick={onSelect}>
      <Icon size={16} strokeWidth={1.75} aria-hidden />
      {t(labelKey)}
    </button>
  );
}

/** Fit the page inside the cell box without ever reflowing it. */
function aspect(page: PageGeom, w: number, h: number): { width: number; height: number } {
  const ratio = page.widthPt > 0 ? page.heightPt / page.widthPt : 1.414;
  const height = Math.min(h, Math.round(w * ratio));
  return { width: Math.round(height / ratio), height };
}

function caretStyle(caret: number, cellBox: (slot: number) => Box, cols: number, cellH: number) {
  const slot = Math.max(0, caret - 1);
  const b = cellBox(slot);
  const atRowStart = caret % cols === 0;
  const x = caret === 0 || atRowStart ? cellBox(caret).x - GAP / 2 : b.x + b.w + GAP / 2 - 1;
  const y = caret === 0 || atRowStart ? cellBox(caret).y : b.y;
  return { left: x, top: y, height: cellH };
}

function nextInsertAt(selected: PageIndex[], count: number): number {
  return selected.length ? Math.max(...selected) + 1 : count;
}
