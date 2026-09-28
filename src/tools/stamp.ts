/**
 * 도장 and 서명 (F-13) — place an image. Arming the tool opens the file picker (UI_SPEC §6: "first
 * use opens 서명 만들기"), the chosen image then follows the cursor as a ghost and a click places
 * it; a drag places it at the dragged rectangle instead.
 *
 * The picked path is module state, not store state: it is per-session, it never renders, and the
 * host feeds it back through `ctx.image` so the state machine stays pure.
 */
import type { AnnotSpec, PageIndex, Point, Rect, Rgb } from "../ipc/types";
import type { StampImage, ToolModule, ToolPreview, ToolResult } from "./ToolController";
import { rectFrom, rectIsEmpty } from "./geometry";
import { builtinStamp } from "./stampCatalog";

export interface StampState {
  page: PageIndex | null;
  from: Point | null;
  at: Point | null;
}

const EMPTY: StampState = { page: null, from: null, at: null };

/** Default placement size, points. A signature is wider and flatter than a stamp. */
export const STAMP_SIZE: Record<"stamp" | "signature", { w: number; h: number }> = {
  stamp: { w: 96, h: 96 },
  signature: { w: 160, h: 56 },
};

/**
 * The default placement size: a built-in stamp has its own (`stampCatalog.ts`), an image whose
 * shape is known (a typed signature) keeps its aspect at the default width, anything else is
 * [`STAMP_SIZE`].
 */
export function placementSize(id: "stamp" | "signature", image?: StampImage | null): { w: number; h: number } {
  const builtin = image && "builtin" in image ? builtinStamp(image.builtin) : null;
  if (builtin) return builtin.size;
  const aspect = image && "path" in image ? pickedAspect[id] : undefined;
  if (aspect && Number.isFinite(aspect) && aspect > 0) {
    const w = STAMP_SIZE[id].w;
    return { w, h: Math.min(160, Math.max(18, w / aspect)) };
  }
  return STAMP_SIZE[id];
}

export function placementRect(id: "stamp" | "signature", at: Point, image: StampImage | null = stampImage(id)): Rect {
  const { w, h } = placementSize(id, image);
  return { l: at[0] - w / 2, r: at[0] + w / 2, b: at[1] - h / 2, t: at[1] + h / 2 };
}

/** Module-level, set by `onArm` through the picker the host installs. */
let picked: Partial<Record<"stamp" | "signature", StampImage | null>> = {};
/** width ÷ height of the picked image, when the picker knows it (a typed signature does). */
let pickedAspect: Partial<Record<"stamp" | "signature", number>> = {};
let picker: ((id: "stamp" | "signature") => void) | null = null;

/**
 * A signature **drawn** in 서명 만들기 (`dialogs/SignatureDialog.tsx`), in unit space
 * (0…1 of its own bounding box, y-down) plus the aspect it was drawn at. Takes precedence
 * over a picked image for the 서명 tool, so the last thing the user made is what gets placed.
 */
export interface DrawnSignature {
  paths: number[][];
  aspect: number;
}
let drawn: DrawnSignature | null = null;

export function setDrawnSignature(signature: DrawnSignature | null): void {
  drawn = signature;
  // A drawn signature replaces a previously picked image, and vice versa (`setStampImage`).
  if (signature) picked = { ...picked, signature: null };
}

export function drawnSignature(): DrawnSignature | null {
  return drawn;
}

/** Ink colour and stroke width of a placed signature, points. */
export const SIGNATURE_INK: { color: Rgb; width: number } = { color: [17, 19, 24], width: 1.6 };

/** Placement rectangle for a drawn signature: `SIGNATURE_WIDTH_PT` wide, aspect preserved. */
const SIGNATURE_WIDTH_PT = 160;

export function drawnPlacementRect(signature: DrawnSignature, at: Point): Rect {
  const w = SIGNATURE_WIDTH_PT;
  const h = Math.min(160, Math.max(18, w / (signature.aspect || 2.5)));
  return { l: at[0] - w / 2, r: at[0] + w / 2, b: at[1] - h / 2, t: at[1] + h / 2 };
}

/**
 * Unit-space strokes → PDF user space inside `rect`. Unit space is y-**down** (it came from a
 * canvas) and PDF user space is y-up, so `v` is measured from the top edge.
 */
export function signaturePathsIn(signature: DrawnSignature, rect: Rect): number[][] {
  const w = rect.r - rect.l;
  const h = rect.t - rect.b;
  return signature.paths.map((stroke) => {
    const out = new Array<number>(stroke.length);
    for (let i = 0; i < stroke.length; i += 2) {
      out[i] = rect.l + stroke[i] * w;
      out[i + 1] = rect.t - stroke[i + 1] * h;
    }
    return out;
  });
}

/** The host installs the real picker (it needs `src/ipc/api.ts`'s dialog wrapper). */
export function setStampPicker(fn: ((id: "stamp" | "signature") => void) | null): void {
  picker = fn;
}

export function setStampImage(id: "stamp" | "signature", image: StampImage | null, aspect?: number): void {
  picked = { ...picked, [id]: image };
  pickedAspect = { ...pickedAspect, [id]: aspect };
  if (id === "signature" && image) drawn = null;
}

/** Open the picker for `id` again (the properties panel's 도장 변경… / 서명 변경…). */
export function reopenStampPicker(id: "stamp" | "signature"): void {
  picker?.(id);
}

export function stampImage(id: "stamp" | "signature"): StampImage | null {
  return picked[id] ?? null;
}

export function resetStampImages(): void {
  picked = {};
  pickedAspect = {};
  drawn = null;
}

/** The ghost that follows the cursor: a built-in stamp shows its label in its colour. */
function stampPreview(page: PageIndex, rect: Rect, image: StampImage | null): ToolPreview {
  const builtin = image && "builtin" in image ? builtinStamp(image.builtin) : null;
  return builtin
    ? { page, kind: "stamp", rect, label: builtin.label, color: builtin.color }
    : { page, kind: "stamp", rect };
}

export function makeStampTool(id: "stamp" | "signature"): ToolModule<StampState> {
  return {
    id,
    cursor: "copy",
    hoverPreview: true,
    onArm() {
      // 서명 with a drawn signature already in hand needs no sheet; otherwise the host
      // decides (서명 만들기 for a signature, the image picker for a 도장).
      if (!stampImage(id) && !(id === "signature" && drawn)) picker?.(id);
    },
    init: () => ({ ...EMPTY }),

    onDown(_state, p): ToolResult<StampState> {
      return { state: { page: p.page, from: p.pt, at: p.pt } };
    },

    onMove(state, p, ctx): ToolResult<StampState> {
      const image = ctx?.image ?? stampImage(id);
      const base =
        id === "signature" && drawn ? drawnPlacementRect(drawn, p.pt) : placementRect(id, p.pt, image);
      const rect = state.from ? rectFrom(state.from, p.pt) : base;
      return {
        state: { ...state, at: p.pt },
        preview: stampPreview(p.page, rectIsEmpty(rect, 4) ? base : rect, id === "signature" && drawn ? null : image),
      };
    },

    onUp(state, p, ctx): ToolResult<StampState> {
      const page = state.page ?? p.page;
      // A drawn signature wins: it is a real `/Ink` annotation with `/Subj
      // "SeePDF:Signature"`, not an image stamp (STAGE1D §7.5, STAGE1A §5.3).
      if (id === "signature" && drawn) {
        const base = drawnPlacementRect(drawn, p.pt);
        const dragged = state.from ? rectFrom(state.from, p.pt) : base;
        const rect = rectIsEmpty(dragged, 4) ? base : dragged;
        const spec: AnnotSpec = {
          kind: "signature",
          paths: signaturePathsIn(drawn, rect),
          color: SIGNATURE_INK.color,
          width: SIGNATURE_INK.width,
          opacity: 1,
        };
        return { state: { ...EMPTY }, preview: null, commit: { page, spec }, done: true };
      }
      const image = ctx.image ?? stampImage(id);
      if (!image) {
        picker?.(id);
        return { state: { ...EMPTY }, preview: null };
      }
      const dragged = state.from ? rectFrom(state.from, p.pt) : placementRect(id, p.pt, image);
      const rect = rectIsEmpty(dragged, 4) ? placementRect(id, p.pt, image) : dragged;
      const spec: AnnotSpec = { kind: "stamp", rect, image };
      return { state: { ...EMPTY }, preview: null, commit: { page, spec }, done: true };
    },

    onKey(state, key): ToolResult<StampState> {
      if (key === "Escape") return { state: { ...EMPTY }, preview: null, done: true };
      return { state };
    },
  };
}
