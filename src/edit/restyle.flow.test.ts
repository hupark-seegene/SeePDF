/**
 * H5 (v0.3): 크기 / 색상 of a text object whose font the engine would have to substitute asks with
 * the same 글꼴이 대체됩니다 confirm as 문단 편집 — never a failure toast — and writes with
 * `allowFontSubstitution` only once the user agreed.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, waitFor } from "@testing-library/react";
import { mock } from "../ipc/mock";
import { useDocStore } from "../store/docStore";
import { useDialogStore } from "../dialogs/dialogState";
import { useToastStore } from "../app/toastStore";
import { restyleText } from "./actions";

const SAMPLE = "/Users/veri/Documents/SeePDF-샘플.pdf";

async function answer(value: boolean): Promise<Record<string, unknown>> {
  const entry = await waitFor(() => {
    const e = useDialogStore.getState().stack.find((d) => d.name === "confirm");
    expect(e).toBeDefined();
    return e!;
  });
  await act(async () => (entry.props.resolve as (v: boolean) => void)(value));
  return entry.props;
}

beforeEach(async () => {
  useDialogStore.getState().closeAll();
  useToastStore.setState({ toasts: [] });
  await useDocStore.getState().open(SAMPLE);
});

afterEach(() => vi.restoreAllMocks());

describe("restyleText", () => {
  it("a font substitution asks first; agreeing writes with allowFontSubstitution", async () => {
    const info = useDocStore.getState().info!;
    const objects = await mock.listPageObjects({ docId: info.docId, page: 0 });
    const text = objects.objects.find((o) => o.type === "text" && o.editable === "full")!;
    const real = mock.editTextObject.bind(mock);
    const edit = vi
      .spyOn(mock, "editTextObject")
      .mockRejectedValueOnce({ code: "fontCoverage", message: "the object's own font cannot draw this text" })
      .mockImplementation(real);
    const done = restyleText(0, text.objectId, { fontSizePt: 18 });
    const prompt = await answer(true);
    expect(prompt).toMatchObject({ titleKey: "edit.font.confirmTitle", confirmKey: "edit.font.replace" });
    expect(await done).toBe(true);
    expect(edit).toHaveBeenCalledTimes(2);
    expect(edit.mock.calls[0][0].allowFontSubstitution).toBe(false);
    expect(edit.mock.calls[1][0]).toMatchObject({ allowFontSubstitution: true, patch: { fontSizePt: 18 } });
    expect(useToastStore.getState().toasts.some((t) => t.tone === "danger")).toBe(false);
  });

  it("declining writes nothing and raises no error", async () => {
    const info = useDocStore.getState().info!;
    const objects = await mock.listPageObjects({ docId: info.docId, page: 0 });
    const text = objects.objects.find((o) => o.type === "text" && o.editable === "full")!;
    const edit = vi
      .spyOn(mock, "editTextObject")
      .mockRejectedValue({ code: "fontCoverage", message: "the object's own font cannot draw this text" });
    const done = restyleText(0, text.objectId, { color: [200, 0, 0] });
    await answer(false);
    expect(await done).toBe(false);
    expect(edit).toHaveBeenCalledTimes(1);
    expect(useToastStore.getState().toasts).toHaveLength(0);
  });
});
