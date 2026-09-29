/**
 * v0.3 D1 (이미지로 PDF 만들기) and H7 (pointer-event reordering) against the mock:
 *  * a .png dropped on the window opens the dialog — `open_document` is never called; a PDF dropped
 *    with images opens in a new window (verification round 1);
 *  * the dialog's list reorders by a pointer drag, and 만들기 sends the paths in that order;
 *  * 파일 합치기's list reorders by a pointer drag of item 1 below item 3;
 *  * 편집 ⌘V with an image on the system clipboard places it with `add_image_object`.
 */
import { beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import App from "../App";
import { mock } from "../ipc/mock";
import { useAppStore } from "../store/appStore";
import { useDocStore } from "../store/docStore";
import { useDialogStore } from "./dialogState";
import { useToastStore } from "../app/toastStore";
import DialogHost from "./DialogHost";
import { MergeDialog } from "./MergeDialog";
import { warmLazyChunks } from "../test/warmLazy";

beforeAll(() => warmLazyChunks(() => import("./ImagesToPdfDialog"), () => import("./imagesFlow")));

beforeEach(() => {
  useDocStore.setState({ docId: null, info: null, outline: [], status: "empty", error: null });
  useDialogStore.getState().closeAll();
});

/** jsdom has no layout: 30 px rows, top to bottom. */
function layOut(list: HTMLElement): void {
  list.querySelectorAll<HTMLElement>(".merge-item").forEach((li, i) => {
    li.getBoundingClientRect = () =>
      ({ top: i * 30, bottom: i * 30 + 30, height: 30, left: 0, right: 300, width: 300, x: 0, y: i * 30 }) as DOMRect;
  });
}

function names(list: HTMLElement): string[] {
  return [...list.querySelectorAll(".merge-name")].map((n) => n.textContent ?? "");
}

/** Drag row `from` (0-based) so it lands below row `below`. */
function dragBelow(list: HTMLElement, from: number, below: number): void {
  const rows = list.querySelectorAll<HTMLElement>(".merge-item");
  fireEvent.pointerDown(rows[from].querySelector(".merge-name")!, { button: 0, clientX: 50, clientY: from * 30 + 15 });
  fireEvent.pointerMove(window, { clientX: 50, clientY: below * 30 + 25 });
  fireEvent.pointerUp(window, { clientX: 50, clientY: below * 30 + 25 });
}

describe("이미지로 PDF 만들기", () => {
  it("a dropped .png opens the dialog instead of open_document", async () => {
    const open = vi.spyOn(mock, "openDocument");
    render(<App />);
    await waitFor(() => expect(useAppStore.getState().ready).toBe(true));
    const file = new File([new Uint8Array([0x89, 0x50])], "스캔-1.png", { type: "image/png" });
    await act(async () => {
      const drop = new Event("drop", { bubbles: true, cancelable: true }) as Event & { dataTransfer: unknown };
      Object.defineProperty(drop, "dataTransfer", { value: { files: [file] } });
      window.dispatchEvent(drop);
    });
    await waitFor(() => expect(useDialogStore.getState().stack.at(-1)?.name).toBe("imagesToPdf"));
    expect(useDialogStore.getState().stack.at(-1)?.props.paths).toEqual(["스캔-1.png"]);
    expect(await screen.findByRole("dialog", { name: "이미지로 PDF 만들기" })).toBeInTheDocument();
    expect(open).not.toHaveBeenCalled();
  });

  it("a PDF dropped together with images opens in a new window, not silently dropped", async () => {
    const open = vi.spyOn(mock, "openDocument");
    const newWindow = vi.spyOn(mock, "openInNewWindow");
    useToastStore.setState({ toasts: [] });
    const { routeDroppedPaths } = await import("./imagesFlow");
    await routeDroppedPaths(["/p/report.pdf", "/p/scan.png"]);
    const top = useDialogStore.getState().stack.at(-1);
    expect(top?.name).toBe("imagesToPdf");
    expect(top?.props.paths).toEqual(["/p/scan.png"]);
    expect(newWindow).toHaveBeenCalledWith({ path: "/p/report.pdf" });
    expect(open).not.toHaveBeenCalled();
    expect(useToastStore.getState().toasts.at(-1)?.messageKey).toBe("imagesToPdf.othersOpened");
    expect(useToastStore.getState().toasts.at(-1)?.params).toEqual({ count: 1 });
    newWindow.mockRestore();
    open.mockRestore();
  });

  it("reorders by pointer drag and creates the document in that order", async () => {
    const create = vi.spyOn(mock, "createFromImages");
    const paths = ["/p/a.png", "/p/b.jpg", "/p/c.jpeg"];
    act(() => useDialogStore.getState().open("imagesToPdf", { paths }));
    render(<DialogHost />);
    const list = await screen.findByRole("list", { name: "페이지 순서" });
    layOut(list);
    dragBelow(list, 0, 2);
    expect(names(list)).toEqual(["b.jpg", "c.jpeg", "a.png"]);
    fireEvent.click(screen.getByRole("radio", { name: "원본 크기" }));
    fireEvent.click(screen.getByRole("button", { name: "만들기" }));
    await waitFor(() => expect(create).toHaveBeenCalledTimes(1));
    expect(create.mock.calls[0][0]).toMatchObject({ paths: ["/p/b.jpg", "/p/c.jpeg", "/p/a.png"], pageSize: "original" });
    await waitFor(() => expect(useDocStore.getState().info?.name).toBe("b.pdf"));
    const info = useDocStore.getState().info!;
    expect(info.pageCount).toBe(3);
    expect(info.dirty).toBe(true);
    expect(info.path).toBeNull();
  });

  it("the reorder buttons are disabled at the list ends and do not take the initial focus", async () => {
    const paths = ["/p/a.png", "/p/b.jpg", "/p/c.jpeg"];
    act(() => useDialogStore.getState().open("imagesToPdf", { paths }));
    render(<DialogHost />);
    const list = await screen.findByRole("list", { name: "페이지 순서" });
    const up = within(list).getAllByRole("button", { name: "위로 이동" });
    const down = within(list).getAllByRole("button", { name: "아래로 이동" });
    expect(up.map((b) => (b as HTMLButtonElement).disabled)).toEqual([true, false, false]);
    expect(down.map((b) => (b as HTMLButtonElement).disabled)).toEqual([false, false, true]);
    expect(list.contains(document.activeElement)).toBe(false);
    fireEvent.click(up[2]);
    expect(names(list)).toEqual(["a.png", "c.jpeg", "b.jpg"]);
  });

  it("편집 ⌘V with an image on the system clipboard adds it to the page", async () => {
    await useDocStore.getState().open("/tmp/sample.pdf");
    useAppStore.getState().setMode("edit");
    const add = vi.spyOn(mock, "addImageObject");
    const { pasteSystemImage } = await import("../edit/actions");
    const png = new File([new Uint8Array([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a])], "clip.png", { type: "image/png" });
    const data = { items: [{ kind: "file", type: "image/png", getAsFile: () => png }] } as unknown as DataTransfer;
    expect(await pasteSystemImage(data)).toBe(true);
    expect(add).toHaveBeenCalledTimes(1);
    const args = add.mock.calls[0][0];
    expect(args.path).toMatch(/^\/mock\/tmp\/seepdf-clipboard\/clip-\d+\.png$/);
    expect(args.keepAspect).toBe(true);
    // centred on the page, half its size
    const page = useDocStore.getState().info!.pages[args.page];
    expect((args.rect.l + args.rect.r) / 2).toBeCloseTo((page.crop.l + page.crop.r) / 2, 3);
    // nothing image-like on the clipboard: nothing happens
    expect(await pasteSystemImage({ items: [] } as unknown as DataTransfer)).toBe(false);
  });
});

describe("파일 합치기 (H7)", () => {
  it("a pointer drag of item 1 below item 3 reorders the list", async () => {
    render(<MergeDialog onClose={() => undefined} initialPaths={["/a/one.pdf", "/a/two.pdf", "/a/three.pdf"]} />);
    const list = screen.getByRole("list", { name: "파일 합치기" });
    layOut(list);
    dragBelow(list, 0, 2);
    expect(names(list)).toEqual(["two.pdf", "three.pdf", "one.pdf"]);
    // a click on a row's button is not a drag
    fireEvent.click(within(list).getAllByRole("button", { name: "위로 이동" })[1]);
    expect(names(list)).toEqual(["three.pdf", "two.pdf", "one.pdf"]);
  });
});
