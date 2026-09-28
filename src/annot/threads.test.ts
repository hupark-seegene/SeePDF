/**
 * P2 threads as pure functions: grouping replies under their thread's top-level annotation,
 * flattening nested replies oldest first, the reply ids a delete takes with it, and dates in both
 * the engine's `D:` format and ISO.
 */
import { describe, expect, it } from "vitest";
import type { Annot } from "../ipc/types";
import { groupByPage } from "../sidebar/AnnotationList";
import {
  annotTime, byCreated, descendantIds, isoOf, threadReplies, threadRootId, threaded, withoutReplies,
} from "./threads";

function annot(over: Partial<Annot>): Annot {
  return {
    id: "a", page: 0, kind: "note", subtype: "Text",
    rect: { l: 0, b: 0, r: 20, t: 20 }, color: [255, 216, 77], fillColor: null, opacity: 1,
    borderWidth: 1, contents: "", author: null, created: null, modified: null,
    hidden: false, printed: true, locked: false, editable: "full", ...over,
  };
}

// root ← r1 ← r1a ; root ← r2 ; other ; orphan (parent not on the page)
const LIST: Annot[] = [
  annot({ id: "root", kind: "highlight", created: "D:20260928100000+09'00'" }),
  annot({ id: "r2", inReplyTo: "root", created: "D:20260928120000+09'00'" }),
  annot({ id: "r1a", inReplyTo: "r1", created: "2026-09-28T02:30:00Z" }), // 11:30 KST
  annot({ id: "other", created: "D:20260928090000" }),
  annot({ id: "r1", inReplyTo: "root", created: "D:20260928110000+09'00'" }),
  annot({ id: "orphan", inReplyTo: "gone", created: null }),
];

describe("annot/threads", () => {
  it("reads PDF and ISO dates", () => {
    expect(annotTime("D:20260928110000+09'00'")).toBe(Date.UTC(2026, 8, 28, 2, 0, 0));
    expect(annotTime("D:2026")).toBe(Date.UTC(2026, 0, 1));
    expect(annotTime("2026-09-28T02:30:00Z")).toBe(Date.UTC(2026, 8, 28, 2, 30));
    expect(annotTime("not a date")).toBeNull();
    expect(annotTime(null)).toBeNull();
    expect(isoOf("D:20260928110000Z")).toBe("2026-09-28T11:00:00.000Z");
    expect(isoOf(undefined)).toBe("");
  });

  it("orders by creation, keeping undated ones in place", () => {
    const ids = byCreated([
      annot({ id: "b", created: "D:20260102" }),
      annot({ id: "x" }),
      annot({ id: "a", created: "D:20260101" }),
    ]).map((a) => a.id);
    expect(ids).toEqual(["a", "x", "b"]);
  });

  it("finds a thread's root and every reply below an annotation", () => {
    expect(threadRootId(LIST, "r1a")).toBe("root");
    expect(threadRootId(LIST, "orphan")).toBe("orphan");
    expect(threadRootId(LIST, "unknown")).toBe("unknown");
    expect(descendantIds(LIST, "root").sort()).toEqual(["r1", "r1a", "r2"]);
    expect(descendantIds(LIST, "r1")).toEqual(["r1a"]);
    expect(descendantIds(LIST, "other")).toEqual([]);
    // a cycle (a damaged file) terminates
    const cyc = [annot({ id: "p", inReplyTo: "q" }), annot({ id: "q", inReplyTo: "p" })];
    expect(threadRootId(cyc, "p")).toMatch(/^[pq]$/);
    expect(descendantIds(cyc, "p")).toEqual(["q"]);
  });

  it("groups replies under their root, flattened and oldest first; an orphan stays top-level", () => {
    const threads = threaded(LIST);
    expect(threads.map((a) => a.id)).toEqual(["root", "other", "orphan"]);
    expect(threads[0].replies?.map((a) => a.id)).toEqual(["r1", "r1a", "r2"]);
    expect(threads[1].replies).toBeUndefined();
    expect(threadReplies(LIST, "r1").map((a) => a.id)).toEqual(["r1", "r1a", "r2"]);
    // the canvas never hit-tests an attached reply
    expect(withoutReplies(LIST).map((a) => a.id)).toEqual(["root", "other", "orphan"]);
  });

  it("the 주석 tab lists threads by page; the filter picks threads by their root's kind", () => {
    const byPage = { 0: LIST };
    const [[page, list]] = groupByPage(byPage, [], null);
    expect(page).toBe(0);
    expect(list.map((a) => a.id)).toEqual(["root", "other", "orphan"]);
    // filtering by 메모 keeps the note threads, and does not surface the highlight's replies alone
    const notes = groupByPage(byPage, [], ["note"])[0][1].map((a) => a.id);
    expect(notes).toEqual(["other", "orphan"]);
    const highlights = groupByPage(byPage, [], ["highlight"])[0][1];
    expect(highlights.map((a) => a.id)).toEqual(["root"]);
    expect(highlights[0].replies).toHaveLength(3);
  });
});
