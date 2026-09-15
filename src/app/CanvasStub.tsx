import { useEffect, useMemo, useRef, useState } from "react";
import { useT } from "../i18n/useT";
import { useDocStore } from "../store/docStore";
import { useViewStore } from "../store/viewStore";
import { useAppStore } from "../store/appStore";
import { pageUrl, scaleKey } from "../ipc/protocol";
import { toolController } from "../tools/ToolController";
import { displaySize, resolveZoom } from "./canvasLayout";

/**
 * Stage 0 canvas: real pages, real layout maths, one `<img>` per page — enough to see the shell
 * alive and to check the zoom / rotation / layout wiring. Stage 1 (c) replaces it with the
 * virtualised Scroller + PageShell (placeholder → tiles → text rects → SVG overlay → form inputs →
 * pointer surface). The `.page-shell` box below is exactly the outer box (c) must reproduce.
 */
export function CanvasStub() {
  const t = useT();
  const info = useDocStore((s) => s.info);
  const changeNonce = useDocStore((s) => s.changeNonce);
  const zoomPercent = useViewStore((s) => s.zoomPercent);
  const zoomMode = useViewStore((s) => s.zoomMode);
  const rotation = useViewStore((s) => s.rotation);
  const layout = useViewStore((s) => s.layout);
  const currentPage = useViewStore((s) => s.currentPage);
  const scrollRequest = useViewStore((s) => s.scrollRequest);
  const setCurrentPage = useViewStore((s) => s.setCurrentPage);
  const setZoomMode = useViewStore((s) => s.setZoomMode);
  const mode = useAppStore((s) => s.mode);

  const scroller = useRef<HTMLDivElement>(null);
  const pageRefs = useRef<(HTMLDivElement | null)[]>([]);
  const box = useElementSize(scroller);
  const pages = info?.pages ?? [];

  const fitZoom = useMemo(
    () => resolveZoom(pages[currentPage], zoomMode, zoomPercent, rotation, layout, box),
    [pages, currentPage, zoomMode, zoomPercent, rotation, layout, box],
  );

  // keep the status-bar percentage in step with 너비 맞춤 / 페이지 맞춤
  useEffect(() => {
    if ((zoomMode === "fit-width" || zoomMode === "fit-page") && fitZoom !== zoomPercent) {
      setZoomMode(zoomMode, fitZoom);
    }
  }, [fitZoom, zoomMode, zoomPercent, setZoomMode]);

  const visible = useMemo(() => {
    if (layout === "single") return pages.slice(currentPage, currentPage + 1);
    if (layout === "two") {
      const start = currentPage - (currentPage % 2);
      return pages.slice(start, start + 2);
    }
    return pages;
  }, [layout, pages, currentPage]);

  useEffect(() => {
    if (!scrollRequest) return;
    pageRefs.current[scrollRequest.page]?.scrollIntoView({ block: "start", behavior: "auto" });
  }, [scrollRequest]);

  useEffect(() => {
    const el = scroller.current;
    if (!el || layout !== "continuous") return;
    let frame = 0;
    const onScroll = () => {
      if (frame) return;
      frame = requestAnimationFrame(() => {
        frame = 0;
        const mid = el.scrollTop + el.clientHeight * 0.35;
        let best = 0;
        for (let i = 0; i < pageRefs.current.length; i++) {
          const node = pageRefs.current[i];
          if (node && node.offsetTop <= mid) best = i;
        }
        setCurrentPage(best);
      });
    };
    el.addEventListener("scroll", onScroll, { passive: true });
    return () => {
      el.removeEventListener("scroll", onScroll);
      cancelAnimationFrame(frame);
    };
  }, [layout, setCurrentPage, info?.docId]);

  if (!info) return null;

  return (
    <div
      className="canvas"
      ref={scroller}
      role="region"
      aria-label={t("a11y.canvas")}
      tabIndex={0}
      data-mode={mode}
      style={{ cursor: toolController.cursor() }}
    >
      <div className="canvas-pages" data-layout={layout}>
        {visible.map((page) => {
          const size = displaySize(page, fitZoom, rotation);
          return (
            <div
              key={page.index}
              className="page-shell"
              ref={(node) => {
                pageRefs.current[page.index] = node;
              }}
              style={{ width: size.w, height: size.h }}
              data-page={page.index}
              data-current={page.index === currentPage || undefined}
              aria-label={t("a11y.page", { n: page.index + 1 })}
            >
              <img
                className="page-bitmap"
                alt=""
                draggable={false}
                key={`${page.index}:${info.docGeneration}:${changeNonce}`}
                src={pageUrl({
                  doc: info.docId,
                  gen: info.docGeneration,
                  page: page.index,
                  sk: scaleKey(fitZoom),
                  rot: rotation,
                })}
              />
              <span className="page-chip text-xs mono">{page.index + 1}</span>
            </div>
          );
        })}
      </div>
    </div>
  );
}

function useElementSize(ref: React.RefObject<HTMLElement | null>): { w: number; h: number } {
  const [size, setSize] = useState({ w: 0, h: 0 });
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    const update = () => setSize({ w: el.clientWidth, h: el.clientHeight });
    update();
    if (typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver(update);
    ro.observe(el);
    return () => ro.disconnect();
  }, [ref]);
  return size;
}
