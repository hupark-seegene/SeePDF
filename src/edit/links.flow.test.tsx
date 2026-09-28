/**
 * 링크 (P2) against the mock adapter: the 편집 mode 링크 tool (drag → popover → `create_link`),
 * the inspector's link section (`update_link`, 링크 열기, 삭제 / ⌫ → `delete_link`), and 읽기 mode's
 * link layer (a page link scrolls, a web link asks before the browser opens it; file: / javascript:
 * never reach the OS).
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { mock, mockOpenedUrls } from "../ipc/mock";
import type { Annot } from "../ipc/types";
import { makePageLayerContext, type PageLayerContext } from "../viewer";
import { useAppStore } from "../store/appStore";
import { useAnnotStore } from "../store/annotStore";
import { useDocStore } from "../store/docStore";
import { setViewProbe, useViewStore } from "../store/viewStore";
import { useToastStore } from "../app/toastStore";
import { useDialogStore } from "../dialogs/dialogState";
import DialogHost from "../dialogs/DialogHost";
import { LinkLayer } from "../annot/LinkLayer";
import { useEditStore } from "./editStore";
import { EditLayer } from "./EditLayer";
import { LinkPanel } from "./LinkPanel";
import { normalizeUrl, useLinkStore } from "./linkActions";
import { editHost } from "./index";

let stop: (() => void) | null = null;

async function open() {
  const info = await useDocStore.getState().open("/tmp/sample.pdf");
  if (!info) throw new Error("mock open failed");
  const page = info.pages[0];
  const ctx = makePageLayerContext({
    docId: info.docId, docGeneration: info.docGeneration, page, rotation: 0, zoomPercent: 100,
    width: page.widthPt, height: page.heightPt,
  });
  return { info, ctx };
}

function at(ctx: PageLayerContext, x: number, y: number) {
  const [cx, cy] = ctx.toDevice(x, y);
  return { clientX: cx, clientY: cy, pointerId: 1, button: 0 };
}

function links(page = 0): Annot[] {
  return (useAnnotStore.getState().byPage[page] ?? []).filter((a) => a.kind === "link");
}

beforeEach(() => {
  useEditStore.getState().reset();
  useLinkStore.getState().reset();
  useAnnotStore.getState().reset();
  useToastStore.setState({ toasts: [] });
  useDialogStore.getState().closeAll();
});

afterEach(() => {
  stop?.();
  stop = null;
  setViewProbe(null);
  vi.restoreAllMocks();
});

describe("편집 · 링크", () => {
  it("drag → 페이지로 이동 → 만들기 is one create_link; the inspector re-targets it; ⌫ deletes it", async () => {
    const { ctx } = await open();
    useAppStore.setState({ mode: "edit", tool: "link" });
    stop = editHost.start();
    const create = vi.spyOn(mock, "createLink");
    const update = vi.spyOn(mock, "updateLink");
    const remove = vi.spyOn(mock, "deleteLink");
    const view = render(
      <>
        <EditLayer ctx={ctx} />
        <LinkPanel link={null} />
      </>,
    );
    const surface = view.container.querySelector(".edit-surface") as HTMLElement;

    // a click is not a link
    fireEvent.pointerDown(surface, at(ctx, 100, 700));
    fireEvent.pointerUp(surface, at(ctx, 100, 700));
    expect(useLinkStore.getState().draft).toBeNull();

    fireEvent.pointerDown(surface, at(ctx, 100, 700));
    fireEvent.pointerMove(surface, at(ctx, 250, 680));
    fireEvent.pointerUp(surface, at(ctx, 250, 680));
    const popover = await screen.findByRole("dialog", { name: "새 링크" });
    const pageBox = popover.querySelector(".link-page-input") as HTMLInputElement;
    expect(pageBox.value).toBe("1");
    fireEvent.change(pageBox, { target: { value: "3" } });
    fireEvent.click(screen.getByRole("button", { name: "만들기" }));

    await waitFor(() => expect(create).toHaveBeenCalledTimes(1));
    expect(create.mock.calls[0][0]).toMatchObject({ page: 0, target: { page: 2 } });
    const rect = create.mock.calls[0][0].rect;
    expect(rect.l).toBeCloseTo(100);
    expect(rect.r).toBeCloseTo(250);
    expect(rect.b).toBeCloseTo(680);
    expect(rect.t).toBeCloseTo(700);
    await waitFor(() => expect(links()).toHaveLength(1));
    const link = links()[0];
    expect(link.dest).toEqual({ page: 2 });
    expect(useLinkStore.getState().selected).toEqual({ page: 0, id: link.id });
    expect(useToastStore.getState().toasts.at(-1)?.messageKey).toBe("link.created");
    expect((await mock.getDocument({ docId: ctx.docId })).undoLabel).toBe("undo.linkCreate");

    // the inspector section (rendered here beside the layer) edits the selected link
    view.rerender(
      <>
        <EditLayer ctx={ctx} />
        <LinkPanel link={useLinkStore.getState().selected} />
      </>,
    );
    expect(screen.getByText("3쪽으로 이동")).toBeInTheDocument();
    fireEvent.click(screen.getAllByRole("radio", { name: "웹 주소" }).at(-1)!);
    fireEvent.change(screen.getByPlaceholderText("https://"), { target: { value: "example.com/문서" } });
    fireEvent.click(screen.getByRole("button", { name: "적용" }));
    await waitFor(() => expect(update).toHaveBeenCalledTimes(1));
    expect(update.mock.calls[0][0]).toMatchObject({ id: link.id, target: { url: "https://example.com/%EB%AC%B8%EC%84%9C" } });
    await waitFor(() => expect(links()[0].uri).toBe("https://example.com/%EB%AC%B8%EC%84%9C"));
    expect(links()[0].dest).toBeUndefined();

    // clicking the link selects it again; ⌫ deletes it
    act(() => useLinkStore.getState().select(null));
    fireEvent.pointerDown(surface, at(ctx, 150, 690));
    expect(useLinkStore.getState().selected?.id).toBe(link.id);
    fireEvent.keyDown(window, { key: "Backspace" });
    await waitFor(() => expect(remove).toHaveBeenCalledTimes(1));
    await waitFor(() => expect(links()).toHaveLength(0));
    expect(useLinkStore.getState().selected).toBeNull();

    // one undo step each: undo the delete → the link is back
    await act(async () => {
      await useDocStore.getState().undo();
    });
    expect((await mock.listAnnotations({ docId: ctx.docId, page: 0 })).annots.filter((a) => a.kind === "link")).toHaveLength(1);
  });

  it("현재 위치 사용 takes the page and y of the current view; Esc drops the rectangle", async () => {
    const { ctx } = await open();
    useAppStore.setState({ mode: "edit", tool: "link" });
    stop = editHost.start();
    setViewProbe(() => ({ page: 1, y: 512.5 }));
    const create = vi.spyOn(mock, "createLink");
    const view = render(<EditLayer ctx={ctx} />);
    const surface = view.container.querySelector(".edit-surface") as HTMLElement;
    fireEvent.pointerDown(surface, at(ctx, 100, 700));
    fireEvent.pointerMove(surface, at(ctx, 200, 650));
    fireEvent.pointerUp(surface, at(ctx, 200, 650));
    await screen.findByRole("dialog", { name: "새 링크" });
    fireEvent.click(screen.getByRole("button", { name: "현재 위치 사용" }));
    expect(screen.getByText("현재 보기 위치 (y 513pt)")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "만들기" }));
    await waitFor(() => expect(create).toHaveBeenCalledTimes(1));
    expect(create.mock.calls[0][0].target).toEqual({ page: 1, y: 512.5 });

    fireEvent.pointerDown(surface, at(ctx, 300, 400));
    fireEvent.pointerMove(surface, at(ctx, 400, 300));
    fireEvent.pointerUp(surface, at(ctx, 400, 300));
    await screen.findByRole("dialog", { name: "새 링크" });
    fireEvent.keyDown(window, { key: "Escape" });
    expect(screen.queryByRole("dialog", { name: "새 링크" })).toBeNull();
    expect(create).toHaveBeenCalledTimes(1);
  });

  it("an encrypted document offers only 웹 주소 (a /Dest needs a lopdf rewrite)", async () => {
    const info = await useDocStore.getState().open("/tmp/encrypted.pdf", "user");
    if (!info) throw new Error("mock open failed");
    expect(info.encrypted).toBe(true);
    const page = info.pages[0];
    const ctx = makePageLayerContext({
      docId: info.docId, docGeneration: info.docGeneration, page, rotation: 0, zoomPercent: 100,
      width: page.widthPt, height: page.heightPt,
    });
    useAppStore.setState({ mode: "edit", tool: "link" });
    stop = editHost.start();
    const view = render(<EditLayer ctx={ctx} />);
    const surface = view.container.querySelector(".edit-surface") as HTMLElement;
    fireEvent.pointerDown(surface, at(ctx, 100, 700));
    fireEvent.pointerMove(surface, at(ctx, 200, 650));
    fireEvent.pointerUp(surface, at(ctx, 200, 650));
    await screen.findByRole("dialog", { name: "새 링크" });
    expect(screen.getByRole("radio", { name: "웹 주소" })).toHaveAttribute("aria-checked", "true");
    fireEvent.click(screen.getByRole("radio", { name: "페이지로 이동" }));
    expect(screen.getByText("암호가 걸린 문서에서는 할 수 없습니다. 보안에서 암호를 먼저 제거하세요.")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "만들기" })).toBeDisabled();
    await expect(mock.createLink({ docId: info.docId, page: 0, rect: { l: 0, b: 0, r: 50, t: 50 }, target: { page: 1 } }))
      .rejects.toMatchObject({ code: "unsupported" });
    // a web link is PDFium's own FPDFAnnot_SetURI: allowed
    await expect(mock.createLink({ docId: info.docId, page: 0, rect: { l: 0, b: 0, r: 50, t: 50 }, target: { url: "https://a.b/" } }))
      .resolves.toMatchObject({ annot: { kind: "link", uri: "https://a.b/" } });
  });
});

describe("읽기 · 링크", () => {
  async function withLinks() {
    const { ctx } = await open();
    await mock.createLink({ docId: ctx.docId, page: 0, rect: { l: 72, b: 700, r: 200, t: 720 }, target: { page: 2, y: 400 } });
    await mock.createLink({ docId: ctx.docId, page: 0, rect: { l: 72, b: 600, r: 200, t: 620 }, target: { url: "https://seegene.com/" } });
    await mock.createLink({ docId: ctx.docId, page: 0, rect: { l: 72, b: 500, r: 200, t: 520 }, target: { url: "javascript:alert(1)" } });
    const list = await mock.listAnnotations({ docId: ctx.docId, page: 0 });
    useAnnotStore.getState().setPage(0, list.annots, list.docGeneration);
    useAppStore.setState({ mode: "read", tool: "select" });
    return { ctx };
  }

  it("a page link scrolls to its destination", async () => {
    const { ctx } = await withLinks();
    render(<LinkLayer ctx={ctx} />);
    fireEvent.click(screen.getByRole("button", { name: "3쪽으로 이동" }));
    expect(useViewStore.getState().currentPage).toBe(2);
    expect(useViewStore.getState().scrollRequest).toMatchObject({ page: 2, yPt: 400 });
  });

  it("a web link asks first, then hands the address to the opener; other schemes are refused", async () => {
    const { ctx } = await withLinks();
    render(
      <>
        <LinkLayer ctx={ctx} />
        <DialogHost />
      </>,
    );
    fireEvent.click(screen.getByRole("button", { name: "https://seegene.com/" }));
    expect(await screen.findByText("브라우저에서 이 주소를 엽니다: https://seegene.com/")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "열기" }));
    await waitFor(() => expect(mockOpenedUrls()).toEqual(["https://seegene.com/"]));

    fireEvent.click(screen.getByRole("button", { name: "javascript:alert(1)" }));
    await act(async () => undefined);
    expect(useDialogStore.getState().stack).toHaveLength(0);
    expect(useToastStore.getState().toasts.at(-1)?.messageKey).toBe("link.blocked");
    expect(mockOpenedUrls()).toHaveLength(1);
  });
});

describe("normalizeUrl", () => {
  it("adds a scheme, keeps one, and percent-encodes non-ASCII", () => {
    expect(normalizeUrl("  example.com ")).toBe("https://example.com");
    expect(normalizeUrl("http://a.b/c")).toBe("http://a.b/c");
    expect(normalizeUrl("me@seegene.com")).toBe("mailto:me@seegene.com");
    expect(normalizeUrl("https://예시.kr/경로")).toBe("https://%EC%98%88%EC%8B%9C.kr/%EA%B2%BD%EB%A1%9C");
    expect(normalizeUrl("   ")).toBe("");
  });
});
