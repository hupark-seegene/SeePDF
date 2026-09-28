/**
 * P2 Bates numbering in 워터마크 / 머리글·바닥글: the pure helpers (`stamp.ts`) and the dialog's
 * preset, options and `add_stamp` payload against the mock adapter.
 */
import { beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import DialogHost from "./DialogHost";
import { openDialog, useDialogStore } from "./dialogState";
import { useDocStore } from "../store/docStore";
import { useToastStore } from "../app/toastStore";
import { mock } from "../ipc/mock";
import {
  batesLabel, batesOptions, batesPreset, buildStampSpec, expandTokens, initialStampForm, insertToken, usesBates,
} from "./stamp";

const SAMPLE = "/Users/veri/Documents/SeePDF-샘플.pdf"; // 3 pages

beforeEach(() => {
  useDialogStore.getState().closeAll();
  useToastStore.setState({ toasts: [] });
  vi.restoreAllMocks();
});

describe("dialogs.stamp bates helpers", () => {
  it("labels are padded, never cut, and follow the engine's limits", () => {
    const form = { ...initialStampForm("footer", "대외비"), batesStart: 101, batesPrefix: "ABC" };
    expect(batesLabel(form, 0)).toBe("ABC000101");
    expect(batesLabel(form, 13)).toBe("ABC000114");
    expect(batesLabel({ ...form, batesDigits: 2, batesStart: 99 }, 1)).toBe("ABC100");
    expect(batesOptions({ ...form, batesDigits: 40, batesStart: -5, batesPrefix: "x".repeat(80) })).toMatchObject({
      digits: 12, start: 0,
    });
    expect(batesOptions({ ...form, batesPrefix: "x".repeat(80) }).prefix).toHaveLength(64);
    const ctx = { page: 2, total: 3, date: "2026-09-28", filename: "a", bates: "ABC000102" };
    expect(expandTokens("{{bates}} · {{page}}", ctx)).toBe("ABC000102 · 2");
    expect(expandTokens("{{bates}}", { ...ctx, bates: undefined })).toBe("{{bates}}");
    expect(insertToken("Exhibit ", "bates").text).toBe("Exhibit {{bates}}");
  });

  it("the spec carries the Bates fields only when the text uses {{bates}}", () => {
    const plain = initialStampForm("footer", "대외비");
    expect(usesBates(plain.text)).toBe(false);
    expect(buildStampSpec(plain, [0], true)).not.toHaveProperty("batesStart");
    const preset = { ...batesPreset(plain), batesStart: 7, batesSuffix: "-K" };
    expect(preset).toMatchObject({ role: "footer", anchor: "br", text: "{{bates}}", rotateDeg: 0, opacityPct: 100 });
    expect(buildStampSpec(preset, [0, 1], false)).toMatchObject({
      role: "footer", pages: [0, 1], batesStart: 7, batesDigits: 6, batesPrefix: "", batesSuffix: "-K",
    });
    // a watermark becomes a footer, a header stays a header (top right)
    expect(batesPreset(initialStampForm("watermark", "대외비")).role).toBe("footer");
    expect(batesPreset(initialStampForm("header", "대외비"))).toMatchObject({ role: "header", anchor: "tr" });
  });
});

describe("dialogs.stamp bates flow", () => {
  it("Bates 번호 매기기 → start / prefix → preview and one 머리글/바닥글 step", async () => {
    await useDocStore.getState().open(SAMPLE);
    const spy = vi.spyOn(mock, "addStamp");
    render(<DialogHost />);
    openDialog("stamp");
    fireEvent.click(await screen.findByRole("button", { name: "Bates 번호 매기기" }));

    expect(screen.getByRole("radio", { name: "바닥글" })).toHaveAttribute("aria-checked", "true");
    expect(screen.getByRole("radio", { name: "오른쪽 아래" })).toHaveAttribute("aria-checked", "true");
    expect(screen.getByLabelText("텍스트", { selector: "textarea" })).toHaveValue("{{bates}}");
    fireEvent.change(screen.getByLabelText("시작 번호"), { target: { value: "101" } });
    fireEvent.change(screen.getByLabelText("접두어"), { target: { value: "ABC" } });
    expect(screen.getByTestId("stamp-preview-mark")).toHaveTextContent("ABC000101");
    expect(screen.getByTestId("bates-sample")).toHaveTextContent("ABC000101 … ABC000103");

    fireEvent.click(screen.getByRole("button", { name: "적용" }));
    await waitFor(() => expect(useDialogStore.getState().stack).toHaveLength(0));
    expect(spy.mock.calls[0][0].spec).toMatchObject({
      role: "footer", anchor: "br", pages: "all", source: { kind: "text", text: "{{bates}}" },
      batesStart: 101, batesDigits: 6, batesPrefix: "ABC", batesSuffix: "",
    });
    expect(useDocStore.getState().info?.undoLabel).toBe("undo.headerFooter");
  });

  it("the Bates options appear only while the text has the token", async () => {
    await useDocStore.getState().open(SAMPLE);
    render(<DialogHost />);
    openDialog("stamp", { role: "header" });
    const text = await screen.findByLabelText("텍스트", { selector: "textarea" });
    expect(screen.queryByLabelText("시작 번호")).toBeNull();
    fireEvent.change(text, { target: { value: "" } });
    fireEvent.click(screen.getByRole("button", { name: "Bates 번호" }));
    expect(text).toHaveValue("{{bates}}");
    expect(screen.getByLabelText("자릿수")).toHaveValue(6);
    fireEvent.change(text, { target: { value: "{{page}}" } });
    expect(screen.queryByLabelText("자릿수")).toBeNull();
  });
});
