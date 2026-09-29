/**
 * The image-only document banner (v0.3 O6): "이 문서에는 검색 가능한 텍스트가 없습니다 · OCR 실행…".
 *
 * Mounted once above the viewer's panes. After a document opens it waits for the window to be idle,
 * samples the first pages with `ocr_page_status` (`needsOcr.ts`) and, when none has text, shows a quiet
 * info strip with 텍스트 인식 one click away. 닫기 hides it for this document (by path) for the rest of
 * the session; a text layer added later (the OCR itself) makes it go away on its own.
 */
import { useEffect } from "react";
import { ScanText, X } from "lucide-react";
import { useT } from "../i18n/useT";
import { useDocStore } from "../store/docStore";
import { openOcrDialog } from "./dialogState";
import { dismiss, dismissKey, probeDocument, recheckAfterChange, useImageOnly, useNeedsOcr } from "./needsOcr";
import "./needsOcr.css";

/** Run `fn` when the window has nothing better to do; returns the cancel. */
function whenIdle(fn: () => void): () => void {
  const w = window as Window & {
    requestIdleCallback?: (cb: () => void, o?: { timeout: number }) => number;
    cancelIdleCallback?: (id: number) => void;
  };
  if (typeof w.requestIdleCallback === "function") {
    const id = w.requestIdleCallback(fn, { timeout: 1500 });
    return () => w.cancelIdleCallback?.(id);
  }
  const id = setTimeout(fn, 250);
  return () => clearTimeout(id);
}

export function NeedsOcrBanner() {
  const t = useT();
  const docId = useDocStore((s) => s.info?.docId ?? null);
  const path = useDocStore((s) => s.info?.path ?? null);
  const pageCount = useDocStore((s) => s.info?.pageCount ?? 0);
  const generation = useDocStore((s) => s.info?.docGeneration ?? 0);
  const imageOnly = useImageOnly(docId);
  const key = docId ? dismissKey(docId, path) : "";
  const dismissed = useNeedsOcr((s) => s.dismissed.includes(key));

  // 1. A newly opened document: sample it once the first pages are on screen.
  useEffect(() => {
    if (!docId) return;
    return whenIdle(() => void probeDocument(docId, pageCount));
    // pageCount is read at open; a page edit does not make a scan searchable
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [docId]);

  // 2. Every later edit (OCR above all): look again while the banner still claims "no text".
  useEffect(() => {
    if (docId && generation > 0) recheckAfterChange(docId, pageCount);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [generation]);

  if (!docId || !imageOnly || dismissed) return null;
  return (
    <div className="needs-ocr" role="status" data-testid="needs-ocr-banner">
      <ScanText size={16} strokeWidth={1.75} aria-hidden className="needs-ocr-icon" />
      <p className="needs-ocr-text">
        <span className="needs-ocr-title">{t("ocr.needsOcr.banner")}</span>
        <span className="needs-ocr-hint">{t("ocr.needsOcr.hint")}</span>
      </p>
      <button type="button" className="btn primary needs-ocr-run" onClick={() => openOcrDialog()}>
        {t("ocr.needsOcr.run")}
      </button>
      <button
        type="button"
        className="icon-btn needs-ocr-close"
        aria-label={t("common.close")}
        onClick={() => dismiss(key)}
      >
        <X size={14} strokeWidth={1.75} aria-hidden />
      </button>
    </div>
  );
}

/** The 검색 panel's empty state on an image-only document: the same hint, and the same way out. */
export function NeedsOcrSearchHint({ docId }: { docId: string | null | undefined }) {
  const t = useT();
  const imageOnly = useImageOnly(docId);
  if (!imageOnly) return null;
  return (
    <div className="needs-ocr-search" data-testid="needs-ocr-search">
      <p>{t("ocr.needsOcr.searchHint")}</p>
      <button type="button" className="btn" onClick={() => openOcrDialog()}>{t("ocr.needsOcr.run")}</button>
    </div>
  );
}

export default NeedsOcrBanner;
