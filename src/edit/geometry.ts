/**
 * 편집 mode arithmetic (Stage 7). Pure — every function here is unit-tested in `geometry.test.ts`.
 *
 * Everything is PDF user space (points, y-up) except where a name says CSS. The page ↔ CSS affine
 * is the viewer's frozen `PageLayerContext` (`viewer/geometry.ts` `pageToDevice`); nothing here
 * re-derives it — a CSS *delta* goes through the linear part of `ctx.inverse`, which is what keeps
 * a drag correct under /Rotate and the view rotation.
 */
import type { Mat6, ObjectId, PageGeom, PageObject, Point, Rect } from "../ipc/types";

/**
 * The angle (clockwise on screen) the 문단 편집 frame turns by: the page's /Rotate plus the view
 * rotation — the same sum `viewer/geometry.ts` `pageToDevice` uses, so text along the page's +x
 * reads on screen the way the engine will write it.
 */
export function editorRotation(pageRotation: number, viewRotation: number): 0 | 90 | 180 | 270 {
  return ((((pageRotation + viewRotation) % 360) + 360) % 360) as 0 | 90 | 180 | 270;
}

/** A CSS-px drag delta → the same delta in page points (translation dropped). */
export function deltaToPage(inverse: Mat6, dx: number, dy: number): Point {
  return [inverse[0] * dx + inverse[2] * dy, inverse[1] * dx + inverse[3] * dy];
}

export function rectArea(r: Rect): number {
  return Math.max(0, r.r - r.l) * Math.max(0, r.t - r.b);
}

export function translateRect(r: Rect, dx: number, dy: number): Rect {
  return { l: r.l + dx, b: r.b + dy, r: r.r + dx, t: r.t + dy };
}

/**
 * The object under a point: the **smallest** one whose box (grown by `tolPt`) contains it, so a
 * full-page background rectangle never hides the text on top of it.
 */
export function hitObject(objects: PageObject[], x: number, y: number, tolPt = 2): PageObject | null {
  let best: PageObject | null = null;
  let bestArea = Infinity;
  for (const o of objects) {
    const r = o.rect;
    if (x < r.l - tolPt || x > r.r + tolPt || y < r.b - tolPt || y > r.t + tolPt) continue;
    const area = rectArea(r);
    // later objects paint on top: on a tie the later one wins
    if (area <= bestArea) {
      best = o;
      bestArea = area;
    }
  }
  return best;
}

/** A corner of a page rect, named by the page-space edges it sits on (rotation-independent). */
export type Corner = "lb" | "lt" | "rb" | "rt";
export const CORNERS: Corner[] = ["lt", "rt", "lb", "rb"];

export function cornerPoint(r: Rect, c: Corner): Point {
  return [c[0] === "l" ? r.l : r.r, c[1] === "b" ? r.b : r.t];
}

/**
 * Move one corner of `r` by `(dx, dy)` page points with the opposite corner fixed. With
 * `keepAspect` the larger relative change wins. Never collapses below `minPt`.
 */
export function resizeRect(r: Rect, corner: Corner, dx: number, dy: number, keepAspect = false, minPt = 4): Rect {
  const w0 = r.r - r.l;
  const h0 = r.t - r.b;
  let w = Math.max(minPt, w0 + (corner[0] === "r" ? dx : -dx));
  let h = Math.max(minPt, h0 + (corner[1] === "t" ? dy : -dy));
  if (keepAspect && w0 > 0 && h0 > 0) {
    const s = Math.max(w / w0, h / h0);
    w = Math.max(minPt, w0 * s);
    h = Math.max(minPt, h0 * s);
  }
  const l = corner[0] === "r" ? r.l : r.r - w;
  const b = corner[1] === "t" ? r.b : r.t - h;
  return { l, b, r: l + w, t: b + h };
}

/**
 * `transform_object` arguments that turn `from` into `to`: the engine scales about the object's own
 * bottom-left (`engine/objects/mod.rs` `transform`), then translates.
 */
export function scaleArgs(from: Rect, to: Rect): { scale: [number, number]; translate: [number, number] } {
  const sx = (to.r - to.l) / Math.max(1e-6, from.r - from.l);
  const sy = (to.t - to.b) / Math.max(1e-6, from.t - from.b);
  return { scale: [sx, sy], translate: [to.l - from.l, to.b - from.b] };
}

/** Rough advance of one character in em — the textarea is the real measurement, this sizes a box. */
function advanceEm(ch: string): number {
  const c = ch.codePointAt(0) ?? 0;
  if (ch === " ") return 0.3;
  if (c >= 0x1100 && c <= 0xffe6) return 1; // Hangul / CJK
  return 0.55;
}

export function measureText(text: string, fontSizePt: number): number {
  let w = 0;
  for (const ch of text) w += advanceEm(ch) * fontSizePt;
  return w;
}

/**
 * The rect `add_text_object` gets for typed text: top-left at `at`, as wide as the longest line
 * (or `widthPt` when the user set one), one 1.2 × size line per `\n`.
 */
export function textRectFor(at: Point, text: string, fontSizePt: number, widthPt?: number): Rect {
  const lines = text.split("\n");
  const natural = Math.max(fontSizePt, ...lines.map((l) => measureText(l, fontSizePt))) + 2;
  const w = widthPt && widthPt > 0 ? Math.max(widthPt, fontSizePt) : natural;
  const h = Math.max(1, lines.length) * fontSizePt * 1.2;
  return { l: at[0], t: at[1], r: at[0] + w, b: at[1] - h };
}

/** Default box for a click with 이미지 추가: 240 pt square, top-left at the click, kept on the page. */
export const IMAGE_CLICK_PT = 240;

export function imageRectAt(at: Point, page: PageGeom, sizePt = IMAGE_CLICK_PT): Rect {
  const c = page.crop;
  const w = Math.min(sizePt, c.r - c.l);
  const h = Math.min(sizePt, c.t - c.b);
  const l = Math.min(Math.max(at[0], c.l), c.r - w);
  const t = Math.max(Math.min(at[1], c.t), c.b + h);
  return { l, b: t - h, r: l + w, t };
}

/** Normalised rect between two page points. */
export function rectFromPoints(a: Point, b: Point): Rect {
  return { l: Math.min(a[0], b[0]), r: Math.max(a[0], b[0]), b: Math.min(a[1], b[1]), t: Math.max(a[1], b[1]) };
}

/**
 * A CSS `font-family` approximating a PDF font name (subset prefix `ABCDEF+` dropped): Hangul,
 * monospace, serif or sans. The engine does the real layout; this only has to look close while
 * typing.
 */
export function fontStack(fontName: string): string {
  const name = fontName.replace(/^[A-Z]{6}\+/, "");
  if (/courier|mono|consol|menlo|code/i.test(name)) return 'ui-monospace, "SF Mono", Menlo, Consolas, monospace';
  if (/myeong|myungjo|batang|gungsuh|song|ming/i.test(name)) return '"AppleMyungjo", Batang, "Noto Serif KR", serif';
  if (/hangul|korean|kr\b|kr-|gothic|gulim|dotum|malgun|nanum|pretendard|apple ?sd|spoqa|cjk|seepdf/i.test(name)) {
    return '"Apple SD Gothic Neo", "Malgun Gothic", "Noto Sans KR", Pretendard, sans-serif';
  }
  if (/times|serif|georgia|garamond|minion|cambria|palatino|roman|book|caslon|baskerville/i.test(name) && !/sans/i.test(name)) {
    return '"Times New Roman", Times, Georgia, serif';
  }
  return 'Helvetica, Arial, "Apple SD Gothic Neo", "Malgun Gothic", sans-serif';
}

/** `inner` lies inside `outer` (grown by `tolPt`). */
export function rectInside(inner: Rect, outer: Rect, tolPt = 0.5): boolean {
  return inner.l >= outer.l - tolPt && inner.r <= outer.r + tolPt && inner.b >= outer.b - tolPt && inner.t <= outer.t + tolPt;
}

/** The bounding box of several rects (`null` for none). */
export function unionRects(rects: Rect[]): Rect | null {
  if (rects.length === 0) return null;
  return rects.reduce((u, r) => ({ l: Math.min(u.l, r.l), b: Math.min(u.b, r.b), r: Math.max(u.r, r.r), t: Math.max(u.t, r.t) }));
}

/** `r` clipped to `box`; `null` when nothing is left. */
export function clipRect(r: Rect, box: Rect): Rect | null {
  const out = { l: Math.max(r.l, box.l), b: Math.max(r.b, box.b), r: Math.min(r.r, box.r), t: Math.min(r.t, box.t) };
  return out.r > out.l && out.t > out.b ? out : null;
}

/** Objects wholly inside a 선택 marquee. */
export function objectsInside(objects: PageObject[], marquee: Rect): PageObject[] {
  return objects.filter((o) => rectInside(o.rect, marquee));
}

/**
 * Does a 영역 표시 drag *cross* this text run? More than a graze: the overlap must reach a third of
 * the run's height — or half the drag's, for a drag thinner than a line — so the lines above and
 * below a dragged line, whose boxes touch it, stay out; and more than a sliver of its width.
 */
export function crossesRun(drag: Rect, run: Rect): boolean {
  const w = Math.min(drag.r, run.r) - Math.max(drag.l, run.l);
  const h = Math.min(drag.t, run.t) - Math.max(drag.b, run.b);
  const need = Math.min((run.t - run.b) / 3, (drag.t - drag.b) / 2);
  return w > Math.min(1, (run.r - run.l) / 2) && h > 0 && h >= need;
}

/**
 * 영역 표시 snap (Stage 8): a dragged rect grows to the whole of every text run it crosses — PDFium
 * removes a text object entirely, so the black box must cover all of it — and keeps its own extent
 * for whatever non-text content it covers. Clipped to the page box; `null` when nothing is left.
 */
export function snapMark(drag: Rect, objects: PageObject[], pageBox: Rect): Rect | null {
  const runs = objects.filter((o) => o.type === "text" && crossesRun(drag, o.rect)).map((o) => o.rect);
  const snapped = unionRects([drag, ...runs]) ?? drag;
  return clipRect(snapped, pageBox);
}

/** `true` when the object with this id is on the list. */
export function hasObject(objects: PageObject[], id: ObjectId): boolean {
  return objects.some((o) => o.objectId === id);
}
