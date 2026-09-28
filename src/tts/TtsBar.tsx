/**
 * The 읽어 주기 floating bar (P2): what is being read, 속도 and 정지. Bottom centre, above the status
 * bar, clear of the toasts on the right. `App` mounts it lazily while the voice speaks.
 */
import { AudioLines, Square } from "lucide-react";
import { useT } from "../i18n/useT";
import { TTS_RATES, useTtsStore } from "./ttsStore";
import { setSpeechRate, stopSpeaking } from "./speak";
import "./tts.css";

export default function TtsBar() {
  const t = useT();
  const { speaking, rate, source, page, voice } = useTtsStore();
  if (!speaking) return null;
  const what = source === "page" && page !== null ? t("a11y.page", { n: page }) : t("tts.readSelection");

  return (
    <div className="tts-bar" role="region" aria-label={t("tts.bar")} data-testid="tts-bar">
      <AudioLines className="tts-wave" size={16} strokeWidth={1.75} aria-hidden />
      <span className="text-sm">{t("tts.speaking")}</span>
      <span className="text-xs dim tts-what">{what}</span>
      {voice && <span className="text-xs dim">{t("tts.voice", { voice })}</span>}
      <label className="inline-row text-xs dim">
        {t("tts.rate")}
        <select
          className="tts-rate text-sm mono"
          aria-label={t("tts.rate")}
          value={String(rate)}
          onChange={(e) => void setSpeechRate(Number(e.target.value))}
        >
          {TTS_RATES.map((r) => (
            <option key={r} value={String(r)}>{`${r}×`}</option>
          ))}
        </select>
      </label>
      <button type="button" className="btn tts-stop" onClick={() => void stopSpeaking()}>
        <Square size={14} strokeWidth={2} aria-hidden />
        {t("tts.stop")}
      </button>
    </div>
  );
}
