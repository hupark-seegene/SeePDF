import { PanelRight } from "lucide-react";
import { IconButton } from "./IconButton";
import { useT } from "../i18n/useT";
import { useAppStore } from "../store/appStore";
import { PALETTE, useAnnotStore } from "../store/annotStore";

/**
 * The 260 px properties panel frame (UI_SPEC §7). Auto-opens outside 읽기; the contents per context
 * are Stage 1 (d)'s `src/app/Inspector/**`. Stage 0 ships the frame plus the swatch row so the
 * annotation style defaults are real from day one.
 */
export function Inspector() {
  const t = useT();
  const mode = useAppStore((s) => s.mode);
  const toggle = useAppStore((s) => s.toggleInspector);
  const style = useAnnotStore((s) => s.style);
  const setStyle = useAnnotStore((s) => s.setStyle);
  const showStyle = mode === "annotate";

  return (
    <aside className="inspector" aria-label={t("prop.title")}>
      <div className="inspector-head">
        <h2 className="text-md">{t("prop.title")}</h2>
        <IconButton icon={PanelRight} label={t("a11y.closePanel")} onClick={() => toggle(false)} />
      </div>

      <div className="inspector-body">
        {showStyle ? (
          <section className="field-group">
            <h3 className="field-label text-xs">{t("prop.color")}</h3>
            <div className="swatches" role="radiogroup" aria-label={t("prop.color")}>
              {PALETTE.map((sw) => {
                const selected = sw.rgb.join() === style.color.join();
                return (
                  <button
                    key={sw.key}
                    type="button"
                    role="radio"
                    aria-checked={selected}
                    aria-label={t("a11y.colorSwatch", { name: t(sw.key) })}
                    className="swatch"
                    data-selected={selected || undefined}
                    style={{ background: `rgb(${sw.rgb.join(" ")})` }}
                    onClick={() => setStyle({ color: sw.rgb, opacity: sw.highlightAlpha })}
                  />
                );
              })}
            </div>

            <h3 className="field-label text-xs">{t("prop.opacity")}</h3>
            <input
              className="slider"
              type="range"
              min={5}
              max={100}
              value={Math.round(style.opacity * 100)}
              aria-label={t("prop.opacity")}
              onChange={(e) => setStyle({ opacity: Number(e.target.value) / 100 })}
            />

            <h3 className="field-label text-xs">{t("prop.thickness")}</h3>
            <div className="chip-row">
              {[1, 2, 4, 8, 12].map((w) => (
                <button
                  key={w}
                  type="button"
                  className="chip mono"
                  data-active={style.width === w || undefined}
                  onClick={() => setStyle({ width: w })}
                >
                  {w}
                </button>
              ))}
            </div>
          </section>
        ) : (
          <p className="empty">{t("prop.empty")}</p>
        )}
      </div>
    </aside>
  );
}
