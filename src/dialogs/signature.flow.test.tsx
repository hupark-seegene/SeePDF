/**
 * 서명 만들기 (P1-9) as a user drives it: the 입력 tab renders a typed name to a PNG that is
 * written through `write_signature_image` and placed as an image; 이 서명 저장 keeps drawn and
 * typed signatures in `Settings.signatures` (max 10); a saved one is placed with one click and
 * deleted with ×. `renderTypedSignature` is mocked — jsdom has no 2D canvas.
 */
import { beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import * as api from "../ipc/api";
import type { SavedSignature } from "../ipc/types";
import { useAppStore } from "../store/appStore";
import { SignatureDialog } from "./SignatureDialog";
import { renderTypedSignature } from "./typedSignature";

vi.mock("./typedSignature", async (importOriginal) => ({
  ...(await importOriginal<typeof import("./typedSignature")>()),
  renderTypedSignature: vi.fn(async () => ({
    bytes: new Uint8Array([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 1, 2, 3]),
    aspect: 4,
  })),
}));

beforeAll(() => {
  // jsdom has no canvas backend; the sheet only needs `getContext` not to throw.
  HTMLCanvasElement.prototype.getContext = (() => null) as unknown as HTMLCanvasElement["getContext"];
});

function props() {
  return { onClose: vi.fn(), onDrawn: vi.fn(), onImage: vi.fn(), onChooseImage: vi.fn() };
}

const signatures = () => useAppStore.getState().settings?.signatures ?? [];

describe("서명 만들기 — 입력 / 보관함", () => {
  beforeEach(async () => {
    await useAppStore.getState().bootstrap();
    await useAppStore.getState().patchSettings({ signatures: [] });
    vi.mocked(renderTypedSignature).mockClear();
  });

  it("types a name, picks a style, saves it and places it as an image", async () => {
    const write = vi.spyOn(api, "writeSignatureImage");
    const p = props();
    render(<SignatureDialog {...p} />);

    fireEvent.click(screen.getByRole("tab", { name: "입력" }));
    fireEvent.change(screen.getByLabelText("이름을 입력하세요"), { target: { value: "  박현우 " } });
    fireEvent.click(screen.getByRole("radio", { name: /손글씨/ }));
    fireEvent.click(screen.getByRole("checkbox", { name: /이 서명 저장/ }));
    fireEvent.click(screen.getByRole("button", { name: "확인" }));

    await waitFor(() => expect(p.onImage).toHaveBeenCalledTimes(1));
    expect(renderTypedSignature).toHaveBeenCalledWith("박현우", "hand");
    expect(write).toHaveBeenCalledTimes(1);
    const placed = p.onImage.mock.calls[0][0];
    expect(placed.aspect).toBe(4);
    expect(placed.path).toMatch(/\/signatures\/sig-[0-9a-f]+\.png$/);
    expect(p.onClose).toHaveBeenCalled();
    await waitFor(() => expect(signatures()).toHaveLength(1));
    expect(signatures()[0]).toMatchObject({ kind: "typed", text: "박현우", style: "hand" });
    write.mockRestore();
  });

  it("does not save unless 이 서명 저장 is ticked", async () => {
    const p = props();
    render(<SignatureDialog {...p} />);
    fireEvent.click(screen.getByRole("tab", { name: "입력" }));
    fireEvent.change(screen.getByLabelText("이름을 입력하세요"), { target: { value: "Kim" } });
    fireEvent.click(screen.getByRole("button", { name: "확인" }));
    await waitFor(() => expect(p.onImage).toHaveBeenCalled());
    expect(signatures()).toHaveLength(0);
  });

  it("saves a drawn signature and places a saved one with a click; × deletes it", async () => {
    const p = props();
    const { unmount } = render(<SignatureDialog {...p} />);
    const canvas = screen.getByLabelText("아래 영역에 서명하세요");
    fireEvent.pointerDown(canvas, { clientX: 10, clientY: 20, pointerId: 1 });
    fireEvent.pointerMove(canvas, { clientX: 110, clientY: 60, pointerId: 1 });
    fireEvent.pointerUp(canvas, { pointerId: 1 });
    fireEvent.click(screen.getByRole("checkbox", { name: /이 서명 저장/ }));
    fireEvent.click(screen.getByRole("button", { name: "확인" }));

    expect(p.onDrawn).toHaveBeenCalledTimes(1);
    const drawn = p.onDrawn.mock.calls[0][0];
    expect(drawn.paths[0]).toEqual([0, 0, 1, 1]);
    await waitFor(() => expect(signatures()).toHaveLength(1));
    expect(signatures()[0]).toMatchObject({ kind: "drawn", paths: [[0, 0, 1, 1]] });
    unmount();

    // Reopen: the saved one is in the 보관함; a click places it without drawing again.
    const q = props();
    render(<SignatureDialog {...q} />);
    fireEvent.click(screen.getByRole("button", { name: "서명 사용: 그린 서명" }));
    expect(q.onDrawn).toHaveBeenCalledWith({ paths: [[0, 0, 1, 1]], aspect: drawn.aspect });
    expect(q.onClose).toHaveBeenCalled();

    fireEvent.click(screen.getByRole("button", { name: "서명 삭제" }));
    await waitFor(() => expect(signatures()).toHaveLength(0));
  });

  it("places a saved typed signature by re-rendering it", async () => {
    await useAppStore.getState().patchSettings({
      signatures: [{ kind: "typed", id: "s1", text: "홍길동", style: "formal", createdAt: "" }],
    });
    const p = props();
    render(<SignatureDialog {...p} />);
    fireEvent.click(screen.getByRole("button", { name: "서명 사용: 홍길동" }));
    await waitFor(() => expect(p.onImage).toHaveBeenCalledTimes(1));
    expect(renderTypedSignature).toHaveBeenCalledWith("홍길동", "formal");
  });

  it("caps the 보관함 at 10: the save box is disabled and says why", async () => {
    const full: SavedSignature[] = Array.from({ length: 10 }, (_, i) => ({
      kind: "typed", id: `t${i}`, text: `N${i}`, style: "script", createdAt: "",
    }));
    await useAppStore.getState().patchSettings({ signatures: full });
    render(<SignatureDialog {...props()} />);
    expect(screen.getByRole("checkbox", { name: /이 서명 저장/ })).toBeDisabled();
    expect(screen.getByText("보관함이 가득 찼습니다 (최대 10개)")).toBeInTheDocument();
    expect(screen.getByText("10/10")).toBeInTheDocument();
  });

  it("이미지 tab hands over to the file picker", () => {
    const p = props();
    render(<SignatureDialog {...p} />);
    fireEvent.click(screen.getByRole("tab", { name: "이미지" }));
    fireEvent.click(screen.getByRole("button", { name: "이미지 선택…" }));
    expect(p.onChooseImage).toHaveBeenCalled();
  });
});
