/**
 * 페이지 범위: 모든 페이지 / 현재 페이지 / 선택한 페이지 / 직접 입력 `1-3, 5, 8-`.
 * Shared by 내보내기, 인쇄, 분할 and 페이지 추출 so the range grammar is identical everywhere.
 */
import { useT } from "../i18n/useT";
import { parsePageRange, type RangeChoice, type RangeMode } from "./pageRange";

export interface RangePickerProps {
  value: RangeChoice;
  onChange(next: RangeChoice): void;
  pageCount: number;
  selectedCount: number;
  /** hide 현재 페이지 where it makes no sense (분할) */
  modes?: RangeMode[];
}

export function RangePicker({ value, onChange, pageCount, selectedCount, modes }: RangePickerProps) {
  const t = useT();
  const available: RangeMode[] = modes ?? ["all", "current", "selected", "custom"];
  const shown = available.filter((m) => m !== "selected" || selectedCount > 0);
  const invalid = value.mode === "custom" && parsePageRange(value.text, pageCount) === null;

  return (
    <div className="range-picker">
      {shown.map((mode) => (
        <label key={mode} className="dlg-radio text-base">
          <input
            type="radio"
            name="range"
            checked={value.mode === mode}
            onChange={() => onChange({ ...value, mode })}
          />
          <span>{t(`pages.range.${mode}`)}</span>
        </label>
      ))}
      {value.mode === "custom" && (
        <>
          <input
            className="field"
            value={value.text}
            aria-label={t("pages.range")}
            aria-invalid={invalid || undefined}
            placeholder={t("pages.range.placeholder")}
            onChange={(e) => onChange({ mode: "custom", text: e.target.value })}
          />
          {invalid && value.text.trim() !== "" && <p className="dlg-hint danger text-xs">{t("pages.range.invalid")}</p>}
        </>
      )}
    </div>
  );
}
