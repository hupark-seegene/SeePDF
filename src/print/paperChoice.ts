/**
 * v0.3.0 — the 인쇄 dialog's 용지 choice for 실제 크기 (see `paper.ts`). Loaded with the dialog only.
 */

/** 인쇄 ▸ 용지 (실제 크기 only). `page` = the document's most common page size. */
export type PaperId = "a4" | "letter" | "legal" | "a3" | "page";

export const PAPERS: PaperId[] = ["a4", "letter", "legal", "a3", "page"];

/** Letter-paper regions (US, Canada, Mexico, the Philippines, Chile, Colombia, Venezuela). */
const LETTER_REGIONS = new Set(["US", "CA", "MX", "PH", "CL", "CO", "VE"]);

/** A4 everywhere but the Letter regions of `locale` (e.g. `en-US` → Letter, `ko-KR` / `ko` → A4). */
export function defaultPaper(locale: string | undefined): PaperId {
  const region = locale?.split(/[-_]/)[1]?.toUpperCase();
  return region && LETTER_REGIONS.has(region) ? "letter" : "a4";
}
