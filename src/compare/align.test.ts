/**
 * Stage 8 `alignPages`: pages pair by text similarity, so a page inserted in B is a row of its
 * own (`pageA: null`) instead of shifting every later pair. Pinned against the mock, which runs the
 * same kind of alignment (word-set similarity, in order) as the engine.
 */
import { describe, expect, it } from "vitest";
import * as api from "../ipc/api";
import type { CompareReport, JobEvent } from "../ipc/types";

const SAMPLE = "/Users/veri/Documents/SeePDF-샘플.pdf"; // 3 pages

function run(docA: string, docB: string, options: Parameters<typeof api.compareDocuments>[0]["options"]) {
  return new Promise<CompareReport>((resolve, reject) => {
    void api.compareDocuments({ docA, docB, options }, (e: JobEvent) => {
      if (e.type === "done" && e.compare) resolve(e.compare);
      if (e.type === "error") reject(e.error);
    });
  });
}

describe("compare.alignPages (mock)", () => {
  it("a page inserted at the front of B becomes a null-sided row; the rest still pair", async () => {
    const a = await api.openDocument({ path: SAMPLE });
    const b = await api.openDocument({ path: "/tmp/b-with-a-new-first-page.pdf" });
    // B = [its page 3, then pages 1 and 2]: A's pages 1–2 are B's 2nd and 3rd candidates
    const aligned = await run(a.docId, b.docId, { pagesA: [0, 1], pagesB: [2, 0, 1], alignPages: true });
    expect(aligned.pages.map((p) => [p.pageA, p.pageB])).toEqual([[null, 2], [0, 0], [1, 1]]);
    expect(aligned.pages[0].changed).toBe(true);

    // by index (the Stage 5 behaviour) every pair would be shifted
    const byIndex = await run(a.docId, b.docId, { pagesA: [0, 1], pagesB: [2, 0, 1], alignPages: false });
    expect(byIndex.pages.map((p) => [p.pageA, p.pageB])).toEqual([[0, 2], [1, 0], [null, 1]]);
  });

  it("a page missing from B is a row with `pageB: null`", async () => {
    const a = await api.openDocument({ path: SAMPLE });
    const b = await api.openDocument({ path: "/tmp/b-without-page-2.pdf" });
    const report = await run(a.docId, b.docId, { pagesB: [0, 2], alignPages: true });
    expect(report.pages.map((p) => [p.pageA, p.pageB])).toEqual([[0, 0], [1, null], [2, 2]]);
  });
});
