/**
 * 읽어 주기 (P2): selected text or a whole page, read by the operating system's own voice (macOS
 * `say`, Windows System.Speech — offline, no download). One utterance at a time for the app; a new
 * one replaces the old. While it speaks, `tts_status` is polled so the floating bar and the status-bar
 * indicator disappear when the voice is done.
 *
 * v0.3 (V4): the text goes to the engine **as sentences** (`sentences.ts`), queued there and spoken
 * one by one; `tts-progress` names the sentence being read, which the page highlights, and a 속도
 * change restarts at that sentence instead of at the beginning (the system voices cannot change rate
 * mid-utterance). A page is read in its reading order — the structure tree's on a tagged PDF (V6).
 *
 * Lazy: the canvas menus, the dispatcher and the bar import this on use.
 */
import * as api from "../ipc/api";
import { onTtsProgress } from "../ipc/events";
import { useDocStore } from "../store/docStore";
import { toast } from "../app/toastStore";
import { loadTextLayer } from "../viewer/text/textLayers";
import { readingOrderFor } from "../viewer/text/readingOrder";
import { useTtsStore } from "./ttsStore";
import { pageSentences, speakableText, textSentences, type TtsSentence } from "./sentences";

export { speakableText };

/** How often the running voice is polled, in ms. */
export const POLL_MS = 500;

let timer: ReturnType<typeof setInterval> | null = null;
let offProgress: (() => void) | null = null;

function stopPolling(): void {
  if (timer !== null) clearInterval(timer);
  timer = null;
}

function finished(): void {
  stopPolling();
  useTtsStore.setState({ speaking: false, index: null });
}

function startPolling(): void {
  stopPolling();
  timer = setInterval(() => {
    void api
      .ttsStatus()
      .then((s) => {
        if (!s.speaking) finished();
        else if (typeof s.sentenceIndex === "number" && s.sentenceIndex !== useTtsStore.getState().index) {
          useTtsStore.setState({ index: s.sentenceIndex });
        }
      })
      .catch(finished);
  }, POLL_MS);
}

/** `tts-progress`: the sentence the engine started (`null`: the queue is done). */
function listenProgress(): void {
  // re-subscribed per utterance: cheap, and it survives a torn-down event bus (tests, reloads)
  offProgress?.();
  offProgress = onTtsProgress((e) => {
    if (!useTtsStore.getState().speaking) return;
    if (e.sentenceIndex === null) finished();
    else useTtsStore.setState({ index: e.sentenceIndex });
  });
}

/** Reads sentences from `startIndex` on. Resolves `false` (after a toast) when there is no voice. */
export async function speakSentences(
  sentences: TtsSentence[],
  source: "selection" | "page",
  page: number | null = null,
  startIndex = 0,
): Promise<boolean> {
  if (sentences.length === 0) {
    toast("tts.noText", undefined, { tone: "info" });
    return false;
  }
  const rate = useTtsStore.getState().rate;
  const info = useDocStore.getState().info;
  listenProgress();
  try {
    const start = Math.max(0, Math.min(sentences.length - 1, startIndex));
    const status = await api.ttsSpeak({ sentences: sentences.map((s) => s.text), startIndex: start, rate });
    useTtsStore.setState({
      speaking: status.speaking,
      text: sentences.map((s) => s.text).join(" "),
      source,
      page,
      voice: status.voice,
      sentences,
      index: typeof status.sentenceIndex === "number" ? status.sentenceIndex : start,
      docId: info?.docId ?? null,
      docGeneration: info?.docGeneration ?? null,
    });
    if (status.speaking) startPolling();
    return true;
  } catch (e) {
    useTtsStore.setState({ speaking: false, index: null });
    const unsupported = api.isSeePdfError(e) && e.code === "unsupported";
    toast(unsupported ? "tts.unsupported" : "tts.failed", undefined, {
      tone: unsupported ? "info" : "danger",
      detail: e instanceof Error ? e.message : String(e),
    });
    return false;
  }
}

/** Reads `text` aloud (a selection): sentence by sentence, with no place on the page. */
export async function speakText(
  text: string,
  source: "selection" | "page",
  page: number | null = null,
): Promise<boolean> {
  return speakSentences(textSentences(text), source, page);
}

/** 보기 ▸ 이 페이지 읽어 주기: the page's text layer, in reading order, sentence by sentence. */
export async function readPageAloud(page: number): Promise<boolean> {
  const info = useDocStore.getState().info;
  if (!info) return false;
  const layer = await loadTextLayer(info.docId, info.docGeneration, page).catch(() => null);
  if (!layer || layer.charCount === 0) {
    toast("tts.noText", undefined, { tone: "info" });
    return false;
  }
  const runs = await readingOrderFor(info.docId, info.docGeneration, page, info.tagged, layer.charCount);
  return speakSentences(pageSentences(layer, runs, page), "page", page + 1);
}

/** 정지. */
export async function stopSpeaking(): Promise<void> {
  stopPolling();
  useTtsStore.setState({ speaking: false, index: null });
  await api.ttsStop().catch(() => undefined);
}

/** 속도: remembered for the next utterance; the current one continues at the new rate from its sentence. */
export async function setSpeechRate(rate: number): Promise<void> {
  const { speaking, sentences, source, page, index } = useTtsStore.getState();
  useTtsStore.setState({ rate });
  if (speaking && sentences.length && source) await speakSentences(sentences, source, page, index ?? 0);
}
