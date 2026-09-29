/**
 * 도장 선택 (P1-12): what arming 도장 opens when no stamp is chosen yet, and what the properties
 * panel's 도장 변경… reopens. The Korean set (결재 / 승인 / 기밀) comes first, then the Latin
 * built-ins, then 이미지 선택… for a scanned seal. A click chooses and closes; the stamp then
 * follows the cursor as a ghost and the next click on a page places it.
 *
 * v0.3 T2 adds:
 *
 * * **빠른 표시** — ✓ ✗ ● and 오늘 날짜 (a borderless `{{date}}` text stamp);
 * * **내 도장** — the user's own stamps in `Settings.stamps`: an image (copied under
 *   `$APPDATA/SeePDF/stamps/` by `copy_library_image`, so it survives the original moving) or a
 *   text stamp (text, colour, 사각 / 둥근 / 테두리 없음) whose `{{date}}` and `{{author}}` the
 *   engine expands at placement. Add and delete here; at most 30.
 */
import { useMemo, useState } from "react";
import { X } from "lucide-react";
import * as api from "../ipc/api";
import type { CustomStamp, Rgb, StampShape } from "../ipc/types";
import { useT } from "../i18n/useT";
import { rgbCss } from "../app/Swatches";
import { useAppStore } from "../store/appStore";
import { toast } from "../app/toastStore";
import { BUILTIN_STAMPS, QUICK_MARKS, QUICK_MARK_COLOR, TODAY_STAMP } from "../tools/stampCatalog";
import { Dialog } from "./Dialog";
import "./dialogs.css";

/** Same cap as the Rust side (`MAX_CUSTOM_STAMPS`). */
export const MAX_CUSTOM_STAMPS = 30;

/** The text stamp colours on offer: 인주 red, ink blue, black. */
const TEXT_COLORS: { key: string; rgb: Rgb }[] = [
  { key: "color.red", rgb: [206, 32, 41] },
  { key: "color.blue", rgb: [24, 64, 170] },
  { key: "color.black", rgb: [43, 47, 51] },
];
const SHAPES: StampShape[] = ["rect", "round", "none"];

const MARK_GLYPH: Record<string, string> = { check: "✓", cross: "✗", dot: "●" };

export interface StampPickerProps {
  onClose(): void;
  onPick(builtin: string): void;
  onChooseImage(): void;
  /** v0.3 T2: a 내 도장 entry or 오늘 날짜 was chosen */
  onPickCustom?(stamp: CustomStamp): void;
  /** the built-in chosen last time, highlighted */
  current?: string | null;
}

function newId(): string {
  const c = globalThis.crypto;
  return c?.randomUUID ? c.randomUUID() : `stamp-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 8)}`;
}

/** `Settings.stamps`, defended against a hand-edited or older file. */
export function readStamps(value: unknown): CustomStamp[] {
  if (!Array.isArray(value)) return [];
  const out: CustomStamp[] = [];
  for (const v of value) {
    if (!v || typeof v !== "object") continue;
    const s = v as Record<string, unknown>;
    if (typeof s.id !== "string") continue;
    const createdAt = String(s.createdAt ?? "");
    if (s.kind === "image" && typeof s.path === "string" && typeof s.aspect === "number" && s.aspect > 0) {
      out.push({ kind: "image", id: s.id, path: s.path, aspect: s.aspect, createdAt });
    } else if (s.kind === "text" && typeof s.text === "string" && Array.isArray(s.color) && s.color.length === 3) {
      const shape: StampShape = s.shape === "round" || s.shape === "none" ? s.shape : "rect";
      out.push({ kind: "text", id: s.id, text: s.text, color: s.color as Rgb, shape, createdAt });
    }
    if (out.length >= MAX_CUSTOM_STAMPS) break;
  }
  return out;
}

export default function StampPickerDialog({ onClose, onPick, onChooseImage, onPickCustom, current }: StampPickerProps) {
  const t = useT();
  const raw = useAppStore((s) => s.settings?.stamps);
  const mine = useMemo(() => readStamps(raw), [raw]);
  const full = mine.length >= MAX_CUSTOM_STAMPS;
  const [text, setText] = useState("");
  const [color, setColor] = useState<Rgb>(TEXT_COLORS[0].rgb);
  const [shape, setShape] = useState<StampShape>("rect");
  const [busy, setBusy] = useState(false);

  const persist = (next: CustomStamp[]) => useAppStore.getState().patchSettings({ stamps: next });

  const pick = (stamp: CustomStamp) => {
    onPickCustom?.(stamp);
    onClose();
  };

  const addText = () => {
    const value = text.trim();
    if (!value || full) return;
    void persist([...mine, { kind: "text", id: newId(), text: value, color, shape, createdAt: new Date().toISOString() }]);
    setText("");
  };

  const addImage = async () => {
    if (full || busy) return;
    const picked = await api
      .openFileDialog({ multiple: false, filters: [{ name: "PNG / JPEG", extensions: ["png", "jpg", "jpeg"] }] })
      .catch(() => null);
    if (!picked?.length) return;
    setBusy(true);
    try {
      const copy = await api.copyLibraryImage({ path: picked[0], library: "stamp" });
      if (!mine.some((s) => s.kind === "image" && s.path === copy.path)) {
        const aspect = copy.width / Math.max(1, copy.height);
        await persist([...mine, { kind: "image", id: newId(), path: copy.path, aspect, createdAt: new Date().toISOString() }]);
      }
    } catch {
      toast("stampPick.error.image", undefined, { tone: "danger" });
    } finally {
      setBusy(false);
    }
  };

  const remove = (id: string) => {
    const gone = mine.find((s) => s.id === id);
    void persist(mine.filter((s) => s.id !== id));
    if (gone?.kind === "image") void api.removeLibraryImage({ path: gone.path, library: "stamp" }).catch(() => false);
  };

  return (
    <Dialog
      titleKey="stampPick.title"
      size="md"
      onClose={onClose}
      footerExtra={
        <button type="button" className="btn quiet" onClick={onChooseImage}>
          {t("sign.chooseImage")}
        </button>
      }
    >
      <p className="text-sm dlg-hint">{t("stampPick.hint")}</p>
      <div className="stamp-pick-grid" role="listbox" aria-label={t("stampPick.title")}>
        {BUILTIN_STAMPS.map((s) => (
          <button
            key={s.id}
            type="button"
            role="option"
            aria-selected={current === s.id}
            aria-label={s.label}
            className="stamp-pick"
            data-active={current === s.id || undefined}
            data-korean={/[가-힣]/.test(s.label) || undefined}
            style={{ color: rgbCss(s.color), borderColor: rgbCss(s.color) }}
            onClick={() => {
              onPick(s.id);
              onClose();
            }}
          >
            {s.label}
          </button>
        ))}
      </div>

      <h3 className="field-label text-xs">{t("stampPick.quick")}</h3>
      <div className="stamp-pick-grid stamp-quick" role="listbox" aria-label={t("stampPick.quick")}>
        {QUICK_MARKS.map((m) => (
          <button
            key={m}
            type="button"
            role="option"
            aria-selected={current === m}
            aria-label={t(`stampPick.mark.${m}`)}
            title={t(`stampPick.mark.${m}`)}
            className="stamp-pick"
            style={{ color: rgbCss(QUICK_MARK_COLOR) }}
            onClick={() => {
              onPick(m);
              onClose();
            }}
          >
            {MARK_GLYPH[m]}
          </button>
        ))}
        <button
          type="button"
          role="option"
          aria-selected={false}
          aria-label={t("stampPick.today")}
          className="stamp-pick"
          style={{ color: rgbCss(TODAY_STAMP.color) }}
          onClick={() => pick({ kind: "text", id: "today", text: TODAY_STAMP.text, color: TODAY_STAMP.color, shape: TODAY_STAMP.shape, createdAt: "" })}
        >
          {t("stampPick.today")}
        </button>
      </div>

      <section className="stamp-mine" aria-label={t("stampPick.mine")}>
        <h3 className="field-label text-xs">
          {t("stampPick.mine")} <span className="mono">{mine.length}/{MAX_CUSTOM_STAMPS}</span>
        </h3>
        {mine.length === 0 ? (
          <p className="text-sm dlg-hint">{t("stampPick.mineEmpty")}</p>
        ) : (
          <ul className="stamp-mine-list">
            {mine.map((s) => (
              <li key={s.id} className="stamp-mine-item">
                <button
                  type="button"
                  className="stamp-pick"
                  aria-label={s.kind === "text" ? s.text : t("stampPick.imageStamp")}
                  style={s.kind === "text" ? { color: rgbCss(s.color), borderColor: rgbCss(s.color), borderRadius: s.shape === "round" ? 999 : undefined, borderStyle: s.shape === "none" ? "dashed" : undefined } : undefined}
                  onClick={() => pick(s)}
                >
                  {s.kind === "text" ? s.text : <span className="text-xs">{t("stampPick.imageStamp")} · {s.path.split(/[\\/]/).pop()}</span>}
                </button>
                <button type="button" className="sign-saved-delete" aria-label={t("stampPick.delete")} title={t("stampPick.delete")} onClick={() => remove(s.id)}>
                  <X size={12} aria-hidden />
                </button>
              </li>
            ))}
          </ul>
        )}
        <div className="stamp-add">
          <input
            className="field"
            type="text"
            value={text}
            maxLength={40}
            placeholder={t("stampPick.textPlaceholder")}
            aria-label={t("stampPick.textPlaceholder")}
            onChange={(e) => setText(e.currentTarget.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") addText();
            }}
          />
          <div className="chip-row" role="radiogroup" aria-label={t("prop.color")}>
            {TEXT_COLORS.map((c) => (
              <button
                key={c.key}
                type="button"
                role="radio"
                aria-checked={color.join() === c.rgb.join()}
                aria-label={t(c.key)}
                className="swatch"
                data-selected={color.join() === c.rgb.join() || undefined}
                style={{ background: rgbCss(c.rgb) }}
                onClick={() => setColor(c.rgb)}
              />
            ))}
          </div>
          <div className="chip-row" role="radiogroup" aria-label={t("stampPick.shape")}>
            {SHAPES.map((s) => (
              <button key={s} type="button" role="radio" aria-checked={shape === s} className="chip" data-active={shape === s || undefined} onClick={() => setShape(s)}>
                {t(`stampPick.shape.${s}`)}
              </button>
            ))}
          </div>
          <p className="text-xs dlg-hint">{t("stampPick.tokens")}</p>
          <div className="stamp-add-actions">
            <button type="button" className="btn" disabled={!text.trim() || full} onClick={addText}>
              {t("stampPick.addText")}
            </button>
            <button type="button" className="btn quiet" disabled={full || busy} onClick={() => void addImage()}>
              {t("stampPick.addImage")}
            </button>
          </div>
          {full && <p className="text-xs dlg-hint">{t("stampPick.full", { max: MAX_CUSTOM_STAMPS })}</p>}
        </div>
      </section>
    </Dialog>
  );
}
