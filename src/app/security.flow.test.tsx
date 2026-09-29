/**
 * v0.3 pkg3-security-save-integrity — the UI of S1 (signed documents) and S5 (permission flags),
 * against the mock (the engine side: `src-tauri/tests/security.rs`).
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import DialogHost from "../dialogs/DialogHost";
import { useDialogStore } from "../dialogs/dialogState";
import { useDocStore } from "../store/docStore";
import { useAppStore } from "../store/appStore";
import { useToastStore } from "./toastStore";
import { mock } from "../ipc/mock";
import * as api from "../ipc/api";
import { DocBadges } from "./DocBadges";
import { ModeSwitcher } from "./ModeSwitcher";
import { installSignatureGate, resetSignatureGate } from "./signatureGate";
import { permissionBlock, restrictions } from "./permissions";
import { copyToClipboard } from "../viewer/viewerCommands";
import { formatPdfDate } from "./BadgePopover";

const SIGNED = "/Users/veri/Documents/signed-계약서.pdf";
const RESTRICTED = "/Users/veri/Documents/restricted-보고서.pdf";

function toastKeys(): string[] {
  return useToastStore.getState().toasts.map((t) => t.messageKey);
}

const highlight = (docId: string) =>
  api.createAnnotation({
    docId,
    page: 0,
    spec: { kind: "highlight", rects: [{ l: 10, b: 10, r: 100, t: 20 }], color: [255, 230, 0], opacity: 0.5 },
  });

beforeEach(() => {
  useDocStore.setState({ docId: null, info: null, outline: [], status: "empty", error: null });
  useDialogStore.getState().closeAll();
  useToastStore.setState({ toasts: [] });
  useAppStore.setState({ mode: "read", tool: "select" });
  resetSignatureGate();
  installSignatureGate();
});

afterEach(() => {
  api.setMutationGate(null);
});

describe("S1 — a signed document", () => {
  it("shows 서명됨 (1) with the signature, the not-validated note and the save mode", async () => {
    await useDocStore.getState().open(SIGNED);
    render(<DocBadges />);
    fireEvent.click(screen.getByRole("button", { name: "서명됨 (1)" }));
    const pop = await screen.findByRole("dialog", { name: "디지털 서명" });
    expect(within(pop).getByText("서명1")).toBeInTheDocument();
    expect(within(pop).getByText("사유: 계약 승인")).toBeInTheDocument();
    expect(within(pop).getByText("서명 시각: 2026-09-01 12:00")).toBeInTheDocument();
    expect(within(pop).getByText(/유효성.*검증하지 않았습니다/)).toBeInTheDocument();
    expect(within(pop).getByText(/기존 서명이 서명한 내용은 그대로 남습니다/)).toBeInTheDocument();
    expect(formatPdfDate("D:2026")).toBe("2026-01-01");
    expect(formatPdfDate("garbage")).toBe("garbage");
  });

  it("the first edit asks once; 편집 계속 lets every later edit through", async () => {
    const info = (await useDocStore.getState().open(SIGNED))!;
    render(<DialogHost />);
    const create = vi.spyOn(mock, "createAnnotation");
    const first = highlight(info.docId);
    const second = highlight(info.docId); // a burst: still one prompt
    await screen.findByText(/편집하면 서명이 무효화될 수 있습니다/);
    expect(create).not.toHaveBeenCalled();
    expect(useDialogStore.getState().stack.filter((e) => e.name === "confirm")).toHaveLength(1);
    fireEvent.click(screen.getByRole("button", { name: "편집 계속" }));
    await first;
    await second;
    expect(create).toHaveBeenCalledTimes(2);
    await highlight(info.docId);
    expect(create).toHaveBeenCalledTimes(3);
    expect(useDialogStore.getState().stack).toHaveLength(0);
  });

  it("취소 rejects the edit quietly (no error toast) and asks again next time", async () => {
    const info = (await useDocStore.getState().open(SIGNED))!;
    render(<DialogHost />);
    const create = vi.spyOn(mock, "createAnnotation");
    const attempt = highlight(info.docId).catch((e: unknown) => e);
    await screen.findByText(/편집하면 서명이 무효화될 수 있습니다/);
    fireEvent.click(screen.getByRole("button", { name: "취소" }));
    const err = await attempt;
    expect(api.isSeePdfError(err) && err.code).toBe("cancelled");
    expect(create).not.toHaveBeenCalled();
    // what every caller does with a failed edit: nothing reaches the screen
    const { toast } = await import("./toastStore");
    toast("error.generic", undefined, { tone: "danger", detail: (err as Error).message });
    expect(toastKeys()).not.toContain("error.generic");
    const again = highlight(info.docId).catch(() => undefined);
    await screen.findByText(/편집하면 서명이 무효화될 수 있습니다/);
    fireEvent.click(screen.getByRole("button", { name: "편집 계속" }));
    await again;
    expect(create).toHaveBeenCalledTimes(1);
  });

  // Verification round 2: 자르기 sends one struct argument (`{ args }`), and 페이지 추출 changes the
  // source only with 원본에서 삭제 — both must still ask.
  it("자르기 (set_page_boxes, nested args) asks before it runs", async () => {
    const info = (await useDocStore.getState().open(SIGNED))!;
    render(<DialogHost />);
    const boxes = vi.spyOn(mock, "setPageBoxes");
    const attempt = api
      .setPageBoxes({ docId: info.docId, pages: [0], crop: { margins: { top: 10, right: 10, bottom: 10, left: 10 } } })
      .catch((e: unknown) => e);
    await screen.findByText(/편집하면 서명이 무효화될 수 있습니다/);
    expect(boxes).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "취소" }));
    const err = await attempt;
    expect(api.isSeePdfError(err) && err.code).toBe("cancelled");
    expect(boxes).not.toHaveBeenCalled();
  });

  it("페이지 추출 asks only with 원본에서 삭제", async () => {
    const info = (await useDocStore.getState().open(SIGNED))!;
    render(<DialogHost />);
    const extract = vi.spyOn(mock, "extractPages");
    await api.extractPages({ docId: info.docId, pages: [0], outPath: "/tmp/copy.pdf", removeAfter: false });
    expect(extract).toHaveBeenCalledTimes(1);
    expect(useDialogStore.getState().stack).toHaveLength(0);
    const moving = api.extractPages({ docId: info.docId, pages: [0], outPath: "/tmp/moved.pdf", removeAfter: true });
    await screen.findByText(/편집하면 서명이 무효화될 수 있습니다/);
    expect(extract).toHaveBeenCalledTimes(1);
    fireEvent.click(screen.getByRole("button", { name: "편집 계속" }));
    await moving;
    expect(extract).toHaveBeenCalledTimes(2);
  });

  it("mutatingCall reads nested args and conditional commands", () => {
    expect(api.mutatingCall("set_page_boxes", { args: { docId: "d1" } })).toEqual({ docId: "d1" });
    expect(api.mutatingCall("create_annotation", { docId: "d1", page: 0 })).toMatchObject({ docId: "d1" });
    expect(api.mutatingCall("extract_pages", { docId: "d1", removeAfter: false })).toBeNull();
    expect(api.mutatingCall("extract_pages", { docId: "d1", removeAfter: true })).toMatchObject({ docId: "d1" });
    expect(api.mutatingCall("get_document_info", { docId: "d1" })).toBeNull();
  });

  it("an unsigned document never asks", async () => {
    const info = (await useDocStore.getState().open("/Users/veri/Documents/SeePDF-샘플.pdf"))!;
    await highlight(info.docId);
    expect(useDialogStore.getState().stack).toHaveLength(0);
  });
});

describe("S5 — permission flags", () => {
  it("the 제한됨 badge lists the restrictions from DocInfo.permissions", async () => {
    const info = (await useDocStore.getState().open(RESTRICTED))!;
    expect(restrictions(info)).toEqual(["print", "extractText", "modify", "assemble"]);
    render(<DocBadges />);
    fireEvent.click(screen.getByRole("button", { name: "제한됨" }));
    const pop = await screen.findByRole("dialog", { name: "이 문서에 설정된 제한" });
    for (const line of ["인쇄할 수 없음", "내용을 복사할 수 없음", "내용을 수정할 수 없음", "페이지를 추가, 삭제, 회전할 수 없음"]) {
      expect(within(pop).getByText(line)).toBeInTheDocument();
    }
    expect(within(pop).queryByText("주석을 추가할 수 없음")).toBeNull();
  });

  it("권한 암호로 잠금 해제 retries a wrong password, then reopens with full rights and the same docId", async () => {
    const info = (await useDocStore.getState().open(RESTRICTED))!;
    const unlock = vi.spyOn(mock, "unlockDocument");
    render(
      <>
        <DocBadges />
        <DialogHost />
      </>,
    );
    fireEvent.click(screen.getByRole("button", { name: "제한됨" }));
    fireEvent.click(await screen.findByRole("button", { name: "권한 암호로 잠금 해제…" }));
    const input = await screen.findByLabelText("암호를 입력하세요");
    fireEvent.change(input, { target: { value: "nope" } });
    fireEvent.click(screen.getByRole("button", { name: "확인" }));
    await screen.findByText("암호가 올바르지 않습니다");
    fireEvent.change(screen.getByLabelText("암호를 입력하세요"), { target: { value: "owner" } });
    fireEvent.click(screen.getByRole("button", { name: "확인" }));
    await waitFor(() => expect(useDocStore.getState().info?.permissions.print).toBe(true));
    expect(unlock).toHaveBeenLastCalledWith({ docId: info.docId, password: "owner" });
    expect(useDocStore.getState().docId).toBe(info.docId);
    expect(screen.queryByRole("button", { name: "제한됨" })).toBeNull();
    expect(toastKeys()).toContain("security.restricted.unlocked");
  });

  it("편집 and 페이지 are disabled with the reason; 인쇄 and 복사 say why instead of running", async () => {
    const info = (await useDocStore.getState().open(RESTRICTED))!;
    render(<ModeSwitcher run={() => undefined} />);
    expect(screen.getByRole("tab", { name: "편집" })).toBeDisabled();
    expect(screen.getByRole("tab", { name: "페이지" })).toBeDisabled();
    expect(screen.getByRole("tab", { name: "주석" })).toBeEnabled();
    expect(permissionBlock("mode.edit", info)).toBe("security.restricted.reason.modify");
    expect(permissionBlock("file.print", info)).toBe("security.restricted.reason.print");
    expect(permissionBlock("mode.annotate", info)).toBeNull();

    const { useCommands } = await import("./useCommands");
    let run: ((id: string) => void) | null = null;
    function Probe() {
      run = useCommands();
      return null;
    }
    render(<Probe />);
    act(() => run!("file.print"));
    expect(useDialogStore.getState().stack).toHaveLength(0);
    expect(toastKeys()).toContain("security.restricted.reason.print");
    act(() => run!("mode.edit"));
    expect(useAppStore.getState().mode).toBe("read");

    expect(copyToClipboard("비밀")).toBe(false);
    expect(toastKeys()).toContain("security.restricted.reason.extractText");
    // and the engine refuses the edits anyway
    await expect(api.pageOps({ docId: info.docId, ops: [{ kind: "rotate", pages: [0], delta: 90 }] }))
      .rejects.toMatchObject({ code: "permissionDenied", detail: "assemble" });
  });

  // verification round 1: entry points that changed mode without asking
  it("도구 › 영역 표시 says why instead of entering 편집", async () => {
    await useDocStore.getState().open(RESTRICTED);
    const { useCommands } = await import("./useCommands");
    let run: ((id: string) => void) | null = null;
    function Probe() {
      run = useCommands();
      return null;
    }
    render(<Probe />);
    act(() => run!("tools.redact"));
    expect(useAppStore.getState().mode).toBe("read");
    expect(useAppStore.getState().tool).not.toBe("redact");
    expect(toastKeys()).toContain("security.restricted.reason.modify");
  });

  it("the canvas menu's 페이지 정리 is disabled with the reason", async () => {
    await useDocStore.getState().open(RESTRICTED);
    const { openPageContextMenu } = await import("./pageMenus");
    const { useContextMenuStore } = await import("./contextMenuStore");
    openPageContextMenu(0, "canvas", 10, 10);
    const organize = useContextMenuStore.getState().menu?.items.find((i) => i.id === "organize");
    expect(organize).toMatchObject({ disabled: true, hintKey: "security.restricted.reason.assemble" });
    useContextMenuStore.getState().close();
  });

  it("a mode the document forbids never stays active", async () => {
    // kept from the previous document
    useAppStore.setState({ mode: "edit" });
    await useDocStore.getState().open(RESTRICTED);
    expect(useAppStore.getState().mode).toBe("read");
    expect(toastKeys()).toContain("security.restricted.reason.modify");
    // set by a caller that did not ask: back to the previous (allowed) mode
    act(() => useAppStore.getState().setMode("annotate"));
    act(() => useAppStore.getState().setMode("pages"));
    expect(useAppStore.getState().mode).toBe("annotate");
    expect(toastKeys()).toContain("security.restricted.reason.assemble");
  });

  it("an unrestricted document shows no badge and blocks nothing", async () => {
    const info = (await useDocStore.getState().open("/Users/veri/Documents/SeePDF-샘플.pdf"))!;
    const { container } = render(<DocBadges />);
    expect(container).toBeEmptyDOMElement();
    expect(permissionBlock("file.print", info)).toBeNull();
    expect(restrictions(info)).toEqual([]);
  });
});
