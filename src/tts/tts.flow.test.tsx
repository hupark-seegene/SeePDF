/**
 * P2 읽어 주기 against the mock adapter: 읽어 주기 in the text-selection menu, 이 페이지 읽어 주기
 * (canvas menu and the dispatcher's `view.readAloud`), the floating bar's 속도 / 정지, the status-bar
 * indicator, the voice ending on its own (polling), and the "no voice" / "no text" answers.
 *
 * v0.3 (V4 / V6): the text goes to the engine as sentences; `tts-progress` highlights the sentence
 * being read on its page; 속도 continues from that sentence; a tagged page is read in its structure
 * tree's order.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { mock, mockEvents, mockTts } from "../ipc/mock";
import { ensureTextLayer, getTextLayer, makePageLayerContext, useSelectionStore } from "../viewer";
import { resetTextLayers } from "../viewer/text/textLayers";
import { resetReadingOrders } from "../viewer/text/readingOrder";
import { useAppStore } from "../store/appStore";
import { useDocStore } from "../store/docStore";
import { useToastStore } from "../app/toastStore";
import { isSeparator, useContextMenuStore, type MenuItem } from "../app/contextMenuStore";
import { openPageContextMenu } from "../app/pageMenus";
import { StatusBar } from "../app/StatusBar";
import TtsBar from "./TtsBar";
import { TtsHighlight } from "./TtsHighlight";
import { useTtsStore } from "./ttsStore";
import { readPageAloud, speakText, speakableText, stopSpeaking } from "./speak";
import { pageSentences } from "./sentences";

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
  useTtsStore.setState({
    speaking: false, text: "", source: null, page: null, voice: null, rate: 1,
    sentences: [], index: null, docId: null, docGeneration: null,
  });
  useToastStore.setState({ toasts: [] });
  resetTextLayers();
  resetReadingOrders();
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
    const call = speak.mock.calls[0][0];
    expect(call.sentences?.join(" ").length).toBeGreaterThan(10);
    expect(call.startIndex).toBe(0);
    expect(call.rate).toBe(1);
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

  it("이 페이지 읽어 주기 reads the page's sentences; 속도 continues from the current sentence", async () => {
    const info = await openDoc();
    const speak = vi.spyOn(mock, "ttsSpeak");
    const readPage = menuItems().find((i) => i.id === "readPage");
    expect(readPage?.labelKey).toBe("menu.view.readAloud");
    readPage!.onSelect?.();
    await waitFor(() => expect(speak).toHaveBeenCalledTimes(1));
    const first = speak.mock.calls[0][0];
    // the page's text, split into sentences — and nothing of it lost
    const pageText = await mock.getPageText({ docId: info.docId, page: 0 });
    expect(first.sentences!.length).toBeGreaterThan(2);
    expect(first.sentences!.join(" ")).toBe(speakableText(pageText));
    expect(first.sentences).toContain("선택과 검색을 실제 단어 상자로 시험할 수 있습니다.");
    await waitFor(() => expect(useTtsStore.getState()).toMatchObject({ speaking: true, source: "page", page: 1 }));

    render(<TtsBar />);
    expect(screen.getByTestId("tts-bar")).toHaveTextContent("1쪽");
    // the engine reports the third sentence, then the speed changes: it continues from there
    act(() => mockEvents.emit("tts-progress", { sentenceIndex: 2 }));
    expect(useTtsStore.getState().index).toBe(2);
    fireEvent.change(screen.getByLabelText("속도"), { target: { value: "1.5" } });
    await waitFor(() => expect(speak).toHaveBeenCalledTimes(2));
    expect(speak.mock.calls[1][0]).toEqual({ sentences: first.sentences, startIndex: 2, rate: 1.5 });
    expect(useTtsStore.getState().rate).toBe(1.5);
    // the end of the queue ends the bar
    act(() => mockEvents.emit("tts-progress", { sentenceIndex: null }));
    expect(useTtsStore.getState().speaking).toBe(false);
  });

  it("a progress event highlights that sentence's rectangles on its page", async () => {
    const info = await openDoc();
    await readPageAloud(0);
    const { sentences } = useTtsStore.getState();
    const layer = getTextLayer(info.docId, info.docGeneration, 0)!;
    expect(layer).not.toBeNull();
    const ctx = makePageLayerContext({
      docId: info.docId,
      docGeneration: info.docGeneration,
      page: info.pages[0],
      rotation: 0,
      zoomPercent: 100,
      width: info.pages[0].widthPt,
      height: info.pages[0].heightPt,
    });
    render(<TtsHighlight ctx={ctx} />);
    const target = sentences.findIndex((s) => s.text.startsWith("선택과 검색을"));
    act(() => mockEvents.emit("tts-progress", { sentenceIndex: target }));
    const marks = await screen.findAllByTestId("tts-highlight");
    const expected = sentences[target].ranges.flatMap(([a, b]) => layer.rangeRects(a, b));
    expect(marks).toHaveLength(expected.length);
    const box = ctx.rectToBox(expected[0]);
    expect(parseFloat(marks[0].style.left)).toBeCloseTo(box.x, 3);
    expect(parseFloat(marks[0].style.top)).toBeCloseTo(box.y, 3);
    // the highlighted characters are exactly that sentence
    const text = sentences[target].ranges.map(([a, b]) => layer.rangeText(a, b)).join("");
    expect(speakableText(text)).toBe(sentences[target].text);
    // another page's sentence paints nothing here
    act(() => useTtsStore.setState({ sentences: [{ ...sentences[target], page: 3 }], index: 0 }));
    expect(screen.queryAllByTestId("tts-highlight")).toHaveLength(0);
  });

  it("a tagged page is read in its structure tree's order (V6)", async () => {
    const info = await openDoc();
    useDocStore.setState({ info: { ...info, tagged: true } });
    const layer = await import("../viewer/text/textLayers").then((m) => m.loadTextLayer(info.docId, info.docGeneration, 0));
    // the tree reads the page's second half first
    const half = layer.lineRange(Math.floor(layer.charCount / 2))[0];
    const order = vi
      .spyOn(mock, "getReadingOrder")
      .mockResolvedValue({ tagged: true, runs: [[half, layer.charCount], [0, half]] });
    const speak = vi.spyOn(mock, "ttsSpeak");
    await readPageAloud(0);
    expect(order).toHaveBeenCalledWith({ docId: info.docId, page: 0 });
    const sent = speak.mock.calls[0][0].sentences!;
    expect(sent[0]).toBe(pageSentences(layer, [[half, layer.charCount]], 0)[0].text);
    expect(sent.join(" ").indexOf("The mock text layer")).toBeLessThan(sent.join(" ").indexOf("SeePDF"));
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
    vi.spyOn(mock, "getTextLayer").mockRejectedValue({ code: "notFound", message: "no text" });
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
