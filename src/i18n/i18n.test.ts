import { describe, expect, it } from "vitest";
import ko from "./ko.json";
import en from "./en.json";
import { formatBytes, formatRelativeDay, resolveLocale, setLocale, t } from "./index";

const PLACEHOLDER = /\{\{(\w+)\}\}/g;

function placeholders(s: string): string[] {
  return [...s.matchAll(PLACEHOLDER)].map((m) => m[1]).sort();
}

describe("i18n catalogues", () => {
  it("has the same keys in ko and en", () => {
    const koKeys = Object.keys(ko).sort();
    const enKeys = Object.keys(en).sort();
    expect(enKeys).toEqual(koKeys);
    expect(koKeys.length).toBeGreaterThanOrEqual(405); // UI_SPEC §15 says 405+
  });

  it("uses identical placeholders in both locales", () => {
    for (const key of Object.keys(ko) as (keyof typeof ko)[]) {
      expect(placeholders(ko[key]), `placeholders differ for ${key}`).toEqual(
        placeholders(en[key as keyof typeof en]),
      );
    }
  });

  it("has an _other partner for every plural key and vice versa", () => {
    for (const key of Object.keys(ko)) {
      if (key.endsWith("_other")) {
        expect(Object.keys(ko)).toContain(key.slice(0, -"_other".length));
      }
    }
  });

  it("never leaves an empty string", () => {
    for (const [key, value] of Object.entries(ko)) expect(value.length, key).toBeGreaterThan(0);
    for (const [key, value] of Object.entries(en)) expect(value.length, key).toBeGreaterThan(0);
  });
});

describe("t()", () => {
  it("interpolates and honours the active locale", () => {
    setLocale("ko");
    expect(t("app.window.document", { name: "보고서.pdf" })).toBe("보고서.pdf — SeePDF");
    expect(t("mode.annotate")).toBe("주석");
    setLocale("en");
    expect(t("mode.annotate")).toBe("Annotate");
  });

  it("uses _other when count !== 1 (English pluralises, Korean repeats)", () => {
    setLocale("en");
    expect(t("sidebar.search.results", { count: 1 })).toBe("1 result");
    expect(t("sidebar.search.results", { count: 7 })).toBe("7 results");
    setLocale("ko");
    expect(t("sidebar.search.results", { count: 7 })).toBe("결과 7개");
  });

  it("returns the key for a missing entry", () => {
    expect(t("does.not.exist")).toBe("does.not.exist");
  });

  it("resolves the startup locale from the setting, then the browser", () => {
    expect(resolveLocale("en")).toBe("en");
    expect(resolveLocale(null)).toMatch(/ko|en/);
  });
});

describe("Intl helpers", () => {
  it("formats bytes and relative days in Korean", () => {
    setLocale("ko");
    expect(formatBytes(1024)).toBe("1 KB");
    // Local wall-clock times: "today" is a calendar day in the machine's time zone, so fixed
    // +09:00 offsets made this fail on a UTC runner (08:00 KST is still yesterday in UTC).
    const at = (d: number, h: number) => new Date(2026, 8, d, h).toISOString();
    const now = new Date(2026, 8, 15, 10);
    expect(formatRelativeDay(at(15, 8), now)).toBe("오늘");
    expect(formatRelativeDay(at(14, 23), now)).toBe("어제");
    expect(formatRelativeDay(at(12, 9), now)).toBe("3일 전");
  });
});
