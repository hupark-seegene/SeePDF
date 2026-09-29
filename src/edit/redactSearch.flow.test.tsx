/**
 * v0.3 pkg1 flows against the mock adapter:
 *
 * * R3 검색해서 표시 — 검색 ▸ 모든 결과를 영역 표시 marks every hit of "홍길동"; after 적용 the text layer
 *   no longer contains it. The panel's 검색해서 표시 finds 주민등록번호 / 이메일 on every page; one entry
 *   is removed from 표시 목록 before 적용 and survives.
 * * R4 — marks over text inside a group confirm once ("그룹을 해제하고 적용할까요?") and apply with
 *   `ungroup: true`.
 * * R6 — the search result menu (이동 · 복사 · 이 결과 형광펜 · 영역 표시; 형광펜 makes a highlight with
 *   the hit's quads) and the outline item menu (이동 · 하위 항목 모두 펼치기 / 접기).
 * * I1 — Enter that ends a Hangul composition neither searches nor steps (검색) nor confirms (서명).
 */
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import * as api from "../ipc/api";
import { mock, mockSetPageText } from "../ipc/mock";
import { useAppStore } from "../store/appStore";
import { useDocStore } from "../store/docStore";
import { useViewStore } from "../store/viewStore";
import { useToastStore } from "../app/toastStore";
import { useDialogStore } from "../dialogs/dialogState";
import { isSeparator, useContextMenuStore, type MenuItem } from "../app/contextMenuStore";
import { useSearchStore } from "../viewer/search/SearchController";
import { RedactPanel } from "../app/Inspector/RedactPanel";
import { SearchPanel } from "../sidebar/SearchPanel";
import { Outline } from "../sidebar/Outline";
import { SignatureDialog } from "../dialogs/SignatureDialog";
import { useEditStore } from "./editStore";
import { applyMarks } from "./redact";
import { editHost } from "./index";

let stop: (() => void) | null = null;

const LINES_0 = [
  { text: "담당자 홍길동 (민원 접수)", x: 72, y: 740, size: 12 },
  { text: "주민등록번호 900101-1234567", x: 72, y: 720, size: 12 },
  { text: "이메일 hong@example.co.kr 연락 바람", x: 72, y: 700, size: 12 },
];
const LINES_1 = [
  { text: "결재: 홍길동 과장", x: 72, y: 740, size: 12 },
  { text: "작성일 2024-01-01", x: 72, y: 720, size: 12 },
];

async function openWithText() {
  const info = await useDocStore.getState().open("/tmp/pii.pdf");
  if (!info) throw new Error("mock open failed");
  mockSetPageText(info.docId, 0, LINES_0);
  mockSetPageText(info.docId, 1, LINES_1);
  return info;
}

function confirmTop() {
  const top = useDialogStore.getState().stack.at(-1);
  return top?.name === "confirm" ? top : null;
}

function answer(value: boolean) {
  act(() => (confirmTop()!.props.resolve as (v: boolean) => void)(value));
}

const marks = () => useEditStore.getState().marks;
const toastKeys = () => useToastStore.getState().toasts.map((t) => t.messageKey);

function menuItems(): MenuItem[] {
  return (useContextMenuStore.getState().menu?.items ?? []).filter((i): i is MenuItem => !isSeparator(i));
}

function item(id: string): MenuItem {
  const found = menuItems().find((i) => i.id === id);
  if (!found) throw new Error(`no menu item ${id}: ${menuItems().map((i) => i.id).join(", ")}`);
  return found;
}

async function searchFor(query: string, pageCount: number) {
  render(<SearchPanel />);
  fireEvent.change(screen.getByRole("searchbox"), { target: { value: query } });
  await waitFor(
    () => {
      const s = useSearchStore.getState();
      expect(s.running).toBe(false);
      expect(s.query).toBe(query);
      expect(s.scanned).toBe(pageCount);
    },
    { timeout: 3000 },
  );
}

beforeAll(() => {
  HTMLCanvasElement.prototype.getContext = (() => null) as unknown as HTMLCanvasElement["getContext"];
});

beforeEach(() => {
  useEditStore.getState().reset();
  useToastStore.setState({ toasts: [] });
  useDialogStore.getState().closeAll();
  useContextMenuStore.getState().close();
  useViewStore.setState({ currentPage: 0 });
  useSearchStore.setState({ query: "", hits: [], byPage: new Map(), total: 0, current: -1, running: false });
});

afterEach(() => {
  if (confirmTop()) answer(false);
  stop?.();
  stop = null;
  vi.restoreAllMocks();
});

describe("v0.3 R3 — 검색해서 표시", () => {
  it("검색 ▸ 모든 결과를 영역 표시 marks every '홍길동'; after 적용 the text layer no longer has it", async () => {
    const info = await openWithText();
    useAppStore.setState({ mode: "read" });
    await searchFor("홍길동", info.pageCount);
    expect(useSearchStore.getState().total).toBe(2);

    fireEvent.click(screen.getByRole("button", { name: "모든 결과를 영역 표시" }));
    await waitFor(() => expect(marks()).toHaveLength(2));
    expect(marks().map((m) => m.page)).toEqual([0, 1]);
    expect(toastKeys()).toContain("redact.find.found");
    expect(useAppStore.getState().mode).toBe("edit");
    expect(useAppStore.getState().tool).toBe("redact");
    // each mark sits inside its hit's line box
    const hit0 = useSearchStore.getState().hits[0];
    expect(marks()[0].rect.l).toBeGreaterThanOrEqual(hit0.rects[0].l);
    expect(marks()[0].rect.r).toBeLessThanOrEqual(hit0.rects[0].r);

    stop = editHost.start();
    render(<RedactPanel />);
    expect(screen.getAllByText("홍길동").length).toBeGreaterThanOrEqual(2); // 표시 목록
    const apply = vi.spyOn(mock, "applyRedactionsBatch");
    const run = applyMarks();
    await waitFor(() => expect(confirmTop()).not.toBeNull());
    answer(true);
    await act(async () => {
      expect(await run).toBe(true);
    });
    expect(apply).toHaveBeenCalledTimes(1);
    expect(apply.mock.calls[0][0].marks.map((m) => m.page)).toEqual([0, 1]);
    for (const page of [0, 1]) {
      const text = await api.getPageText({ docId: info.docId, page });
      expect(text).not.toContain("홍길동");
    }
    expect(await api.getPageText({ docId: info.docId, page: 0 })).toContain("담당자");
    expect(await api.getPageText({ docId: info.docId, page: 1 })).toContain("과장");
  });

  it("the panel finds 주민등록번호 + 이메일 on every page, a reviewed entry can be removed before 적용", async () => {
    const info = await openWithText();
    useAppStore.setState({ mode: "edit", tool: "redact" });
    stop = editHost.start();
    render(<RedactPanel />);

    // nothing chosen: the button is disabled
    expect(screen.getByRole("button", { name: "찾아서 표시" })).toBeDisabled();
    fireEvent.click(screen.getByRole("checkbox", { name: "주민등록번호" }));
    fireEvent.click(screen.getByRole("checkbox", { name: "이메일" }));
    fireEvent.click(screen.getByRole("button", { name: "찾아서 표시" }));
    await waitFor(() => expect(screen.getByTestId("redact-find-result")).toHaveTextContent("2개를 찾아 영역 표시했습니다"));
    expect(marks()).toHaveLength(2);
    // the date on page 2 is not a 주민등록번호
    expect(marks().every((m) => m.page === 0)).toBe(true);

    // 표시 목록: remove the e-mail entry
    expect(screen.getByText("900101-1234567")).toBeInTheDocument();
    const emailRow = screen.getByText("hong@example.co.kr").closest("li")!;
    fireEvent.click(emailRow.querySelector("button[aria-label='이 표시 지우기']")!);
    expect(marks()).toHaveLength(1);

    const run = applyMarks();
    await waitFor(() => expect(confirmTop()).not.toBeNull());
    answer(true);
    await act(async () => {
      expect(await run).toBe(true);
    });
    const text = await api.getPageText({ docId: info.docId, page: 0 });
    expect(text).not.toContain("900101-1234567");
    expect(text).toContain("hong@example.co.kr");
  });

  it("scope 현재 쪽 / 쪽 지정, and a bad range is refused", async () => {
    await openWithText();
    useAppStore.setState({ mode: "edit", tool: "redact" });
    stop = editHost.start();
    render(<RedactPanel />);
    fireEvent.change(screen.getByLabelText("검색어"), { target: { value: "홍길동" } });

    useViewStore.setState({ currentPage: 1 });
    fireEvent.click(screen.getByRole("radio", { name: "현재 쪽" }));
    fireEvent.click(screen.getByRole("button", { name: "찾아서 표시" }));
    await waitFor(() => expect(marks()).toHaveLength(1));
    expect(marks()[0].page).toBe(1);

    fireEvent.click(screen.getByRole("radio", { name: "쪽 지정" }));
    fireEvent.change(screen.getByPlaceholderText("예: 1-3, 5"), { target: { value: "99" } });
    fireEvent.click(screen.getByRole("button", { name: "찾아서 표시" }));
    await waitFor(() => expect(screen.getByTestId("redact-find-result")).toHaveTextContent("쪽 범위를 확인하세요"));
    fireEvent.change(screen.getByPlaceholderText("예: 1-3, 5"), { target: { value: "1-2" } });
    fireEvent.click(screen.getByRole("button", { name: "찾아서 표시" }));
    // page 2's mark exists already: only page 1's is new
    await waitFor(() => expect(marks()).toHaveLength(2));
    expect(marks().map((m) => m.page).sort()).toEqual([0, 1]);
  });
});

describe("v0.3 R4 — marks inside a group", () => {
  it("one confirm names the ungroup, and the batch goes with ungroup: true", async () => {
    const info = await useDocStore.getState().open("/tmp/sample.pdf");
    useAppStore.setState({ mode: "edit", tool: "redact" });
    stop = editHost.start();
    render(<RedactPanel />);
    // the mock's first run on every page sits inside a Form XObject
    const list = await api.listPageObjects({ docId: info!.docId, page: 0 });
    const grouped = list.objects.find((o) => o.reason === "insideXObject")!;
    useEditStore.getState().bind(info!.docId);
    useEditStore.getState().addMarks(0, [grouped.rect]);
    await waitFor(() => expect(screen.getByText(/적용할 때 그룹을 해제합니다/)).toBeInTheDocument());

    const apply = vi.spyOn(mock, "applyRedactionsBatch");
    const run = applyMarks();
    await waitFor(() => expect(confirmTop()).not.toBeNull());
    expect(confirmTop()!.props).toMatchObject({ titleKey: "redact.groups.title", danger: true });
    answer(true);
    await act(async () => {
      expect(await run).toBe(true);
    });
    expect(apply.mock.calls[0][0].options).toEqual({ fill: [0, 0, 0], ungroup: true });
  });
});

describe("v0.3 R6 — sidebar context menus", () => {
  it("search result: 이동 · 복사 · 이 결과 형광펜 (the hit's quads) · 영역 표시", async () => {
    const info = await openWithText();
    const writeText = vi.fn(async () => undefined);
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    await searchFor("홍길동", info.pageCount);
    const hit = useSearchStore.getState().hits[1];

    const results = screen.getAllByRole("button", { name: /홍길동/ }).filter((b) => b.classList.contains("search-result"));
    fireEvent.contextMenu(results[1]);
    expect(menuItems().map((i) => i.id)).toEqual(["go", "copy", "highlight", "mark"]);

    item("go").onSelect!();
    expect(useSearchStore.getState().current).toBe(1);

    fireEvent.contextMenu(results[1]);
    item("copy").onSelect!();
    expect(writeText).toHaveBeenCalledWith("홍길동");

    fireEvent.contextMenu(results[1]);
    const create = vi.spyOn(mock, "createAnnotation");
    item("highlight").onSelect!();
    await waitFor(() => expect(create).toHaveBeenCalledTimes(1));
    const call = create.mock.calls[0][0];
    expect(call.page).toBe(hit.page);
    expect(call.spec.kind).toBe("highlight");
    expect((call.spec as { rects: unknown }).rects).toEqual(hit.rects);
    await waitFor(async () => {
      const annots = await api.listAnnotations({ docId: info.docId, page: hit.page });
      const made = annots.annots.find((a) => a.kind === "highlight");
      expect(made?.quads).toEqual(hit.rects);
    });

    fireEvent.contextMenu(results[1]);
    item("mark").onSelect!();
    await waitFor(() => expect(marks()).toHaveLength(1));
    expect(marks()[0].page).toBe(hit.page);
  });

  it("outline item: 이동 · 하위 항목 모두 접기 / 펼치기", async () => {
    await useDocStore.getState().open("/tmp/sample.pdf");
    render(<Outline />);
    const first = await screen.findByText("1. 시작하기");
    const all = screen.getAllByRole("treeitem").length;

    fireEvent.contextMenu(first);
    expect(menuItems().map((i) => i.id)).toEqual(["go", "expandAll", "collapseAll"]);
    act(() => item("collapseAll").onSelect!());
    expect(screen.queryByText("1.1 문서 열기")).toBeNull();
    expect(screen.getAllByRole("treeitem").length).toBe(all - 2);

    fireEvent.contextMenu(screen.getByText("1. 시작하기"));
    act(() => item("expandAll").onSelect!());
    expect(screen.getByText("1.1 문서 열기")).toBeInTheDocument();

    // a leaf has nothing to fold
    fireEvent.contextMenu(screen.getByText("2.1 형광펜"));
    expect(item("expandAll").disabled).toBe(true);
    item("go").onSelect!();
    expect(useViewStore.getState().currentPage).toBe(1);
  });
});

describe("v0.3 I1 — Hangul IME and Enter", () => {
  it("검색: a composing Enter neither runs the search nor steps; a plain Enter does", async () => {
    const info = await openWithText();
    render(<SearchPanel />);
    const field = screen.getByRole("searchbox");
    fireEvent.change(field, { target: { value: "홍길동" } });
    fireEvent.keyDown(field, { key: "Enter", isComposing: true });
    fireEvent.keyDown(field, { key: "Enter", keyCode: 229 });
    expect(useSearchStore.getState().query).toBe(""); // the debounce has not fired, and Enter did nothing
    fireEvent.keyDown(field, { key: "Enter" });
    await waitFor(() => expect(useSearchStore.getState().query).toBe("홍길동"));
    await waitFor(() => expect(useSearchStore.getState().scanned).toBe(info.pageCount), { timeout: 3000 });
    await waitFor(() => expect(useSearchStore.getState().running).toBe(false));

    const current = useSearchStore.getState().current;
    fireEvent.keyDown(field, { key: "Enter", isComposing: true });
    expect(useSearchStore.getState().current).toBe(current);
    fireEvent.keyDown(field, { key: "Enter" });
    expect(useSearchStore.getState().current).toBe((current + 1) % useSearchStore.getState().total);
  });

  it("서명 입력: a composing Enter does not confirm; a plain Enter does", async () => {
    await useAppStore.getState().bootstrap();
    const p = { onClose: vi.fn(), onDrawn: vi.fn(), onImage: vi.fn(), onChooseImage: vi.fn() };
    const typed = await import("../dialogs/typedSignature");
    vi.spyOn(typed, "renderTypedSignature").mockResolvedValue({
      bytes: new Uint8Array([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 1, 2, 3]),
      aspect: 4,
    });
    render(<SignatureDialog {...p} />);
    fireEvent.click(screen.getByRole("tab", { name: "입력" }));
    const input = screen.getByLabelText("이름을 입력하세요");
    fireEvent.change(input, { target: { value: "홍길동" } });
    fireEvent.keyDown(input, { key: "Enter", isComposing: true });
    fireEvent.keyDown(input, { key: "Enter", keyCode: 229 });
    await act(async () => undefined);
    expect(p.onImage).not.toHaveBeenCalled();
    expect(p.onClose).not.toHaveBeenCalled();
    fireEvent.keyDown(input, { key: "Enter" });
    await waitFor(() => expect(p.onImage).toHaveBeenCalledTimes(1));
  });
});
