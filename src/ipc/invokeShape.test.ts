/**
 * The invoke payload of every command must name the parameters the Rust command declares.
 *
 * Tauri maps the keys of the object passed to `invoke(cmd, args)` onto the command function's
 * parameter names (camelCase → snake_case). A command that takes **one struct parameter** —
 * `set_viewport(hint: ViewportHint)`, `export_images(args: ExportImagesArgs, …)` — therefore
 * needs `{ hint: … }` / `{ args: … }`, not the struct's fields spread at the top level. Both of
 * those were spread and had never worked: `set_viewport` rejected every prefetch hint with
 * `missing required key hint`, and 내보내기 › PNG failed with `missing required key args`
 * (found by the Stage 2 end-to-end smoke; `docs/STAGE2_INTEGRATION.md`).
 *
 * This test is a *shape* test, not a transport test: it captures what `api.*` would send.
 */
import { describe, expect, it, vi, beforeEach, afterEach } from "vitest";

const invoked: { cmd: string; args: Record<string, unknown> }[] = [];

vi.mock("@tauri-apps/api/core", () => ({
  invoke: (cmd: string, args: Record<string, unknown>) => {
    invoked.push({ cmd, args });
    return Promise.resolve(null);
  },
  Channel: class {
    onmessage: unknown = null;
  },
}));

// Force the real transport: `useMock()` decides per call, so stub the flag this module reads.
vi.mock("./env", async (importOriginal) => {
  const actual = await importOriginal<typeof import("./env")>();
  return { ...actual, useMock: () => false };
});

const api = await import("./api");

describe("invoke payload shape", () => {
  beforeEach(() => {
    invoked.length = 0;
  });
  afterEach(() => {
    invoked.length = 0;
  });

  it("set_viewport sends { hint }, not the hint's fields", async () => {
    await api.setViewport({
      docId: "d1",
      scaleKey: 100,
      rotation: 0,
      centrePage: 0,
      firstPage: 0,
      lastPage: 2,
      velocityPxPerMs: 0,
    });
    expect(invoked).toHaveLength(1);
    expect(invoked[0].cmd).toBe("set_viewport");
    expect(Object.keys(invoked[0].args)).toEqual(["hint"]);
    expect((invoked[0].args.hint as { docId: string }).docId).toBe("d1");
  });

  it("export_images sends { args, onProgress }", async () => {
    await api.exportImages(
      {
        docId: "d1",
        pages: [0],
        format: "png",
        dpi: 150,
        outDir: "/tmp",
        baseName: "page",
      },
      () => undefined,
    );
    expect(invoked).toHaveLength(1);
    expect(invoked[0].cmd).toBe("export_images");
    expect(Object.keys(invoked[0].args).sort()).toEqual(["args", "onProgress"]);
    expect((invoked[0].args.args as { baseName: string }).baseName).toBe("page");
  });

  it("commands with plain parameters keep sending them at the top level", async () => {
    await api.getPageText({ docId: "d1", page: 3 });
    expect(invoked[0]).toEqual({ cmd: "get_page_text", args: { docId: "d1", page: 3 } });
  });

  it("P2: reply_annotation and list_pdf_files name the Rust parameters", async () => {
    await api.replyAnnotation({ docId: "d1", page: 2, parentId: "a-1", contents: "답글", author: "박현우" });
    expect(invoked[0]).toEqual({
      cmd: "reply_annotation",
      args: { docId: "d1", page: 2, parentId: "a-1", contents: "답글", author: "박현우" },
    });
    await api.listPdfFiles({ dir: "/archive", recursive: true });
    expect(invoked[1]).toEqual({ cmd: "list_pdf_files", args: { dir: "/archive", recursive: true } });
  });
});
