/**
 * 도장 and 서명 (F-13) — place an image. Arming the tool opens the file picker (UI_SPEC §6: "first
 * use opens 서명 만들기"), the chosen image then follows the cursor as a ghost and a click places
 * it; a drag places it at the dragged rectangle instead.
 *
 * The picked path is module state, not store state: it is per-session, it never renders, and the
 * host feeds it back through `ctx.image` so the state machine stays pure.
 */
import type { AnnotSpec, PageIndex, Point, Rect } from "../ipc/types";
import type { StampImage, ToolModule, ToolResult } from "./ToolController";
import { rectFrom, rectIsEmpty } from "./geometry";

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

export function placementRect(id: "stamp" | "signature", at: Point): Rect {
  const { w, h } = STAMP_SIZE[id];
  return { l: at[0] - w / 2, r: at[0] + w / 2, b: at[1] - h / 2, t: at[1] + h / 2 };
}

/** Module-level, set by `onArm` through the picker the host installs. */
let picked: Partial<Record<"stamp" | "signature", StampImage | null>> = {};
let picker: ((id: "stamp" | "signature") => void) | null = null;

/** The host installs the real picker (it needs `src/ipc/api.ts`'s dialog wrapper). */
export function setStampPicker(fn: ((id: "stamp" | "signature") => void) | null): void {
  picker = fn;
}

export function setStampImage(id: "stamp" | "signature", image: StampImage | null): void {
  picked = { ...picked, [id]: image };
}

export function stampImage(id: "stamp" | "signature"): StampImage | null {
  return picked[id] ?? null;
}

export function resetStampImages(): void {
  picked = {};
}

export function makeStampTool(id: "stamp" | "signature"): ToolModule<StampState> {
  return {
    id,
    cursor: "copy",
    hoverPreview: true,
    onArm() {
      if (!stampImage(id)) picker?.(id);
    },
    init: () => ({ ...EMPTY }),

    onDown(_state, p): ToolResult<StampState> {
      return { state: { page: p.page, from: p.pt, at: p.pt } };
    },

    onMove(state, p): ToolResult<StampState> {
      const rect = state.from ? rectFrom(state.from, p.pt) : placementRect(id, p.pt);
      return {
        state: { ...state, at: p.pt },
        preview: { page: p.page, kind: "stamp", rect: rectIsEmpty(rect, 4) ? placementRect(id, p.pt) : rect },
      };
    },

    onUp(state, p, ctx): ToolResult<StampState> {
      const page = state.page ?? p.page;
      const image = ctx.image ?? stampImage(id);
      if (!image) {
        picker?.(id);
        return { state: { ...EMPTY }, preview: null };
      }
      const dragged = state.from ? rectFrom(state.from, p.pt) : placementRect(id, p.pt);
      const rect = rectIsEmpty(dragged, 4) ? placementRect(id, p.pt) : dragged;
      const spec: AnnotSpec = { kind: "stamp", rect, image };
      return { state: { ...EMPTY }, preview: null, commit: { page, spec }, done: true };
    },

    onKey(state, key): ToolResult<StampState> {
      if (key === "Escape") return { state: { ...EMPTY }, preview: null, done: true };
      return { state };
    },
  };
}
