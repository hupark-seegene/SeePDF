/**
 * The contextual tool strip of UI_SPEC §3, as data. The icon map is §14.6.
 * `id` doubles as the `tool.*` i18n key suffix and the keymap id (`tool.<id>`).
 */
import {
  Circle, Eraser, Highlighter, ImagePlus, Minus, MessageSquarePlus, MousePointer2, MoveUpRight, PenLine,
  Signature, Square, SquareDashed, SquarePen, Stamp, Strikethrough, Type, Underline, Waves, Eye, Trash2, Link2,
  // v0.3 pkg2: 필드 만들기
  TextCursorInput, SquareCheck, CircleDot, ChevronsUpDown, FileDown, FileUp, Layers,
  // v0.3 pkg4-annotations-stamps-objects
  Pentagon, MessageSquareQuote,
} from "lucide-react";
import type { Mode, ToolId } from "../store/appStore";
import type { IconProps } from "./IconButton";
import type { ComponentType } from "react";

export interface ToolDef {
  id: ToolId;
  labelKey: string;
  icon: ComponentType<IconProps>;
  /** keymap id whose first chord is shown in the tooltip */
  keyId?: string;
  /** a toggle (양식 필드 강조) or a one-shot action rather than an armed tool */
  kind?: "tool" | "toggle" | "action";
  /** v0.3: which `forms/formActions` action a `kind: "action"` button runs (default: 모든 필드 지우기) */
  action?: "clearAll" | "flatten" | "exportData" | "importData";
  /** v0.3: a strip section (a divider is drawn where the group changes); the i18n key names it */
  group?: string;
  /** v0.3 T2: a ⌄ beside the button opens quick choices (도장: ✓ ✗ ● 오늘 날짜) */
  flyout?: "stamp";
}

const SELECT: ToolDef = { id: "select", labelKey: "tool.select", icon: MousePointer2, keyId: "tool.select" };

export const TOOL_STRIP: Record<Mode, ToolDef[]> = {
  read: [],
  annotate: [
    SELECT,
    { id: "highlight", labelKey: "tool.highlight", icon: Highlighter, keyId: "tool.highlight" },
    { id: "underline", labelKey: "tool.underline", icon: Underline, keyId: "tool.underline" },
    { id: "strikeout", labelKey: "tool.strikeout", icon: Strikethrough, keyId: "tool.strikeout" },
    { id: "squiggly", labelKey: "tool.squiggly", icon: Waves },
    { id: "note", labelKey: "tool.note", icon: MessageSquarePlus, keyId: "tool.note" },
    { id: "pen", labelKey: "tool.pen", icon: PenLine, keyId: "tool.pen" },
    { id: "eraser", labelKey: "tool.eraser", icon: Eraser, keyId: "tool.eraser" },
    { id: "rectangle", labelKey: "tool.rectangle", icon: Square, keyId: "tool.rectangle" },
    { id: "ellipse", labelKey: "tool.ellipse", icon: Circle, keyId: "tool.ellipse" },
    { id: "line", labelKey: "tool.line", icon: Minus, keyId: "tool.line" },
    { id: "arrow", labelKey: "tool.arrow", icon: MoveUpRight, keyId: "tool.arrow" },
    // v0.3 pkg4 (A2): 다각형 (다각형 / 꺾은선 / 구름, 측정) and 설명선
    { id: "polygon", labelKey: "tool.polygon", icon: Pentagon },
    { id: "textbox", labelKey: "tool.textbox", icon: Type, keyId: "tool.textbox" },
    { id: "callout", labelKey: "tool.callout", icon: MessageSquareQuote },
    { id: "stamp", labelKey: "tool.stamp", icon: Stamp, keyId: "tool.stamp", flyout: "stamp" },
    { id: "signature", labelKey: "tool.signature", icon: Signature, keyId: "tool.signature" },
  ],
  edit: [
    SELECT,
    { id: "editText", labelKey: "tool.editText", icon: SquarePen },
    { id: "addText", labelKey: "tool.addText", icon: Type },
    { id: "addImage", labelKey: "tool.addImage", icon: ImagePlus },
    { id: "redact", labelKey: "tool.redact", icon: SquareDashed, keyId: "tool.redact" },
    // P2: drag a rectangle → 페이지로 이동 | 웹 주소
    { id: "link", labelKey: "tool.link", icon: Link2 },
  ],
  pages: [],
  form: [
    SELECT,
    { id: "highlightFields", labelKey: "form.highlightFields", icon: Eye, kind: "toggle" },
    { id: "fillForm", labelKey: "form.clearAll", icon: Trash2, kind: "action" },
    // v0.3 pkg2-pages-structure-forms: 필드 만들기 (draw a rectangle), then the form's data and 평면화
    { id: "fieldText", labelKey: "form.author.text", icon: TextCursorInput, group: "form.author.section" },
    { id: "fieldCheckbox", labelKey: "form.author.checkbox", icon: SquareCheck, group: "form.author.section" },
    { id: "fieldRadio", labelKey: "form.author.radio", icon: CircleDot, group: "form.author.section" },
    { id: "fieldCombo", labelKey: "form.author.combo", icon: ChevronsUpDown, group: "form.author.section" },
    { id: "fieldSignature", labelKey: "form.author.signature", icon: Signature, group: "form.author.section" },
    { id: "formExport", labelKey: "form.data.export", icon: FileDown, kind: "action", action: "exportData", group: "form.data.section" },
    { id: "formImport", labelKey: "form.data.import", icon: FileUp, kind: "action", action: "importData", group: "form.data.section" },
    { id: "formFlatten", labelKey: "form.flatten", icon: Layers, kind: "action", action: "flatten", group: "form.data.section" },
  ],
};

export const MODES: { id: Mode; labelKey: string; keyId: string }[] = [
  { id: "read", labelKey: "mode.read", keyId: "mode.read" },
  { id: "annotate", labelKey: "mode.annotate", keyId: "mode.annotate" },
  { id: "edit", labelKey: "mode.edit", keyId: "mode.edit" },
  { id: "pages", labelKey: "mode.pages", keyId: "mode.pages" },
  { id: "form", labelKey: "mode.form", keyId: "mode.form" },
];
