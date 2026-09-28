/**
 * The virtualised continuous scroller (F-02, F-03, F-06).
 *
 * Everything that happens per frame — scroll offset, in-gesture zoom, tile inventory, pointer
 * state — lives in refs, not in a store (ARCHITECTURE §10). React re-renders only when the set of
 * *mounted* things changes: a new row enters the ±1.5/2.5-screen window, a tile is admitted, the
 * zoom settles. A fling over 500 pages is therefore zero React work per frame.
 *
 * Zoom is instant and sharp-later: the page boxes, the text layer and every overlay are laid out
 * at the live zoom immediately, while the bitmap layer keeps the tiles of the previous scale key
 * and is CSS-scaled (≤ 1 ms) until the gesture settles 120 ms later and the sharp tiles land.
 */
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, useSyncExternalStore } from "react";
import * as api from "../ipc/api";
import { pageUrl, tileUrl } from "../ipc/protocol";
import { onEnginePressure } from "../ipc/events";
import type { DocInfo, PageIndex } from "../ipc/types";
import { useT } from "../i18n/useT";
import { useAppStore } from "../store/appStore";
import { useDocStore } from "../store/docStore";
import { useViewStore, type ViewState } from "../store/viewStore";
import { useAnnotStore } from "../store/annotStore";
import {
  pageBoxCss,
  pagePixels,
  placeholderScaleKey,
  scaleFromKey,
  scaleKeyFor,
  shouldTile,
  tileGridFor,
  tilePriority,
  tileRect,
} from "./geometry";
import {
  computeLayout,
  currentPageAt,
  fitZoomForView,
  onScreenRange,
  scrollTopForPage,
  visibleRange,
  type DocLayout,
} from "./layout";
import { anchorAt, clampZoom, scrollForAnchor, zoomForWheel } from "./zoom";
import { TileManager, type TileRequest } from "./TileManager";
import { makePageLayerContext, PageShell, type PageLayerRenderer, type PageShellProps } from "./PageShell";
import { PageMarks } from "./text/PageMarks";
import { ensureTextLayers, getTextLayer, invalidateTextLayers } from "./text/textLayers";
import { useSelectionStore, type TextSelection } from "./text/selection";
import { useSearchStore } from "./search/SearchController";
import { useDevicePixelRatio } from "./useDevicePixelRatio";

/** How far the scroll offset may drift before the mounted set is recomputed. */
const SCROLL_COMMIT_PX = 96;
/** Scroll/zoom settle, ARCHITECTURE §10. */
const SETTLE_MS = 120;
/** > 1.5 px/ms is a fling: the in-flight budget drops to 8 (ARCHITECTURE §3.3). */
const FLING_PX_PER_MS = 1.5;
/** One tile ring beyond the viewport, so a slow scroll never shows a gap. */
const TILE_MARGIN = 1;

interface Vec2 {
  x: number;
  y: number;
}

export interface ScrollerProps {
  info: DocInfo;
  /** (d)/(e) plug their overlays in here; called once per mounted page. */
  layers?: PageLayerRenderer;
  /** 필드 강조 표시 — adds `hl=1` to every page/tile URL so pdfium tints the widgets. */
  fieldHighlight?: boolean;
  /**
   * `false` adds `forms=0` to every page/tile URL, so PDFium skips `FPDF_FFLDraw` and the
   * AcroForm widgets are not in the bitmap. (d) sets it while the 양식 HTML overlay is
   * mounted; otherwise the value would be drawn twice, a pixel or two apart (F-20).
   */
  renderFormWidgets?: boolean;
  /** forwarded to every `PageShell`: the engine has painted this page at this generation. */
  onPageRendered?: PageShellProps["onPageRendered"];
}

export function Scroller({
  info,
  layers,
  fieldHighlight = false,
  renderFormWidgets = true,
  onPageRendered,
}: ScrollerProps) {
  const t = useT();
  const zoomPercent = useViewStore((s) => s.zoomPercent);
  const zoomMode = useViewStore((s) => s.zoomMode);
  const rotation = useViewStore((s) => s.rotation);
  const mode = useViewStore((s) => s.layout);
  const night = useViewStore((s) => s.night);
  // P1-12: a page whose annotation is hidden mid-drag carries its view nonce in its URLs.
  const viewNonce = useAnnotStore((s) => s.viewNonce);
  const currentPage = useViewStore((s) => s.currentPage);
  const scrollRequest = useViewStore((s) => s.scrollRequest);
  const setCurrentPage = useViewStore((s) => s.setCurrentPage);
  const setZoomMode = useViewStore((s) => s.setZoomMode);
  const setZoom = useViewStore((s) => s.setZoom);
  const tool = useAppStore((s) => s.tool);
  const changedPages = useDocStore((s) => s.changedPages);
  const changeNonce = useDocStore((s) => s.changeNonce);

  const elRef = useRef<HTMLDivElement>(null);
  const viewport = useElementSize(elRef);
  const dpr = useDevicePixelRatio();
  // The navigation effects below read the document through this ref: a new `info` arrives with
  // every mutation, and must not replay the last 이동 or search jump.
  const infoRef = useRef(info);
  infoRef.current = info;

  const [scroll, setScroll] = useState<Vec2>({ x: 0, y: 0 });
  const scrollRef = useRef(scroll);
  const committedRef = useRef(scroll);
  const viewportRef = useRef(viewport);
  viewportRef.current = viewport;

  // ---------------------------------------------------------------- layout
  const liveScaleKey = scaleKeyFor(zoomPercent, dpr);
  const [renderScaleKey, setRenderScaleKey] = useState(liveScaleKey);

  // Only 단일 mode's layout depends on the current page; letting the other two depend on it would
  // rebuild 500 page boxes on every scroll frame that crosses a page boundary.
  const layoutPage = mode === "single" ? currentPage : 0;
  const layout = useMemo(
    () =>
      computeLayout({
        pages: info.pages,
        zoomPercent,
        rotation,
        mode,
        viewport,
        currentPage: layoutPage,
        dpr,
      }),
    [info.pages, zoomPercent, rotation, mode, viewport, layoutPage, dpr],
  );
  const layoutRef = useRef(layout);
  layoutRef.current = layout;

  // 너비 맞춤 / 페이지 맞춤 resolve to a percentage as soon as the scroller has a size. Outside 단일
  // the fit must not depend on the current page: a refit re-anchors the scroll, the scroll moves
  // the current page onto a page of the other orientation, and the zoom would flip forever.
  useEffect(() => {
    if (zoomMode !== "fit-width" && zoomMode !== "fit-page") return;
    const fit = fitZoomForView(info.pages, zoomMode, rotation, mode, viewport, layoutPage);
    if (fit !== null && fit !== zoomPercent) setZoomMode(zoomMode, fit);
  }, [zoomMode, zoomPercent, rotation, mode, viewport, layoutPage, info.pages, setZoomMode]);

  // The bitmap layer lags the live zoom by one settle, so a gesture never storms the engine.
  useEffect(() => {
    if (renderScaleKey === liveScaleKey) return;
    const timer = window.setTimeout(() => setRenderScaleKey(liveScaleKey), SETTLE_MS);
    return () => window.clearTimeout(timer);
  }, [liveScaleKey, renderScaleKey]);

  // ---------------------------------------------------------------- tiles
  const tilesRef = useRef<TileManager | null>(null);
  if (!tilesRef.current) tilesRef.current = new TileManager();
  const tiles = tilesRef.current;

  useEffect(() => onEnginePressure((e) => tiles.setPressure(e.level)), [tiles]);

  const mountedTiles = useSyncExternalStore(
    useCallback((cb: () => void) => tiles.subscribe(cb), [tiles]),
    useCallback(() => tiles.mounted(), [tiles]),
    useCallback(() => tiles.mounted(), [tiles]),
  );

  const renderKey = `${info.docId}:${info.docGeneration}:${renderScaleKey}:${rotation}:${night}`;
  const renderGenRef = useRef({ key: "", gen: 0 });
  if (renderGenRef.current.key !== renderKey) {
    renderGenRef.current = { key: renderKey, gen: renderGenRef.current.gen + 1 };
  }
  const renderGen = renderGenRef.current.gen;

  const range = useMemo(() => visibleRange(layout, scroll.y, viewport.h), [layout, scroll.y, viewport.h]);
  const mountedItems = useMemo(
    () => (range.first < 0 ? [] : layout.items.filter((item) => item.row >= range.first && item.row <= range.last)),
    [layout, range.first, range.last],
  );

  const flingRef = useRef(false);
  useEffect(() => {
    const desired = collectTiles({
      info,
      items: mountedItems,
      scroll,
      viewport,
      renderScaleKey,
      rotation,
      night: night !== "off",
      hl: fieldHighlight,
      forms: renderFormWidgets,
      dpr,
      currentPage,
      viewNonce,
    });
    tiles.setDesired(renderGen, desired, { fling: flingRef.current });
  }, [
    info, mountedItems, scroll, viewport, renderScaleKey, rotation, night, fieldHighlight, dpr,
    currentPage, renderGen, tiles, viewNonce,
  ]);

  // A mutation invalidates the pages it touched: the URLs carry the new generation, so the old
  // `<img>`s must go away in the same commit the new ones arrive (no stale pixels, no flash).
  useEffect(() => {
    if (changedPages === "all") tiles.reset();
    invalidateTextLayers(info.docId, changedPages);
  }, [changeNonce, changedPages, tiles, info.docId]);

  // text layers for everything mounted (selection, ⌘A, search highlight)
  useEffect(() => {
    ensureTextLayers(info.docId, info.docGeneration, [...new Set(mountedItems.map((i) => i.page))]);
  }, [info.docId, info.docGeneration, mountedItems]);

  // ---------------------------------------------------------------- scrolling
  const settleRef = useRef(0);
  const velocityRef = useRef(0);
  const lastSampleRef = useRef({ y: 0, at: 0 });

  const commitScroll = useCallback((next: Vec2) => {
    committedRef.current = next;
    setScroll(next);
  }, []);

  const settle = useCallback(() => {
    window.clearTimeout(settleRef.current);
    settleRef.current = window.setTimeout(() => {
      flingRef.current = false;
      velocityRef.current = 0;
      commitScroll({ ...scrollRef.current });
      const l = layoutRef.current;
      const vp = viewportRef.current;
      const onScreen = onScreenRange(l, scrollRef.current.y, vp.h);
      const pages = onScreen.pages.length ? onScreen.pages : [0];
      void api
        .setViewport({
          docId: info.docId,
          scaleKey: scaleKeyFor(useViewStore.getState().zoomPercent, dpr),
          rotation: useViewStore.getState().rotation,
          centrePage: currentPageAt(l, scrollRef.current.y, vp.h),
          firstPage: pages[0],
          lastPage: pages[pages.length - 1],
          velocityPxPerMs: 0,
        })
        .catch(() => undefined);
    }, SETTLE_MS);
  }, [commitScroll, dpr, info.docId]);

  useEffect(() => {
    const el = elRef.current;
    if (!el) return;
    let frame = 0;
    const tick = () => {
      frame = 0;
      const next = { x: el.scrollLeft, y: el.scrollTop };
      scrollRef.current = next;

      const now = performance.now();
      const sample = lastSampleRef.current;
      if (sample.at) {
        const dt = now - sample.at;
        if (dt > 0) velocityRef.current = Math.abs(next.y - sample.y) / dt;
      }
      lastSampleRef.current = { y: next.y, at: now };
      flingRef.current = velocityRef.current > FLING_PX_PER_MS;

      setCurrentPage(currentPageAt(layoutRef.current, next.y, viewportRef.current.h));

      const committed = committedRef.current;
      if (Math.abs(next.y - committed.y) >= SCROLL_COMMIT_PX || Math.abs(next.x - committed.x) >= SCROLL_COMMIT_PX) {
        commitScroll(next);
      }
      settle();
    };
    const onScroll = () => {
      if (!frame) frame = requestAnimationFrame(tick);
    };
    el.addEventListener("scroll", onScroll, { passive: true });
    return () => {
      el.removeEventListener("scroll", onScroll);
      if (frame) cancelAnimationFrame(frame);
      window.clearTimeout(settleRef.current);
    };
  }, [commitScroll, setCurrentPage, settle]);

  /**
   * Re-anchoring after a zoom. A cursor zoom (ctrl+wheel, pinch) computes its target before the
   * store changes and parks it here; a zoom with no cursor (⌘+/⌘−, the slider, 너비 맞춤) anchors
   * on the middle of the viewport, comparing the layout that just rendered with the previous one.
   * Both must land before the browser paints, so this is a layout effect.
   */
  const pendingScrollRef = useRef<Vec2 | null>(null);
  const previousLayoutRef = useRef<DocLayout | null>(null);
  useLayoutEffect(() => {
    const el = elRef.current;
    const previous = previousLayoutRef.current;
    previousLayoutRef.current = layout;
    if (!el) return;
    const pending = pendingScrollRef.current;
    if (pending) {
      pendingScrollRef.current = null;
      el.scrollLeft = pending.x;
      el.scrollTop = pending.y;
    } else if (
      previous &&
      (previous.zoomPercent !== layout.zoomPercent ||
        previous.rotation !== layout.rotation ||
        previous.mode !== layout.mode)
    ) {
      // At the very top there is nothing to preserve — staying there is what 너비 맞춤 resolving
      // on mount (100 % → the fit percentage) and a zoom step at page 1 both want.
      if (el.scrollTop === 0 && el.scrollLeft === 0) return;
      const vp = viewportRef.current;
      const cursor = { x: vp.w / 2, y: vp.h / 2 };
      const anchor = anchorAt(previous, { x: el.scrollLeft, y: el.scrollTop }, cursor);
      if (!anchor) return;
      const target = scrollForAnchor(layout, anchor, cursor, vp);
      el.scrollLeft = target.x;
      el.scrollTop = target.y;
    } else {
      return;
    }
    scrollRef.current = { x: el.scrollLeft, y: el.scrollTop };
    commitScroll(scrollRef.current);
  }, [layout, commitScroll]);

  const applyZoom = useCallback(
    (next: number, cursor: Vec2) => {
      const el = elRef.current;
      if (!el) return;
      // A pinch at the 25 %/400 % limit, or a delta too small to reach the next integer: nothing
      // will relayout, so nothing may be parked for the layout effect to apply later.
      if (clampZoom(next) === useViewStore.getState().zoomPercent) return;
      const before = layoutRef.current;
      const anchor = anchorAt(before, { x: el.scrollLeft, y: el.scrollTop }, cursor);
      const after = computeLayout({
        pages: info.pages,
        zoomPercent: next,
        rotation: useViewStore.getState().rotation,
        mode: useViewStore.getState().layout,
        viewport: viewportRef.current,
        currentPage: useViewStore.getState().currentPage,
        dpr,
      });
      pendingScrollRef.current = anchor
        ? scrollForAnchor(after, anchor, cursor, viewportRef.current)
        : { x: el.scrollLeft, y: el.scrollTop };
      setZoom(next);
      settle();
    },
    [dpr, info.pages, setZoom, settle],
  );

  // ctrl/⌘ + wheel and trackpad pinch (WKWebView and WebView2 both send wheel + ctrlKey).
  useEffect(() => {
    const el = elRef.current;
    if (!el) return;
    const onWheel = (e: WheelEvent) => {
      if (!e.ctrlKey && !e.metaKey) return;
      e.preventDefault();
      const rect = el.getBoundingClientRect();
      applyZoom(zoomForWheel(useViewStore.getState().zoomPercent, e.deltaY, e.deltaMode), {
        x: e.clientX - rect.left,
        y: e.clientY - rect.top,
      });
    };
    const blockGesture = (e: Event) => e.preventDefault();
    el.addEventListener("wheel", onWheel, { passive: false });
    el.addEventListener("gesturestart", blockGesture);
    el.addEventListener("gesturechange", blockGesture);
    return () => {
      el.removeEventListener("wheel", onWheel);
      el.removeEventListener("gesturestart", blockGesture);
      el.removeEventListener("gesturechange", blockGesture);
    };
  }, [applyZoom]);

  // 이동: the status bar, the thumbnail rail, the outline and ⌘↑/⌘↓ all go through `scrollRequest`.
  // A request is consumed once: the document changing afterwards (an annotation, a form value,
  // undo, save) must not scroll back to it.
  const handledRequestRef = useRef<ViewScrollRequest | null>(null);
  useEffect(() => {
    if (!scrollRequest || scrollRequest === handledRequestRef.current) return;
    const el = elRef.current;
    if (!el) return;
    handledRequestRef.current = scrollRequest;
    const info = infoRef.current;
    let top = scrollTopForPage(layoutRef.current, scrollRequest.page);
    // An outline destination carries a y in PDF user space: land on the heading, a little below
    // the top edge, rather than on the top of the page (STAGE1C_NOTES §7.1).
    const item = layoutRef.current.byPage.get(scrollRequest.page);
    const geom = info.pages[scrollRequest.page];
    if (scrollRequest.yPt !== undefined && item && geom) {
      const ctx = makePageLayerContext({
        docId: info.docId,
        docGeneration: info.docGeneration,
        page: geom,
        rotation: useViewStore.getState().rotation,
        zoomPercent: useViewStore.getState().zoomPercent,
        width: item.w,
        height: item.h,
      });
      const [, y] = ctx.toDevice(geom.crop.l, scrollRequest.yPt);
      top = Math.max(0, item.y + y - 12);
    }
    el.scrollTop = top;
    scrollRef.current = { x: el.scrollLeft, y: top };
    commitScroll(scrollRef.current);
    settle();
  }, [scrollRequest, commitScroll, settle]);

  // A search hit is scrolled into view by its rectangle, not by its page — once per ⌘G / click.
  const navNonce = useSearchStore((s) => s.navNonce);
  const handledNavRef = useRef(0);
  useEffect(() => {
    if (!navNonce || navNonce === handledNavRef.current) return;
    const { hits, current } = useSearchStore.getState();
    const hit = hits[current];
    const el = elRef.current;
    if (!hit || !el) return;
    handledNavRef.current = navNonce;
    const info = infoRef.current;
    const item = layoutRef.current.byPage.get(hit.page);
    if (!item) {
      useViewStore.getState().goToPage(hit.page);
      return;
    }
    const geom = info.pages[hit.page];
    const rect = hit.rects[0];
    if (!geom || !rect) return;
    const ctx = makePageLayerContext({
      docId: info.docId,
      docGeneration: info.docGeneration,
      page: geom,
      rotation: useViewStore.getState().rotation,
      zoomPercent: useViewStore.getState().zoomPercent,
      width: item.w,
      height: item.h,
    });
    const box = ctx.rectToBox(rect);
    const vp = viewportRef.current;
    const top = item.y + box.y;
    if (top < el.scrollTop + 48 || top > el.scrollTop + vp.h - 48) {
      el.scrollTop = Math.max(0, top - vp.h * 0.35);
    }
    const left = item.x + box.x;
    if (left < el.scrollLeft || left > el.scrollLeft + vp.w - 48) {
      el.scrollLeft = Math.max(0, left - vp.w * 0.4);
    }
    scrollRef.current = { x: el.scrollLeft, y: el.scrollTop };
    commitScroll(scrollRef.current);
    settle();
  }, [navNonce, commitScroll, settle]);

  // ---------------------------------------------------------------- pointer
  const selecting = useRef<{ page: PageIndex; offset: number; mode: "char" | "word" | "line" } | null>(null);
  const panning = useRef<{ x: number; y: number } | null>(null);
  const setSelection = useSelectionStore((s) => s.setSelection);

  const pointToPage = useCallback(
    (clientX: number, clientY: number) => {
      const el = elRef.current;
      if (!el) return null;
      const rect = el.getBoundingClientRect();
      const cx = clientX - rect.left + el.scrollLeft;
      const cy = clientY - rect.top + el.scrollTop;
      const l = layoutRef.current;
      let best = null as (typeof l.items)[number] | null;
      let bestDistance = Infinity;
      for (const item of l.items) {
        const dx = cx < item.x ? item.x - cx : cx > item.x + item.w ? cx - (item.x + item.w) : 0;
        const dy = cy < item.y ? item.y - cy : cy > item.y + item.h ? cy - (item.y + item.h) : 0;
        const distance = dx * dx + dy * dy;
        if (distance < bestDistance) {
          bestDistance = distance;
          best = item;
        }
      }
      if (!best) return null;
      const geom = info.pages[best.page];
      if (!geom) return null;
      const ctx = makePageLayerContext({
        docId: info.docId,
        docGeneration: info.docGeneration,
        page: geom,
        rotation: useViewStore.getState().rotation,
        zoomPercent: useViewStore.getState().zoomPercent,
        width: best.w,
        height: best.h,
      });
      const [px, py] = ctx.toPage(cx - best.x, cy - best.y);
      return { page: best.page, x: px, y: py };
    },
    [info],
  );

  const selectsText = tool === "select" || tool === "highlight" || tool === "underline" || tool === "strikeout" || tool === "squiggly";
  const pans = tool === "hand";

  const onPointerDown = useCallback(
    (e: React.PointerEvent<HTMLDivElement>) => {
      const el = elRef.current;
      if (!el) return;
      // `preventDefault` below stops the browser's own focus handling, and the canvas must have
      // focus for ⌘A / ⌘C / the tool keys (UI_SPEC §13 `canvas` context).
      el.focus({ preventScroll: true });
      if (e.button === 1 || pans) {
        panning.current = { x: e.clientX, y: e.clientY };
        el.setPointerCapture(e.pointerId);
        el.dataset.panning = "1";
        e.preventDefault();
        return;
      }
      if (e.button !== 0 || !selectsText) return;
      const at = pointToPage(e.clientX, e.clientY);
      if (!at) return;
      const layer = getTextLayer(info.docId, info.docGeneration, at.page);
      if (!layer) {
        setSelection(null);
        return;
      }
      const hit = layer.hitTest(at.x, at.y);
      if (!hit) {
        setSelection(null);
        return;
      }
      const clicks = e.detail;
      if (clicks >= 3) {
        const [from, to] = layer.lineRange(hit.char);
        selecting.current = { page: at.page, offset: from, mode: "line" };
        setSelection({ docId: info.docId, anchor: { page: at.page, offset: from }, focus: { page: at.page, offset: to } });
      } else if (clicks === 2) {
        const [from, to] = layer.wordRange(hit.char);
        selecting.current = { page: at.page, offset: from, mode: "word" };
        setSelection({ docId: info.docId, anchor: { page: at.page, offset: from }, focus: { page: at.page, offset: to } });
      } else if (e.shiftKey) {
        const existing = useSelectionStore.getState().selection;
        const anchor = existing?.anchor ?? { page: at.page, offset: hit.offset };
        selecting.current = { page: anchor.page, offset: anchor.offset, mode: "char" };
        setSelection({ docId: info.docId, anchor, focus: { page: at.page, offset: hit.offset } });
      } else {
        selecting.current = { page: at.page, offset: hit.offset, mode: "char" };
        setSelection({
          docId: info.docId,
          anchor: { page: at.page, offset: hit.offset },
          focus: { page: at.page, offset: hit.offset },
        });
      }
      el.setPointerCapture(e.pointerId);
      e.preventDefault();
    },
    [info, pans, pointToPage, selectsText, setSelection],
  );

  const onPointerMove = useCallback(
    (e: React.PointerEvent<HTMLDivElement>) => {
      const el = elRef.current;
      if (!el) return;
      if (panning.current) {
        el.scrollLeft -= e.clientX - panning.current.x;
        el.scrollTop -= e.clientY - panning.current.y;
        panning.current = { x: e.clientX, y: e.clientY };
        return;
      }
      const drag = selecting.current;
      if (!drag) return;
      const at = pointToPage(e.clientX, e.clientY);
      if (!at) return;
      const layer = getTextLayer(info.docId, info.docGeneration, at.page);
      if (!layer) return;
      const hit = layer.hitTest(at.x, at.y);
      if (!hit) return;
      let focusOffset = hit.offset;
      if (drag.mode === "word") {
        const [from, to] = layer.wordRange(hit.char);
        focusOffset = at.page > drag.page || (at.page === drag.page && to > drag.offset) ? to : from;
      } else if (drag.mode === "line") {
        const [from, to] = layer.lineRange(hit.char);
        focusOffset = at.page > drag.page || (at.page === drag.page && to > drag.offset) ? to : from;
      }
      const next: TextSelection = {
        docId: info.docId,
        anchor: { page: drag.page, offset: drag.offset },
        focus: { page: at.page, offset: focusOffset },
      };
      const previous = useSelectionStore.getState().selection;
      if (
        previous &&
        previous.focus.page === next.focus.page &&
        previous.focus.offset === next.focus.offset &&
        previous.anchor.page === next.anchor.page &&
        previous.anchor.offset === next.anchor.offset
      ) {
        return;
      }
      setSelection(next);
    },
    [info, pointToPage, setSelection],
  );

  const endPointer = useCallback((e: React.PointerEvent<HTMLDivElement>) => {
    const el = elRef.current;
    panning.current = null;
    selecting.current = null;
    if (el) {
      delete el.dataset.panning;
      if (el.hasPointerCapture(e.pointerId)) el.releasePointerCapture(e.pointerId);
    }
  }, []);

  // ---------------------------------------------------------------- render
  const placeholderKeys = useMemo(() => {
    const map = new Map<PageIndex, number>();
    for (const item of mountedItems) {
      const geom = info.pages[item.page];
      if (geom) map.set(item.page, placeholderScaleKey(geom, rotation));
    }
    return map;
  }, [mountedItems, info.pages, rotation]);

  const tilesByPage = useMemo(() => {
    const map = new Map<PageIndex, TileRequest[]>();
    for (const tile of mountedTiles) {
      const list = map.get(tile.page);
      if (list) list.push(tile);
      else map.set(tile.page, [tile]);
    }
    return map;
  }, [mountedTiles]);

  const renderScale = scaleFromKey(renderScaleKey);
  const nightOn = night !== "off";

  return (
    <div
      className="canvas viewer"
      ref={elRef}
      role="region"
      aria-label={t("a11y.canvas")}
      tabIndex={0}
      data-tool={tool}
      data-night={nightOn ? night : undefined}
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={endPointer}
      onPointerCancel={endPointer}
    >
      <div className="viewer-content" style={{ width: layout.width, height: layout.height }}>
        {mountedItems.map((item) => {
          const geom = info.pages[item.page];
          if (!geom) return null;
          const ctx = makePageLayerContext({
            docId: info.docId,
            docGeneration: info.docGeneration,
            page: geom,
            rotation,
            zoomPercent,
            width: item.w,
            height: item.h,
          });
          const renderBox = pageBoxCss(geom, rotation, renderScaleKey, dpr);
          const px = pagePixels(geom, rotation, renderScale);
          const tiled = shouldTile(renderScale, px.w, px.h);
          return (
            <PageShell
              key={item.page}
              ctx={ctx}
              left={item.x}
              top={item.y}
              bitmapScale={renderBox.w > 0 ? item.w / renderBox.w : 1}
              bitmapWidth={renderBox.w}
              bitmapHeight={renderBox.h}
              placeholderUrl={pageUrl({
                doc: info.docId,
                gen: info.docGeneration,
                page: item.page,
                sk: placeholderKeys.get(item.page) ?? 100,
                rot: rotation,
                night: nightOn,
                hl: fieldHighlight,
                forms: renderFormWidgets,
                vn: viewNonce[item.page],
              })}
              bitmapUrl={
                tiled
                  ? null
                  : pageUrl({
                      doc: info.docId,
                      gen: info.docGeneration,
                      page: item.page,
                      sk: renderScaleKey,
                      rot: rotation,
                      night: nightOn,
                      hl: fieldHighlight,
                      forms: renderFormWidgets,
                      vn: viewNonce[item.page],
                    })
              }
              tiles={tilesByPage.get(item.page) ?? EMPTY_TILES}
              night={night}
              current={item.page === currentPage}
              label={t("a11y.page", { n: item.page + 1 })}
              marks={<PageMarks ctx={ctx} />}
              layers={layers?.(ctx)}
              onTileLoad={(key) => tiles.notifyLoaded(key)}
              onTileError={(key) => tiles.notifyError(key)}
              onPageRendered={onPageRendered}
            />
          );
        })}
      </div>
    </div>
  );
}

const EMPTY_TILES: TileRequest[] = [];

type ViewScrollRequest = NonNullable<ViewState["scrollRequest"]>;

// ---------------------------------------------------------------------------

/**
 * Every tile the viewport wants, centre-out. Tiles live on the *render* scale grid (the engine's
 * 512 device px), while the viewport is measured at the live zoom — the ratio between the two is
 * the CSS scale the bitmap layer is wearing right now.
 */
function collectTiles(a: {
  info: DocInfo;
  items: { page: PageIndex; x: number; y: number; w: number; h: number }[];
  scroll: Vec2;
  viewport: { w: number; h: number };
  renderScaleKey: number;
  rotation: DocInfo["pages"][number]["rotation"];
  night: boolean;
  hl: boolean;
  forms: boolean;
  dpr: number;
  currentPage: PageIndex;
  /** per-page view nonce (`set_annotations_hidden`), part of the key and the URL */
  viewNonce: Record<PageIndex, number>;
}): TileRequest[] {
  const s = scaleFromKey(a.renderScaleKey);
  const out: TileRequest[] = [];
  for (const item of a.items) {
    const geom = a.info.pages[item.page];
    if (!geom) continue;
    const px = pagePixels(geom, a.rotation, s);
    if (!shouldTile(s, px.w, px.h)) continue;

    const renderW = px.w / a.dpr;
    const k = renderW > 0 ? item.w / renderW : 1;
    // viewport rectangle in the page's own render-space device pixels
    const toDevice = (v: number) => (v / k) * a.dpr;
    const vx0 = toDevice(a.scroll.x - item.x);
    const vy0 = toDevice(a.scroll.y - item.y);
    const vx1 = toDevice(a.scroll.x + a.viewport.w - item.x);
    const vy1 = toDevice(a.scroll.y + a.viewport.h - item.y);

    const grid = tileGridFor(px.w, px.h);
    const tx0 = clampInt(Math.floor(vx0 / 512) - TILE_MARGIN, 0, grid.cols - 1);
    const tx1 = clampInt(Math.ceil(vx1 / 512) + TILE_MARGIN, 0, grid.cols - 1);
    const ty0 = clampInt(Math.floor(vy0 / 512) - TILE_MARGIN, 0, grid.rows - 1);
    const ty1 = clampInt(Math.ceil(vy1 / 512) + TILE_MARGIN, 0, grid.rows - 1);
    const centreTx = clampInt(Math.floor(((vx0 + vx1) / 2) / 512), 0, grid.cols - 1);
    const centreTy = clampInt(Math.floor(((vy0 + vy1) / 2) / 512), 0, grid.rows - 1);
    const pageCost = Math.abs(item.page - a.currentPage) * 64;

    for (let ty = ty0; ty <= ty1; ty++) {
      for (let tx = tx0; tx <= tx1; tx++) {
        const rect = tileRect(px.w, px.h, tx, ty);
        if (!rect) continue;
        out.push({
          key: `${a.info.docId}:${a.info.docGeneration}:${item.page}:${a.renderScaleKey}:${a.rotation}:${tx}:${ty}:${a.night ? 1 : 0}:${a.hl ? 1 : 0}:${a.forms ? 1 : 0}:${a.viewNonce[item.page] ?? 0}`,
          page: item.page,
          tx,
          ty,
          priority: tilePriority(tx, ty, centreTx, centreTy) + pageCost,
          url: tileUrl({
            doc: a.info.docId,
            gen: a.info.docGeneration,
            page: item.page,
            sk: a.renderScaleKey,
            rot: a.rotation,
            tx,
            ty,
            night: a.night,
            hl: a.hl,
            forms: a.forms,
            vn: a.viewNonce[item.page],
          }),
          box: { x: rect.x / a.dpr, y: rect.y / a.dpr, w: rect.w / a.dpr, h: rect.h / a.dpr },
        });
      }
    }
  }
  return out;
}

function clampInt(v: number, lo: number, hi: number): number {
  return Math.max(lo, Math.min(hi, v));
}

/** jsdom and a hidden panel both report 0; a sane fallback keeps the virtualiser honest. */
function useElementSize(ref: React.RefObject<HTMLElement | null>): { w: number; h: number } {
  const [size, setSize] = useState({ w: 0, h: 0 });
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    const update = () => {
      const w = el.clientWidth || 900;
      const h = el.clientHeight || 800;
      setSize((prev) => (prev.w === w && prev.h === h ? prev : { w, h }));
    };
    update();
    if (typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(update);
    observer.observe(el);
    return () => observer.disconnect();
  }, [ref]);
  return size;
}
