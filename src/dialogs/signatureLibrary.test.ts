/**
 * 서명 보관함 (P1-9): the list rules — cap at 10 without evicting, duplicates are no-ops, drawn
 * strokes are rounded before they go into settings.json, and a hand-edited file cannot break
 * the dialog.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createElement } from "react";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import * as api from "../ipc/api";
import { resetMock, setMockImageSize } from "../ipc/mock";
import { useAppStore } from "../store/appStore";
import SignatureDialog from "./SignatureDialog";
import type { SavedSignature } from "../ipc/types";
import {
  MAX_SAVED_SIGNATURES,
  addSignature,
  isFull,
  readSignatures,
  removeSignature,
  roundPaths,
  savedFromDrawn,
  savedFromImage,
  savedFromTyped,
} from "./signatureLibrary";

type Typed = Extract<SavedSignature, { kind: "typed" }>;
const typed = (i: number): Typed => ({ kind: "typed", id: `t${i}`, text: `Name ${i}`, style: "script", createdAt: "" });

describe("signature library", () => {
  it("appends newest last and refuses the 11th without evicting anything", () => {
    let list: SavedSignature[] = [];
    for (let i = 0; i < MAX_SAVED_SIGNATURES; i++) list = addSignature(list, typed(i))!;
    expect(list).toHaveLength(10);
    expect(isFull(list)).toBe(true);
    expect(addSignature(list, typed(99))).toBeNull();
    expect(list.map((s) => s.id)[0]).toBe("t0");
  });

  it("treats the same text + style, or the same strokes, as the same signature", () => {
    const list = [typed(1)];
    expect(addSignature(list, { ...typed(1), id: "other" })).toBe(list);
    expect(addSignature(list, { ...typed(1), id: "other", style: "formal" })).toHaveLength(2);
    const drawn = savedFromDrawn({ paths: [[0, 0, 1, 1]], aspect: 2 });
    const withDrawn = addSignature(list, drawn)!;
    expect(addSignature(withDrawn, { ...drawn, id: "again" })).toBe(withDrawn);
  });

  it("removes by id", () => {
    expect(removeSignature([typed(1), typed(2)], "t1").map((s) => s.id)).toEqual(["t2"]);
  });

  it("rounds drawn strokes to 4 decimals and trims typed text", () => {
    expect(roundPaths([[0.123456, 0.987654]])).toEqual([[0.1235, 0.9877]]);
    const d = savedFromDrawn({ paths: [[0.333333, 0.666666]], aspect: 2.5 }, new Date("2026-09-28T00:00:00Z"));
    expect(d).toMatchObject({ kind: "drawn", paths: [[0.3333, 0.6667]], aspect: 2.5, createdAt: "2026-09-28T00:00:00.000Z" });
    expect(savedFromTyped("  박현우  ", "hand")).toMatchObject({ kind: "typed", text: "박현우", style: "hand" });
    expect(d.id).not.toBe(savedFromDrawn({ paths: [[0, 0]], aspect: 1 }).id);
  });

  it("reads settings defensively", () => {
    expect(readSignatures(undefined)).toEqual([]);
    expect(readSignatures("nope")).toEqual([]);
    const out = readSignatures([
      typed(1),
      { kind: "drawn", id: "d", paths: [[0, 0, 1, 1]], aspect: 3 },
      { kind: "drawn", id: "bad", paths: [["x"]], aspect: 3 },
      { kind: "typed", text: "no id", style: "script" },
      42,
    ]);
    expect(out.map((s) => s.id)).toEqual(["t1", "d"]);
    expect(readSignatures(Array.from({ length: 15 }, (_, i) => typed(i)))).toHaveLength(10);
  });
});

// ---------------------------------------------------------------------------------------------
// v0.3 pkg4-annotations-stamps-objects (T3): image signatures
// ---------------------------------------------------------------------------------------------

describe("저장된 서명 — image signatures (v0.3 T3)", () => {
  beforeEach(async () => {
    resetMock();
    await useAppStore.getState().bootstrap();
    await useAppStore.getState().patchSettings({ signatures: [] });
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("an image signature is kept by path: a re-save is a no-op, a malformed one is dropped", () => {
    const img = savedFromImage("/app/signatures/img-1.png", 3);
    expect(img).toMatchObject({ kind: "image", path: "/app/signatures/img-1.png", aspect: 3 });
    const list = addSignature([], img)!;
    expect(addSignature(list, { ...img, id: "again" })).toBe(list);
    expect(readSignatures([img, { kind: "image", id: "x", path: 5, aspect: 1 }, { kind: "image", id: "y", path: "/a.png", aspect: 0 }]))
      .toEqual([img]);
  });

  it("이미지 저장… copies the image, it is listed after a settings reload and placed at its aspect", async () => {
    vi.spyOn(api, "openFileDialog").mockResolvedValue(["/Users/veri/Pictures/scan.png"]);
    setMockImageSize("/Users/veri/Pictures/scan.png", 600, 200);
    const onImage = vi.fn();
    const onClose = vi.fn();
    const view = render(
      createElement(SignatureDialog, { onClose, onDrawn: vi.fn(), onImage, onChooseImage: vi.fn() }),
    );
    fireEvent.click(screen.getByRole("tab", { name: "이미지" }));
    fireEvent.click(screen.getByRole("button", { name: "이미지 저장…" }));
    await waitFor(() => expect(useAppStore.getState().settings?.signatures).toHaveLength(1));
    view.unmount();

    // reload: what the backend kept, read back through the defensive reader
    const settings = await api.getSettings();
    const saved = readSignatures(settings.signatures);
    expect(saved).toHaveLength(1);
    expect(saved[0]).toMatchObject({ kind: "image", aspect: 3 });
    expect((saved[0] as { path: string }).path).toMatch(/^\/mock\/app-data\/signatures\/img-[0-9a-f]+\.png$/);

    useAppStore.setState({ settings });
    render(createElement(SignatureDialog, { onClose, onDrawn: vi.fn(), onImage, onChooseImage: vi.fn() }));
    fireEvent.click(screen.getByRole("button", { name: /이미지 서명/ }));
    expect(onImage).toHaveBeenCalledWith({ path: (saved[0] as { path: string }).path, aspect: 3 });
    expect(onClose).toHaveBeenCalled();
  });
});
