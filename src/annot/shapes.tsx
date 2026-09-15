/**
 * Drawing an annotation in the SVG overlay.
 *
 * The overlay lives inside `<svg viewBox="0 0 w h"><g transform="matrix(…)">` (PageShell), so
 * every coordinate below is **PDF user space** and nothing recomputes on zoom — the `<g>` matrix
 * is the only thing that changes. Because that matrix flips y, an SVG `<rect y={b} height={t-b}>`
 * already covers `[b, t]`; text has to counter-flip with `scale(1 -1)`.
 *
 * What is painted and what is not matters for correctness, not only for looks:
 *
 * * an annotation **the engine has already rendered into the page bitmap** is painted as a
 *   transparent hit shape plus, when hovered or selected, an outline — painting it again would
 *   double its opacity;
 * * a **ghost** (optimistic, not yet in any bitmap) and a **tool preview** are painted in full.
 */
import type { Annot, Rect, Rgb } from "../ipc/types";
import type { ToolPreview } from "../tools/ToolController";

export function rgb(c: Rgb): string {
  return `rgb(${c[0]} ${c[1]} ${c[2]})`;
}

function box(r: Rect) {
  return { x: Math.min(r.l, r.r), y: Math.min(r.b, r.t), width: Math.abs(r.r - r.l), height: Math.abs(r.t - r.b) };
}

/** The squiggle of a 물결선, as a path along the bottom of a line run. */
function squigglePath(r: Rect, amplitude = 1.1): string {
  const step = amplitude * 2;
  const y = r.b + amplitude;
  let d = `M ${r.l} ${y}`;
  let up = true;
  for (let x = r.l; x < r.r; x += step) {
    d += ` Q ${Math.min(x + step / 2, r.r)} ${up ? y + amplitude : y - amplitude} ${Math.min(x + step, r.r)} ${y}`;
    up = !up;
  }
  return d;
}

function inkPath(points: number[]): string {
  if (points.length < 4) return "";
  let d = `M ${points[0]} ${points[1]}`;
  for (let i = 2; i + 1 < points.length; i += 2) d += ` L ${points[i]} ${points[i + 1]}`;
  return d;
}

/** The two short strokes of an arrow head at (x, y), pointing away from (fx, fy). */
function arrowHead(x: number, y: number, fx: number, fy: number, size: number): string {
  const angle = Math.atan2(y - fy, x - fx);
  const spread = Math.PI / 7;
  const a = [x - size * Math.cos(angle - spread), y - size * Math.sin(angle - spread)];
  const b = [x - size * Math.cos(angle + spread), y - size * Math.sin(angle + spread)];
  return `M ${a[0]} ${a[1]} L ${x} ${y} L ${b[0]} ${b[1]}`;
}

/** One annotation, fully painted. Used for ghosts and for the 텍스트 상자 draft. */
export function AnnotShape({ annot: a }: { annot: Annot }) {
  const stroke = rgb(a.color);
  const fill = a.fillColor ? rgb(a.fillColor) : "none";
  switch (a.kind) {
    case "highlight":
      return (
        <g style={{ mixBlendMode: "multiply" }}>
          {(a.quads ?? [a.rect]).map((q, i) => (
            <rect key={i} {...box(q)} fill={stroke} opacity={a.opacity} />
          ))}
        </g>
      );
    case "underline":
      return (
        <g>
          {(a.quads ?? [a.rect]).map((q, i) => (
            <rect key={i} x={q.l} y={q.b} width={q.r - q.l} height={1} fill={stroke} opacity={a.opacity} />
          ))}
        </g>
      );
    case "strikeout":
      return (
        <g>
          {(a.quads ?? [a.rect]).map((q, i) => (
            <rect key={i} x={q.l} y={(q.b + q.t) / 2 - 0.5} width={q.r - q.l} height={1} fill={stroke} opacity={a.opacity} />
          ))}
        </g>
      );
    case "squiggly":
      return (
        <g>
          {(a.quads ?? [a.rect]).map((q, i) => (
            <path key={i} d={squigglePath(q)} stroke={stroke} strokeWidth={0.8} fill="none" opacity={a.opacity} />
          ))}
        </g>
      );
    case "square":
      return <rect {...box(a.rect)} fill={fill} stroke={stroke} strokeWidth={a.borderWidth} opacity={a.opacity} />;
    case "circle": {
      const b = box(a.rect);
      return (
        <ellipse
          cx={b.x + b.width / 2}
          cy={b.y + b.height / 2}
          rx={Math.max(0, b.width / 2 - a.borderWidth / 2)}
          ry={Math.max(0, b.height / 2 - a.borderWidth / 2)}
          fill={fill}
          stroke={stroke}
          strokeWidth={a.borderWidth}
          opacity={a.opacity}
        />
      );
    }
    case "line":
    case "arrow": {
      const p = a.linePoints ?? [a.rect.l, a.rect.b, a.rect.r, a.rect.t];
      const size = Math.max(4, a.borderWidth * 3);
      return (
        <g stroke={stroke} strokeWidth={a.borderWidth} fill="none" opacity={a.opacity} strokeLinecap="round">
          <line x1={p[0]} y1={p[1]} x2={p[2]} y2={p[3]} />
          {a.kind === "arrow" && <path d={arrowHead(p[2], p[3], p[0], p[1], size)} />}
        </g>
      );
    }
    case "ink":
    case "signature":
      return (
        <g stroke={stroke} strokeWidth={a.borderWidth} fill="none" opacity={a.opacity} strokeLinecap="round" strokeLinejoin="round">
          {(a.inkPaths ?? []).map((path, i) => (
            <path key={i} d={inkPath(path)} />
          ))}
        </g>
      );
    case "note": {
      const b = box(a.rect);
      return (
        <g opacity={a.opacity}>
          <rect {...b} rx={Math.min(3, b.width / 4)} fill={stroke} />
          <path
            d={`M ${b.x + b.width * 0.22} ${b.y + b.height * 0.42} H ${b.x + b.width * 0.78}
                M ${b.x + b.width * 0.22} ${b.y + b.height * 0.62} H ${b.x + b.width * 0.62}`}
            stroke="#fff"
            strokeWidth={Math.max(0.8, b.height * 0.07)}
            fill="none"
          />
        </g>
      );
    }
    case "textbox": {
      const b = box(a.rect);
      const size = a.fontSize ?? 12;
      return (
        <g>
          {a.fillColor && <rect {...b} fill={fill} opacity={a.opacity} />}
          <rect {...b} fill="none" stroke={stroke} strokeWidth={0.75} opacity={a.opacity} />
          <g transform={`translate(${b.x + 2} ${b.y + b.height - 2}) scale(1 -1)`}>
            <text x={0} y={0} fontSize={size} fill={stroke} dominantBaseline="hanging" style={{ whiteSpace: "pre" }}>
              {(a.text ?? a.contents ?? "").split("\n").map((line, i) => (
                <tspan key={i} x={0} dy={i === 0 ? 0 : size * 1.25}>
                  {line}
                </tspan>
              ))}
            </text>
          </g>
        </g>
      );
    }
    case "stamp":
      return (
        <g opacity={a.opacity}>
          <rect {...box(a.rect)} fill="none" stroke={stroke} strokeWidth={1} strokeDasharray="4 3" />
        </g>
      );
    default:
      return <rect {...box(a.rect)} fill="none" stroke={stroke} strokeWidth={1} opacity={a.opacity} />;
  }
}

/** What a gesture is drawing right now. Same coordinates, dashed where nothing exists yet. */
export function PreviewShape({ preview, scale }: { preview: ToolPreview; scale: number }) {
  const stroke = preview.color ? rgb(preview.color) : "var(--accent)";
  const fill = preview.fillColor ? rgb(preview.fillColor) : "none";
  const width = preview.width ?? 1;
  const opacity = preview.opacity ?? 1;
  switch (preview.kind) {
    case "quads":
      return (
        <g style={{ mixBlendMode: "multiply" }}>
          {(preview.quads ?? []).map((q, i) => (
            <rect key={i} {...box(q)} fill={stroke} opacity={opacity} />
          ))}
        </g>
      );
    case "rect":
      return preview.rect ? <rect {...box(preview.rect)} fill={fill} stroke={stroke} strokeWidth={width} opacity={opacity} /> : null;
    case "ellipse": {
      if (!preview.rect) return null;
      const b = box(preview.rect);
      return (
        <ellipse cx={b.x + b.width / 2} cy={b.y + b.height / 2} rx={b.width / 2} ry={b.height / 2} fill={fill} stroke={stroke} strokeWidth={width} opacity={opacity} />
      );
    }
    case "ink":
      return <path d={inkPath(preview.points ?? [])} stroke={stroke} strokeWidth={width} fill="none" opacity={opacity} strokeLinecap="round" strokeLinejoin="round" />;
    case "line": {
      const p = preview.points ?? [0, 0, 0, 0];
      const size = Math.max(4, width * 3);
      return (
        <g stroke={stroke} strokeWidth={width} fill="none" opacity={opacity} strokeLinecap="round">
          <line x1={p[0]} y1={p[1]} x2={p[2]} y2={p[3]} />
          {preview.heads?.[1] && <path d={arrowHead(p[2], p[3], p[0], p[1], size)} />}
          {preview.heads?.[0] && <path d={arrowHead(p[0], p[1], p[2], p[3], size)} />}
        </g>
      );
    }
    case "marquee":
      return preview.rect ? (
        <rect {...box(preview.rect)} className="annot-marquee" vectorEffect="non-scaling-stroke" strokeWidth={1} />
      ) : null;
    case "eraser": {
      if (!preview.rect) return null;
      const b = box(preview.rect);
      return (
        <circle
          cx={b.x + b.width / 2}
          cy={b.y + b.height / 2}
          r={b.width / 2}
          className="annot-eraser"
          vectorEffect="non-scaling-stroke"
        />
      );
    }
    case "stamp":
      return preview.rect ? (
        <rect {...box(preview.rect)} className="annot-placing" strokeDasharray={`${6 / scale} ${4 / scale}`} vectorEffect="non-scaling-stroke" />
      ) : null;
    default:
      return null;
  }
}
