/**
 * 설정 (UI_SPEC §10, F-27/F-28): 일반 / 모양 / 주석 / 고급.
 * Every change is persisted with `set_settings` **and** applied live — the theme and the language
 * switch without a reload (`appStore.setTheme` / `setLocale` stamp `<html>` and re-render `useT`).
 */
import { useState } from "react";
import { useT } from "../i18n/useT";
import { useAppStore } from "../store/appStore";
import { Dialog, Row } from "./Dialog";
import type { Locale } from "../i18n";
import type { Settings, ThemePref, ViewLayout } from "../ipc/types";

type Tab = "general" | "appearance" | "annotation" | "advanced";
const TABS: Tab[] = ["general", "appearance", "annotation", "advanced"];
const LAYOUTS: ViewLayout[] = ["single", "continuous", "two"];
const THEMES: ThemePref[] = ["system", "light", "dark"];
const ZOOMS = ["fit-width", "fit-page", "actual", 100, 125, 150] as const;
const OCR_LANGS = ["kor", "eng"] as const;
const OCR_DPIS = ["auto", 200, 300, 400] as const;

/**
 * Stage 2: `Settings.recentsCount` is a first-class contract field (IPC_CONTRACT §11); it used
 * to ride in `toolDefaults`, which is still read as a fallback so a settings file written before
 * the change keeps the user's number.
 */
export const RECENTS_COUNT_KEY = "recentsCount";
export const DEFAULT_RECENTS_COUNT = 20;

export function recentsCountOf(settings: Settings | null): number {
  if (typeof settings?.recentsCount === "number" && settings.recentsCount >= 0) return settings.recentsCount;
  const legacy = settings?.toolDefaults?.[RECENTS_COUNT_KEY];
  return typeof legacy === "number" && legacy > 0 ? legacy : DEFAULT_RECENTS_COUNT;
}

export function SettingsDialog({ onClose }: { onClose(): void }) {
  const t = useT();
  const [tab, setTab] = useState<Tab>("general");
  const settings = useAppStore((s) => s.settings);
  const locale = useAppStore((s) => s.locale);
  const theme = useAppStore((s) => s.theme);
  const patch = useAppStore((s) => s.patchSettings);
  const setLocale = useAppStore((s) => s.setLocale);
  const setTheme = useAppStore((s) => s.setTheme);

  const ocrLangs = settings?.ocrLanguages ?? ["kor", "eng"];

  return (
    <Dialog titleKey="settings.title" size="lg" onClose={onClose} cancelKey="common.close">
      <div className="settings-grid">
        <ul className="settings-tabs" role="tablist" aria-label={t("settings.title")}>
          {TABS.map((id) => (
            <li key={id}>
              <button
                type="button"
                role="tab"
                aria-selected={tab === id}
                data-active={tab === id || undefined}
                onClick={() => setTab(id)}
              >
                {t(`settings.tab.${id}`)}
              </button>
            </li>
          ))}
        </ul>

        <div className="settings-panel" role="tabpanel">
          {tab === "general" && (
            <>
              <Row labelKey="settings.language" hintKey="settings.language.restart">
                <div className="segmented">
                  {(["ko", "en"] as Locale[]).map((l) => (
                    <button
                      key={l}
                      type="button"
                      className="segment"
                      data-active={locale === l || undefined}
                      onClick={() => setLocale(l)}
                    >
                      {t(`settings.language.${l}`)}
                    </button>
                  ))}
                </div>
              </Row>
              <Row labelKey="settings.defaultView">
                <select
                  className="field"
                  aria-label={t("settings.defaultView")}
                  value={settings?.defaultLayout ?? "continuous"}
                  onChange={(e) => void patch({ defaultLayout: e.target.value as ViewLayout })}
                >
                  {LAYOUTS.map((l) => (
                    <option key={l} value={l}>
                      {t(l === "two" ? "view.layout.twoPage" : `view.layout.${l}`)}
                    </option>
                  ))}
                </select>
              </Row>
              <Row labelKey="settings.defaultZoom">
                <select
                  className="field"
                  aria-label={t("settings.defaultZoom")}
                  value={String(settings?.defaultZoom ?? "fit-width")}
                  onChange={(e) => {
                    const v = e.target.value;
                    const zoom = v === "fit-width" || v === "fit-page" || v === "actual" ? v : Number(v);
                    void patch({ defaultZoom: zoom });
                  }}
                >
                  {ZOOMS.map((z) => (
                    <option key={String(z)} value={String(z)}>
                      {typeof z === "number" ? `${z}%` : t(`view.zoom.${z === "fit-width" ? "fitWidth" : z === "fit-page" ? "fitPage" : "actual"}`)}
                    </option>
                  ))}
                </select>
              </Row>
              <label className="dlg-check text-base">
                <input
                  type="checkbox"
                  checked={settings?.restorePosition ?? true}
                  onChange={(e) => void patch({ restorePosition: e.target.checked })}
                />
                <span>{t("settings.restorePosition")}</span>
              </label>
              <Row labelKey="settings.recentsCount">
                <input
                  className="field num"
                  type="number"
                  min={0}
                  max={50}
                  value={recentsCountOf(settings)}
                  aria-label={t("settings.recentsCount")}
                  onChange={(e) => void patch({ recentsCount: Math.max(0, Number(e.target.value) || 0) })}
                />
              </Row>
            </>
          )}

          {tab === "appearance" && (
            <Row labelKey="settings.theme">
              <div className="segmented">
                {THEMES.map((th) => (
                  <button
                    key={th}
                    type="button"
                    className="segment"
                    data-active={theme === th || undefined}
                    onClick={() => setTheme(th)}
                  >
                    {t(`settings.theme.${th}`)}
                  </button>
                ))}
              </div>
            </Row>
          )}

          {tab === "annotation" && (
            <Row labelKey="settings.authorName">
              <input
                className="field"
                value={settings?.author ?? ""}
                aria-label={t("settings.authorName")}
                onChange={(e) => void patch({ author: e.target.value })}
              />
            </Row>
          )}

          {tab === "advanced" && (
            <>
              <Row labelKey="settings.renderQuality">
                <div className="segmented">
                  {(["balanced", "high"] as const).map((q) => (
                    <button
                      key={q}
                      type="button"
                      className="segment"
                      data-active={(settings?.renderQuality ?? "balanced") === q || undefined}
                      onClick={() => void patch({ renderQuality: q })}
                    >
                      {t(q === "high" ? "export.quality.high" : "export.quality.balanced")}
                    </button>
                  ))}
                </div>
              </Row>
              <Row labelKey="settings.cacheSize">
                <div className="inline-row">
                  <input
                    className="slider"
                    type="range"
                    min={16}
                    max={512}
                    step={16}
                    value={settings?.tileCacheMb ?? 64}
                    aria-label={t("settings.cacheSize")}
                    onChange={(e) => void patch({ tileCacheMb: Number(e.target.value) })}
                  />
                  <span className="text-sm mono">{settings?.tileCacheMb ?? 64} MB</span>
                </div>
              </Row>
              <Row labelKey="ocr.language">
                <div className="chip-row">
                  {OCR_LANGS.map((lang) => (
                    <button
                      key={lang}
                      type="button"
                      className="chip"
                      data-active={ocrLangs.includes(lang) || undefined}
                      onClick={() => {
                        const next = ocrLangs.includes(lang) ? ocrLangs.filter((l) => l !== lang) : [...ocrLangs, lang];
                        void patch({ ocrLanguages: next.length ? next : ["kor", "eng"] });
                      }}
                    >
                      {t(lang === "kor" ? "ocr.language.ko" : "ocr.language.en")}
                    </button>
                  ))}
                </div>
              </Row>
              <Row labelKey="ocr.option.dpi">
                <select
                  className="field"
                  aria-label={t("ocr.option.dpi")}
                  value={String(settings?.ocrDpi ?? "auto")}
                  onChange={(e) => {
                    const v = e.target.value;
                    void patch({ ocrDpi: v === "auto" ? "auto" : (Number(v) as 200 | 300 | 400) });
                  }}
                >
                  {OCR_DPIS.map((d) => (
                    <option key={String(d)} value={String(d)}>
                      {d === "auto" ? t("ocr.dpi.auto") : `${d} DPI`}
                    </option>
                  ))}
                </select>
              </Row>
              <label className="dlg-check text-base">
                <input
                  type="checkbox"
                  checked={settings?.backupsEnabled ?? true}
                  onChange={(e) => void patch({ backupsEnabled: e.target.checked })}
                />
                <span>{t("settings.backups")}</span>
              </label>
            </>
          )}
        </div>
      </div>
    </Dialog>
  );
}
