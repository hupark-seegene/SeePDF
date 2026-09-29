/**
 * Pure logic behind 워터마크 / 머리글·바닥글 (P1-4): per-role defaults, token insertion, the
 * token preview and the `add_stamp` payload. Kept out of the component so it is unit-testable.
 */
import type { PageGeom, PageIndex, Rgb, StampAnchor, StampRole, StampSpec } from "../ipc/types";

export const STAMP_TOKENS = ["page", "total", "date", "filename", "bates"] as const;
export type StampToken = (typeof STAMP_TOKENS)[number];

export const ANCHORS: StampAnchor[] = ["tl", "tc", "tr", "ml", "mc", "mr", "bl", "bc", "br"];

export interface StampForm {
  role: StampRole;
  /** v0.3 T4: `background` = a solid colour filling the page, always behind the content */
  source: "text" | "image" | "background";
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
  /** P2 Bates numbering — used when the text has `{{bates}}` */
  batesStart: number;
  batesDigits: number;
  batesPrefix: string;
  batesSuffix: string;
  /** v0.3 T4 뒤에 배치: under the page content instead of on top */
  behind: boolean;
  /** v0.3 T4: the 배경색 source's colour */
  backgroundColor: Rgb;
}

/** v0.3 T4: the default 배경색 — a pale paper yellow. */
export const DEFAULT_BACKGROUND: Rgb = [255, 248, 220];

/** Bates limits, the engine's (`stamp::BATES_MAX_DIGITS`, `BATES_MAX_AFFIX`). */
export const BATES_MAX_DIGITS = 12;
export const BATES_MAX_AFFIX = 64;

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
    batesStart: 1,
    batesDigits: 6,
    batesPrefix: "",
    batesSuffix: "",
    behind: false,
    backgroundColor: DEFAULT_BACKGROUND,
  };
}

/** The text asks for a Bates number. */
export function usesBates(text: string): boolean {
  return text.includes("{{bates}}");
}

/**
 * Bates 번호 매기기: a footer at the bottom right holding just `{{bates}}` in black 10 pt — what
 * legal document productions look like. A header stays a header (the role is the user's call).
 */
export function batesPreset(form: StampForm): StampForm {
  return {
    ...form,
    role: form.role === "header" ? "header" : "footer",
    source: "text",
    text: "{{bates}}",
    fontSizePt: 10,
    color: [0, 0, 0],
    opacityPct: 100,
    rotateDeg: 0,
    anchor: form.role === "header" ? "tr" : "br",
    marginPt: 24,
  };
}

/** The Bates options as the engine will use them (`stamp::validate` limits). */
export function batesOptions(form: StampForm): { start: number; digits: number; prefix: string; suffix: string } {
  const start = Number.isFinite(form.batesStart) ? Math.max(0, Math.floor(form.batesStart)) : 1;
  const digits = Number.isFinite(form.batesDigits) ? Math.min(BATES_MAX_DIGITS, Math.max(1, Math.round(form.batesDigits))) : 6;
  return {
    start,
    digits,
    prefix: [...form.batesPrefix].slice(0, BATES_MAX_AFFIX).join(""),
    suffix: [...form.batesSuffix].slice(0, BATES_MAX_AFFIX).join(""),
  };
}

/** The `n`-th stamped page's number (0-based), like `stamp::bates_label`: never cut, only padded. */
export function batesLabel(form: StampForm, n: number): string {
  const o = batesOptions(form);
  return `${o.prefix}${String(o.start + n).padStart(o.digits, "0")}${o.suffix}`;
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
  bates?: string;    // this page's Bates number (P2)
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
      case "bates":
        return ctx.bates ?? whole;
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
  const bates =
    form.source === "text" && usesBates(form.text)
      ? (({ start, digits, prefix, suffix }) => ({
          batesStart: start, batesDigits: digits, batesPrefix: prefix, batesSuffix: suffix,
        }))(batesOptions(form))
      : {};
  return {
    ...bates,
    // v0.3 T4: only sent when set (a background is behind by itself)
    ...(form.behind && form.source !== "background" ? { behind: true } : {}),
    role: form.role,
    source:
      form.source === "text"
        ? { kind: "text", text: form.text, fontSizePt: clamp(form.fontSizePt, 4, 400), color: form.color }
        : form.source === "background"
          ? { kind: "background", color: form.backgroundColor }
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
