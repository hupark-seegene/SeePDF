/**
 * 설정 (UI_SPEC §10, F-27/F-28): 일반 / 모양 / 주석 / 고급.
 * Every change is persisted with `set_settings` **and** applied live — the theme and the language
 * switch without a reload (`appStore.setTheme` / `setLocale` stamp `<html>` and re-render `useT`).
 *
 * v0.3 (U3): 렌더링 품질 and 캐시 크기 take effect at once (the viewer reads the quality, the engine
 * resizes its tile cache in `set_settings`); 고급 adds 캐시 비우기 and 기본값으로 되돌리기.
 */
import { useState } from "react";
import * as api from "../ipc/api";
import { useT } from "../i18n/useT";
import { useAppStore } from "../store/appStore";
import { useAnnotStore } from "../store/annotStore";
import { readNight, useViewStore } from "../store/viewStore";
import { toast } from "../app/toastStore";
import { formatBytes } from "../i18n";
import { Dialog, Row } from "./Dialog";
import { askConfirm } from "./dialogState";
import { AUTOSAVE_CHOICES, autosaveSecOf } from "../app/autosave";
import type { Locale } from "../i18n";
import type { Settings, ThemePref, ViewLayout } from "../ipc/types";

type Tab = "general" | "appearance" | "annotation" | "advanced";
const TABS: Tab[] = ["general", "appearance", "annotation", "advanced"];
const LAYOUTS: ViewLayout[] = ["single", "continuous", "two", "twoCover"];
const LAYOUT_LABEL: Record<ViewLayout, string> = {
  single: "view.layout.single",
  continuous: "view.layout.continuous",
  two: "view.layout.twoPage",
  twoCover: "view.layout.twoCover",
};

/**
 * 기본값으로 되돌리기 keeps what is the user's own data rather than a preference: the author name,
 * the saved signatures and saved stamps. (Recents live in their own file and are untouched.)
 */
export const KEPT_ON_RESET = ["author", "signatures", "stamps"] as const;

/** The patch that resets every preference to the engine's built-in default. */
export function resetPatch(defaults: Settings): Partial<Settings> {
  const patch: Record<string, unknown> = { ...defaults };
  for (const key of KEPT_ON_RESET) delete patch[key];
  return patch as Partial<Settings>;
}

/** 기본값으로 되돌리기: confirm, write the defaults, and apply what is live (language, theme, 야간). */
export async function resetToDefaults(): Promise<boolean> {
  const ok = await askConfirm({
    titleKey: "settings.resetDefaults.title",
    bodyKey: "settings.resetDefaults.body",
    confirmKey: "settings.resetDefaults",
  });
  if (!ok) return false;
  try {
    const defaults = await api.getDefaultSettings();
    const app = useAppStore.getState();
    await app.patchSettings(resetPatch(defaults));
    app.setLocale(defaults.locale);
    app.setTheme(defaults.theme);
    useViewStore.getState().setNight(readNight(defaults.night));
    useAnnotStore.getState().loadToolDefaults({});
    toast("settings.resetDefaults.done", undefined, { tone: "success" });
    return true;
  } catch (e) {
    toast("error.generic", undefined, { tone: "danger", detail: e instanceof Error ? e.message : String(e) });
    return false;
  }
}

/** 캐시 비우기: the engine drops every cached tile and page image. */
export async function clearCache(): Promise<void> {
  try {
    const freed = await api.clearRenderCache();
    toast("settings.clearCache.done", { size: formatBytes(freed) }, { tone: "success" });
  } catch (e) {
    toast("error.generic", undefined, { tone: "danger", detail: e instanceof Error ? e.message : String(e) });
  }
}
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
                      {t(LAYOUT_LABEL[l])}
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
              <label className="dlg-check text-base">
                <input
                  type="checkbox"
                  checked={settings?.checkUpdates !== false}
                  onChange={(e) => void patch({ checkUpdates: e.target.checked })}
                />
                <span>{t("settings.checkUpdates")}</span>
              </label>
              <Row labelKey="settings.autosave" hintKey="settings.autosave.hint">
                <div className="segmented" role="radiogroup" aria-label={t("settings.autosave")}>
                  {AUTOSAVE_CHOICES.map((sec) => (
                    <button
                      key={sec}
                      type="button"
                      role="radio"
                      className="segment"
                      aria-checked={autosaveSecOf(settings) === sec}
                      data-active={autosaveSecOf(settings) === sec || undefined}
                      onClick={() => void patch({ autosaveSec: sec })}
                    >
                      {t(`settings.autosave.${sec}`)}
                    </button>
                  ))}
                </div>
              </Row>
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
              <Row labelKey="settings.renderQuality" hintKey="settings.renderQuality.hint">
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
              <Row labelKey="settings.cacheSize" hintKey="settings.cacheSize.hint">
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
                  <button type="button" className="btn quiet" onClick={() => void clearCache()}>
                    {t("settings.clearCache")}
                  </button>
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
              {/* v0.3 pkg3 (U2): backups live in <app data>/backups */}
              <p className="dlg-hint text-xs">{t("settings.backups.hint")}</p>
              <button
                type="button"
                className="btn"
                onClick={() =>
                  void api
                    .backupFolder()
                    .then((path) => api.revealInFileManager({ path }))
                    .catch(() => undefined)
                }
              >
                {t("settings.backups.openFolder")}
              </button>
              <div className="inline-row">
                <button type="button" className="btn" onClick={() => void resetToDefaults()}>
                  {t("settings.resetDefaults")}
                </button>
              </div>
            </>
          )}
        </div>
      </div>
    </Dialog>
  );
}
