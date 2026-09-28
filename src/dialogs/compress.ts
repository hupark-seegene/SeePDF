/**
 * Pure logic behind 압축 (P1-5): presets, the before/after arithmetic, the "no gain" warning and
 * when 적용 is off.
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

/**
 * Why 적용 is off for an estimate, or `null` when there is a real saving. Applying a result that
 * saves nothing would still rewrite the document — one undo step and a dirty document for no gain —
 * so the dialog only offers 닫기 / 다시 예상. "Not smaller" wins over "no image downsampled": it is
 * the fact the size table shows.
 */
export type CompressBlock = "noGain" | "noImages";

export function applyBlock(r: Pick<CompressReport, "beforeBytes" | "afterBytes" | "imagesDownsampled">): CompressBlock | null {
  if (r.afterBytes >= r.beforeBytes) return "noGain";
  if (r.imagesDownsampled === 0) return "noImages";
  return null;
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
