/**
 * H9 (v0.3): what a screen reader reads of a page. The page itself is pixels — PDFium's bitmap —
 * so every mounted page carries a visually hidden `role="document"` region labelled "N쪽" with the
 * page's text, one paragraph per line, in reading order (the structure tree's on a tagged PDF, V6).
 * One DOM node per *line*, never per character (ARCHITECTURE §5 keeps selection on typed arrays).
 */
import { memo, useMemo } from "react";
import type { DocGeneration, DocId, PageIndex } from "../../ipc/types";
import { useTextLayer } from "./textLayers";
import { readingLines, useReadingOrder } from "./readingOrder";

export interface PageA11yTextProps {
  docId: DocId;
  docGeneration: DocGeneration;
  page: PageIndex;
  /** `DocInfo.tagged`: ask for the structure tree's order */
  tagged: boolean;
  label: string;
}

export const PageA11yText = memo(function PageA11yText({ docId, docGeneration, page, tagged, label }: PageA11yTextProps) {
  const layer = useTextLayer(docId, docGeneration, page);
  const runs = useReadingOrder(docId, docGeneration, page, tagged, layer?.charCount ?? 0);
  const lines = useMemo(() => (layer ? readingLines(layer, runs) : []), [layer, runs]);
  return (
    <div className="visually-hidden page-a11y" role="document" aria-label={label} data-testid="page-a11y">
      {lines.map((line, i) => (
        <p key={i}>{line}</p>
      ))}
    </div>
  );
});
