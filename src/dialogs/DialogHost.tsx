/**
 * Renders whichever dialog is on top of the stack, plus the OCR sheet owned by module (f).
 *
 * `App.tsx` mounts this component **lazily and only while something is open**, so the whole dialog
 * chunk (and, behind its own `lazy`, tesseract.js) stays out of the critical path.
 */
import { lazy, Suspense } from "react";
import { useOcrDialogOpen } from "../ocr/dialogState";
import { closeDialog, useDialogStore, type DialogEntry } from "./dialogState";
import { ExportDialog } from "./ExportDialog";
import { MergeDialog } from "./MergeDialog";
import { SplitDialog } from "./SplitDialog";
import { PrintDialog } from "./PrintDialog";
import { SettingsDialog } from "./SettingsDialog";
import { DocInfoDialog } from "./DocInfoDialog";
import { ExtractDialog, InsertFromDialog, MultipleFilesDialog, PasswordDialog, UnsavedDialog } from "./Prompts";
import type { MultipleFilesAnswer, UnsavedAnswer } from "./dialogState";
import type { PageIndex } from "../ipc/types";

const OcrDialog = lazy(() => import("../ocr").then((m) => ({ default: m.OcrDialog })));

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
      return <ExportDialog onClose={close} />;
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
    case "extract":
      return <ExtractDialog pages={p.pages as PageIndex[]} onClose={close} />;
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
    case "unsaved":
      return <UnsavedDialog name={p.name as string} resolve={p.resolve as (v: UnsavedAnswer) => void} />;
    case "multipleFiles":
      return (
        <MultipleFilesDialog
          paths={p.paths as string[]}
          resolve={p.resolve as (v: MultipleFilesAnswer) => void}
        />
      );
  }
}
