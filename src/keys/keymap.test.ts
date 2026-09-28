import { describe, expect, it } from "vitest";
import { KEYMAP, chordsFor, formatChord, lookup, matchesChord, parseChord, shortcutFor, type KeyContext } from "./keymap";

function key(
  k: string,
  mods: Partial<{ metaKey: boolean; ctrlKey: boolean; altKey: boolean; shiftKey: boolean }> = {},
  code = "",
) {
  return { key: k, code, metaKey: false, ctrlKey: false, altKey: false, shiftKey: false, ...mods };
}

describe("keymap table", () => {
  it("has unique command ids", () => {
    const ids = KEYMAP.map((b) => b.id);
    expect(new Set(ids).size).toBe(ids.length);
  });

  it("has no duplicate chord inside one context, on either platform", () => {
    for (const os of ["macos", "windows"] as const) {
      const seen = new Map<string, string>();
      for (const binding of KEYMAP) {
        for (const chord of chordsFor(binding, os)) {
          const slot = `${binding.when}:${chord}`;
          const other = seen.get(slot);
          const documented = other !== undefined && binding.sharesChordWith === other;
          expect(other === undefined || documented, `${os} ${slot} used by ${other} and ${binding.id}`).toBe(true);
          if (!documented) seen.set(slot, binding.id);
        }
      }
    }
  });

  it("documents the only shared chord: Space = 다음 페이지 (tap) / 손 도구 (hold)", () => {
    const shared = KEYMAP.filter((b) => b.sharesChordWith);
    expect(shared.map((b) => b.id).sort()).toEqual(["go.nextPage", "tool.hand"]);
    expect(KEYMAP.find((b) => b.id === "tool.hand")?.momentary).toBe(true);
  });

  it("gives every binding a chord and an i18n label on both platforms", () => {
    for (const binding of KEYMAP) {
      expect(chordsFor(binding, "macos").length, binding.id).toBeGreaterThan(0);
      expect(chordsFor(binding, "windows").length, binding.id).toBeGreaterThan(0);
      expect(binding.labelKey).toMatch(/^[a-z]+\./);
    }
  });
});

describe("chord matching", () => {
  it("maps Cmd to Meta on macOS and Ctrl on Windows", () => {
    expect(matchesChord(key("s", { metaKey: true }), "Cmd+S", "macos")).toBe(true);
    expect(matchesChord(key("s", { ctrlKey: true }), "Cmd+S", "macos")).toBe(false);
    expect(matchesChord(key("s", { ctrlKey: true }), "Ctrl+S", "windows")).toBe(true);
    expect(matchesChord(key("s", { metaKey: true }), "Ctrl+S", "windows")).toBe(false);
  });

  it("keeps ⌃⌘S distinct from ⌘S on macOS", () => {
    expect(matchesChord(key("s", { metaKey: true, ctrlKey: true }), "Ctrl+Cmd+S", "macos")).toBe(true);
    expect(matchesChord(key("s", { metaKey: true }), "Ctrl+Cmd+S", "macos")).toBe(false);
  });

  it("falls back to the physical key when Alt rewrites e.key on macOS", () => {
    expect(matchesChord(key("´", { altKey: true, metaKey: true }, "KeyE"), "Alt+Cmd+E", "macos")).toBe(true);
  });

  it("looks a command up in the active contexts only", () => {
    expect(lookup(key("h"), "macos", ["always", "doc", "canvas"])?.id).toBe("tool.highlight");
    expect(lookup(key("h"), "macos", ["always", "doc"])).toBeUndefined();
    // ⌘L is 보기 회전 in the document context and 페이지 회전 in 페이지 mode
    expect(lookup(key("l", { metaKey: true }), "macos", ["always", "doc"])?.id).toBe("view.rotateLeft");
    expect(lookup(key("l", { metaKey: true }), "macos", ["pages"])?.id).toBe("pages.rotateLeft");
  });
});

describe("chord display", () => {
  it("renders mac symbols and Windows names", () => {
    expect(formatChord("Cmd+Shift+S", "macos")).toBe("⇧⌘S");
    expect(formatChord("Ctrl+Cmd+S", "macos")).toBe("⌃⌘S");
    expect(formatChord("Ctrl+Shift+S", "windows")).toBe("Ctrl+Shift+S");
    expect(formatChord("Backspace", "macos")).toBe("⌫");
    expect(shortcutFor("file.open", "macos")).toBe("⌘O");
    expect(shortcutFor("file.open", "windows")).toBe("Ctrl+O");
  });

  it("parses modifiers and named keys", () => {
    expect(parseChord("Alt+Cmd+G")).toEqual({ cmd: true, ctrl: false, alt: true, shift: false, key: "G" });
    expect(parseChord("PageDown").key).toBe("PageDown");
  });
});

describe("keymap reachability", () => {
  const NAMED: Record<string, string> = { Space: " ", Comma: ",", Plus: "+", Minus: "-", BracketLeft: "[", BracketRight: "]" };

  /** Every binding, pressed in each context set the shell uses: which row answers instead? */
  function shadows(): string[] {
    const out: string[] = [];
    for (const os of ["macos", "windows"] as const) {
      for (const contexts of [["always", "doc", "canvas"], ["always", "doc", "pages"], ["always"]] as KeyContext[][]) {
        for (const b of KEYMAP) {
          if (!contexts.includes(b.when) || b.native) continue;
          for (const chord of chordsFor(b, os)) {
            const c = parseChord(chord);
            const mac = os === "macos";
            const e = {
              key: c.key.length === 1 ? c.key.toLowerCase() : NAMED[c.key] ?? c.key,
              code: /^[A-Z]$/.test(c.key) ? `Key${c.key}` : /^[0-9]$/.test(c.key) ? `Digit${c.key}` : "",
              metaKey: mac ? c.cmd : false,
              ctrlKey: mac ? c.ctrl : c.cmd || c.ctrl,
              altKey: c.alt,
              shiftKey: c.shift,
            };
            const hit = lookup(e, os, contexts);
            if (hit?.id !== b.id) out.push(`${os} ${contexts.at(-1)}: ${b.id} → ${hit?.id}`);
          }
        }
      }
    }
    return out;
  }

  it("only the documented Space tap/hold and 페이지 mode's own rows win over another binding", () => {
    expect(shadows()).toEqual([
      "macos canvas: tool.hand → go.nextPage",
      "macos pages: edit.selectAll → pages.selectAll",
      "macos pages: view.rotateLeft → pages.rotateLeft",
      "macos pages: view.rotateRight → pages.rotateRight",
      "windows canvas: tool.hand → go.nextPage",
      // UI_SPEC §13: Ctrl+D is 문서 정보, but 복제 in 페이지 mode
      "windows pages: file.docInfo → edit.duplicate",
      "windows pages: edit.selectAll → pages.selectAll",
      "windows pages: view.rotateLeft → pages.rotateLeft",
      "windows pages: view.rotateRight → pages.rotateRight",
    ]);
  });

  it("never claims the chords the OS owns (⌘Q, Alt+F4)", () => {
    const none = { metaKey: false, ctrlKey: false, altKey: false, shiftKey: false };
    expect(lookup({ ...none, key: "q", code: "KeyQ", metaKey: true }, "macos", ["always", "doc", "canvas"])).toBeUndefined();
    expect(lookup({ ...none, key: "F4", code: "F4", altKey: true }, "windows", ["always", "doc", "canvas"])).toBeUndefined();
    expect(shortcutFor("app.quit", "macos")).toBe("⌘Q");
  });
});
