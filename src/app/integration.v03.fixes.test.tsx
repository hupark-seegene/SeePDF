/**
 * v0.3 integration — regressions found by the cross-package verification (windows /
 * reachability / interaction lenses), against the mock adapter.
 *
 * - H2 × one document per window: an `open-file` event only says "something was queued for this
 *   window"; the window takes its own queue, so a request is opened once (event and mount drain
 *   together, or the event twice) and never by a window it was not meant for.
 * - X8 (PDF 앱으로 인쇄) on Windows where SeePDF is the default PDF app: printed through the webview,
 *   not handed back to SeePDF.
 * - D1 클립보드에서 새로 만들기 from the native menu: the engine reads the clipboard (WebKit refuses
 *   `navigator.clipboard.read` without a user gesture); a refused webview read says so.
 * - R2 / R3 × S1: 적용 on a signed document says the signature goes on save.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, render, waitFor } from "@testing-library/react";
import App from "../App";
import { mock, mockEvents, mockSetClipboardImage, mockSetPdfHandlerIsSelf, pushPendingOpen } from "../ipc/mock";
import { useAppStore } from "../store/appStore";
import { useDocStore } from "../store/docStore";
import { useDialogStore } from "../dialogs/dialogState";
import { useToastStore } from "./toastStore";
import { usePrintStore } from "../print/printStore";
import { useEditStore } from "../edit/editStore";

const openerCalls: string[] = [];
vi.mock("@tauri-apps/plugin-opener", () => ({
  openPath: async (path: string) => {
    openerCalls.push(path);
  },
}));

const PNG = new Uint8Array([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 0]);

function toastKeys(): string[] {
  return useToastStore.getState().toasts.map((t) => t.messageKey);
}

beforeEach(() => {
  openerCalls.length = 0;
  useDialogStore.getState().closeAll();
  useToastStore.setState({ toasts: [] });
  useDocStore.setState({ docId: null, info: null, outline: [], status: "empty", error: null });
  useAppStore.setState({ os: "macos", mode: "read", tool: "select", momentaryFrom: null });
  usePrintStore.setState({ job: null } as never);
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe("H2: an open request is taken by one window, once", () => {
  it("the open-file event drains this window's queue; a repeated event opens nothing again", async () => {
    const opened: string[] = [];
    const real = mock.openDocument.bind(mock);
    vi.spyOn(mock, "openDocument").mockImplementation(async (a) => {
      opened.push(a.path);
      return real(a);
    });
    render(<App />);
    // let the mount drain run first (nothing queued yet)
    await waitFor(() => expect(useAppStore.getState().settings).toBeTruthy());
    pushPendingOpen("/docs/a-from-explorer.pdf", "argv");
    act(() => {
      mockEvents.emit("open-file", { path: "/docs/a-from-explorer.pdf", source: "argv" });
      mockEvents.emit("open-file", { path: "/docs/a-from-explorer.pdf", source: "argv" });
    });
    await waitFor(() => expect(useDocStore.getState().info?.path).toBe("/docs/a-from-explorer.pdf"));
    // an event without a request for this window (another window's) opens nothing
    act(() => mockEvents.emit("open-file", { path: "/docs/b-for-doc-2.pdf", source: "dialog" }));
    await new Promise((r) => setTimeout(r, 50));
    expect(opened).toEqual(["/docs/a-from-explorer.pdf"]);
  });
});

describe("X8: PDF 앱으로 인쇄 where SeePDF is the default PDF app", () => {
  it("prints through the webview instead of handing the copy back to SeePDF", async () => {
    useAppStore.setState({ os: "windows" });
    mockSetPdfHandlerIsSelf(true);
    await useDocStore.getState().open("/docs/sample.pdf");
    const prepare = vi.spyOn(mock, "printPrepare");
    const { runPrint } = await import("../dialogs/flows");
    await runPrint(undefined, "handler");
    expect(openerCalls).toEqual([]);
    expect(prepare).not.toHaveBeenCalled();
    expect(toastKeys()).toContain("print.handlerIsSelf");
    expect(usePrintStore.getState().job).toBeTruthy();
  });

  it("another default PDF app still gets the flattened copy", async () => {
    useAppStore.setState({ os: "windows" });
    mockSetPdfHandlerIsSelf(false);
    await useDocStore.getState().open("/docs/sample.pdf");
    const { runPrint } = await import("../dialogs/flows");
    await runPrint(undefined, "handler");
    expect(openerCalls).toHaveLength(1);
  });
});

describe("D1: 클립보드에서 새로 만들기 from the native menu", () => {
  it("uses the engine's clipboard read — no user gesture needed", async () => {
    mockSetClipboardImage(PNG);
    const web = vi.fn(async () => {
      throw Object.assign(new Error("not allowed"), { name: "NotAllowedError" });
    });
    Object.defineProperty(navigator, "clipboard", { value: { read: web }, configurable: true });
    const create = vi.spyOn(mock, "createFromImages");
    const { newFromClipboardFlow } = await import("../dialogs/imagesFlow");
    await newFromClipboardFlow();
    expect(web).not.toHaveBeenCalled();
    expect(create).toHaveBeenCalledTimes(1);
    expect(create.mock.calls[0][0].paths[0]).toMatch(/seepdf-clipboard\/clip-\d+\.png$/);
  });

  it("an empty clipboard says so", async () => {
    mockSetClipboardImage(null);
    const { newFromClipboardFlow } = await import("../dialogs/imagesFlow");
    expect(await newFromClipboardFlow()).toBeNull();
    expect(toastKeys()).toContain("imagesToPdf.noClipboardImage");
  });

  it("without the engine read, a refused webview read says the clipboard could not be read", async () => {
    vi.spyOn(mock, "clipboardImageToTemp").mockRejectedValue(new Error("unknown command"));
    Object.defineProperty(navigator, "clipboard", {
      value: {
        read: async () => {
          throw Object.assign(new Error("not allowed"), { name: "NotAllowedError" });
        },
      },
      configurable: true,
    });
    const { newFromClipboardFlow } = await import("../dialogs/imagesFlow");
    expect(await newFromClipboardFlow()).toBeNull();
    expect(toastKeys()).toContain("imagesToPdf.clipboardUnreadable");
    expect(toastKeys()).not.toContain("imagesToPdf.noClipboardImage");
  });
});

describe("R2 / R3 × S1: 적용 on a signed document", () => {
  async function confirmBodyFor(path: string): Promise<string | undefined> {
    const info = await useDocStore.getState().open(path);
    if (!info) throw new Error("open failed");
    useEditStore.setState({ docId: info.docId, marks: [{ id: 1, page: 0, rect: { l: 72, b: 600, r: 200, t: 620 } }] });
    const { applyMarks } = await import("../edit/redact");
    const done = applyMarks();
    await waitFor(() => expect(useDialogStore.getState().stack.at(-1)?.name).toBe("confirm"));
    const top = useDialogStore.getState().stack.at(-1)!;
    act(() => (top.props.resolve as (v: boolean) => void)(false));
    await done;
    return top.props.bodyKey as string | undefined;
  }

  it("says the signature goes on save", async () => {
    expect(await confirmBodyFor("/docs/signed-계약서.pdf")).toBe("redact.applyWarningSigned");
  });

  it("an unsigned document keeps the plain warning", async () => {
    expect(await confirmBodyFor("/docs/plain.pdf")).toBe("redact.applyWarning");
  });
});
