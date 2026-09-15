/**
 * The print-only DOM (F-26, STAGE1E_NOTES §7.4).
 *
 * `plugin:webview|print` prints **the webview's DOM**, so before this existed ⌘P produced one
 * sheet of the SeePDF toolbar, sidebar and canvas element (`s2-16.png`). The fix is not a
 * different print call but a different document: while a print job is active this component
 * mounts one full-width `<img>` per requested page, straight off the `seepdf://page` route at
 * 150 DPI, and `print.css` hides every other element of `<body>` behind `@media print`.
 *
 * `window.print()` is only called once every image has settled — a `<img>` that has not
 * decoded yet prints as a blank box, and WebKit does not wait for us. Each image gets its own
 * `onLoad`/`onError`, and a watchdog fires the print anyway after `SETTLE_TIMEOUT_MS` so a
 * single 410 (a generation that moved under us) cannot hang the job forever.
 */
import { useEffect, useRef } from "react";
import { pageUrl } from "../ipc/protocol";
import { usePrintStore } from "./printStore";
import "./print.css";

/** Print anyway after this long, even if a page image never answered. */
const SETTLE_TIMEOUT_MS = 20_000;
/** Give up waiting for `afterprint` after this long and unmount the page images regardless. */
const PANEL_TIMEOUT_MS = 180_000;

export function PrintRoot() {
  const job = usePrintStore((s) => s.job);
  const settled = usePrintStore((s) => s.settled);
  const noteSettled = usePrintStore((s) => s.noteSettled);
  const clear = usePrintStore((s) => s.clear);
  const printed = useRef(false);

  useEffect(() => {
    printed.current = false;
  }, [job]);

  useEffect(() => {
    if (!job) return;
    /**
     * `window.print()` in WKWebView **returns before the panel has rendered**, so clearing the
     * job in a `finally` unmounted the page images while the sheet was still laying them out —
     * the preview came back as one blank sheet. The job is therefore cleared on `afterprint`
     * (or a long timeout, for a webview that never fires it), which keeps the images in the
     * DOM for as long as the panel needs them.
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
          clear();
        };
        cleanup = () => window.removeEventListener("afterprint", done);
        window.addEventListener("afterprint", done);
        timer = window.setTimeout(done, PANEL_TIMEOUT_MS);
        window.print();
      });
      return () => cleanup();
    };
    if (settled >= job.pages.length) {
      return fire();
    }
    const timer = window.setTimeout(fire, SETTLE_TIMEOUT_MS);
    return () => window.clearTimeout(timer);
  }, [job, settled, clear]);

  if (!job) return null;
  return (
    <div id="seepdf-print-root" aria-hidden="true">
      {job.pages.map((page) => (
        <img
          key={page}
          className="print-page"
          alt=""
          draggable={false}
          src={pageUrl({
            doc: job.docId,
            gen: job.generation,
            page,
            sk: job.scaleKey,
            rot: job.rotation,
          })}
          onLoad={noteSettled}
          onError={noteSettled}
        />
      ))}
    </div>
  );
}

export default PrintRoot;
