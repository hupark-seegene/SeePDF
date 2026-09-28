/**
 * P2 읽어 주기 against the mock adapter: 읽어 주기 in the text-selection menu, 이 페이지 읽어 주기
 * (canvas menu and the dispatcher's `view.readAloud`), the floating bar's 속도 / 정지, the status-bar
 * indicator, the voice ending on its own (polling), and the "no voice" / "no text" answers.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { mock, mockTts } from "../ipc/mock";
import { ensureTextLayer, getTextLayer, useSelectionStore } from "../viewer";
import { useAppStore } from "../store/appStore";
import { useDocStore } from "../store/docStore";
import { useToastStore } from "../app/toastStore";
import { isSeparator, useContextMenuStore, type MenuItem } from "../app/contextMenuStore";
import { openPageContextMenu } from "../app/pageMenus";
import { StatusBar } from "../app/StatusBar";
import TtsBar from "./TtsBar";
import { useTtsStore } from "./ttsStore";
import { readPageAloud, speakText, speakableText, stopSpeaking } from "./speak";

async function openDoc() {
  const info = await useDocStore.getState().open("/tmp/sample.pdf");
  if (!info) throw new Error("mock open failed");
  useAppStore.setState({ mode: "read", tool: "select" });
  return info;
}

function menuItems(): MenuItem[] {
  openPageContextMenu(0, "canvas", 10, 10);
  return (useContextMenuStore.getState().menu?.items ?? []).filter((i): i is MenuItem => !isSeparator(i));
}

beforeEach(() => {
  useTtsStore.setState({ speaking: false, text: "", source: null, page: null, voice: null, rate: 1 });
  useToastStore.setState({ toasts: [] });
});

afterEach(async () => {
  await stopSpeaking();
  useContextMenuStore.getState().close();
  useSelectionStore.getState().clear();
  vi.restoreAllMocks();
});

describe("tts.flow", () => {
  it("읽어 주기 in the selection menu reads the selected text; 정지 stops it", async () => {
    const info = await openDoc();
    ensureTextLayer(info.docId, info.docGeneration, 0);
    await waitFor(() => expect(getTextLayer(info.docId, info.docGeneration, 0)).not.toBeNull());
    useSelectionStore.getState().setSelection({ docId: info.docId, anchor: { page: 0, offset: 0 }, focus: { page: 0, offset: 40 } });
    const speak = vi.spyOn(mock, "ttsSpeak");
    const item = menuItems().find((i) => i.id === "readSelection");
    expect(item?.labelKey).toBe("tts.readSelection");
    item!.onSelect?.();
    await waitFor(() => expect(speak).toHaveBeenCalledTimes(1));
    expect(speak.mock.calls[0][0].text.length).toBeGreaterThan(10);
    expect(speak.mock.calls[0][0].rate).toBe(1);
    await waitFor(() => expect(useTtsStore.getState().speaking).toBe(true));
    expect(useTtsStore.getState().source).toBe("selection");

    render(<TtsBar />);
    expect(screen.getByTestId("tts-bar")).toHaveTextContent("읽는 중…");
    const stop = vi.spyOn(mock, "ttsStop");
    fireEvent.click(screen.getByRole("button", { name: "정지" }));
    await waitFor(() => expect(stop).toHaveBeenCalledTimes(1));
    expect(useTtsStore.getState().speaking).toBe(false);
    expect(mockTts.speaking).toBe(false);
  });

  it("이 페이지 읽어 주기 reads get_page_text; 속도 restarts at the new rate", async () => {
    await openDoc();
    const speak = vi.spyOn(mock, "ttsSpeak");
    const readPage = menuItems().find((i) => i.id === "readPage");
    expect(readPage?.labelKey).toBe("menu.view.readAloud");
    readPage!.onSelect?.();
    await waitFor(() => expect(speak).toHaveBeenCalledTimes(1));
    const pageText = await mock.getPageText({ docId: useDocStore.getState().info!.docId, page: 0 });
    expect(speak.mock.calls[0][0].text).toBe(speakableText(pageText));
    await waitFor(() => expect(useTtsStore.getState()).toMatchObject({ speaking: true, source: "page", page: 1 }));

    render(<TtsBar />);
    expect(screen.getByTestId("tts-bar")).toHaveTextContent("1쪽");
    fireEvent.change(screen.getByLabelText("속도"), { target: { value: "1.5" } });
    await waitFor(() => expect(speak).toHaveBeenCalledTimes(2));
    expect(speak.mock.calls[1][0]).toEqual({ text: speak.mock.calls[0][0].text, rate: 1.5 });
    expect(useTtsStore.getState().rate).toBe(1.5);
  });

  it("the bar and the status-bar indicator go away when the voice ends by itself", async () => {
    await openDoc();
    await speakText("안녕하세요. 읽어 주기 시험입니다.", "selection");
    expect(useTtsStore.getState()).toMatchObject({ speaking: true, voice: "Yuna" });
    render(<StatusBar />);
    const indicator = screen.getByRole("button", { name: "읽는 중… — 정지" });
    expect(indicator).toBeInTheDocument();
    // the voice finishes: the next poll notices
    mockTts.speaking = false;
    await waitFor(() => expect(useTtsStore.getState().speaking).toBe(false), { timeout: 2000 });
    expect(screen.queryByRole("button", { name: "읽는 중… — 정지" })).toBeNull();
  });

  it("no text and no system voice answer with a note, not an error", async () => {
    await openDoc();
    const speak = vi.spyOn(mock, "ttsSpeak");
    vi.spyOn(mock, "getPageText").mockResolvedValue(" \r\n ");
    expect(await readPageAloud(0)).toBe(false);
    expect(speak).not.toHaveBeenCalled();
    expect(useToastStore.getState().toasts.at(-1)?.messageKey).toBe("tts.noText");

    speak.mockRejectedValue({ code: "unsupported", message: "no system voice on this platform" });
    expect(await speakText("hello", "selection")).toBe(false);
    expect(useToastStore.getState().toasts.at(-1)).toMatchObject({ messageKey: "tts.unsupported", tone: "info" });
    expect(useTtsStore.getState().speaking).toBe(false);
  });

  it("speakable text folds whitespace and joins hyphenated line breaks", () => {
    expect(speakableText("Trace-\r\nbased  compi-\nlation\r\n\r\nNext")).toBe("Tracebased compilation Next");
    expect(speakableText("한국어\r\n문장")).toBe("한국어 문장");
  });
});
