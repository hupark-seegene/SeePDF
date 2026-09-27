import { describe, expect, it } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { historyTitle } from "./historyLabel";
import { TitleBar } from "./TitleBar";
import { catalogues, setLocale } from "../i18n";
import { useDocStore } from "../store/docStore";
import * as api from "../ipc/api";

const SAMPLE = "/Users/veri/Documents/SeePDF-샘플.pdf";

/** Every key the engine sends as `undoLabel` / `redoLabel` (`Mutation::new("undo.*")`). */
const ENGINE_KEYS = [
  "undo.annotCreate", "undo.annotDelete", "undo.annotEdit", "undo.formFill", "undo.formReset",
  "undo.metadataEdit", "undo.metadataRemove", "undo.objectAdd", "undo.objectDelete", "undo.objectEdit",
  "undo.objectTransform", "undo.ocrApply", "undo.redact", "undo.watermark", "undo.headerFooter", "undo.compress",
  "undo.pageOps", "undo.pageMove", "undo.pageDelete", "undo.pageRotate", "undo.pageInsert", "undo.pageDuplicate",
  "undo.pageInsertFrom", "undo.pageReverse",
];

describe("undo labels", () => {
  it("every engine key exists in both locales", () => {
    for (const key of ENGINE_KEYS) {
      expect(catalogues.ko[key], `ko ${key}`).toBeTruthy();
      expect(catalogues.en[key], `en ${key}`).toBeTruthy();
    }
    expect(catalogues.ko["undo.watermark"]).toBe("워터마크");
    expect(catalogues.ko["undo.headerFooter"]).toBe("머리글/바닥글");
    expect(catalogues.ko["undo.compress"]).toBe("압축");
  });

  it("names the step, and falls back to the plain command for no / unknown keys", () => {
    expect(historyTitle("undo", "undo.watermark")).toBe("실행 취소: 워터마크");
    expect(historyTitle("redo", "undo.compress")).toBe("다시 실행: 압축");
    expect(historyTitle("undo", "undo.fromTheFuture")).toBe("실행 취소");
    expect(historyTitle("undo", null)).toBe("실행 취소");
    expect(historyTitle("redo", undefined, "en")).toBe("Redo");
    expect(historyTitle("undo", "undo.headerFooter", "en")).toBe("Undo Header/Footer");
  });

  it("the title bar shows the step after a stamp, then the redo side after 실행 취소", async () => {
    await useDocStore.getState().open(SAMPLE);
    const info = useDocStore.getState().info!;
    const result = await api.addStamp({
      docId: info.docId,
      spec: {
        role: "watermark", source: { kind: "text", text: "대외비", fontSizePt: 40, color: [0, 0, 0] },
        anchor: "mc", marginPt: 36, rotateDeg: 45, opacity: 0.3, pages: "all",
      },
    });
    useDocStore.getState().adopt(result.info);
    render(<TitleBar run={() => {}} />);
    expect(screen.getByRole("button", { name: "실행 취소: 워터마크" })).toBeEnabled();

    await useDocStore.getState().undo();
    await waitFor(() => expect(screen.getByRole("button", { name: "다시 실행: 워터마크" })).toBeEnabled());
    expect(screen.getByRole("button", { name: "실행 취소" })).toBeDisabled();

    setLocale("en");
    await waitFor(() => expect(screen.getByRole("button", { name: "Redo Watermark" })).toBeInTheDocument());
  });

  it("a doc-changed for a newer generation drops the labels; hovering ↶ fetches them again", async () => {
    await useDocStore.getState().open(SAMPLE);
    const info = useDocStore.getState().info!;
    // an edit whose DocInfo the caller never adopts (e.g. an annotation): only the event arrives
    await api.setMetadata({ docId: info.docId, meta: { title: "새 제목" } });
    useDocStore.getState().applyDocChanged({
      docId: info.docId, docGeneration: info.docGeneration + 1, changedPages: "all", structure: false,
      dirty: true, reason: "edit", canUndo: true, canRedo: false,
    });
    expect(useDocStore.getState().info?.undoLabel).toBeNull();
    const { container } = render(<TitleBar run={() => {}} />);
    expect(screen.getByRole("button", { name: "실행 취소" })).toBeEnabled();
    fireEvent.pointerEnter(container.querySelector(".titlebar-left")!);
    await waitFor(() => expect(screen.getByRole("button", { name: "실행 취소: 메타데이터 편집" })).toBeInTheDocument());
  });
});
