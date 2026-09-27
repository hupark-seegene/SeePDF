/**
 * Pure logic behind 압축 (P1-5): presets, the before/after arithmetic and the "no gain" warning.
 */
import type { CompressOptions, CompressPreset, CompressReport, PageIndex } from "../ipc/types";

export const COMPRESS_PRESETS: { dpi: CompressPreset; labelKey: string }[] = [
  { dpi: 300, labelKey: "compress.preset.300" },
  { dpi: 150, labelKey: "compress.preset.150" },
  { dpi: 96, labelKey: "compress.preset.96" },
];

export const DEFAULT_PRESET: CompressPreset = 150;

export interface CompressSummary {
  deltaBytes: number;   // after − before (negative = smaller)
  /** signed percentage of `beforeBytes`, one decimal; 0 when before is 0 */
  deltaPct: number;
  /** after ≥ before: applying would not help — warn in amber */
  noGain: boolean;
}

export function summarize(r: Pick<CompressReport, "beforeBytes" | "afterBytes">): CompressSummary {
  const deltaBytes = r.afterBytes - r.beforeBytes;
  const deltaPct = r.beforeBytes > 0 ? Math.round((deltaBytes / r.beforeBytes) * 1000) / 10 : 0;
  return { deltaBytes, deltaPct, noGain: r.afterBytes >= r.beforeBytes };
}

/** `−42.3%` / `+0.2%` / `0%` — a real minus sign, and the sign is always shown for a change. */
export function formatDeltaPct(pct: number, locale: string): string {
  const abs = new Intl.NumberFormat(locale, { maximumFractionDigits: 1 }).format(Math.abs(pct));
  if (pct === 0) return `${abs}%`;
  return `${pct < 0 ? "−" : "+"}${abs}%`;
}

/** `pages` omitted = the whole document. */
export function buildCompressOptions(targetDpi: CompressPreset, pages: PageIndex[] | null, allPages: boolean): CompressOptions {
  return allPages || !pages ? { targetDpi } : { targetDpi, pages };
}
