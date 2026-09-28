/**
 * 여러 파일 OCR (P1-7) — the queue model and the output naming rules, without a document.
 */
import { describe, expect, it } from "vitest";
import {
  MAX_CANDIDATES, addPaths, isRunnable, outputCandidate, pathKey, pickOutputPath, removeItem, requeue,
  summarize, type BatchItem, type BatchStatus,
} from "./queue";

function ids() {
  let n = 0;
  return () => ++n;
}

function item(id: number, status: BatchStatus, path = `/d/${id}.pdf`): BatchItem {
  return { id, path, name: `${id}.pdf`, status, done: 0, total: 0 };
}

describe("batchOcr.queue", () => {
  it("appends files in order and drops duplicates (case-insensitively)", () => {
    const next = ids();
    let items = addPaths([], ["/a/스캔.pdf", "/a/보고서.PDF"], next);
    items = addPaths(items, ["/A/스캔.pdf", "/b/스캔.pdf", ""], next);
    expect(items.map((i) => [i.id, i.name, i.status])).toEqual([
      [1, "스캔.pdf", "queued"],
      [2, "보고서.PDF", "queued"],
      [3, "스캔.pdf", "queued"],
    ]);
    // nothing new → the same array, so the store does not re-render
    expect(addPaths(items, ["/a/스캔.pdf"], next)).toBe(items);
  });

  it("never removes the file that is being worked on", () => {
    const items = [item(1, "running"), item(2, "queued")];
    expect(removeItem(items, 1).map((i) => i.id)).toEqual([1, 2]);
    expect(removeItem(items, 2).map((i) => i.id)).toEqual([1]);
  });

  it("시작 takes every file without a copy yet — resume after 취소, retry after 실패/건너뜀", () => {
    const statuses: BatchStatus[] = ["queued", "opening", "running", "saving", "done", "skipped", "failed", "cancelled"];
    expect(statuses.filter((s) => isRunnable({ status: s }))).toEqual(["queued", "skipped", "failed", "cancelled"]);
  });

  it("requeue clears the previous outcome of the files a new run takes", () => {
    const before: BatchItem[] = [
      { ...item(1, "failed"), reasonKey: "error.openFailed", detail: "x" },
      { ...item(2, "done"), output: "/d/2-ocr.pdf" },
    ];
    const after = requeue(before, new Set([1]));
    expect(after[0]).toEqual({ ...item(1, "queued"), reasonKey: undefined, detail: undefined, output: undefined });
    expect(after[1]).toBe(before[1]);
  });

  it("summarises a run", () => {
    const s = summarize([
      item(1, "done"), item(2, "done"), item(3, "skipped"), item(4, "failed"),
      item(5, "cancelled"), item(6, "queued"), item(7, "saving"),
    ]);
    expect(s).toEqual({ total: 7, queued: 1, active: 1, done: 2, skipped: 1, failed: 1, cancelled: 1 });
  });
});

describe("batchOcr.outputName", () => {
  it("puts <name>-ocr.pdf beside the source, keeping the extension's case", () => {
    expect(outputCandidate("/Users/veri/스캔 문서.pdf", null)).toBe("/Users/veri/스캔 문서-ocr.pdf");
    expect(outputCandidate("/Users/veri/SCAN.PDF", null)).toBe("/Users/veri/SCAN-ocr.PDF");
    expect(outputCandidate("/Users/veri/a.b.pdf", null, 2)).toBe("/Users/veri/a.b-ocr (2).pdf");
    expect(outputCandidate("C:\\Users\\veri\\스캔.pdf", null, 3)).toBe("C:\\Users\\veri\\스캔-ocr (3).pdf");
    expect(outputCandidate("/tmp/noext", null)).toBe("/tmp/noext-ocr.pdf");
  });

  it("or in the chosen folder, whatever its trailing separator", () => {
    expect(outputCandidate("/a/스캔.pdf", "/out")).toBe("/out/스캔-ocr.pdf");
    expect(outputCandidate("/a/스캔.pdf", "/out/")).toBe("/out/스캔-ocr.pdf");
    expect(outputCandidate("/a/스캔.pdf", "/")).toBe("/스캔-ocr.pdf");
    expect(outputCandidate("C:\\a\\스캔.pdf", "D:\\OCR\\", 2)).toBe("D:\\OCR\\스캔-ocr (2).pdf");
    expect(outputCandidate("C:\\a\\스캔.pdf", "D:\\")).toBe("D:\\스캔-ocr.pdf");
  });

  it("appends (2), (3)… while the name exists on disk", async () => {
    const onDisk = new Set(["/a/스캔-ocr.pdf", "/a/스캔-ocr (2).pdf"]);
    const probed: string[] = [];
    const exists = async (p: string) => {
      probed.push(p);
      return onDisk.has(p);
    };
    await expect(pickOutputPath("/a/스캔.pdf", null, exists)).resolves.toBe("/a/스캔-ocr (3).pdf");
    expect(probed).toEqual(["/a/스캔-ocr.pdf", "/a/스캔-ocr (2).pdf", "/a/스캔-ocr (3).pdf"]);
  });

  it("skips names this batch has already written, compared case-insensitively", async () => {
    const reserved = new Set([pathKey("/OUT/보고서-ocr.pdf")]);
    const exists = async () => false;
    await expect(pickOutputPath("/b/보고서.pdf", "/out", exists, reserved)).resolves.toBe("/out/보고서-ocr (2).pdf");
  });

  it("an -ocr file queued again gets a new name, never its own", async () => {
    await expect(pickOutputPath("/out/x-ocr.pdf", "/out", async () => false)).resolves.toBe("/out/x-ocr-ocr.pdf");
  });

  it("gives up instead of probing forever", async () => {
    let calls = 0;
    await expect(pickOutputPath("/a/x.pdf", null, async () => (calls++, true))).rejects.toThrow(/no free output name/);
    expect(calls).toBe(MAX_CANDIDATES);
  });
});
