/**
 * The small prompts of UI_SPEC §10 — each resolves the promise its opener is awaiting
 * (`dialogState.ask*`): 암호 입력 (retried in place), 저장하지 않은 변경 사항, 여러 파일을 어떻게
 * 열까요?, 페이지 추출, 파일에서 페이지 삽입.
 */
import { useState } from "react";
import { useT } from "../i18n/useT";
import { useDocStore } from "../store/docStore";
import { Dialog, Row } from "./Dialog";
import { runExtract, baseName, runPageOps } from "./flows";
import { parsePageRange } from "./pageRange";
import type { PageIndex } from "../ipc/types";
import type { MultipleFilesAnswer, UnsavedAnswer } from "./dialogState";

/** 암호 입력 — `passwordRequired` / `passwordWrong` (F-01). */
export function PasswordDialog({
  fileName,
  wrong,
  resolve,
}: {
  fileName: string;
  wrong: boolean;
  resolve(value: string | null): void;
}) {
  const t = useT();
  const [value, setValue] = useState("");
  return (
    <Dialog
      titleKey="security.password.required"
      size="sm"
      onClose={() => resolve(null)}
      primary={{ labelKey: "common.ok", onSelect: () => resolve(value), disabled: value.length === 0 }}
    >
      <p className="text-sm dim">{fileName}</p>
      <form
        onSubmit={(e) => {
          e.preventDefault();
          if (value) resolve(value);
        }}
      >
        <input
          className="field grow"
          type="password"
          value={value}
          autoFocus
          aria-label={t("security.password.enter")}
          placeholder={t("security.password.enter")}
          onChange={(e) => setValue(e.target.value)}
        />
      </form>
      {wrong && <p className="dlg-hint danger text-xs">{t("security.password.wrong")}</p>}
    </Dialog>
  );
}

/** 저장 / 저장 안 함 / 취소 (F-23). */
export function UnsavedDialog({ name, resolve }: { name: string; resolve(value: UnsavedAnswer): void }) {
  const t = useT();
  return (
    <Dialog
      titleKey="dialog.unsaved.title"
      titleParams={{ name }}
      size="sm"
      onClose={() => resolve("cancel")}
      secondary={{ labelKey: "common.dontSave", onSelect: () => resolve("dontSave") }}
      primary={{ labelKey: "common.save", onSelect: () => resolve("save") }}
    >
      <p className="text-base">{t("dialog.unsaved.body")}</p>
    </Dialog>
  );
}

/** 여러 파일을 어떻게 열까요? — 각각 열기 / 하나로 합치기 (UI_SPEC §11). */
export function MultipleFilesDialog({
  paths,
  resolve,
}: {
  paths: string[];
  resolve(value: MultipleFilesAnswer): void;
}) {
  const t = useT();
  return (
    <Dialog
      titleKey="welcome.multipleFiles"
      size="sm"
      onClose={() => resolve(null)}
      secondary={{ labelKey: "welcome.openSeparately", onSelect: () => resolve("separate") }}
      primary={{ labelKey: "welcome.mergeIntoOne", onSelect: () => resolve("merge") }}
    >
      <ul className="drop-list">
        {paths.map((p) => (
          <li key={p} className="text-sm">
            {baseName(p)}
          </li>
        ))}
      </ul>
      <p className="dlg-hint text-xs">{t("pages.merge.outlineWarning")}</p>
    </Dialog>
  );
}

/** 페이지 추출: 새 파일로 저장 + 추출 후 원본에서 삭제. */
export function ExtractDialog({ pages, onClose }: { pages: PageIndex[]; onClose(): void }) {
  const t = useT();
  const [removeAfter, setRemoveAfter] = useState(false);
  return (
    <Dialog
      titleKey="pages.extract.title"
      size="sm"
      onClose={onClose}
      primary={{
        labelKey: "pages.extract.asNewFile",
        onSelect: () => {
          void runExtract(pages, removeAfter);
          onClose();
        },
      }}
    >
      <p className="text-base">{t("pages.selected", { count: pages.length })}</p>
      <label className="dlg-check text-base">
        <input type="checkbox" checked={removeAfter} onChange={(e) => setRemoveAfter(e.target.checked)} />
        <span>{t("pages.extract.removeAfter")}</span>
      </label>
    </Dialog>
  );
}

/** 파일에서 페이지 삽입: a 1-based range from the picked file, inserted at `at`. */
export function InsertFromDialog({ at, path, onClose }: { at: PageIndex; path: string; onClose(): void }) {
  const t = useT();
  const [range, setRange] = useState("");
  const info = useDocStore((s) => s.info);
  const invalid = range.trim() !== "" && parsePageRange(range, 10_000) === null;
  return (
    <Dialog
      titleKey="pages.insertFrom.title"
      size="sm"
      onClose={onClose}
      primary={{
        labelKey: "common.apply",
        disabled: invalid || !info,
        onSelect: () => {
          void runPageOps([{ kind: "insertFrom", at, path, range: range.trim() || undefined }]);
          onClose();
        },
      }}
    >
      <p className="text-sm dim">{baseName(path)}</p>
      <Row labelKey="pages.range">
        <input
          className="field"
          value={range}
          aria-label={t("pages.range")}
          placeholder={t("pages.range.placeholder")}
          onChange={(e) => setRange(e.target.value)}
        />
      </Row>
      {invalid && <p className="dlg-hint danger text-xs">{t("pages.range.invalid")}</p>}
    </Dialog>
  );
}
