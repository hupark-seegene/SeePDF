/**
 * v0.3 H7: in the app (not the mock) the drop highlight follows Tauri's native drag-drop events —
 * HTML5 `dragover` never fires in the Windows webview while the native handler is on.
 */
import { describe, expect, it, vi } from "vitest";

type Handler = (e: { payload: { type: string; paths?: string[]; position?: { x: number; y: number } } }) => void;
const hooks: { handler: Handler | null } = { handler: null };

vi.mock("./env", async (importOriginal) => ({
  ...(await importOriginal<typeof import("./env")>()),
  useMock: () => false,
}));

vi.mock("@tauri-apps/api/webview", () => ({
  getCurrentWebview: () => ({
    onDragDropEvent: (h: Handler) => {
      hooks.handler = h;
      return Promise.resolve(() => undefined);
    },
  }),
}));

describe("onFileDragState (Tauri)", () => {
  it("maps enter / over / leave / drop to the highlight states", async () => {
    const { onFileDragState } = await import("./events");
    const states: string[] = [];
    const off = onFileDragState((s) => states.push(s));
    await Promise.resolve();
    expect(hooks.handler).not.toBeNull();
    hooks.handler!({ payload: { type: "enter", paths: ["/a.pdf"] } });
    hooks.handler!({ payload: { type: "over", position: { x: 10, y: 10 } } });
    hooks.handler!({ payload: { type: "leave" } });
    hooks.handler!({ payload: { type: "drop", paths: ["/a.pdf"] } });
    expect(states).toEqual(["over", "over", "leave", "drop"]);
    off();
  });

  it("a native drop carries the position in CSS pixels", async () => {
    const { onFileDrop } = await import("./events");
    const drops: unknown[] = [];
    const off = onFileDrop((e) => drops.push(e));
    await Promise.resolve();
    const ratio = window.devicePixelRatio || 1;
    hooks.handler!({ payload: { type: "drop", paths: ["/x.png"], position: { x: 200 * ratio, y: 100 * ratio } } });
    expect(drops).toEqual([{ paths: ["/x.png"], position: { x: 200, y: 100 } }]);
    off();
  });
});
