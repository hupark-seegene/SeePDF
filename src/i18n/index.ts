/**
 * SeePDF i18n — a 60-line `t()` over two flat catalogues (UI_SPEC §15).
 * No i18next: 414 flat keys, two locales, no runtime negotiation.
 *
 *   t('sidebar.search.results', { count: 3 })   // ko: "결과 3개"  en: "3 results"
 *
 * Rules (UI_SPEC §15.21): never concatenate, always interpolate; `{{placeholders}}` are identical in
 * both locales; `_other` is used when `count !== 1` (Korean repeats the singular on purpose).
 */
import ko from "./ko.json";
import en from "./en.json";

export type Locale = "ko" | "en";
export type Catalogue = Record<string, string>;
export type TParams = Record<string, string | number>;

export const catalogues: Record<Locale, Catalogue> = { ko, en };
export const LOCALES: Locale[] = ["ko", "en"];
export const DEFAULT_LOCALE: Locale = "ko";

let current: Locale = DEFAULT_LOCALE;
const listeners = new Set<(l: Locale) => void>();

/** Resolve the startup locale from an explicit setting, else the OS/browser language. */
export function resolveLocale(pref?: string | null): Locale {
  if (pref === "ko" || pref === "en") return pref;
  const nav = typeof navigator !== "undefined" ? navigator.language : "";
  return nav?.toLowerCase().startsWith("ko") ? "ko" : nav ? "en" : DEFAULT_LOCALE;
}

export function getLocale(): Locale {
  return current;
}

export function setLocale(locale: Locale): void {
  if (locale === current) return;
  current = locale;
  if (typeof document !== "undefined") document.documentElement.lang = locale;
  for (const fn of listeners) fn(locale);
}

export function onLocaleChange(fn: (l: Locale) => void): () => void {
  listeners.add(fn);
  return () => listeners.delete(fn);
}

function interpolate(template: string, params?: TParams): string {
  if (!params) return template;
  return template.replace(/\{\{(\w+)\}\}/g, (whole, name: string) => {
    const v = params[name];
    if (v === undefined) return whole;
    return typeof v === "number" ? formatNumber(v) : v;
  });
}

/** Translate `key` in the active locale (or `locale` when given). Missing keys return the key itself. */
export function t(key: string, params?: TParams, locale: Locale = current): string {
  const cat = catalogues[locale] ?? catalogues[DEFAULT_LOCALE];
  let entry = cat[key];
  if (params && typeof params.count === "number" && params.count !== 1) {
    entry = cat[`${key}_other`] ?? entry;
  }
  if (entry === undefined) {
    if (import.meta.env?.DEV) console.warn(`[i18n] missing key: ${key}`);
    return key;
  }
  return interpolate(entry, params);
}

/** `Intl` helpers — every number and date in the UI goes through these (UI_SPEC §15.21). */
export function formatNumber(n: number, opts?: Intl.NumberFormatOptions): string {
  return new Intl.NumberFormat(current, opts).format(n);
}

export function formatBytes(bytes: number): string {
  const units = ["B", "KB", "MB", "GB"];
  let v = bytes;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i++;
  }
  return `${formatNumber(v, { maximumFractionDigits: i === 0 ? 0 : 1 })} ${units[i]}`;
}

/** 오늘 / 어제 / 3일 전 — the welcome screen's relative date (UI_SPEC §11). */
export function formatRelativeDay(iso: string, now = new Date()): string {
  const then = new Date(iso);
  if (Number.isNaN(then.getTime())) return "";
  const days = Math.floor((startOfDay(now) - startOfDay(then)) / 86_400_000);
  if (days <= 0) return t("common.today");
  if (days === 1) return t("common.yesterday");
  if (days < 7) return t("common.daysAgo", { count: days });
  return new Intl.DateTimeFormat(current, { year: "numeric", month: "short", day: "numeric" }).format(then);
}

function startOfDay(d: Date): number {
  return new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime();
}
