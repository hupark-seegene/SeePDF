/**
 * 도장 선택 (P1-12): what arming 도장 opens when no stamp is chosen yet, and what the properties
 * panel's 도장 변경… reopens. The Korean set (결재 / 승인 / 기밀) comes first, then the Latin
 * built-ins, then 이미지 선택… for a scanned seal. A click chooses and closes; the stamp then
 * follows the cursor as a ghost and the next click on a page places it.
 */
import { useT } from "../i18n/useT";
import { rgbCss } from "../app/Swatches";
import { BUILTIN_STAMPS } from "../tools/stampCatalog";
import { Dialog } from "./Dialog";
import "./dialogs.css";

export interface StampPickerProps {
  onClose(): void;
  onPick(builtin: string): void;
  onChooseImage(): void;
  /** the built-in chosen last time, highlighted */
  current?: string | null;
}

export default function StampPickerDialog({ onClose, onPick, onChooseImage, current }: StampPickerProps) {
  const t = useT();
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
    </Dialog>
  );
}
