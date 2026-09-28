/**
 * UI_SPEC §12 "Text selection" — what the canvas menu's text items do: 형광펜 · 밑줄 · 취소선 ·
 * 메모 추가 · 검색 (복사 stays in `pageMenus.ts`: the clipboard write must happen inside the click's
 * user gesture, before any `await`; 영역 표시로 표시 is `edit/redact.ts` `markTextSelection`).
 *
 * Lazy: `pageMenus.ts` imports this only once an item is chosen. The markups are exactly what the
 * 형광펜 / 밑줄 / 취소선 tools make from the same selection (`tools/markup.ts` `markupSpec`, one
 * annotation per page, the tool's remembered style), so the two paths cannot drift apart.
 */
import type { AnnotSpec, PageIndex, Point, Rect } from "../ipc/types";
import { useAnnotStore } from "../store/annotStore";
import { useAppStore } from "../store/appStore";
import { useDocStore } from "../store/docStore";
import { styleFor } from "../store/toolStyles";
import { clearTextSelection, useSearchStore } from "../viewer";
import { selectedPages, selectionRectsForPage } from "../annot/selectionQuads";
import { markupSpec, usableRects, type MarkupKind } from "../tools/markup";
import type { ToolContext } from "../tools/ToolController";
import { NOTE_SIZE_PT } from "../tools/note";

/** The longest query 검색 hands to the panel (a whole selected page is not a useful search). */
export const SEARCH_MAX_CHARS = 200;

function toolContext(tool: string): ToolContext | null {
  const info = useDocStore.getState().info;
  if (!info) return null;
  return {
    docId: info.docId,
    docGeneration: info.docGeneration,
    style: styleFor(tool, useAnnotStore.getState().toolDefaults),
    modifiers: { shift: false, alt: false, meta: false, ctrl: false },
    scale: 1,
  };
}

/** 형광펜 · 밑줄 · 취소선 on the selection: one annotation per page it touches. */
export async function markupSelection(kind: MarkupKind): Promise<number> {
  const ctx = toolContext(kind);
  if (!ctx) return 0;
  // read the selection before anything is awaited: it is cleared below
  const specs: { page: PageIndex; spec: AnnotSpec }[] = [];
  for (const page of selectedPages()) {
    const spec = markupSpec(kind, selectionRectsForPage(page), ctx);
    if (spec) specs.push({ page, spec });
  }
  if (specs.length === 0) return 0;
  clearTextSelection();
  const { createAnnotation } = await import("../annot/actions");
  await Promise.all(specs.map(({ page, spec }) => createAnnotation(page, spec, { select: false })));
  return specs.length;
}

/** Where 메모 추가 puts the note: just after the selection's last line, kept on the page. */
export function noteAnchor(lastLine: Rect, crop: Rect): Point {
  const x = Math.min(Math.max(lastLine.r + 2, crop.l), crop.r - NOTE_SIZE_PT);
  const y = Math.min(Math.max(lastLine.t, crop.b + NOTE_SIZE_PT), crop.t);
  return [x, y];
}

/** 메모 추가: a note at the end of the selection, opened for typing. */
export async function noteOnSelection(): Promise<boolean> {
  const info = useDocStore.getState().info;
  const ctx = toolContext("note");
  if (!info || !ctx) return false;
  const pages = selectedPages();
  let target: { page: PageIndex; line: Rect } | null = null;
  for (let i = pages.length - 1; i >= 0 && !target; i--) {
    const lines = usableRects(selectionRectsForPage(pages[i]));
    if (lines.length) target = { page: pages[i], line: lines[lines.length - 1] };
  }
  const crop = target && info.pages[target.page]?.crop;
  if (!target || !crop) return false;
  const spec: AnnotSpec = { kind: "note", at: noteAnchor(target.line, crop), color: ctx.style.color, contents: "" };
  clearTextSelection();
  const { createAnnotation } = await import("../annot/actions");
  return (await createAnnotation(target.page, spec, { edit: true })) !== null;
}

/** Set a React-controlled input as if typed, so the component's own `onChange` sees it. */
function typeInto(input: HTMLInputElement, value: string): void {
  const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")?.set;
  if (setter) setter.call(input, value);
  else input.value = value;
  input.dispatchEvent(new Event("input", { bubbles: true }));
}

/**
 * 검색: the selected text (whitespace folded, capped) becomes the 검색 panel's query. An open panel
 * gets it typed into its field — its draft would otherwise overwrite a query set from outside —
 * and a closed one opens with it and runs the search as it mounts.
 */
export function searchSelectedText(text: string): boolean {
  const info = useDocStore.getState().info;
  const query = text.replace(/\s+/g, " ").trim().slice(0, SEARCH_MAX_CHARS);
  if (!info || !query) return false;
  const field = typeof document === "undefined" ? null : document.querySelector<HTMLInputElement>(".search-panel input[type='search']");
  if (field) {
    typeInto(field, query);
    field.focus();
  } else {
    useSearchStore.setState({ query });
  }
  useAppStore.getState().setSidebarTab("search");
  return true;
}
