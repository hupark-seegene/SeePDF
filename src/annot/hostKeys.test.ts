/**
 * The annotation host's own ⌘/Ctrl + letter chords must not depend on the input source: with the
 * Korean input source on, `e.key` is the jamo (ㅇ for D), so the physical key decides — like
 * 편집 mode's `shortcutLetter`.
 */
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { waitFor } from "@testing-library/react";
import * as api from "../ipc/api";
import type { AnnotSpec } from "../ipc/types";
import { useAnnotStore } from "../store/annotStore";
import { useAppStore } from "../store/appStore";
import { useDocStore } from "../store/docStore";
import { createAnnotation, resetPatchQueue } from "./actions";
import { host } from "./AnnotationHost";

const SQUARE: AnnotSpec = { kind: "square", rect: { l: 10, b: 10, r: 60, t: 60 }, color: [245, 83, 61], fillColor: null, width: 2, opacity: 1 };

let stop: (() => void) | null = null;

describe("annotation host — ⌘D with the Korean input source", () => {
  beforeEach(() => {
    useAnnotStore.getState().reset();
    resetPatchQueue();
    useAppStore.setState({ mode: "annotate" });
    stop = host.start();
  });
  afterEach(() => {
    stop?.();
    stop = null;
  });

  it.each([
    ["d", "KeyD"],
    ["ㅇ", "KeyD"],
  ])("key %s (%s) duplicates the selection", async (key, code) => {
    const info = await useDocStore.getState().open("/tmp/sample.pdf");
    const annot = await createAnnotation(0, SQUARE);
    expect(annot).not.toBeNull();
    useAnnotStore.getState().select([annot!.id]);
    const before = (await api.listAnnotations({ docId: info!.docId, page: 0 })).annots.length;

    const event = new KeyboardEvent("keydown", { key, code, metaKey: true, bubbles: true, cancelable: true });
    window.dispatchEvent(event);
    expect(event.defaultPrevented).toBe(true);
    await waitFor(async () =>
      expect((await api.listAnnotations({ docId: info!.docId, page: 0 })).annots.length).toBe(before + 1),
    );
  });
});
