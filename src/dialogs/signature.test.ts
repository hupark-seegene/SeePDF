/**
 * 서명 만들기 (STAGE1D §7.5) — the drawn signature becomes a real `/Ink` annotation.
 *
 * The sheet hands `stamp.ts` unit-space strokes; the 서명 tool maps them into the placement
 * rectangle in PDF **user space** (y-up) and commits `AnnotSpec { kind: "signature" }`, which
 * the engine writes with `/Subj "SeePDF:Signature"`. These tests pin the coordinate flip and
 * the "drawn beats picked" rule, which are the two things a coordinate bug hides in.
 */
import { beforeEach, describe, expect, it } from "vitest";
import { normaliseStrokes } from "./SignatureDialog";
import {
  drawnPlacementRect,
  drawnSignature,
  makeStampTool,
  resetStampImages,
  setDrawnSignature,
  setStampImage,
  signaturePathsIn,
} from "../tools/stamp";
import type { ToolContext } from "../tools/ToolController";
import type { AnnotSpec } from "../ipc/types";

beforeEach(() => resetStampImages());

describe("normaliseStrokes", () => {
  it("maps the drawn bounding box onto 0…1 and reports its aspect", () => {
    // A 200 × 100 box drawn at an offset: the offset must not survive normalisation.
    const { paths, aspect } = normaliseStrokes([[50, 20, 250, 120]])!;
    expect(paths).toEqual([[0, 0, 1, 1]]);
    expect(aspect).toBe(2);
  });

  it("survives a degenerate box (a dot, a perfectly horizontal line)", () => {
    expect(normaliseStrokes([[10, 10, 10, 10]])!.paths).toEqual([[0, 0, 0, 0]]);
    const flat = normaliseStrokes([[0, 5, 100, 5]])!;
    expect(flat.paths).toEqual([[0, 0, 1, 0]]);
    expect(Number.isFinite(flat.aspect)).toBe(true);
  });

  it("returns null for nothing drawn", () => {
    expect(normaliseStrokes([])).toBeNull();
  });
});

describe("placing a drawn signature", () => {
  it("flips unit space (y-down) into PDF user space (y-up)", () => {
    const rect = { l: 100, b: 200, r: 300, t: 260 };
    const paths = signaturePathsIn({ paths: [[0, 0, 1, 1]], aspect: 2 }, rect);
    // (0,0) is the **top** left of the drawing → the top edge of the rect.
    expect(paths).toEqual([[100, 260, 300, 200]]);
  });

  it("keeps the aspect the signature was drawn at", () => {
    const wide = drawnPlacementRect({ paths: [], aspect: 4 }, [300, 400]);
    expect(wide.r - wide.l).toBe(160);
    expect(wide.t - wide.b).toBe(40);
    const tall = drawnPlacementRect({ paths: [], aspect: 1 }, [300, 400]);
    expect(tall.t - tall.b).toBe(160);
  });

  it("commits a `signature` spec, not a `stamp`", () => {
    setDrawnSignature({ paths: [[0, 0, 0.5, 1, 1, 0]], aspect: 2.5 });
    const tool = makeStampTool("signature");
    const at = { page: 2, pt: [300, 400] as [number, number], modifiers: { shift: false, alt: false, meta: false } };
    const down = tool.onDown(tool.init({} as ToolContext), at, {} as ToolContext);
    const up = tool.onUp(down.state, at, {} as ToolContext);
    expect(up.commit?.page).toBe(2);
    expect(up.commit?.spec.kind).toBe("signature");
    const spec = up.commit!.spec as Extract<AnnotSpec, { kind: "ink" | "signature" }>;
    expect(spec.paths).toHaveLength(1);
    expect(spec.paths[0]).toHaveLength(6);
    expect(spec.opacity).toBe(1);
  });

  it("a picked image replaces a drawn signature and vice versa", () => {
    setDrawnSignature({ paths: [[0, 0, 1, 1]], aspect: 2 });
    expect(drawnSignature()).not.toBeNull();
    setStampImage("signature", { path: "/tmp/sig.png" });
    expect(drawnSignature()).toBeNull();
    setDrawnSignature({ paths: [[0, 0, 1, 1]], aspect: 2 });
    expect(drawnSignature()).not.toBeNull();
  });
});
