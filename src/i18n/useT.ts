import { useSyncExternalStore } from "react";
import { getLocale, onLocaleChange, t, type Locale, type TParams } from "./index";

/**
 * `const t = useT()` — re-renders the component when the locale changes.
 * The returned function is identity-stable per locale, so it is safe in deps arrays.
 */
export function useT(): (key: string, params?: TParams) => string {
  const locale = useLocale();
  return (key, params) => t(key, params, locale);
}

export function useLocale(): Locale {
  return useSyncExternalStore(onLocaleChange, getLocale, getLocale);
}
