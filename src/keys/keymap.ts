/**
 * The keymap of UI_SPEC §13 as data: one row per command, per-platform chords, and a `when` guard.
 *
 * `id` is the single identifier used by three things: this table, the native macOS menu
 * (`menu:<id>` events, IPC_CONTRACT §8) and the command dispatcher in `useKeymap.ts`.
 * `⌘` is Cmd on macOS and Ctrl on Windows unless a row says otherwise.
 */
import type { OsName } from "../ipc/env";

/** Where a binding is live. `canvas` = the canvas has focus and no input is editing. */
export type KeyContext = "always" | "doc" | "canvas" | "pages";

export interface KeyBinding {
  id: string;
  labelKey: string;
  mac: string[];
  win: string[];
  when: KeyContext;
  group: "file" | "edit" | "view" | "go" | "mode" | "tool" | "pages";
  /** true when the binding is a momentary (hold) tool switch — UI_SPEC §6 */
  momentary?: boolean;
  /**
   * The one documented chord collision: `Space` pages down on a tap (< 200 ms, no movement) and
   * pans while held (UI_SPEC §13). Both rows exist; the 200 ms tap/hold discrimination lives in the
   * viewer's pointer layer (Stage 1 (c)), which is the only place that knows about drag distance.
   */
  sharesChordWith?: string;
}

export const KEYMAP: KeyBinding[] = [
  // File ---------------------------------------------------------------------
  { id: "file.open", labelKey: "menu.file.open", mac: ["Cmd+O"], win: ["Ctrl+O"], when: "always", group: "file" },
  { id: "file.openRecent", labelKey: "menu.file.openRecent", mac: ["Cmd+Shift+O"], win: ["Ctrl+Shift+O"], when: "always", group: "file" },
  { id: "file.close", labelKey: "menu.file.close", mac: ["Cmd+W"], win: ["Ctrl+W"], when: "doc", group: "file" },
  { id: "file.save", labelKey: "menu.file.save", mac: ["Cmd+S"], win: ["Ctrl+S"], when: "doc", group: "file" },
  { id: "file.saveAs", labelKey: "menu.file.saveAs", mac: ["Cmd+Shift+S"], win: ["Ctrl+Shift+S"], when: "doc", group: "file" },
  { id: "file.export", labelKey: "menu.file.export", mac: ["Alt+Cmd+E"], win: ["Ctrl+Alt+E"], when: "doc", group: "file" },
  { id: "file.print", labelKey: "menu.file.print", mac: ["Cmd+P"], win: ["Ctrl+P"], when: "doc", group: "file" },
  { id: "file.docInfo", labelKey: "menu.file.docInfo", mac: ["Cmd+I"], win: ["Ctrl+D"], when: "doc", group: "file" },
  { id: "tools.stamp", labelKey: "menu.tools.stamp", mac: ["Alt+Cmd+W"], win: ["Ctrl+Alt+W"], when: "doc", group: "file" },
  { id: "app.settings", labelKey: "menu.settings", mac: ["Cmd+Comma"], win: ["Ctrl+Comma"], when: "always", group: "file" },
  { id: "app.quit", labelKey: "menu.quit", mac: ["Cmd+Q"], win: ["Alt+F4"], when: "always", group: "file" },

  // Edit ---------------------------------------------------------------------
  { id: "edit.undo", labelKey: "menu.edit.undo", mac: ["Cmd+Z"], win: ["Ctrl+Z"], when: "doc", group: "edit" },
  { id: "edit.redo", labelKey: "menu.edit.redo", mac: ["Cmd+Shift+Z"], win: ["Ctrl+Y"], when: "doc", group: "edit" },
  { id: "edit.cut", labelKey: "menu.edit.cut", mac: ["Cmd+X"], win: ["Ctrl+X"], when: "doc", group: "edit" },
  { id: "edit.copy", labelKey: "menu.edit.copy", mac: ["Cmd+C"], win: ["Ctrl+C"], when: "doc", group: "edit" },
  { id: "edit.paste", labelKey: "menu.edit.paste", mac: ["Cmd+V"], win: ["Ctrl+V"], when: "doc", group: "edit" },
  { id: "edit.selectAll", labelKey: "menu.edit.selectAll", mac: ["Cmd+A"], win: ["Ctrl+A"], when: "doc", group: "edit" },
  { id: "edit.delete", labelKey: "menu.edit.delete", mac: ["Backspace"], win: ["Delete"], when: "canvas", group: "edit" },
  { id: "edit.duplicate", labelKey: "menu.edit.duplicate", mac: ["Cmd+D"], win: ["Ctrl+D"], when: "pages", group: "edit" },
  { id: "edit.find", labelKey: "menu.edit.find", mac: ["Cmd+F"], win: ["Ctrl+F"], when: "doc", group: "edit" },
  { id: "edit.findNext", labelKey: "menu.edit.findNext", mac: ["Cmd+G"], win: ["F3"], when: "doc", group: "edit" },
  { id: "edit.findPrevious", labelKey: "menu.edit.findPrevious", mac: ["Cmd+Shift+G"], win: ["Shift+F3"], when: "doc", group: "edit" },

  // View ---------------------------------------------------------------------
  { id: "view.sidebar", labelKey: "menu.view.sidebar", mac: ["Ctrl+Cmd+S"], win: ["Ctrl+F9"], when: "always", group: "view" },
  { id: "view.inspector", labelKey: "menu.view.inspector", mac: ["Alt+Cmd+P"], win: ["Ctrl+F10"], when: "always", group: "view" },
  { id: "view.sidebar.thumbnails", labelKey: "sidebar.tab.thumbnails", mac: ["Cmd+Alt+1"], win: ["Ctrl+Alt+1"], when: "doc", group: "view" },
  { id: "view.sidebar.outline", labelKey: "sidebar.tab.outline", mac: ["Cmd+Alt+2"], win: ["Ctrl+Alt+2"], when: "doc", group: "view" },
  { id: "view.sidebar.annotations", labelKey: "sidebar.tab.annotations", mac: ["Cmd+Alt+3"], win: ["Ctrl+Alt+3"], when: "doc", group: "view" },
  { id: "view.sidebar.search", labelKey: "sidebar.tab.search", mac: ["Cmd+Alt+4"], win: ["Ctrl+Alt+4"], when: "doc", group: "view" },
  { id: "view.zoomIn", labelKey: "menu.view.zoomIn", mac: ["Cmd+Plus"], win: ["Ctrl+Plus"], when: "doc", group: "view" },
  { id: "view.zoomOut", labelKey: "menu.view.zoomOut", mac: ["Cmd+Minus"], win: ["Ctrl+Minus"], when: "doc", group: "view" },
  { id: "view.actualSize", labelKey: "menu.view.actualSize", mac: ["Cmd+0"], win: ["Ctrl+0"], when: "doc", group: "view" },
  { id: "view.fitPage", labelKey: "menu.view.fitPage", mac: ["Cmd+9"], win: ["Ctrl+9"], when: "doc", group: "view" },
  { id: "view.fitWidth", labelKey: "menu.view.fitWidth", mac: ["Cmd+8"], win: ["Ctrl+8"], when: "doc", group: "view" },
  { id: "view.layout.single", labelKey: "view.layout.single", mac: ["Ctrl+1"], win: ["Ctrl+Shift+1"], when: "doc", group: "view" },
  { id: "view.layout.continuous", labelKey: "view.layout.continuous", mac: ["Ctrl+2"], win: ["Ctrl+Shift+2"], when: "doc", group: "view" },
  { id: "view.layout.twoPage", labelKey: "view.layout.twoPage", mac: ["Ctrl+3"], win: ["Ctrl+Shift+3"], when: "doc", group: "view" },
  { id: "view.rotateLeft", labelKey: "view.rotateLeft", mac: ["Cmd+L"], win: ["Ctrl+L"], when: "doc", group: "view" },
  { id: "view.rotateRight", labelKey: "view.rotateRight", mac: ["Cmd+R"], win: ["Ctrl+R"], when: "doc", group: "view" },
  { id: "view.night", labelKey: "view.night.label", mac: ["Ctrl+Cmd+N"], win: ["Ctrl+Shift+N"], when: "doc", group: "view" },
  { id: "view.readingMode", labelKey: "menu.view.readingMode", mac: ["Ctrl+Cmd+R"], win: ["F8"], when: "doc", group: "view" },
  { id: "view.fullScreen", labelKey: "menu.view.fullScreen", mac: ["Ctrl+Cmd+F"], win: ["F11"], when: "always", group: "view" },

  // Navigation ---------------------------------------------------------------
  { id: "go.nextPage", labelKey: "menu.go.nextPage", mac: ["ArrowDown", "PageDown", "Space"], win: ["ArrowDown", "PageDown", "Space"], when: "canvas", group: "go", sharesChordWith: "tool.hand" },
  { id: "go.previousPage", labelKey: "menu.go.previousPage", mac: ["ArrowUp", "PageUp", "Shift+Space"], win: ["ArrowUp", "PageUp", "Shift+Space"], when: "canvas", group: "go" },
  { id: "go.firstPage", labelKey: "menu.go.firstPage", mac: ["Cmd+ArrowUp"], win: ["Ctrl+Home"], when: "doc", group: "go" },
  { id: "go.lastPage", labelKey: "menu.go.lastPage", mac: ["Cmd+ArrowDown"], win: ["Ctrl+End"], when: "doc", group: "go" },
  { id: "go.goToPage", labelKey: "menu.go.goToPage", mac: ["Alt+Cmd+G"], win: ["Ctrl+G"], when: "doc", group: "go" },
  { id: "go.back", labelKey: "menu.go.back", mac: ["Cmd+BracketLeft"], win: ["Alt+ArrowLeft"], when: "doc", group: "go" },
  { id: "go.forward", labelKey: "menu.go.forward", mac: ["Cmd+BracketRight"], win: ["Alt+ArrowRight"], when: "doc", group: "go" },

  // Modes --------------------------------------------------------------------
  { id: "mode.read", labelKey: "mode.read", mac: ["Cmd+1"], win: ["Ctrl+1"], when: "doc", group: "mode" },
  { id: "mode.annotate", labelKey: "mode.annotate", mac: ["Cmd+2"], win: ["Ctrl+2"], when: "doc", group: "mode" },
  { id: "mode.edit", labelKey: "mode.edit", mac: ["Cmd+3"], win: ["Ctrl+3"], when: "doc", group: "mode" },
  { id: "mode.pages", labelKey: "mode.pages", mac: ["Cmd+4"], win: ["Ctrl+4"], when: "doc", group: "mode" },
  { id: "mode.form", labelKey: "mode.form", mac: ["Cmd+5"], win: ["Ctrl+5"], when: "doc", group: "mode" },

  // Tools (canvas focus, no input editing) ------------------------------------
  { id: "tool.select", labelKey: "tool.select", mac: ["V"], win: ["V"], when: "canvas", group: "tool" },
  { id: "tool.hand", labelKey: "tool.hand", mac: ["Space"], win: ["Space"], when: "canvas", group: "tool", momentary: true, sharesChordWith: "go.nextPage" },
  { id: "tool.highlight", labelKey: "tool.highlight", mac: ["H"], win: ["H"], when: "canvas", group: "tool" },
  { id: "tool.underline", labelKey: "tool.underline", mac: ["U"], win: ["U"], when: "canvas", group: "tool" },
  { id: "tool.strikeout", labelKey: "tool.strikeout", mac: ["K"], win: ["K"], when: "canvas", group: "tool" },
  { id: "tool.note", labelKey: "tool.note", mac: ["N"], win: ["N"], when: "canvas", group: "tool" },
  { id: "tool.pen", labelKey: "tool.pen", mac: ["P"], win: ["P"], when: "canvas", group: "tool" },
  { id: "tool.eraser", labelKey: "tool.eraser", mac: ["E"], win: ["E"], when: "canvas", group: "tool" },
  { id: "tool.rectangle", labelKey: "tool.rectangle", mac: ["R"], win: ["R"], when: "canvas", group: "tool" },
  { id: "tool.ellipse", labelKey: "tool.ellipse", mac: ["O"], win: ["O"], when: "canvas", group: "tool" },
  { id: "tool.line", labelKey: "tool.line", mac: ["L"], win: ["L"], when: "canvas", group: "tool" },
  { id: "tool.arrow", labelKey: "tool.arrow", mac: ["A"], win: ["A"], when: "canvas", group: "tool" },
  { id: "tool.textbox", labelKey: "tool.textbox", mac: ["T"], win: ["T"], when: "canvas", group: "tool" },
  { id: "tool.stamp", labelKey: "tool.stamp", mac: ["S"], win: ["S"], when: "canvas", group: "tool" },
  { id: "tool.signature", labelKey: "tool.signature", mac: ["G"], win: ["G"], when: "canvas", group: "tool" },
  { id: "tool.redact", labelKey: "tool.redact", mac: ["Shift+R"], win: ["Shift+R"], when: "canvas", group: "tool" },
  { id: "tool.none", labelKey: "menu.edit.deselect", mac: ["Escape"], win: ["Escape"], when: "canvas", group: "tool" },

  // 페이지 mode ---------------------------------------------------------------
  { id: "pages.rotateLeft", labelKey: "pages.rotateLeft", mac: ["Cmd+L"], win: ["Ctrl+L"], when: "pages", group: "pages" },
  { id: "pages.rotateRight", labelKey: "pages.rotateRight", mac: ["Cmd+R"], win: ["Ctrl+R"], when: "pages", group: "pages" },
  { id: "pages.delete", labelKey: "pages.delete", mac: ["Backspace"], win: ["Delete"], when: "pages", group: "pages" },
  { id: "pages.extract", labelKey: "pages.extract", mac: ["Alt+Cmd+X"], win: ["Ctrl+Alt+X"], when: "pages", group: "pages" },
  { id: "pages.insert", labelKey: "pages.insertBlank", mac: ["Alt+Cmd+I"], win: ["Ctrl+Alt+I"], when: "pages", group: "pages" },
  { id: "pages.selectAll", labelKey: "menu.edit.selectAll", mac: ["Cmd+A"], win: ["Ctrl+A"], when: "pages", group: "pages" },
];

export const KEYMAP_BY_ID: Record<string, KeyBinding> = Object.fromEntries(KEYMAP.map((b) => [b.id, b]));
/**
 * Native-menu items with no shortcut row above (the 도구 menu in `src-tauri/src/app/menu.rs`).
 * Without them here `menu:tools/…` would never reach the dispatcher. `settings` is the app menu's
 * 설정… (its id predates the keymap's `app.settings`), `app.checkUpdates` its 업데이트 확인….
 */
export const MENU_ONLY_IDS: readonly string[] = [
  "tools.ocr", "tools.batchOcr", "tools.security", "tools.compress", "tools.compare", "tools.merge",
  "settings", "app.checkUpdates",
  // P2: 보기 ▸ 이 페이지 읽어 주기
  "view.readAloud",
];
export const MENU_IDS: readonly string[] = [...KEYMAP.map((b) => b.id), ...MENU_ONLY_IDS];

export function chordsFor(binding: KeyBinding, os: OsName): string[] {
  return os === "windows" ? binding.win : binding.mac;
}

// ---------------------------------------------------------------------------
// Matching
// ---------------------------------------------------------------------------

export interface Chord {
  cmd: boolean;   // ⌘ on macOS, Ctrl on Windows (the "Cmd" token)
  ctrl: boolean;  // literal Control
  alt: boolean;
  shift: boolean;
  key: string;
}

const ALIASES: Record<string, string> = {
  esc: "Escape", del: "Delete", plus: "Plus", minus: "Minus", comma: "Comma", space: "Space",
};

export function parseChord(chord: string): Chord {
  const out: Chord = { cmd: false, ctrl: false, alt: false, shift: false, key: "" };
  for (const raw of chord.split("+")) {
    const token = raw.trim();
    const lower = token.toLowerCase();
    if (lower === "cmd" || lower === "meta") out.cmd = true;
    else if (lower === "ctrl" || lower === "control") out.ctrl = true;
    else if (lower === "alt" || lower === "option") out.alt = true;
    else if (lower === "shift") out.shift = true;
    else out.key = ALIASES[lower] ?? (token.length === 1 ? token.toUpperCase() : token);
  }
  return out;
}

/** Normalise a keyboard event to the token vocabulary of the table. */
export function eventTokens(e: Pick<KeyboardEvent, "key" | "code">): string[] {
  const tokens: string[] = [];
  const k = e.key;
  if (k === " ") tokens.push("Space");
  else if (k === ",") tokens.push("Comma");
  else if (k === "[") tokens.push("BracketLeft");
  else if (k === "]") tokens.push("BracketRight");
  else if (k === "+" || k === "=") tokens.push("Plus");
  else if (k === "-" || k === "_") tokens.push("Minus");
  else if (k.length === 1) tokens.push(k.toUpperCase());
  else tokens.push(k);
  // Alt on macOS rewrites e.key (Alt+E -> "´"), so always offer the physical key too.
  const code = e.code ?? "";
  if (code.startsWith("Key")) tokens.push(code.slice(3));
  else if (code.startsWith("Digit")) tokens.push(code.slice(5));
  else if (code === "Equal" || code === "NumpadAdd") tokens.push("Plus");
  else if (code === "Minus" || code === "NumpadSubtract") tokens.push("Minus");
  else if (code) tokens.push(code);
  return tokens;
}

export interface ModifierState { metaKey: boolean; ctrlKey: boolean; altKey: boolean; shiftKey: boolean }

export function matchesChord(
  e: ModifierState & Pick<KeyboardEvent, "key" | "code">,
  chord: string,
  os: OsName,
): boolean {
  const c = parseChord(chord);
  if (os === "macos") {
    // "Cmd" is Meta; "Ctrl" is the literal Control key (⌃⌘S is both).
    if (c.cmd !== e.metaKey) return false;
    if (c.ctrl !== e.ctrlKey) return false;
  } else {
    // On Windows/Linux both tokens collapse onto Control, and Meta is never part of a chord.
    if ((c.cmd || c.ctrl) !== e.ctrlKey) return false;
    if (e.metaKey) return false;
  }
  if (c.alt !== e.altKey) return false;
  if (c.shift !== e.shiftKey) return false;
  return eventTokens(e).includes(c.key);
}

/** Find the command for an event, honouring the active context. */
export function lookup(
  e: ModifierState & Pick<KeyboardEvent, "key" | "code">,
  os: OsName,
  contexts: KeyContext[],
): KeyBinding | undefined {
  return KEYMAP.find(
    (b) => contexts.includes(b.when) && chordsFor(b, os).some((chord) => matchesChord(e, chord, os)),
  );
}

// ---------------------------------------------------------------------------
// Display
// ---------------------------------------------------------------------------

const MAC_SYMBOLS: Record<string, string> = {
  Cmd: "⌘", Ctrl: "⌃", Alt: "⌥", Shift: "⇧",
  ArrowUp: "↑", ArrowDown: "↓", ArrowLeft: "←", ArrowRight: "→",
  Backspace: "⌫", Delete: "⌦", Escape: "⎋", Space: "␣", Enter: "↩",
  Comma: ",", BracketLeft: "[", BracketRight: "]", Plus: "+", Minus: "−",
  PageUp: "⇞", PageDown: "⇟", Home: "↖", End: "↘",
};

const WIN_NAMES: Record<string, string> = {
  Cmd: "Ctrl", Comma: ",", BracketLeft: "[", BracketRight: "]", Plus: "+", Minus: "-",
  ArrowUp: "↑", ArrowDown: "↓", ArrowLeft: "←", ArrowRight: "→",
};

/** "⌘⇧S" on macOS, "Ctrl+Shift+S" on Windows — for tooltips and menus (UI_SPEC §14.7 rule 5). */
export function formatChord(chord: string, os: OsName): string {
  const c = parseChord(chord);
  if (os === "macos") {
    let out = "";
    if (c.ctrl) out += MAC_SYMBOLS.Ctrl;
    if (c.alt) out += MAC_SYMBOLS.Alt;
    if (c.shift) out += MAC_SYMBOLS.Shift;
    if (c.cmd) out += MAC_SYMBOLS.Cmd;
    return out + (MAC_SYMBOLS[c.key] ?? c.key);
  }
  const parts: string[] = [];
  if (c.cmd) parts.push("Ctrl");
  if (c.ctrl && !c.cmd) parts.push("Ctrl");
  if (c.alt) parts.push("Alt");
  if (c.shift) parts.push("Shift");
  parts.push(WIN_NAMES[c.key] ?? c.key);
  return parts.join("+");
}

export function shortcutFor(id: string, os: OsName): string {
  const binding = KEYMAP_BY_ID[id];
  if (!binding) return "";
  const chord = chordsFor(binding, os)[0];
  return chord ? formatChord(chord, os) : "";
}
