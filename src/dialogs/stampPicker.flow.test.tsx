/**
 * 도장 선택 (P1-12): arming 도장 opens the picker; 결재 / 승인 / 기밀 lead the list; choosing one
 * makes the ghost show the label and the next click commit `image: { builtin: "결재" }` at the
 * seal's own size. And 서명 → 입력's PNG is placed at the aspect it was rendered at.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen } from "@testing-library/react";
import { useDialogStore } from "./dialogState";
import StampPickerDialog from "./StampPickerDialog";
import { host } from "../annot/AnnotationHost";
import { toolController, type ToolContext } from "../tools/ToolController";
import { makeStampTool, placementSize, resetStampImages, setStampImage, stampImage } from "../tools/stamp";
import { BUILTIN_STAMPS, KOREAN_SEAL_RED } from "../tools/stampCatalog";
import type { AnnotSpec } from "../ipc/types";

const ctx = {} as ToolContext;
const at = (x: number, y: number) => ({ page: 1, pt: [x, y] as [number, number] });

describe("도장 선택", () => {
  beforeEach(() => {
    resetStampImages();
    useDialogStore.getState().closeAll();
  });

  it("lists the Korean set first and picks with one click", () => {
    expect(BUILTIN_STAMPS.slice(0, 3).map((s) => s.id)).toEqual(["결재", "승인", "기밀"]);
    const onPick = vi.fn();
    const onClose = vi.fn();
    render(<StampPickerDialog onPick={onPick} onClose={onClose} onChooseImage={vi.fn()} current="승인" />);
    expect(screen.getByRole("option", { name: "승인" })).toHaveAttribute("aria-selected", "true");
    fireEvent.click(screen.getByRole("option", { name: "결재" }));
    expect(onPick).toHaveBeenCalledWith("결재");
    expect(onClose).toHaveBeenCalled();
  });

  it("a chosen 결재 follows the cursor with its label and commits at the seal size", () => {
    setStampImage("stamp", { builtin: "결재" });
    const tool = makeStampTool("stamp");
    const hover = tool.onMove(tool.init(ctx), at(300, 400), ctx);
    expect(hover.preview).toMatchObject({ kind: "stamp", label: "결재", color: KOREAN_SEAL_RED });
    const down = tool.onDown(tool.init(ctx), at(300, 400), ctx);
    const up = tool.onUp(down.state, at(300, 400), ctx);
    const spec = up.commit?.spec as Extract<AnnotSpec, { kind: "stamp" }>;
    expect(spec.image).toEqual({ builtin: "결재" });
    expect(spec.rect.r - spec.rect.l).toBe(64);
    expect(spec.rect.t - spec.rect.b).toBe(64);
  });

  it("a typed signature keeps its aspect when placed", () => {
    setStampImage("signature", { path: "/sig.png" }, 4);
    expect(placementSize("signature", stampImage("signature"))).toEqual({ w: 160, h: 40 });
    setStampImage("signature", { path: "/scan.png" });
    expect(placementSize("signature", stampImage("signature"))).toEqual({ w: 160, h: 56 });
  });
});

describe("the annotation host opens the right picker", () => {
  let stop: (() => void) | null = null;
  beforeEach(() => {
    resetStampImages();
    useDialogStore.getState().closeAll();
    stop = host.start();
  });
  afterEach(() => {
    stop?.();
    toolController.arm("select");
  });

  it("arming 도장 opens 도장 선택, and its choice becomes the stamp", () => {
    toolController.arm("stamp");
    const top = useDialogStore.getState().stack.at(-1);
    expect(top?.name).toBe("stampPicker");
    (top?.props.onPick as (id: string) => void)("기밀");
    expect(stampImage("stamp")).toEqual({ builtin: "기밀" });
  });

  it("arming 서명 opens 서명 만들기, and a typed signature becomes an image with its aspect", () => {
    toolController.arm("signature");
    const top = useDialogStore.getState().stack.at(-1);
    expect(top?.name).toBe("signature");
    (top?.props.onImage as (s: { path: string; aspect: number }) => void)({ path: "/app/signatures/sig-1.png", aspect: 5 });
    expect(stampImage("signature")).toEqual({ path: "/app/signatures/sig-1.png" });
    expect(placementSize("signature", stampImage("signature")).h).toBe(32);
  });
});
