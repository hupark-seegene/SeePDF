/**
 * Renders whichever dialog is on top of the stack, plus the OCR sheet owned by module (f).
 *
 * `App.tsx` mounts this component **lazily and only while something is open**, so the whole dialog
 * chunk (and, behind its own `lazy`, tesseract.js) stays out of the critical path.
 */
import { lazy, Suspense } from "react";
import { useOcrDialogOpen } from "../ocr/dialogState";
import { closeDialog, useDialogStore, type DialogEntry } from "./dialogState";
import { ExportDialog, type ExportFormat } from "./ExportDialog";
import { MergeDialog } from "./MergeDialog";
import { SplitDialog } from "./SplitDialog";
import { PrintDialog } from "./PrintDialog";
import { SettingsDialog } from "./SettingsDialog";
import { DocInfoDialog } from "./DocInfoDialog";
import { ShortcutsDialog } from "./ShortcutsDialog";
import { ChoiceDialog, ConfirmDialog, ExtractDialog, InsertFromDialog, MoveToDialog, MultipleFilesDialog, PasswordDialog, UnsavedDialog } from "./Prompts";
import type { ChoiceRequest, MultipleFilesAnswer, UnsavedAnswer } from "./dialogState";
import type { PageIndex, RecoveryEntry, StampRole } from "../ipc/types";
import type { DrawnSignature, ImageSignature } from "./SignatureDialog";

const OcrDialog = lazy(() => import("../ocr").then((m) => ({ default: m.OcrDialog })));
// 서명 만들기 carries a canvas and its own drawing state; only 주석 mode ever opens it.
const SignatureDialog = lazy(() => import("./SignatureDialog"));
// 보안 is rarely opened; keep it out of the dialog chunk.
const SecurityDialog = lazy(() => import("./SecurityDialog"));
// 워터마크 / 머리글·바닥글 and 압축 (Stage 4): rare, each in its own chunk.
const StampDialog = lazy(() => import("./StampDialog"));
const CompressDialog = lazy(() => import("./CompressDialog"));
// 문서 비교 and 복구 (Stage 5): rare, each in its own chunk.
const CompareDialog = lazy(() => import("../compare/CompareDialog"));
const RecoveryDialog = lazy(() => import("./RecoveryDialog"));
// 여러 파일 OCR (Stage 6a, P1-7): its own chunk, sharing tesseract.js with the OCR sheet's.
const BatchOcrDialog = lazy(() => import("../ocr/batch/BatchOcrDialog"));
// 도장 선택 (Stage 6b, P1-12): only 주석 mode opens it.
const StampPickerDialog = lazy(() => import("./StampPickerDialog"));
// 업데이트 확인 (v0.2.0): rare, its own chunk (the updater plugin is behind a further import).
const UpdateDialog = lazy(() => import("../update/UpdateDialog"));
// 자르기 / 페이지 크기 변경 (P2): 페이지 mode only, each in its own chunk.
const CropDialog = lazy(() => import("../organize/CropDialog"));
const ResizeDialog = lazy(() => import("./ResizeDialog"));
// 페이지 레이블 (P2): rare, its own chunk.
const PageLabelsDialog = lazy(() => import("./PageLabelsDialog"));
// 여러 파일에서 검색 (P2): its own chunk.
const MultiSearchDialog = lazy(() => import("../multisearch/MultiSearchDialog"));
// v0.3 pkg5: SeePDF 정보 (H1) with 오픈 소스 라이선스 (H11) — rare, its own chunk.
const AboutDialog = lazy(() => import("./AboutDialog"));
// v0.3 pkg2-pages-structure-forms: 이미지로 PDF 만들기, its own chunk.
const ImagesToPdfDialog = lazy(() => import("./ImagesToPdfDialog"));

export default function DialogHost() {
  const stack = useDialogStore((s) => s.stack);
  const ocrOpen = useOcrDialogOpen();
  const top = stack[stack.length - 1];

  return (
    <>
      {top && <Current entry={top} key={top.key} />}
      {ocrOpen && (
        <Suspense fallback={null}>
          <OcrDialog />
        </Suspense>
      )}
    </>
  );
}

function Current({ entry }: { entry: DialogEntry }) {
  const p = entry.props;
  const close = () => closeDialog(entry.name);
  switch (entry.name) {
    case "export":
      return <ExportDialog onClose={close} initialFormat={p.format as ExportFormat | undefined} />;
    case "crop":
      return (
        <Suspense fallback={null}>
          <CropDialog onClose={close} pages={p.pages as PageIndex[] | undefined} />
        </Suspense>
      );
    case "resize":
      return (
        <Suspense fallback={null}>
          <ResizeDialog onClose={close} pages={p.pages as PageIndex[] | undefined} />
        </Suspense>
      );
    case "merge":
      return <MergeDialog onClose={close} initialPaths={p.paths as string[] | undefined} />;
    case "split":
      return <SplitDialog onClose={close} />;
    case "print":
      return <PrintDialog onClose={close} />;
    case "settings":
      return <SettingsDialog onClose={close} />;
    case "docInfo":
      return <DocInfoDialog onClose={close} />;
    case "shortcuts":
      return <ShortcutsDialog onClose={close} />;
    case "extract":
      return <ExtractDialog pages={p.pages as PageIndex[]} onClose={close} />;
    // v0.3 pkg2-pages-structure-forms
    case "imagesToPdf":
      return (
        <Suspense fallback={null}>
          <ImagesToPdfDialog onClose={close} paths={(p.paths as string[] | undefined) ?? []} />
        </Suspense>
      );
    case "moveTo":
      return <MoveToDialog pages={p.pages as PageIndex[]} onClose={close} />;
    case "insertFrom":
      return <InsertFromDialog at={p.at as PageIndex} path={p.path as string} onClose={close} />;
    case "password":
      return (
        <PasswordDialog
          fileName={p.fileName as string}
          wrong={Boolean(p.wrong)}
          resolve={p.resolve as (v: string | null) => void}
        />
      );
    case "confirm":
      return (
        <ConfirmDialog
          titleKey={p.titleKey as string}
          bodyKey={p.bodyKey as string}
          bodyParams={p.bodyParams as Record<string, string | number> | undefined}
          confirmKey={p.confirmKey as string | undefined}
          cancelKey={p.cancelKey as string | undefined}
          danger={p.danger === true}
          resolve={p.resolve as (v: boolean) => void}
        />
      );
    case "choice":
      return <ChoiceDialog {...(p as unknown as ChoiceRequest<string>)} resolve={p.resolve as (v: string) => void} />;
    case "unsaved":
      return <UnsavedDialog name={p.name as string} resolve={p.resolve as (v: UnsavedAnswer) => void} />;
    case "multipleFiles":
      return (
        <MultipleFilesDialog
          paths={p.paths as string[]}
          resolve={p.resolve as (v: MultipleFilesAnswer) => void}
        />
      );
    case "security":
      return (
        <Suspense fallback={null}>
          <SecurityDialog onClose={close} />
        </Suspense>
      );
    case "stamp":
      return (
        <Suspense fallback={null}>
          <StampDialog onClose={close} role={p.role as StampRole | undefined} />
        </Suspense>
      );
    case "compress":
      return (
        <Suspense fallback={null}>
          <CompressDialog onClose={close} />
        </Suspense>
      );
    case "compare":
      return (
        <Suspense fallback={null}>
          <CompareDialog onClose={close} />
        </Suspense>
      );
    case "batchOcr":
      return (
        <Suspense fallback={null}>
          <BatchOcrDialog onClose={close} />
        </Suspense>
      );
    case "multiSearch":
      return (
        <Suspense fallback={null}>
          <MultiSearchDialog onClose={close} />
        </Suspense>
      );
    case "recovery":
      return (
        <Suspense fallback={null}>
          <RecoveryDialog onClose={close} entries={(p.entries as RecoveryEntry[] | undefined) ?? []} />
        </Suspense>
      );
    case "signature":
      return (
        <Suspense fallback={null}>
          <SignatureDialog
            onClose={close}
            onDrawn={p.onDrawn as (s: DrawnSignature) => void}
            onImage={p.onImage as ((s: ImageSignature) => void) | undefined}
            onChooseImage={p.onChooseImage as () => void}
          />
        </Suspense>
      );
    case "update":
      return (
        <Suspense fallback={null}>
          <UpdateDialog onClose={close} />
        </Suspense>
      );
    case "about":
      return (
        <Suspense fallback={null}>
          <AboutDialog onClose={close} />
        </Suspense>
      );
    case "pageLabels":
      return (
        <Suspense fallback={null}>
          <PageLabelsDialog onClose={close} />
        </Suspense>
      );
    case "stampPicker":
      return (
        <Suspense fallback={null}>
          <StampPickerDialog
            onClose={close}
            onPick={p.onPick as (builtin: string) => void}
            onChooseImage={p.onChooseImage as () => void}
            current={p.current as string | null | undefined}
          />
        </Suspense>
      );
  }
}
