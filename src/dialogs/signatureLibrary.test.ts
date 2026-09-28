/**
 * 서명 보관함 (P1-9): the list rules — cap at 10 without evicting, duplicates are no-ops, drawn
 * strokes are rounded before they go into settings.json, and a hand-edited file cannot break
 * the dialog.
 */
import { describe, expect, it } from "vitest";
import type { SavedSignature } from "../ipc/types";
import {
  MAX_SAVED_SIGNATURES,
  addSignature,
  isFull,
  readSignatures,
  removeSignature,
  roundPaths,
  savedFromDrawn,
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
