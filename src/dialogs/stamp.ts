/**
 * Pure logic behind 워터마크 / 머리글·바닥글 (P1-4): per-role defaults, token insertion, the
 * token preview and the `add_stamp` payload. Kept out of the component so it is unit-testable.
 */
import type { PageGeom, PageIndex, Rgb, StampAnchor, StampRole, StampSpec } from "../ipc/types";

export const STAMP_TOKENS = ["page", "total", "date", "filename"] as const;
export type StampToken = (typeof STAMP_TOKENS)[number];

export const ANCHORS: StampAnchor[] = ["tl", "tc", "tr", "ml", "mc", "mr", "bl", "bc", "br"];

export interface StampForm {
  role: StampRole;
  source: "text" | "image";
  text: string;
  fontSizePt: number;
  color: Rgb;
  imagePath: string | null;
  imageWidthPt: number;
  /** 0..100 in the form; the wire value is 0..1 */
  opacityPct: number;
  rotateDeg: number;
  anchor: StampAnchor;
  marginPt: number;
}

/** What changes with the role. The text is only swapped while the user has not edited it. */
export interface RoleDefaults {
  text: string;
  fontSizePt: number;
  color: Rgb;
  opacityPct: number;
  rotateDeg: number;
  anchor: StampAnchor;
}

/** `watermarkText` is the localised 대외비 / CONFIDENTIAL. */
export function roleDefaults(role: StampRole, watermarkText: string): RoleDefaults {
  switch (role) {
    case "watermark":
      return { text: watermarkText, fontSizePt: 60, color: [200, 30, 30], opacityPct: 25, rotateDeg: 45, anchor: "mc" };
    case "header":
      return { text: "{{filename}}", fontSizePt: 10, color: [80, 80, 80], opacityPct: 100, rotateDeg: 0, anchor: "tc" };
    case "footer":
      return { text: "{{page}} / {{total}}", fontSizePt: 10, color: [80, 80, 80], opacityPct: 100, rotateDeg: 0, anchor: "bc" };
  }
}

export function initialStampForm(role: StampRole, watermarkText: string): StampForm {
  return {
    role,
    source: "text",
    ...roleDefaults(role, watermarkText),
    imagePath: null,
    imageWidthPt: 144,
    marginPt: 36,
  };
}

/**
 * Switch 워터마크 ↔ 머리글 ↔ 바닥글: anchor, rotation, size, colour and opacity follow the role;
 * the text follows too unless the user typed their own.
 */
export function switchRole(form: StampForm, role: StampRole, watermarkText: string): StampForm {
  if (role === form.role) return form;
  const prev = roleDefaults(form.role, watermarkText);
  const next = roleDefaults(role, watermarkText);
  const edited = form.text !== prev.text;
  return { ...form, ...next, role, text: edited ? form.text : next.text };
}

/** Insert `{{token}}` at the caret (replacing a selection); returns the new text and caret. */
export function insertToken(
  text: string,
  token: StampToken,
  selStart: number = text.length,
  selEnd: number = selStart,
): { text: string; caret: number } {
  const start = Math.max(0, Math.min(selStart, text.length));
  const end = Math.max(start, Math.min(selEnd, text.length));
  const tag = `{{${token}}}`;
  return { text: text.slice(0, start) + tag + text.slice(end), caret: start + tag.length };
}

export interface TokenContext {
  page: number;      // 1-based
  total: number;
  date: string;      // YYYY-MM-DD
  filename: string;  // without extension
}

/** Same substitution as the engine: known tokens are replaced, unknown `{{x}}` stays literal. */
export function expandTokens(text: string, ctx: TokenContext): string {
  return text.replace(/\{\{(\w+)\}\}/g, (whole, name: string) => {
    switch (name) {
      case "page":
        return String(ctx.page);
      case "total":
        return String(ctx.total);
      case "date":
        return ctx.date;
      case "filename":
        return ctx.filename;
      default:
        return whole;
    }
  });
}

/** Local `YYYY-MM-DD`, like the engine's `{{date}}`. */
export function isoDate(d: Date = new Date()): string {
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;
}

export function stemOf(name: string): string {
  const dot = name.lastIndexOf(".");
  return dot > 0 ? name.slice(0, dot) : name;
}

export type StampError = "emptyText" | "noImage" | "range";

export function validateStamp(form: StampForm, pages: PageIndex[] | null): StampError | null {
  if (form.source === "text" && !form.text.trim()) return "emptyText";
  if (form.source === "image" && !form.imagePath) return "noImage";
  if (!pages?.length) return "range";
  return null;
}

const clamp = (v: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, Number.isFinite(v) ? v : lo));

/** The `add_stamp` spec. `allPages` sends `'all'` so the engine need not receive a long list. */
export function buildStampSpec(form: StampForm, pages: PageIndex[], allPages: boolean): StampSpec {
  return {
    role: form.role,
    source:
      form.source === "text"
        ? { kind: "text", text: form.text, fontSizePt: clamp(form.fontSizePt, 4, 400), color: form.color }
        : { kind: "image", path: form.imagePath ?? "", widthPt: clamp(form.imageWidthPt, 8, 2000) },
    anchor: form.anchor,
    marginPt: clamp(form.marginPt, 0, 500),
    rotateDeg: form.role === "watermark" ? clamp(form.rotateDeg, -180, 180) : 0,
    opacity: clamp(form.opacityPct, 0, 100) / 100,
    pages: allPages ? "all" : pages,
  };
}

/**
 * The page the engine lays a stamp out on, as the user sees it (`stamp.rs` "visual space"): the
 * crop box, width and height swapped for `/Rotate` 90 / 270. Falls back to the display size when
 * the crop is degenerate (a page the engine has not opened yet reports a provisional one).
 */
export function visualPageSize(geom: PageGeom | undefined): { widthPt: number; heightPt: number } {
  if (!geom) return { widthPt: 612, heightPt: 792 };
  const cw = Math.abs(geom.crop.r - geom.crop.l);
  const ch = Math.abs(geom.crop.t - geom.crop.b);
  if (!(cw > 1 && ch > 1)) return { widthPt: geom.widthPt || 612, heightPt: geom.heightPt || 792 };
  return geom.rotation % 180 === 90 ? { widthPt: ch, heightPt: cw } : { widthPt: cw, heightPt: ch };
}

/** Height ÷ width of an image stamp's box: the image's own aspect once known, else a placeholder. */
export const PLACEHOLDER_IMAGE_ASPECT = 0.6;

export function imageStampHeight(widthPt: number, naturalW?: number, naturalH?: number): number {
  const aspect = naturalW && naturalH && naturalW > 0 && naturalH > 0 ? naturalH / naturalW : PLACEHOLDER_IMAGE_ASPECT;
  return widthPt * aspect;
}

/** The `remove_stamps` payload: `role` omitted = every role, including stamps from before Stage 8. */
export function removeStampsArgs(
  scope: "role" | "all",
  role: StampRole,
  pages: PageIndex[],
  allPages: boolean,
): { pages: PageIndex[] | "all"; role?: StampRole } {
  return scope === "all" ? { pages: allPages ? "all" : pages } : { pages: allPages ? "all" : pages, role };
}

/**
 * Where the preview puts the stamp on the page-shaped box: CSS percentages + a translate. The
 * margin is converted from points with the page width so it scales with the box.
 */
export function anchorStyle(anchor: StampAnchor, marginPt: number, pageWidthPt: number, pageHeightPt: number) {
  const mx = `${((marginPt / pageWidthPt) * 100).toFixed(2)}%`;
  const my = `${((marginPt / pageHeightPt) * 100).toFixed(2)}%`;
  const v = anchor[0] as "t" | "m" | "b";
  const h = anchor[1] as "l" | "c" | "r";
  const style: Record<string, string> = {};
  let tx = "0";
  let ty = "0";
  if (h === "l") style.left = mx;
  else if (h === "r") style.right = mx;
  else {
    style.left = "50%";
    tx = "-50%";
  }
  if (v === "t") style.top = my;
  else if (v === "b") style.bottom = my;
  else {
    style.top = "50%";
    ty = "-50%";
  }
  return { style, translate: `translate(${tx}, ${ty})` };
}
