/**
 * 문서 비교 view (P1-6): full window, current document (A) on the left, the other file (B) on the
 * right. One scroller; every row holds both pages of one `ComparePage` pair, so the two sides stay
 * aligned by construction. Deleted / replaced words are marked red on A, inserted / replaced words
 * green on B. Pages are paired by text similarity (Stage 8 `alignPages`), so a page only B has is
 * a row labelled 삽입된 페이지 and a page only A has one labelled 삭제된 페이지. The window's
 * document is never touched; 닫기 (or Esc) closes B.
 */
import { useCallback, useEffect, useMemo, useRef, useState, type CSSProperties } from "react";
import { useT } from "../i18n/useT";
import { pageUrl } from "../ipc/protocol";
import { devicePixelRatio } from "../viewer/geometry";
import { useDocStore } from "../store/docStore";
import { useCompareStore, type CompareSession } from "./state";
import { buildRows, fitScale, nextChanged, pageScaleKey, rectToCss, rowAtOffset, visibleRows, type CompareRow } from "./model";
import type { DocInfo, PageIndex, Rect } from "../ipc/types";
import "./compare.css";

/** Column width before the first measurement (and in jsdom, which never lays out). */
const DEFAULT_COLUMN_PX = 420;
const GUTTER_PX = 24;

export default function CompareView() {
  const session = useCompareStore((s) => s.session);
  if (!session) return null;
  return <CompareBody session={session} />;
}

function CompareBody({ session }: { session: CompareSession }) {
  const t = useT();
  const { report, infoA, infoB } = session;
  const [changedOnly, setChangedOnly] = useState(false);
  // v0.3 (X4): the figure-change marks blink so a small change is easy to spot
  const [blink, setBlink] = useState(true);
  const [cursor, setCursor] = useState(-1);
  const [column, setColumn] = useState(DEFAULT_COLUMN_PX);
  const scroller = useRef<HTMLDivElement>(null);
  const programmatic = useRef(false);

  const rows = useMemo(() => buildRows(report), [report]);
  const hasVisual = rows.some((r) => r.visualA.length > 0 || r.visualB.length > 0);
  const shown = visibleRows(rows, changedOnly);
  const prev = nextChanged(shown, cursor, -1);
  const next = nextChanged(shown, cursor, 1);
  const close = useCallback(() => void useCompareStore.getState().exit(), []);
  const docA = useDocStore((s) => s.docId);

  // the window's document went away (an OS open replaced it): the comparison is meaningless now
  useEffect(() => {
    if (docA !== infoA.docId) close();
  }, [docA, infoA.docId, close]);

  // fit-width columns
  useEffect(() => {
    const el = scroller.current;
    if (!el || typeof ResizeObserver === "undefined") return;
    const measure = () => {
      const width = el.clientWidth;
      if (width > 0) setColumn(Math.max(120, Math.floor((width - GUTTER_PX * 3) / 2)));
    };
    measure();
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  // Esc closes compare mode (a dialog on top handles its own Esc first)
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape" || e.defaultPrevented) return;
      if (document.querySelector(".dlg-backdrop")) return;
      e.preventDefault();
      e.stopPropagation();
      close();
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [close]);

  const scrollToRow = (index: number) => {
    const el = scroller.current;
    const row = el?.querySelector<HTMLElement>(`[data-row="${index}"]`);
    if (!el || !row) return;
    programmatic.current = true;
    el.scrollTop = Math.max(0, row.offsetTop - GUTTER_PX / 2);
  };

  const go = (dir: 1 | -1) => {
    const target = nextChanged(shown, cursor, dir);
    if (target === null) return;
    setCursor(target);
    scrollToRow(target);
  };

  const onScroll = () => {
    if (programmatic.current) {
      programmatic.current = false;
      return;
    }
    const el = scroller.current;
    if (!el) return;
    const offsets = [...el.querySelectorAll<HTMLElement>("[data-row]")].map((r) => ({
      index: Number(r.dataset.row),
      top: r.offsetTop - GUTTER_PX / 2,
    }));
    setCursor(rowAtOffset(offsets, el.scrollTop));
  };

  return (
    <div className="cmp-root" role="region" aria-label={t("compare.title")} data-blink={blink || undefined}>
      <header className="cmp-head">
        <h2 className="text-md cmp-title">{t("compare.title")}</h2>
        <p className="text-sm cmp-summary" data-testid="compare-summary">
          <span>{t("compare.summary.pages", { count: report.changedPages })}</span>
          <span className="cmp-ins">{t("compare.summary.inserted", { count: report.inserted })}</span>
          <span className="cmp-del">{t("compare.summary.deleted", { count: report.deleted })}</span>
        </p>
        <span className="cmp-spacer" />
        <button type="button" className="btn" disabled={prev === null} onClick={() => go(-1)}>
          {t("compare.prevChange")}
        </button>
        <button type="button" className="btn" disabled={next === null} onClick={() => go(1)}>
          {t("compare.nextChange")}
        </button>
        <label className="dlg-check text-base cmp-toggle">
          <input
            type="checkbox"
            checked={changedOnly}
            onChange={(e) => {
              setChangedOnly(e.target.checked);
              setCursor(-1);
              if (scroller.current) scroller.current.scrollTop = 0;
            }}
          />
          <span>{t("compare.changedOnly")}</span>
        </label>
        {hasVisual && (
          <label className="dlg-check text-base cmp-toggle">
            <input type="checkbox" checked={blink} onChange={(e) => setBlink(e.target.checked)} />
            <span>{t("compare.blink")}</span>
          </label>
        )}
        <button type="button" className="btn primary" onClick={close}>
          {t("common.close")}
        </button>
      </header>

      <div className="cmp-columns text-sm" aria-hidden="true">
        <span className="cmp-colname" style={{ width: column }} title={infoA.path ?? infoA.name}>
          <b>A</b> {infoA.name}
        </span>
        <span className="cmp-colname" style={{ width: column }} title={infoB.path ?? infoB.name}>
          <b>B</b> {infoB.name}
        </span>
      </div>

      <div
        className="cmp-scroller"
        ref={scroller}
        onScroll={onScroll}
        data-testid="compare-scroller"
        style={{ "--cmp-col": `${column}px` } as CSSProperties}
      >
        {shown.length === 0 && <p className="cmp-empty text-base">{t("compare.noChanges")}</p>}
        {shown.map((row) => (
          <Row key={row.index} row={row} infoA={infoA} infoB={infoB} column={column} current={row.index === cursor} />
        ))}
      </div>
    </div>
  );
}

function pageHeight(info: DocInfo, page: PageIndex | null, column: number): number | null {
  const geom = page === null ? undefined : info.pages[page];
  return geom ? geom.heightPt * fitScale(geom, column) : null;
}

function Row({
  row, infoA, infoB, column, current,
}: { row: CompareRow; infoA: DocInfo; infoB: DocInfo; column: number; current: boolean }) {
  const t = useT();
  const hA = pageHeight(infoA, row.pageA, column);
  const hB = pageHeight(infoB, row.pageB, column);
  const height = Math.max(hA ?? 0, hB ?? 0, 120);
  const label = (p: PageIndex | null) => (p === null ? t("compare.none") : String(p + 1));
  // a null side is a page the alignment found in one document only
  const side = row.pageA === null && row.pageB !== null ? "inserted" : row.pageB === null && row.pageA !== null ? "deleted" : null;
  return (
    <section
      className="cmp-row"
      data-row={row.index}
      data-changed={row.changed || undefined}
      data-current={current || undefined}
      data-side={side ?? undefined}
      aria-label={t("compare.rowLabel", { a: label(row.pageA), b: label(row.pageB) })}
    >
      <div className="cmp-row-head text-xs">
        <span>{t("compare.rowLabel", { a: label(row.pageA), b: label(row.pageB) })}</span>
        {side ? (
          <span className={`cmp-badge ${side === "inserted" ? "cmp-ins" : "cmp-del"}`} data-testid="cmp-side">
            {t(side === "inserted" ? "compare.insertedPage" : "compare.deletedPage")}
          </span>
        ) : row.changed ? (
          <span className="cmp-badge">
            {row.inserted > 0 && <span className="cmp-ins">+{row.inserted}</span>}
            {row.deleted > 0 && <span className="cmp-del">−{row.deleted}</span>}
            {(row.visualA.length > 0 || row.visualB.length > 0) && (
              <span className="cmp-vis">{t("compare.visual")}</span>
            )}
            {row.inserted === 0 && row.deleted === 0 && row.visualA.length === 0 && row.visualB.length === 0 &&
              t("compare.changed")}
          </span>
        ) : (
          <span className="cmp-same">{t("compare.same")}</span>
        )}
      </div>
      <div className="cmp-pair">
        <PageCell
          info={infoA} page={row.pageA} rects={row.rectsA} visual={row.visualA} tone="del" column={column}
          height={height} missingKey={side === "inserted" ? "compare.insertedPage" : undefined}
        />
        <PageCell
          info={infoB} page={row.pageB} rects={row.rectsB} visual={row.visualB} tone="ins" column={column}
          height={height} missingKey={side === "deleted" ? "compare.deletedPage" : undefined}
        />
      </div>
    </section>
  );
}

function PageCell({
  info, page, rects, visual, tone, column, height, missingKey,
}: {
  info: DocInfo; page: PageIndex | null; rects: Rect[]; tone: "ins" | "del"; column: number; height: number;
  /** v0.3 (X4): regions whose pixels changed outside the text */
  visual: Rect[];
  /** what the empty side says: 삽입된 페이지 (only B has it) / 삭제된 페이지 (only A has it) */
  missingKey?: string;
}) {
  const t = useT();
  const geom = page === null ? undefined : info.pages[page];
  if (page === null || !geom) {
    return (
      <div
        className="cmp-page cmp-missing text-sm"
        style={{ width: column, height }}
        data-tone={missingKey ? (missingKey === "compare.insertedPage" ? "ins" : "del") : undefined}
      >
        {t(missingKey ?? "compare.noPage")}
      </div>
    );
  }
  const s = fitScale(geom, column);
  const sk = pageScaleKey(geom, s, devicePixelRatio());
  return (
    <div className="cmp-page" style={{ width: geom.widthPt * s, height: geom.heightPt * s }}>
      <img
        src={pageUrl({ doc: info.docId, gen: info.docGeneration, page, sk, rot: 0 })}
        alt=""
        loading="lazy"
        decoding="async"
        draggable={false}
      />
      {rects.map((r, i) => {
        const box = rectToCss(r, geom, s);
        return (
          <span
            key={i}
            className={`cmp-mark ${tone}`}
            data-testid={`cmp-mark-${tone}`}
            style={{ left: box.x, top: box.y, width: box.w, height: box.h }}
          />
        );
      })}
      {visual.map((r, i) => {
        const box = rectToCss(r, geom, s);
        return (
          <span
            key={`v${i}`}
            className="cmp-mark visual"
            data-testid="cmp-mark-visual"
            style={{ left: box.x, top: box.y, width: box.w, height: box.h }}
          />
        );
      })}
    </div>
  );
}
