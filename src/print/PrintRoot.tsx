/**
 * The print-only DOM (F-26, STAGE1E_NOTES §7.4).
 *
 * `plugin:webview|print` prints **the webview's DOM**, so before this existed ⌘P produced one
 * sheet of the SeePDF toolbar, sidebar and canvas element (`s2-16.png`). The fix is not a
 * different print call but a different document: while a print job is active this component
 * mounts one sheet per requested page, each an `<img>` straight off the `seepdf://page` route at
 * 150 DPI, and `print.css` hides every other element of `<body>` behind `@media print`.
 *
 * `window.print()` is only called once every image has settled — a `<img>` that has not
 * decoded yet prints as a blank box, and WebKit does not wait for us. Each image gets its own
 * `onLoad`/`onError`, and a watchdog fires the print anyway after `SETTLE_TIMEOUT_MS` so a
 * single 410 (a generation that moved under us) cannot hang the job forever.
 *
 * v0.3 (pkg8):
 * * X7 — every image URL carries `print=<annots>`: the engine renders with `FPDF_PRINTING`, so
 *   annotation Print / NoView flags are honoured, and 문서만 / 문서와 도장·서명 filter the markup.
 * * X8 — each sheet is a `100vw × 100vh` box with the page `object-fit: contain`ed in it; a page
 *   whose orientation differs from the sheet's is turned 90° (`data-orient` + the print media
 *   query `orientation`). More than `PRINT_CHUNK` pages are printed as consecutive print jobs of
 *   at most that many images each, with an on-screen progress line, so a 500-page job never
 *   decodes 500 bitmaps at once.
 * * X2 — 실제 크기 (`data-fit="actual"`: physical size, centred) and 흑백 (CSS grayscale); an n-up
 *   job prints a temporary document, closed when the job ends.
 */
import { useEffect, useRef } from "react";
import { pageUrl } from "../ipc/protocol";
import * as api from "../ipc/api";
import { useT } from "../i18n/useT";
import { PRINT_CHUNK, usePrintStore, type PrintJob } from "./printStore";
import "./print.css";

/** Print anyway after this long, even if a page image never answered. */
const SETTLE_TIMEOUT_MS = 20_000;
/** Give up waiting for `afterprint` after this long and unmount the page images regardless. */
const PANEL_TIMEOUT_MS = 180_000;

/** `landscape` when the page is wider than tall (ties are portrait). */
export function orientationOf(size: [number, number] | undefined): "portrait" | "landscape" {
  return size && size[0] > size[1] ? "landscape" : "portrait";
}

/** The slice of `job.pages` chunk `chunk` mounts. */
export function chunkPages(job: PrintJob, chunk: number): { page: number; at: number }[] {
  const start = chunk * PRINT_CHUNK;
  return job.pages.slice(start, start + PRINT_CHUNK).map((page, i) => ({ page, at: start + i }));
}

export function PrintRoot() {
  const t = useT();
  const job = usePrintStore((s) => s.job);
  const chunk = usePrintStore((s) => s.chunk);
  const settled = usePrintStore((s) => s.settled);
  const noteSettled = usePrintStore((s) => s.noteSettled);
  const advance = usePrintStore((s) => s.advance);
  const printed = useRef(false);

  useEffect(() => {
    printed.current = false;
  }, [job, chunk]);

  // An n-up job prints a temporary document: close it when the job is over.
  useEffect(() => {
    const temp = job?.tempDocId;
    if (!temp) return;
    return () => {
      void api.closeDocument({ docId: temp }).catch(() => undefined);
    };
  }, [job]);

  const mounted = job ? chunkPages(job, chunk) : [];

  useEffect(() => {
    if (!job) return;
    /**
     * `window.print()` in WKWebView **returns before the panel has rendered**, so clearing the
     * job in a `finally` unmounted the page images while the sheet was still laying them out —
     * the preview came back as one blank sheet. The chunk is therefore advanced (or the job
     * cleared) on `afterprint` (or a long timeout, for a webview that never fires it), which
     * keeps the images in the DOM for as long as the panel needs them.
     */
    let cleanup = () => undefined as void;
    const fire = () => {
      if (printed.current) return undefined;
      printed.current = true;
      // One frame, so the browser has laid the images out before it snapshots them.
      requestAnimationFrame(() => {
        let timer = 0;
        const done = () => {
          window.removeEventListener("afterprint", done);
          window.clearTimeout(timer);
          advance();
        };
        cleanup = () => window.removeEventListener("afterprint", done);
        window.addEventListener("afterprint", done);
        timer = window.setTimeout(done, PANEL_TIMEOUT_MS);
        window.print();
      });
      return () => cleanup();
    };
    if (settled >= mounted.length) {
      return fire();
    }
    const timer = window.setTimeout(fire, SETTLE_TIMEOUT_MS);
    return () => window.clearTimeout(timer);
  }, [job, chunk, settled, mounted.length, advance]);

  if (!job) return null;
  const chunks = Math.ceil(job.pages.length / PRINT_CHUNK);
  const fit = job.fit ?? "fit";
  return (
    <>
      {chunks > 1 && (
        <div className="print-progress" role="status" aria-live="polite">
          {t("print.progress", {
            done: Math.min(chunk * PRINT_CHUNK + settled, job.pages.length),
            total: job.pages.length,
            part: chunk + 1,
            parts: chunks,
          })}
        </div>
      )}
      <div
        id="seepdf-print-root"
        aria-hidden="true"
        data-gray={job.grayscale ? "1" : undefined}
      >
        {mounted.map(({ page, at }) => {
          const size = job.sizes?.[at];
          return (
            <div
              key={at}
              className="print-sheet"
              data-orient={orientationOf(size)}
              data-fit={fit}
            >
              <img
                className="print-page"
                alt=""
                draggable={false}
                style={fit === "actual" && size ? { width: `${size[0] / 72}in`, height: `${size[1] / 72}in` } : undefined}
                src={pageUrl({
                  doc: job.docId,
                  gen: job.generation,
                  page,
                  sk: job.scaleKey,
                  rot: job.rotation,
                  print: job.annots ?? "all",
                })}
                onLoad={noteSettled}
                onError={noteSettled}
              />
            </div>
          );
        })}
      </div>
    </>
  );
}

export default PrintRoot;
