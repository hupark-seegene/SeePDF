/**
 * The colour swatches of the properties panel (UI_SPEC §7): the annotation palette plus a custom
 * colour well. Shared by the Inspector and the 워터마크 dialog; styles live in `shell.css`.
 */
import type { Rgb } from "../ipc/types";
import { useT } from "../i18n/useT";
import { PALETTE } from "../store/annotStore";

export function rgbCss(c: Rgb): string {
  return `rgb(${c[0]} ${c[1]} ${c[2]})`;
}

export function hexOf(c: Rgb): string {
  return `#${c.map((v) => v.toString(16).padStart(2, "0")).join("")}`;
}

export function rgbOf(hex: string): Rgb {
  const n = Number.parseInt(hex.slice(1), 16);
  return [(n >> 16) & 255, (n >> 8) & 255, n & 255];
}

export function Swatches({ value, onPick, label }: { value: Rgb | null; onPick(c: Rgb): void; label: string }) {
  const t = useT();
  return (
    <div className="swatches" role="radiogroup" aria-label={label}>
      {PALETTE.map((sw) => {
        const selected = !!value && sw.rgb.join() === value.join();
        return (
          <button
            key={sw.key}
            type="button"
            role="radio"
            aria-checked={selected}
            aria-label={t("a11y.colorSwatch", { name: t(sw.key) })}
            className="swatch"
            data-selected={selected || undefined}
            style={{ background: rgbCss(sw.rgb) }}
            onClick={() => onPick(sw.rgb)}
          />
        );
      })}
      <label className="swatch swatch-custom" title={t("color.custom")}>
        <input
          type="color"
          aria-label={t("color.custom")}
          value={hexOf(value ?? [0, 0, 0])}
          onChange={(e) => onPick(rgbOf(e.currentTarget.value))}
        />
      </label>
    </div>
  );
}
