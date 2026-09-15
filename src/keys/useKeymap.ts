/**
 * Binds the keymap to the window. One listener for the whole app; the command id is dispatched to
 * the handler, which is also what the native menu (`menu:<id>`) calls — one code path for both.
 *
 * Guards (UI_SPEC §13): tool and navigation keys are live only while the canvas has focus and no
 * input is editing. Momentary tools (`Space` = 손) fire `onRelease` when the key goes up.
 */
import { useEffect, useRef } from "react";
import { chordsFor, eventTokens, lookup, parseChord, type KeyBinding, type KeyContext } from "./keymap";
import type { OsName } from "../ipc/env";

export interface KeymapOptions {
  os: OsName;
  contexts: KeyContext[];
  onCommand(id: string, binding: KeyBinding, e: KeyboardEvent): void;
  onRelease?(id: string, binding: KeyBinding): void;
  enabled?: boolean;
}

/** True while the user is typing into a field — single-key bindings must not fire. */
export function isEditingTarget(target: EventTarget | null): boolean {
  const el = target as HTMLElement | null;
  if (!el || !el.tagName) return false;
  const tag = el.tagName.toLowerCase();
  return tag === "input" || tag === "textarea" || tag === "select" || el.isContentEditable === true;
}

/** A chord with no modifier is a "bare" key — blocked while an input is focused. */
export function isBareChord(chord: string): boolean {
  const c = parseChord(chord);
  return !c.cmd && !c.ctrl && !c.alt;
}

export function useKeymap(opts: KeymapOptions): void {
  const ref = useRef(opts);
  ref.current = opts;

  useEffect(() => {
    /** key token -> momentary binding currently held down */
    const held = new Map<string, KeyBinding>();

    const onKeyDown = (e: KeyboardEvent) => {
      const { os, contexts, onCommand } = ref.current;
      if (ref.current.enabled === false) return;
      const editing = isEditingTarget(e.target);
      const active = editing ? contexts.filter((c) => c !== "canvas" && c !== "pages") : contexts;
      const binding = lookup(e, os, active);
      if (!binding) return;
      const chord = chordsFor(binding, os).find(() => true) ?? "";
      if (editing && isBareChord(chord)) return;
      if (binding.momentary) {
        const token = parseChord(chord).key;
        if (held.has(token)) return;
        held.set(token, binding);
      } else if (e.repeat && binding.group === "tool") {
        return;
      }
      e.preventDefault();
      onCommand(binding.id, binding, e);
    };

    const onKeyUp = (e: KeyboardEvent) => {
      if (held.size === 0) return;
      const tokens = eventTokens(e);
      for (const token of tokens) {
        const binding = held.get(token);
        if (binding) {
          held.delete(token);
          ref.current.onRelease?.(binding.id, binding);
        }
      }
    };

    const onBlur = () => {
      for (const [token, binding] of held) {
        held.delete(token);
        ref.current.onRelease?.(binding.id, binding);
      }
    };

    window.addEventListener("keydown", onKeyDown);
    window.addEventListener("keyup", onKeyUp);
    window.addEventListener("blur", onBlur);
    return () => {
      window.removeEventListener("keydown", onKeyDown);
      window.removeEventListener("keyup", onKeyUp);
      window.removeEventListener("blur", onBlur);
    };
  }, []);
}
