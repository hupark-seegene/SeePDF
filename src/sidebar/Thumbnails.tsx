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
import { devicePixelRatio } from "../viewer/geometry";
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

  const listRef = useRef<HTMLDivElement>(null);
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
  const dpr = devicePixelRatio();

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
            style={{ top: row.y, height: row.h - GAP }}
            onClick={() => goToPage(row.page)}
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
            <span className="thumb-num text-xs mono">{row.page + 1}</span>
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
