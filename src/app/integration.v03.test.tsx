/**
 * v0.3 integration: behaviour that only exists once two work packages meet.
 *
 * - pkg5 V7 (the 읽기 status-bar tool selector) × pkg6 V1 (스냅샷): 스냅샷 is a third latchable tool there.
 * - pkg5 U4 (the native window theme + `theme-changed`) × pkg6 U3 (기본값으로 되돌리기): resetting the
 *   settings also resets every window's chrome, not only this window's CSS.
 * - pkg3 S5 (permission flags) × pkg6 V1 / V2 / caret, × pkg1 R4: a document that forbids copying
 *   forbids 스냅샷 (every entry point); one that forbids comments disables the canvas menu's 주석
 *   entries and caret N / H / U / K; 그룹 해제 asks first on a signed document (S1's gate).
 * - pkg3 S1 / S5 × pkg2 F1 / F2 / P3: the form-authoring commands and pages dragged in from another
 *   window go through the signed-document gate (the latter on its target); 필드 만들기 / 양식
 *   평면화 are off on a document that forbids changes; pages of a restricted document cannot be
 *   dragged into another one.
 * - pkg3 H8 × pkg2 D1: a PDF dropped together with images that another window already shows is
 *   focused there, not opened a second time.
 * - pkg3 S5 × pkg7 O6 / O2: 텍스트 인식 writes a text layer (a modification), so on a document that
 *   forbids changes the 스캔 문서 banner's and the 검색 hint's 'OCR 실행…', the ⋯ menu item and the
 *   command are off with the reason, and the OCR sheet cannot start; 페이지 회전 자동 감지 also turns
 *   pages, so it is off where page assembly is forbidden. The mock refuses a turning OCR as
 *   "modify" first, like the engine (whose structural guard alone would only check "assemble").
 * - pkg3 S1 × pkg4 A6 / A3 / A4: 맨 앞으로 / 맨 뒤로 and the one-gesture annotation batches (partial
 *   eraser, pressure pen) go through the signed-document gate.
 * - pkg6 V2 (canvas menu 이 스타일을 기본값으로) × pkg4 A2: a polygon / polyline keeps its stroke width and
 *   a polygon its fill, and a polyline's default belongs to the 다각형 tool.
 * - pkg3 S5 × pkg8 X3 / X6 / X2: 텍스트 (레이아웃 유지), 이미지 추출 and Word / 한글 / HTML / Markdown carry
 *   the document's content out, so a document that forbids copying cannot export them (the button is
 *   off and the reason shows, and the mock refuses like the engine); 모아찍기 PDF is built from the
 *   print bytes and needs the print permission. Page images and the flattened PDF stay available.
 * - pkg4 T4 (배경색 도장) × pkg8 X1: the 여러 파일 처리 stamp summary names a background stamp.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import App from "../App";
import { useAppStore } from "../store/appStore";
import { useDocStore } from "../store/docStore";
import { useDialogStore } from "../dialogs/dialogState";
import { useToastStore } from "./toastStore";
import { useContextMenuStore } from "./contextMenuStore";
import { setWindowThemeBackend } from "./windowTheme";
import { resetToDefaults } from "../dialogs/SettingsDialog";
import { copySnapshot, toolAfterSnapshot } from "../tools/snapshot";
import { mutatingCall } from "../ipc/api";
import { permissionBlock } from "./permissions";
import { styleOfAnnot } from "./pageMenus";
import { toolOfKind } from "../store/toolStyles";
import type { Annot } from "../ipc/types";
import { onCaretKey } from "../viewer/text/caretKeys";
import { useCaretStore } from "../viewer/text/caret";
import { mock, mockEvents, mockOpenInOtherWindow } from "../ipc/mock";
import * as api from "../ipc/api";
import { t as tr } from "../i18n";
import { NeedsOcrBanner, NeedsOcrSearchHint } from "../ocr/NeedsOcrBanner";
import { resetNeedsOcr } from "../ocr/needsOcr";
import { OcrDialog } from "../ocr/OcrDialog";
import { closeOcrDialog, openOcrDialog, useOcrDialogStore } from "../ocr/dialogState";
import { ExportDialog, exportNeeds } from "../dialogs/ExportDialog";
import { stampSummary } from "../batch/BatchDialog";
import type { StampSpec } from "../ipc/types";

const SAMPLE = "/Users/veri/Documents/SeePDF-샘플.pdf";
/** pkg3's mock fixture: print, copy, modify and assemble are forbidden; comments are allowed. */
const RESTRICTED = "/Users/veri/Documents/restricted-보고서.pdf";

function toastKeys(): string[] {
  return useToastStore.getState().toasts.map((t) => t.messageKey);
}

beforeEach(() => {
  useDialogStore.getState().closeAll();
  useToastStore.setState({ toasts: [] });
  useContextMenuStore.setState({ menu: null });
  useDocStore.setState({ docId: null, info: null, outline: [], status: "empty", error: null });
  useAppStore.setState({ os: "macos", mode: "read", tool: "select", momentaryFrom: null, theme: "system", appInfo: null });
});

afterEach(() => {
  setWindowThemeBackend(null);
  vi.restoreAllMocks();
});

describe("V7 × V1: 스냅샷 in the 읽기 tool selector", () => {
  it("the status bar offers 선택 / 손 도구 / 스냅샷, and 스냅샷 arms the snapshot tool", async () => {
    render(<App />);
    await useDocStore.getState().open(SAMPLE);
    const group = await screen.findByRole("group", { name: "도구" });
    const snap = await screen.findByRole("button", { name: "스냅샷" });
    expect(group.contains(snap)).toBe(true);
    expect(group.contains(screen.getByRole("button", { name: "손 도구" }))).toBe(true);
    fireEvent.click(snap);
    expect(useAppStore.getState().tool).toBe("snapshot");
    expect(snap).toHaveAttribute("aria-pressed", "true");
    // one-shot: after the marquee the pointer goes back to 선택 (in 읽기), not to 손
    expect(toolAfterSnapshot()).toBe("select");
  });
});

describe("U4 × U3: 기본값으로 되돌리기 resets the window chrome too", () => {
  it("resetting from 어둡게 sets the native window theme back to the system and tells the other windows", async () => {
    const setTheme = vi.fn(async () => undefined);
    setWindowThemeBackend(setTheme);
    useAppStore.getState().setTheme("dark");
    await waitFor(() => expect(setTheme).toHaveBeenLastCalledWith("dark"));
    const broadcast: unknown[] = [];
    const off = mockEvents.on("theme-changed", (p) => broadcast.push(p));
    try {
      const done = resetToDefaults();
      const prompt = await waitFor(() => {
        const entry = useDialogStore.getState().stack.find((e) => e.name === "confirm");
        expect(entry).toBeDefined();
        return entry!;
      });
      await act(async () => (prompt.props.resolve as (v: boolean) => void)(true));
      expect(await done).toBe(true);
    } finally {
      off();
    }
    expect(useAppStore.getState().theme).toBe("system");
    await waitFor(() => expect(setTheme).toHaveBeenLastCalledWith(null));
    expect(broadcast).toContainEqual({ theme: "system" });
  });
});

describe("S5 × V1: a document that forbids copying forbids 스냅샷", () => {
  it("⌥⌘C, the status-bar segment, the canvas menu and the copy itself all refuse with the reason", async () => {
    render(<App />);
    const info = (await useDocStore.getState().open(RESTRICTED))!;
    const reason = "security.restricted.reason.extractText";
    expect(permissionBlock("tool.snapshot", info)).toBe(reason);

    // the status bar's 스냅샷 segment is disabled, 선택 / 손 도구 are not
    const snap = await screen.findByRole("button", { name: "스냅샷" });
    expect(snap).toBeDisabled();
    expect(screen.getByRole("button", { name: "손 도구" })).toBeEnabled();

    // the shortcut goes through the dispatcher: a toast, and the tool stays 선택
    const { runCommand } = await import("./useCommands");
    act(() => runCommand("tool.snapshot"));
    expect(useAppStore.getState().tool).toBe("select");
    expect(toastKeys()).toContain(reason);

    // the canvas menu's 스냅샷 is disabled with the reason
    const { openPageContextMenu } = await import("./pageMenus");
    openPageContextMenu(0, "canvas", 10, 10);
    const item = useContextMenuStore.getState().menu?.items.find((i) => i.id === "snapshot");
    expect(item).toMatchObject({ disabled: true, hintKey: reason });
    useContextMenuStore.getState().close();

    // and a marquee that was armed before (a tool kept from another document) copies nothing
    useToastStore.setState({ toasts: [] });
    const write = vi.fn();
    vi.stubGlobal("navigator", { ...navigator, clipboard: { write } });
    try {
      const ok = await copySnapshot({
        docId: info.docId, page: 0, rect: { l: 0, b: 0, r: 100, t: 100 }, zoomPercent: 100, rotation: 0, dpr: 1,
      });
      expect(ok).toBe(false);
      expect(write).not.toHaveBeenCalled();
      expect(toastKeys()).toContain(reason);
    } finally {
      vi.unstubAllGlobals();
    }
  });

  it("an unrestricted document keeps 스냅샷 everywhere", async () => {
    const info = (await useDocStore.getState().open(SAMPLE))!;
    expect(permissionBlock("tool.snapshot", info)).toBeNull();
    const { openPageContextMenu } = await import("./pageMenus");
    openPageContextMenu(0, "canvas", 10, 10);
    const item = useContextMenuStore.getState().menu?.items.find((i) => i.id === "snapshot");
    expect(item && "disabled" in item ? item.disabled : false).toBeFalsy();
    useContextMenuStore.getState().close();
  });
});

describe("S5 × caret browsing: N / H / U / K make comments", () => {
  it("on a document that forbids comments, N says why and adds no note", async () => {
    const opened = (await useDocStore.getState().open(SAMPLE))!;
    // the same document, with comments forbidden (pkg3's flags as DocInfo carries them)
    const info = { ...opened, encrypted: true, permissions: { ...opened.permissions, annotate: false } };
    useDocStore.setState({ info });
    useCaretStore.getState().set(true, { docId: info.docId, page: 0, offset: 0 });
    const create = vi.spyOn(await import("../annot/actions"), "createAnnotation");
    try {
      const e = new KeyboardEvent("keydown", { key: "n", code: "KeyN", bubbles: true, cancelable: true });
      document.body.dispatchEvent(e);
      onCaretKey(e);
      expect(e.defaultPrevented).toBe(true);
      expect(toastKeys()).toContain("security.restricted.reason.annotate");
      // addNote imports annot/actions lazily: give that a moment before checking nothing was made
      await new Promise((r) => setTimeout(r, 50));
      expect(create).not.toHaveBeenCalled();
      expect(useAppStore.getState().mode).toBe("read");
    } finally {
      useCaretStore.getState().set(false);
    }
  });
});

describe("S1 × R4: 그룹 해제 is an edit", () => {
  it("ungroup_object goes through the signed-document gate", () => {
    expect(mutatingCall("ungroup_object", { docId: "d1", page: 0, objectId: 3 })).toMatchObject({ docId: "d1" });
  });
});

describe("S1 / S5 × pkg2: form authoring and pages from another window", () => {
  it("the new form and page-import commands go through the signed-document gate", () => {
    for (const command of ["create_form_field", "update_form_field", "delete_form_field", "import_form_data", "flatten_form"]) {
      expect(mutatingCall(command, { docId: "d1", page: 0, index: 0 })).toMatchObject({ docId: "d1" });
    }
    // exporting reads the form only
    expect(mutatingCall("export_form_data", { docId: "d1", format: "csv", outPath: "/x.csv" })).toBeNull();
    // pages dragged in from another window change the *target*, which is what the gate asks about
    expect(mutatingCall("import_pages_from_doc", { srcDocId: "src", pages: [0], dstDocId: "dst", at: 0 })).toMatchObject({
      docId: "dst",
    });
  });

  it("필드 만들기 and 양식 평면화 are off, with the reason, when the document forbids changes", async () => {
    render(<App />);
    const info = (await useDocStore.getState().open(RESTRICTED))!;
    // the restricted fixture still allows filling forms, so 양식 mode itself is open
    expect(permissionBlock("mode.form", info)).toBeNull();
    act(() => useAppStore.getState().setMode("form"));
    const reason = "security.restricted.reason.modify";
    for (const name of ["텍스트 필드", "확인란", "라디오 단추", "목록 상자", "서명 필드", "양식 평면화"]) {
      expect(await screen.findByRole("button", { name })).toBeDisabled();
    }
    expect(permissionBlock("tool.fieldText", info)).toBe(reason);
    // filling and the form data stay available
    expect(screen.getByRole("button", { name: "양식 데이터 가져오기…" })).toBeEnabled();
    expect(screen.getByRole("button", { name: "양식 데이터 내보내기…" })).toBeEnabled();
  });

  it("an unrestricted document keeps 필드 만들기", async () => {
    render(<App />);
    await useDocStore.getState().open(SAMPLE);
    act(() => useAppStore.getState().setMode("form"));
    expect(await screen.findByRole("button", { name: "텍스트 필드" })).toBeEnabled();
    expect(screen.getByRole("button", { name: "양식 평면화" })).toBeEnabled();
  });

  it("pages of a restricted document cannot be dragged into another one", async () => {
    const src = await api.openDocument({ path: RESTRICTED });
    const dst = await api.openDocument({ path: SAMPLE });
    await expect(api.importPagesFromDoc({ srcDocId: src.docId, pages: [0], dstDocId: dst.docId, at: 0 })).rejects.toMatchObject({
      code: "permissionDenied",
      detail: "security",
    });
    const ok = await api.openDocument({ path: "/Users/veri/Documents/other.pdf" });
    const info = await api.importPagesFromDoc({ srcDocId: ok.docId, pages: [0], dstDocId: dst.docId, at: 0 });
    expect(info.pageCount).toBe(dst.pageCount + 1);
  });
});

describe("H8 × D1: a PDF dropped with images", () => {
  it("is focused in the window that already shows it instead of opening twice", async () => {
    const newWindow = vi.spyOn(mock, "openInNewWindow");
    mockOpenInOtherWindow("/p/already-open.pdf", "doc-7");
    const { routeDroppedPaths } = await import("../dialogs/imagesFlow");
    await routeDroppedPaths(["/p/already-open.pdf", "/p/new.pdf", "/p/scan.png"]);
    expect(useDialogStore.getState().stack.at(-1)?.name).toBe("imagesToPdf");
    expect(newWindow).toHaveBeenCalledTimes(1);
    expect(newWindow).toHaveBeenCalledWith({ path: "/p/new.pdf" });
    expect(useToastStore.getState().toasts.at(-1)?.params).toEqual({ count: 1 });
  });
});

describe("S5 × pkg7: OCR on a document that forbids changes", () => {
  function scanPages() {
    return vi.spyOn(mock, "ocrPageStatus").mockImplementation(async (a) =>
      a.pages.map((page) => ({ page, hasText: false, charCount: 0 })));
  }

  beforeEach(() => {
    resetNeedsOcr();
    closeOcrDialog();
  });

  it("the banner and the 검색 hint keep saying why, with 'OCR 실행…' off; the command refuses", async () => {
    scanPages();
    const info = await useDocStore.getState().open(RESTRICTED);
    const reason = tr("security.restricted.reason.modify");
    expect(permissionBlock("tools.ocr", info)).toBe("security.restricted.reason.modify");
    render(
      <>
        <NeedsOcrBanner />
        <NeedsOcrSearchHint docId={info!.docId} />
      </>,
    );
    const banner = await screen.findByTestId("needs-ocr-banner", {}, { timeout: 4000 });
    const runs = screen.getAllByRole("button", { name: tr("ocr.needsOcr.run") });
    expect(runs).toHaveLength(2);
    for (const run of runs) {
      expect(run).toBeDisabled();
      expect(run).toHaveAttribute("title", reason);
    }
    expect(banner).toHaveTextContent(tr("ocr.needsOcr.banner"));

    const { runCommand } = await import("./useCommands");
    act(() => runCommand("tools.ocr"));
    expect(toastKeys()).toContain("security.restricted.reason.modify");
    await new Promise((r) => setTimeout(r, 0));
    expect(useOcrDialogStore.getState().open).toBe(false);
  });

  it("an unrestricted scan keeps 'OCR 실행…'", async () => {
    scanPages();
    const info = await useDocStore.getState().open(SAMPLE);
    expect(permissionBlock("tools.ocr", info)).toBeNull();
    render(<NeedsOcrBanner />);
    await screen.findByTestId("needs-ocr-banner", {}, { timeout: 4000 });
    const run = screen.getByRole("button", { name: tr("ocr.needsOcr.run") });
    expect(run).toBeEnabled();
    fireEvent.click(run);
    expect(useOcrDialogStore.getState().open).toBe(true);
  });

  it("the OCR sheet cannot start, and 페이지 회전 자동 감지 is off, on that document", async () => {
    await useDocStore.getState().open(RESTRICTED);
    render(<OcrDialog />);
    act(() => openOcrDialog());
    expect(screen.getByRole("button", { name: tr("ocr.start") })).toBeDisabled();
    expect(screen.getByText(tr("security.restricted.reason.modify"))).toBeInTheDocument();
    const rotate = screen.getByLabelText(tr("ocr.option.autoRotate"));
    expect(rotate).toBeDisabled();
    expect(rotate).not.toBeChecked();
  });

  it("the mock refuses a turning OCR as 'modify', as the engine does", async () => {
    const info = (await useDocStore.getState().open(RESTRICTED))!;
    const ocr = { page: 0, dpi: 150, widthPx: 100, heightPx: 100, rotation: 90 as const, lines: [] };
    await expect(
      mock.ocrApply({ docId: info.docId, pages: [{ page: 0, ocr, setRotation: 90 }], replaceExisting: false }, () => {}),
    ).rejects.toMatchObject({ code: "permissionDenied", detail: "modify" });
  });
});

describe("S1 × pkg4: 맨 앞으로 / 맨 뒤로 and annotation batches are edits", () => {
  it("restack_objects and annotation_batch go through the signed-document gate", () => {
    expect(mutatingCall("restack_objects", { docId: "d1", page: 0, objectIds: [1], expectGeneration: 1, toFront: true })).toMatchObject({ docId: "d1" });
    expect(mutatingCall("annotation_batch", { docId: "d1", page: 0, ops: [] })).toMatchObject({ docId: "d1" });
    // the read-only pkg4 commands are not gated
    expect(mutatingCall("image_preview", { path: "/x.png" })).toBeNull();
    expect(mutatingCall("copy_library_image", { path: "/x.png", library: "stamp" })).toBeNull();
  });
});

describe("V2 × pkg4 A2: 이 스타일을 기본값으로 on the new kinds", () => {
  const base = { id: "a", page: 0, rect: { l: 0, b: 0, r: 10, t: 10 }, color: [1, 2, 3], opacity: 1, borderWidth: 3 } as unknown as Annot;
  it("a polygon keeps its width and fill, a polyline its width (the 다각형 tool's default)", () => {
    expect(styleOfAnnot({ ...base, kind: "polygon", fillColor: [9, 9, 9] } as Annot)).toMatchObject({ width: 3, fillColor: [9, 9, 9] });
    expect(styleOfAnnot({ ...base, kind: "polyline" } as Annot)).toMatchObject({ width: 3 });
    expect(styleOfAnnot({ ...base, kind: "polyline" } as Annot)).not.toHaveProperty("fillColor");
    expect(toolOfKind("polyline")).toBe("polygon");
    expect(toolOfKind("polygon")).toBe("polygon");
    expect(toolOfKind("callout")).toBe("callout");
  });
});

describe("S5 × pkg8: exports that carry the content out need the document's permission", () => {
  it("maps each format to the permission the engine checks", () => {
    for (const f of ["text", "embedded", "docx", "hwpx", "html", "md"] as const) expect(exportNeeds(f)).toBe("extractText");
    expect(exportNeeds("nup")).toBe("print");
    for (const f of ["png", "jpeg", "tiff", "pdfFlattened", "annotations"] as const) expect(exportNeeds(f)).toBeNull();
  });

  it("on a document that forbids copying and printing, those formats are off with the reason", async () => {
    await useDocStore.getState().open(RESTRICTED);
    render(<ExportDialog onClose={() => {}} />);
    const button = () => screen.getByRole("button", { name: tr("export.button") });
    const estimate = () => screen.getByTestId("export-estimate");
    for (const [label, reason] of [
      [tr("export.format.docx"), "security.restricted.reason.extractText"],
      [tr("export.format.embedded"), "security.restricted.reason.extractText"],
      [tr("export.format.text"), "security.restricted.reason.extractText"],
      [tr("export.format.nup"), "security.restricted.reason.print"],
    ] as const) {
      fireEvent.click(screen.getByRole("option", { name: label }));
      expect(button()).toBeDisabled();
      expect(estimate()).toHaveTextContent(tr(reason));
    }
    // a page image is still allowed (like export_images)
    fireEvent.click(screen.getByRole("option", { name: tr("export.format.png") }));
    expect(button()).toBeEnabled();
    expect(estimate()).not.toHaveTextContent(tr("security.restricted.reason.extractText"));
  });

  it("an unrestricted document exports them", async () => {
    await useDocStore.getState().open(SAMPLE);
    render(<ExportDialog onClose={() => {}} />);
    fireEvent.click(screen.getByRole("option", { name: tr("export.format.hwpx") }));
    expect(screen.getByRole("button", { name: tr("export.button") })).toBeEnabled();
  });

  it("the mock refuses them as the engine does", async () => {
    const info = (await useDocStore.getState().open(RESTRICTED))!;
    const d = info.docId;
    await expect(mock.exportText({ docId: d, pages: [0], outPath: "/o.txt", preserveLayout: true }))
      .rejects.toMatchObject({ code: "permissionDenied", detail: "extractText" });
    await expect(mock.exportTextFlow({ docId: d, pages: [0], format: "docx", outPath: "/o.docx" }, () => {}))
      .rejects.toMatchObject({ code: "permissionDenied", detail: "extractText" });
    await expect(mock.exportEmbeddedImages({ docId: d, pages: [0], outDir: "/o", baseName: "x" }, () => {}))
      .rejects.toMatchObject({ code: "permissionDenied", detail: "extractText" });
    await expect(mock.makeNup({ docId: d, options: { perSheet: 2 } }))
      .rejects.toMatchObject({ code: "permissionDenied", detail: "print" });
  });
});

describe("T4 × X1: 여러 파일 처리 with a background stamp", () => {
  it("the summary names the 배경색 source (it has no text or file)", () => {
    const spec = {
      role: "watermark", source: { kind: "background", color: [255, 250, 230] },
      anchor: "center", marginPt: 0, rotateDeg: 0, opacity: 1, pages: "all",
    } as unknown as StampSpec;
    expect(stampSummary(spec, tr)).toBe(`${tr("stamp.role.watermark")} · ${tr("stamp.source.background")}`);
    const text = { ...spec, source: { kind: "text", text: "대외비", fontSizePt: 48, color: [0, 0, 0] } } as StampSpec;
    expect(stampSummary(text, tr)).toBe(`${tr("stamp.role.watermark")} · 대외비`);
  });
});
