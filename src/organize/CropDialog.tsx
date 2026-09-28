/**
 * 자르기 (P2, 페이지 mode): drag a rectangle over the page — handles on the edges and corners, drag
 * inside to move it, drag outside it to draw a new one — or type the four margins; 여백 자동 감지
 * finds the content in a render of the page; 적용 crops the selected pages or every page with the same
 * margins, each relative to its own crop box (`set_page_boxes { crop: { margins } }`, one undo step
 * `undo.pageCrop`); 원래대로 gives the target pages their full media box back (`crop: null`).
 *
 * The preview is the first target page as it is SEEN (its current crop box, `/Rotate` applied), so
 * margins are what the user sees. Lazy-loaded from `DialogHost`.
 */
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { ScanSearch, Undo2 } from "lucide-react";
import * as api from "../ipc/api";
import { pageUrl, scaleKey } from "../ipc/protocol";
import { useT } from "../i18n/useT";
import { useDocStore } from "../store/docStore";
import { usePagesStore } from "../store/pagesStore";
import { toast } from "../app/toastStore";
import { Dialog, Row } from "../dialogs/Dialog";
import { message } from "../dialogs/flows";
import type { Margins, PageIndex } from "../ipc/types";
import { ptToMm, visualSize } from "./cropGeometry";
import {
  HANDLES, boxFromMargins, boxFromPoints, clampBox, detectCropBox, dragBox, fullBox, isFullPage, marginsFromBox,
  type CropBox, type CropHandle,
} from "./crop";
import "./crop.css";

/** The preview fits inside this many CSS px. */
const PREVIEW_W = 380;
const PREVIEW_H = 440;
/** Longest side of the auto-detect render, in device px. */
const DETECT_PX = 900;

type Scope = "targets" | "all";

const undoAction = {
  labelKey: "common.undo",
  onSelect: () => void import("../annot/sync").then((m) => m.undoWithAnnots()),
};

export default function CropDialog({ onClose, pages: given }: { onClose(): void; pages?: PageIndex[] }) {
  const t = useT();
  const info = useDocStore((s) => s.info);
  const selected = usePagesStore((s) => s.selected);
  const focus = usePagesStore((s) => s.focus);
  // pages the user picked (a cell's menu, the organizer selection); else the focused page is only
  // the preview and the default is every page
  const explicit = given?.length ? given : selected.length ? selected : null;
  const targets = useMemo<PageIndex[]>(
    () => [...new Set(explicit ?? [focus ?? 0])].sort((a, b) => a - b),
    [explicit, focus],
  );
  const [scope, setScope] = useState<Scope>(explicit ? "targets" : "all");
  const [busy, setBusy] = useState(false);

  const page = targets[0] ?? 0;
  const geom = info?.pages[page];
  const { w: pageW, h: pageH } = geom ? visualSize(geom) : { w: 612, h: 792 };
  const scale = Math.min(PREVIEW_W / pageW, PREVIEW_H / pageH);
  const [box, setBox] = useState<CropBox>(() => fullBox(pageW, pageH));
  // a new generation (원래대로, undo) may change the page size: start from the whole page again
  const generation = info?.docGeneration;
  useEffect(() => setBox(fullBox(pageW, pageH)), [generation, pageW, pageH]);

  const surface = useRef<HTMLDivElement>(null);
  const margins = marginsFromBox(box, pageW, pageH);

  /** Pointer position on the page in points, origin top-left. */
  const toPage = useCallback(
    (clientX: number, clientY: number) => {
      const rect = surface.current?.getBoundingClientRect();
      return { x: (clientX - (rect?.left ?? 0)) / scale, y: (clientY - (rect?.top ?? 0)) / scale };
    },
    [scale],
  );

  const startDrag = (e: React.PointerEvent, handle: CropHandle | "draw") => {
    if (e.button !== 0) return;
    e.preventDefault();
    e.stopPropagation();
    const origin = toPage(e.clientX, e.clientY);
    const start = box;
    const move = (ev: PointerEvent) => {
      const p = toPage(ev.clientX, ev.clientY);
      if (handle === "draw") {
        const drawn = boxFromPoints(origin.x, origin.y, p.x, p.y, pageW, pageH);
        if (drawn.w >= 2 && drawn.h >= 2) setBox(clampBox(drawn, pageW, pageH));
      } else {
        setBox(dragBox(start, handle, p.x - origin.x, p.y - origin.y, pageW, pageH));
      }
    };
    const up = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  };

  const setMargin = (side: keyof Margins, value: string) => {
    const n = Number(value);
    if (!Number.isFinite(n)) return;
    setBox(boxFromMargins({ ...margins, [side]: Math.max(0, n) }, pageW, pageH));
  };

  const detect = async () => {
    if (!info) return;
    setBusy(true);
    try {
      const renderScale = Math.min(2, DETECT_PX / Math.max(pageW, pageH));
      const raw = await api.renderPageRaw({ docId: info.docId, page, scale: renderScale });
      // the render is a whole number of pixels: measure its real scale
      const found = detectCropBox(raw, raw.width / pageW, pageW, pageH);
      if (found) setBox(found);
      else toast("crop.autoDetect.none", undefined, { tone: "info" });
    } catch (e) {
      toast("crop.failed", undefined, { tone: "danger", detail: message(e) });
    } finally {
      setBusy(false);
    }
  };

  const scopePages = (): PageIndex[] | "all" => (scope === "all" ? "all" : targets);
  const count = scope === "all" ? (info?.pageCount ?? 0) : targets.length;

  const apply = async () => {
    if (!info || isFullPage(margins)) return;
    setBusy(true);
    try {
      const next = await api.setPageBoxes({ docId: info.docId, pages: scopePages(), crop: { margins } });
      useDocStore.getState().adopt(next);
      toast("crop.done", { count }, { tone: "success", actions: [undoAction] });
      onClose();
    } catch (e) {
      toast("crop.failed", undefined, { tone: "danger", detail: message(e) });
      setBusy(false);
    }
  };

  /** 원래대로: the target pages get their whole media box back; the dialog stays open. */
  const reset = async () => {
    if (!info) return;
    setBusy(true);
    try {
      const next = await api.setPageBoxes({ docId: info.docId, pages: scopePages(), crop: null });
      useDocStore.getState().adopt(next);
      toast("crop.resetDone", { count }, { tone: "success", actions: [undoAction] });
    } catch (e) {
      toast("crop.failed", undefined, { tone: "danger", detail: message(e) });
    } finally {
      setBusy(false);
    }
  };

  if (!info || !geom) return null;
  const dpr = typeof window === "undefined" ? 1 : window.devicePixelRatio || 1;
  const css = (v: number) => `${Math.round(v * scale * 100) / 100}px`;

  return (
    <Dialog
      titleKey="crop.title"
      size="xl"
      onClose={onClose}
      primary={{ labelKey: "common.apply", onSelect: () => void apply(), disabled: busy || isFullPage(margins) }}
      footerExtra={
        <button type="button" className="btn quiet" disabled={busy} onClick={() => void reset()}>
          <Undo2 size={16} strokeWidth={1.75} aria-hidden />
          {t("crop.reset")}
        </button>
      }
    >
      <div className="crop-grid">
        <figure className="crop-preview">
          <div
            ref={surface}
            className="crop-surface"
            data-testid="crop-surface"
            style={{ width: css(pageW), height: css(pageH) }}
            onPointerDown={(e) => startDrag(e, "draw")}
          >
            <img
              className="crop-page"
              src={pageUrl({ doc: info.docId, gen: info.docGeneration, page, sk: scaleKey(scale * 100, dpr), rot: 0 })}
              alt=""
              draggable={false}
            />
            <div
              className="crop-box"
              role="group"
              aria-label={t("crop.area")}
              data-testid="crop-box"
              style={{ left: css(box.x), top: css(box.y), width: css(box.w), height: css(box.h) }}
              onPointerDown={(e) => startDrag(e, "move")}
            >
              {HANDLES.map((h) => (
                <span
                  key={h}
                  className="crop-handle"
                  data-handle={h}
                  aria-hidden
                  onPointerDown={(e) => startDrag(e, h)}
                />
              ))}
            </div>
          </div>
          <figcaption className="dlg-hint text-xs">{t("crop.hint")}</figcaption>
        </figure>

        <div className="crop-options">
          <Row labelKey="crop.margins">
            <div className="crop-margins">
              {(["top", "bottom", "left", "right"] as const).map((side) => (
                <label key={side} className="inline-row text-sm">
                  <span className="dim crop-side">{t(`crop.margin.${side}`)}</span>
                  <input
                    className="field num"
                    type="number"
                    min={0}
                    step={1}
                    aria-label={t(`crop.margin.${side}`)}
                    value={margins[side]}
                    onChange={(e) => setMargin(side, e.target.value)}
                  />
                </label>
              ))}
            </div>
            <p className="dlg-hint text-xs mono" data-testid="crop-result">
              {t("crop.result", { w: ptToMm(box.w).toFixed(0), h: ptToMm(box.h).toFixed(0) })}
            </p>
          </Row>
          <Row>
            <div className="inline-row">
              <button type="button" className="btn" disabled={busy} onClick={() => void detect()}>
                <ScanSearch size={16} strokeWidth={1.75} aria-hidden />
                {t("crop.autoDetect")}
              </button>
            </div>
          </Row>
          <Row labelKey="crop.applyTo">
            <div className="dlg-radio-group" role="radiogroup" aria-label={t("crop.applyTo")}>
              <label className="dlg-radio text-base">
                <input type="radio" name="crop-scope" checked={scope === "targets"} onChange={() => setScope("targets")} />
                <span>{t("crop.applyTo.selected", { count: targets.length })}</span>
              </label>
              <label className="dlg-radio text-base">
                <input type="radio" name="crop-scope" checked={scope === "all"} onChange={() => setScope("all")} />
                <span>{t("crop.applyTo.all")}</span>
              </label>
            </div>
          </Row>
        </div>
      </div>
    </Dialog>
  );
}
