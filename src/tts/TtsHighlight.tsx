/**
 * 읽어 주기 follows along (V4, v0.3): the sentence the voice is reading is tinted on its page — its
 * text-layer ranges as one rectangle per line, in the page's marks layer (never night-filtered).
 * Rendered by `PageMarks` for every mounted page; draws nothing unless that page's sentence is on.
 */
import { useMemo } from "react";
import type { Rect } from "../ipc/types";
import type { PageLayerContext } from "../viewer/PageShell";
import { useTextLayer } from "../viewer/text/textLayers";
import { useTtsStore } from "./ttsStore";

export function TtsHighlight({ ctx }: { ctx: PageLayerContext }) {
  const sentence = useTtsStore((s) =>
    s.speaking && s.index !== null && s.docId === ctx.docId && s.docGeneration === ctx.docGeneration
      ? s.sentences[s.index]
      : undefined,
  );
  const layer = useTextLayer(ctx.docId, ctx.docGeneration, ctx.index);
  const rects = useMemo<Rect[]>(() => {
    if (!sentence || sentence.page !== ctx.index || !layer) return [];
    return sentence.ranges.flatMap(([a, b]) => layer.rangeRects(a, b));
  }, [sentence, layer, ctx.index]);
  if (rects.length === 0) return null;
  return (
    <>
      {rects.map((rect, i) => {
        const box = ctx.rectToBox(rect);
        return (
          <div
            key={`t${i}`}
            className="mark tts"
            data-testid="tts-highlight"
            style={{ left: box.x, top: box.y, width: Math.max(1, box.w), height: Math.max(1, box.h) }}
          />
        );
      })}
    </>
  );
}
