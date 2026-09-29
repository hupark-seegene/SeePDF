/**
 * 축소판 sidebar (F-04, UI_SPEC §4).
 *
 * Virtualised the same way as the canvas: the row heights come from `DocInfo.pages[]`, so the
 * scrollbar is correct for 500 pages before a single thumbnail exists, and only the visible rows
 * ± one screen request `/thumb`. The current page keeps a 2 px accent ring and is scrolled into
 * view when it changes from somewhere else (status bar, outline, search).
 */
import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { thumbUrl } from "../ipc/protocol";
import type { PageGeom } from "../ipc/types";
import { useT } from "../i18n/useT";
import { useDocStore } from "../store/docStore";
import { useViewStore } from "../store/viewStore";
import { displayLabel } from "../viewer/pageLabel";
import { useDevicePixelRatio } from "../viewer/useDevicePixelRatio";
import "./sidebar.css";

const GAP = 12;
const LABEL_H = 18;
const MIN_W = 56;
const MAX_W = 220;
const OVERSCAN = 1;

export function Thumbnails() {
  const t = useT();
  const info = useDocStore((s) => s.info);
  const rotation = useViewStore((s) => s.rotation);
  const currentPage = useViewStore((s) => s.currentPage);
  const goToPage = useViewStore((s) => s.goToPage);
  const dpr = useDevicePixelRatio();

  const listRef = useRef<HTMLDivElement>(null);
  /** a drag just ended on a thumbnail: its click must not jump to the page */
  const dragged = useRef(false);
  const [box, setBox] = useState({ w: 0, h: 0 });
  const [scrollTop, setScrollTop] = useState(0);

  useEffect(() => {
    const el = listRef.current;
    if (!el) return;
    const update = () => {
      const w = el.clientWidth || 220;
      const h = el.clientHeight || 600;
      setBox((prev) => (prev.w === w && prev.h === h ? prev : { w, h }));
    };
    update();
    if (typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(update);
    observer.observe(el);
    return () => observer.disconnect();
  }, []);

  const pages = info?.pages ?? [];
  const thumbW = Math.max(MIN_W, Math.min(MAX_W, (box.w || 220) - 28));

  const rows = useMemo(() => {
    let y = GAP;
    const out: { page: number; y: number; h: number; w: number; imgH: number }[] = [];
    for (const page of pages) {
      const size = thumbSize(page, rotation, thumbW);
      const h = size.h + LABEL_H + GAP;
      out.push({ page: page.index, y, h, w: size.w, imgH: size.h });
      y += h;
    }
    return { items: out, height: y };
  }, [pages, rotation, thumbW]);

  const first = useMemo(() => {
    const top = scrollTop - box.h * OVERSCAN;
    let lo = 0;
    let hi = rows.items.length - 1;
    while (lo < hi) {
      const mid = (lo + hi) >> 1;
      if (rows.items[mid].y + rows.items[mid].h < top) lo = mid + 1;
      else hi = mid;
    }
    return lo;
  }, [rows, scrollTop, box.h]);

  const visible = useMemo(() => {
    const bottom = scrollTop + box.h * (1 + OVERSCAN);
    const out: typeof rows.items = [];
    for (let i = first; i < rows.items.length; i++) {
      const row = rows.items[i];
      if (row.y > bottom) break;
      out.push(row);
    }
    return out;
  }, [rows, first, scrollTop, box.h]);

  // follow the document: a page change from anywhere else scrolls the rail
  const followRef = useRef(-1);
  useLayoutEffect(() => {
    const el = listRef.current;
    if (!el || followRef.current === currentPage) return;
    followRef.current = currentPage;
    const row = rows.items[currentPage];
    if (!row) return;
    if (row.y < el.scrollTop || row.y + row.h > el.scrollTop + el.clientHeight) {
      el.scrollTop = Math.max(0, row.y - el.clientHeight / 2 + row.h / 2);
      setScrollTop(el.scrollTop);
    }
  }, [currentPage, rows]);

  if (!info) return null;

  /**
   * v0.3 P3: a thumbnail dragged out of the window and released over another SeePDF window's
   * 축소판 or page grid is copied there (`crossWindow.ts`). Inside the window nothing happens; the
   * click that follows a drag is swallowed.
   */
  function beginPageDrag(e: React.PointerEvent, docId: string, page: number) {
    if (e.button !== 0) return;
    const x0 = e.clientX;
    const y0 = e.clientY;
    let moved = false;
    const move = (ev: PointerEvent) => {
      if (!moved && Math.hypot(ev.clientX - x0, ev.clientY - y0) >= 6) moved = true;
    };
    const up = (ev: PointerEvent) => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      if (!moved) return;
      // the click (if any) is dispatched right after this pointerup; clear the flag after it
      dragged.current = true;
      setTimeout(() => {
        dragged.current = false;
      }, 0);
      void import("../organize/crossWindow").then((m) => {
        if (m.releasedOutside(ev.clientX, ev.clientY)) m.publishPagesDrop(docId, [page], ev.screenX, ev.screenY);
      });
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  }

  return (
    <div
      className="thumb-list"
      ref={listRef}
      onScroll={(e) => setScrollTop(e.currentTarget.scrollTop)}
      role="listbox"
      aria-label={t("sidebar.tab.thumbnails")}
    >
      <div className="thumb-sizer" style={{ height: rows.height }}>
        {visible.map((row) => (
          <button
            key={row.page}
            type="button"
            className="thumb"
            /* the context menu reads the page off the DOM (STAGE1E_NOTES §5.1) */
            data-page={row.page}
            role="option"
            aria-selected={row.page === currentPage}
            aria-label={t("a11y.pageThumbnail", { n: row.page + 1 })}
            data-current={row.page === currentPage || undefined}
            // v0.3 H3: the current page, for screen readers
            aria-current={row.page === currentPage ? "page" : undefined}
            style={{ top: row.y, height: row.h - GAP }}
            onPointerDown={(e) => beginPageDrag(e, info.docId, row.page)}
            onClick={() => {
              if (dragged.current) {
                dragged.current = false;
                return;
              }
              goToPage(row.page);
            }}
          >
            <span className="thumb-page" style={{ width: row.w, height: row.imgH }}>
              <img
                src={thumbUrl({
                  doc: info.docId,
                  gen: info.docGeneration,
                  page: row.page,
                  w: Math.round(row.w * dpr),
                  rot: rotation,
                })}
                alt=""
                draggable={false}
              />
            </span>
            <span className="thumb-num text-xs mono" title={info.pageLabels?.[row.page] ? String(row.page + 1) : undefined}>
              {displayLabel(info.pageLabels, row.page)}
            </span>
          </button>
        ))}
      </div>
    </div>
  );
}

function thumbSize(page: PageGeom, rotation: number, width: number): { w: number; h: number } {
  const swap = rotation === 90 || rotation === 270;
  const wPt = swap ? page.heightPt : page.widthPt;
  const hPt = swap ? page.widthPt : page.heightPt;
  const ratio = wPt > 0 ? hPt / wPt : 1.414;
  // landscape pages keep the row height sane; portrait pages use the full width
  const h = Math.round(width * ratio);
  const maxH = Math.round(width * 2);
  if (h <= maxH) return { w: width, h };
  return { w: Math.round(maxH / ratio), h: maxH };
}
