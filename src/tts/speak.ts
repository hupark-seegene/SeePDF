/**
 * 읽어 주기 (P2): selected text or a whole page, read by the operating system's own voice (macOS
 * `say`, Windows System.Speech — offline, no download). One utterance at a time for the app; a new
 * one replaces the old. While it speaks, `tts_status` is polled so the floating bar and the status-bar
 * indicator disappear when the voice is done. Changing 속도 restarts the text at the new rate (the
 * system voices cannot change rate mid-utterance).
 *
 * Lazy: the canvas menus, the dispatcher and the bar import this on use.
 */
import * as api from "../ipc/api";
import { useDocStore } from "../store/docStore";
import { toast } from "../app/toastStore";
import { useTtsStore } from "./ttsStore";

/** How often the running voice is polled, in ms. */
export const POLL_MS = 500;

let timer: ReturnType<typeof setInterval> | null = null;

/** Whitespace folded (a PDF's line ends become pauses no voice needs), hyphenated line breaks joined. */
export function speakableText(text: string): string {
  return text
    .replace(/(\p{L})-\r?\n(\p{Ll})/gu, "$1$2")
    .replace(/\s+/g, " ")
    .trim();
}

function stopPolling(): void {
  if (timer !== null) clearInterval(timer);
  timer = null;
}

function startPolling(): void {
  stopPolling();
  timer = setInterval(() => {
    void api
      .ttsStatus()
      .then((s) => {
        if (!s.speaking) {
          stopPolling();
          useTtsStore.setState({ speaking: false });
        }
      })
      .catch(() => {
        stopPolling();
        useTtsStore.setState({ speaking: false });
      });
  }, POLL_MS);
}

/** Reads `text` aloud. Resolves `false` (after a toast) when there is nothing to read or no voice. */
export async function speakText(
  text: string,
  source: "selection" | "page",
  page: number | null = null,
): Promise<boolean> {
  const clean = speakableText(text);
  if (!clean) {
    toast("tts.noText", undefined, { tone: "info" });
    return false;
  }
  const rate = useTtsStore.getState().rate;
  try {
    const status = await api.ttsSpeak({ text: clean, rate });
    useTtsStore.setState({ speaking: status.speaking, text: clean, source, page, voice: status.voice });
    if (status.speaking) startPolling();
    return true;
  } catch (e) {
    useTtsStore.setState({ speaking: false });
    const unsupported = api.isSeePdfError(e) && e.code === "unsupported";
    toast(unsupported ? "tts.unsupported" : "tts.failed", undefined, {
      tone: unsupported ? "info" : "danger",
      detail: e instanceof Error ? e.message : String(e),
    });
    return false;
  }
}

/** 보기 ▸ 이 페이지 읽어 주기: the page's extracted text (`get_page_text`). */
export async function readPageAloud(page: number): Promise<boolean> {
  const info = useDocStore.getState().info;
  if (!info) return false;
  const text = await api.getPageText({ docId: info.docId, page }).catch(() => "");
  return speakText(text, "page", page + 1);
}

/** 정지. */
export async function stopSpeaking(): Promise<void> {
  stopPolling();
  useTtsStore.setState({ speaking: false });
  await api.ttsStop().catch(() => undefined);
}

/** 속도: remembered for the next utterance; the current one restarts at the new rate. */
export async function setSpeechRate(rate: number): Promise<void> {
  const { speaking, text, source, page } = useTtsStore.getState();
  useTtsStore.setState({ rate });
  if (speaking && text && source) await speakText(text, source, page);
}
