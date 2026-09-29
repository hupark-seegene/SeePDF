/**
 * 양식 mode actions (v0.3 F1 / F2), lazily imported by the tool strip, the form overlay and the
 * inspector — none of this is in the entry chunk.
 *
 *  * 필드 만들기: a rectangle drawn with one of the five field tools → `create_form_field` with a
 *    default name (텍스트1, 확인란1, 라디오1, 목록1, 서명1 — the next free number), then the new field
 *    is focused so the inspector shows its properties;
 *  * 필드 속성: `update_form_field` / `delete_form_field`;
 *  * 양식 데이터 내보내기 / 가져오기: CSV or XFDF (the save / open panel's filter picks the format);
 *  * 양식 평면화: confirm (`form.flattenWarning`), then `flatten_form` — one undo step;
 *  * the field context menu: 값 지우기 · 모든 필드 지우기 · 필드 강조 표시 전환.
 */
import * as api from "../ipc/api";
import { useDocStore } from "../store/docStore";
import { useAppStore, type ToolId } from "../store/appStore";
import { toast } from "../app/toastStore";
import { openContextMenu } from "../app/contextMenuStore";
import { askConfirm } from "../dialogs/dialogState";
import { t } from "../i18n";
import { commitField, fieldKey, loadFields, resetFormFields, useFormStore } from "./formStore";
import type { FieldValue, FormEditResult, FormField, FormFieldPatch, NewFieldType, PageIndex, Rect } from "../ipc/types";

export type FormAction = "clearAll" | "flatten" | "exportData" | "importData";

/** The field tools of the 양식 strip and the field type each draws. */
export const FIELD_TOOLS: Partial<Record<ToolId, NewFieldType>> = {
  fieldText: "text",
  fieldCheckbox: "checkbox",
  fieldRadio: "radio",
  fieldCombo: "combo",
  fieldSignature: "signature",
};

export function fieldToolType(tool: ToolId): NewFieldType | null {
  return FIELD_TOOLS[tool] ?? null;
}

function docId(): string | null {
  return useDocStore.getState().info?.docId ?? null;
}

function fail(e: unknown): void {
  const unsupported = api.isSeePdfError(e) && e.code === "unsupported";
  toast(unsupported ? "structure.encrypted" : "error.generic", undefined, {
    tone: "danger",
    detail: e instanceof Error ? e.message : String(e),
  });
}

/** Take an authoring result: the document, its fields, and focus on the edited field. */
function adopt(result: FormEditResult): void {
  useDocStore.getState().adopt(result.info);
  useFormStore.getState().setFields(result.info.docId, result.info.docGeneration, result.fields);
  useFormStore.getState().setFocused(result.field ? fieldKey(result.field.page, result.field.index) : null);
}

/** 텍스트3 — the type's word plus the next number no field uses yet. */
export function defaultFieldName(type: NewFieldType, fields: FormField[]): string {
  const base = t(`form.author.name.${type}`);
  const used = new Set(fields.map((f) => f.name));
  let n = 1;
  while (used.has(`${base}${n}`)) n++;
  return `${base}${n}`;
}

/** A field tool's rectangle (PDF user space) on `page`. */
export async function createFieldAt(page: PageIndex, rect: Rect, type: NewFieldType): Promise<FormField | null> {
  const doc = docId();
  if (!doc) return null;
  const fields = useFormStore.getState().fields;
  const name = defaultFieldName(type, fields);
  const options =
    type === "combo"
      ? [t("form.author.choice", { n: 1 }), t("form.author.choice", { n: 2 })]
      : type === "radio"
        ? [t("form.author.choice", { n: fields.filter((f) => f.type === "radio").length + 1 })]
        : [];
  try {
    const result = await api.createFormField({ docId: doc, spec: { page, rect, type, name, options } });
    adopt(result);
    return result.field ?? null;
  } catch (e) {
    fail(e);
    return null;
  }
}

export async function updateField(field: FormField, patch: FormFieldPatch): Promise<boolean> {
  const doc = docId();
  if (!doc) return false;
  try {
    adopt(await api.updateFormField({ docId: doc, page: field.page, index: field.index, patch }));
    return true;
  } catch (e) {
    fail(e);
    return false;
  }
}

export async function deleteField(field: FormField): Promise<boolean> {
  const doc = docId();
  if (!doc) return false;
  try {
    adopt(await api.deleteFormField({ docId: doc, page: field.page, index: field.index }));
    return true;
  } catch (e) {
    fail(e);
    return false;
  }
}

/** 값 지우기 — a radio button clears its whole group (v0.3 F2). */
export function clearValue(field: FormField): Promise<FormField | null> {
  const value: FieldValue =
    field.type === "checkbox" || field.type === "radio"
      ? { checked: false }
      : field.type === "list"
        ? { selected: [] }
        : { text: "" };
  return commitField(field.page, field.index, value);
}

async function flatten(): Promise<void> {
  const doc = docId();
  if (!doc) return;
  if (!useFormStore.getState().fields.length) {
    toast("form.noFields", undefined, { tone: "info" });
    return;
  }
  const ok = await askConfirm({
    titleKey: "form.flatten",
    bodyKey: "form.flattenWarning",
    confirmKey: "form.flatten",
    danger: true,
  });
  if (!ok) return;
  try {
    const info = await api.flattenForm({ docId: doc });
    useDocStore.getState().adopt(info);
    await loadFields(info.docId, info.docGeneration);
    toast("form.flattened", undefined, {
      tone: "success",
      actions: [{ labelKey: "common.undo", onSelect: () => void useDocStore.getState().undo() }],
    });
  } catch (e) {
    fail(e);
  }
}

const DATA_FILTERS = {
  xfdf: { name: "XFDF", extensions: ["xfdf"] },
  csv: { name: "CSV", extensions: ["csv"] },
};

function formatOf(path: string): "csv" | "xfdf" {
  return /\.csv$/i.test(path) ? "csv" : "xfdf";
}

async function exportData(): Promise<void> {
  const info = useDocStore.getState().info;
  if (!info) return;
  const stem = info.name.replace(/\.[^.]+$/, "");
  const outPath = await api.saveFileDialog({
    defaultPath: `${stem}-${t("form.data.fileSuffix")}.xfdf`,
    filters: [DATA_FILTERS.xfdf, DATA_FILTERS.csv],
  });
  if (!outPath) return;
  try {
    const result = await api.exportFormData({ docId: info.docId, format: formatOf(outPath), outPath });
    toast("form.data.exported", { count: result.fields }, { tone: "success" });
  } catch (e) {
    fail(e);
  }
}

async function importData(): Promise<void> {
  const info = useDocStore.getState().info;
  if (!info) return;
  const picked = await api.openFileDialog({ multiple: false, filters: [{ name: "XFDF / CSV", extensions: ["xfdf", "csv"] }] });
  if (!picked?.length) return;
  try {
    const result = await api.importFormData({ docId: info.docId, path: picked[0], format: formatOf(picked[0]) });
    await useDocStore.getState().refresh();
    const fresh = useDocStore.getState().info;
    if (fresh) await loadFields(fresh.docId, fresh.docGeneration);
    toast("form.data.imported", { count: result.fields }, {
      tone: result.unknown.length ? "info" : "success",
      detail: result.unknown.length ? t("form.data.unknown", { names: result.unknown.slice(0, 5).join(", ") }) : undefined,
    });
  } catch (e) {
    fail(e);
  }
}

/** The 양식 strip's one-shot buttons. */
export async function runFormAction(action: FormAction): Promise<void> {
  switch (action) {
    case "clearAll":
      return resetFormFields();
    case "flatten":
      return flatten();
    case "exportData":
      return exportData();
    case "importData":
      return importData();
  }
}

/** Right-click on a field (v0.3 F2): 값 지우기 · 모든 필드 지우기 · 필드 강조 표시 전환 (+ 필드 삭제). */
export function openFieldMenu(field: FormField, x: number, y: number): void {
  openContextMenu({
    x,
    y,
    labelKey: "mode.form",
    items: [
      {
        id: "clearField",
        labelKey: "form.clearField",
        disabled: field.readOnly || field.type === "button" || field.type === "signature",
        onSelect: () => void clearValue(field),
      },
      { id: "clearAll", labelKey: "form.clearAll", onSelect: () => void resetFormFields() },
      { id: "highlight", labelKey: "form.highlightFields", onSelect: () => useFormStore.getState().toggleHighlight() },
      { id: "sep", separator: true },
      {
        id: "properties",
        labelKey: "form.author.properties",
        onSelect: () => {
          useFormStore.getState().setFocused(fieldKey(field.page, field.index));
          useAppStore.getState().toggleInspector(true);
        },
      },
      { id: "delete", labelKey: "form.author.delete", danger: true, onSelect: () => void deleteField(field) },
    ],
  });
}
