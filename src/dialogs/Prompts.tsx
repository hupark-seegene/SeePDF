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
import type { ChoiceRequest, MultipleFilesAnswer, UnsavedAnswer } from "./dialogState";

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

/** A generic 확인 prompt (`askConfirm`) — 취소 / Esc resolves `false`. */
export function ConfirmDialog({
  titleKey,
  bodyKey,
  bodyParams,
  confirmKey,
  cancelKey,
  danger,
  resolve,
}: {
  titleKey: string;
  bodyKey: string;
  bodyParams?: Record<string, string | number>;
  confirmKey?: string;
  cancelKey?: string;
  danger?: boolean;
  resolve(value: boolean): void;
}) {
  const t = useT();
  return (
    <Dialog
      titleKey={titleKey}
      size="sm"
      onClose={() => resolve(false)}
      cancelKey={cancelKey}
      primary={{ labelKey: confirmKey ?? "common.ok", onSelect: () => resolve(true), danger }}
    >
      <p className="text-base">{t(bodyKey, bodyParams)}</p>
    </Dialog>
  );
}

/**
 * A choice prompt (`askChoice`): the options as a stack of full-width buttons, the footer's quiet
 * button (and Esc) answers `cancel`. The first enabled option takes the focus.
 */
export function ChoiceDialog({
  titleKey,
  bodyKey,
  bodyParams,
  hintKey,
  hintParams,
  options,
  cancel,
  resolve,
}: ChoiceRequest<string> & { resolve(value: string): void }) {
  const t = useT();
  const firstEnabled = options.findIndex((o) => !o.disabled);
  return (
    <Dialog titleKey={titleKey} size="sm" onClose={() => resolve(cancel.value)} cancelKey={cancel.labelKey}>
      <p className="text-base">{t(bodyKey, bodyParams)}</p>
      {hintKey && <p className="dlg-hint text-xs">{t(hintKey, hintParams)}</p>}
      <div className="dlg-choices" role="group" aria-label={t(titleKey)}>
        {options.map((o, i) => (
          <button
            key={o.value}
            type="button"
            className={o.primary ? "btn primary" : "btn"}
            disabled={o.disabled}
            autoFocus={i === firstEnabled}
            onClick={() => resolve(o.value)}
          >
            {t(o.labelKey, o.labelParams)}
          </button>
        ))}
      </div>
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
      <p className="dlg-hint text-xs">{t("pages.merge.carries")}</p>
    </Dialog>
  );
}

/**
 * v0.3 P2 — 위치 이동…: the selected pages as one block to 맨 앞, 맨 뒤 or so that the block starts
 * at page N (1-based, counted in the document after the move). One `move` op = one undo step; the
 * block stays selected at its new place.
 */
export function MoveToDialog({ pages, onClose }: { pages: PageIndex[]; onClose(): void }) {
  const t = useT();
  const info = useDocStore((s) => s.info);
  const count = info?.pageCount ?? 0;
  const sorted = [...new Set(pages)].sort((a, b) => a - b);
  const last = Math.max(1, count - sorted.length + 1);
  const [where, setWhere] = useState<"first" | "last" | "page">("page");
  const [page, setPage] = useState(String(Math.min(last, (sorted[0] ?? 0) + 1)));
  const n = Number(page);
  const invalid = where === "page" && (!Number.isInteger(n) || n < 1 || n > last);
  const to = where === "first" ? 0 : where === "last" ? count - sorted.length : n - 1;
  return (
    <Dialog
      titleKey="pages.moveTo.title"
      size="sm"
      onClose={onClose}
      primary={{
        labelKey: "pages.moveTo.apply",
        disabled: invalid || !info || sorted.length === 0,
        onSelect: () => {
          onClose();
          void movePagesTo(sorted, to);
        },
      }}
    >
      <p className="text-base">{t("pages.selected", { count: sorted.length })}</p>
      <div className="dlg-radio-group" role="radiogroup" aria-label={t("pages.moveTo.title")}>
        <label className="dlg-radio text-base">
          <input type="radio" name="moveTo" checked={where === "first"} onChange={() => setWhere("first")} />
          <span>{t("pages.moveTo.first")}</span>
        </label>
        <label className="dlg-radio text-base">
          <input type="radio" name="moveTo" checked={where === "last"} onChange={() => setWhere("last")} />
          <span>{t("pages.moveTo.last")}</span>
        </label>
        <label className="dlg-radio text-base">
          <input type="radio" name="moveTo" checked={where === "page"} onChange={() => setWhere("page")} />
          <span>{t("pages.moveTo.page")}</span>
        </label>
        {where === "page" && (
          <input
            className="field num"
            type="number"
            min={1}
            max={last}
            value={page}
            aria-label={t("pages.moveTo.page")}
            onChange={(e) => setPage(e.target.value)}
          />
        )}
      </div>
      {invalid && <p className="dlg-hint danger text-xs">{t("pages.moveTo.invalid", { max: last })}</p>}
    </Dialog>
  );
}

/** One `move` op; the moved block stays selected (like a drag). */
async function movePagesTo(pages: PageIndex[], to: number): Promise<void> {
  const { usePagesStore } = await import("../store/pagesStore");
  const ok = await runPageOps([{ kind: "move", pages, to }]);
  if (ok) usePagesStore.getState().setSelected(pages.map((_, i) => to + i));
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
