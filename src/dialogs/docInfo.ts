/**
 * 문서 정보 helpers: PDF date strings and the editable `/Info` fields (P1-1).
 */
import type { DocMeta } from "../ipc/types";

/**
 * "D:YYYYMMDDHHmmSSOHH'mm'" → a localised date. Every part after the year is optional (PDF 32000
 * §7.9.4); anything that does not parse comes back unchanged.
 */
export function formatPdfDate(raw: string | undefined, locale: string): string {
  if (!raw) return "—";
  const m = /^(?:D:)?(\d{4})(\d{2})?(\d{2})?(\d{2})?(\d{2})?(\d{2})?(?:([Zz+-])(?:(\d{2})'?(?:(\d{2})'?)?)?)?$/.exec(
    raw.trim(),
  );
  if (!m) return raw;
  const [, y, mo, d, h, mi, s, sign, oh, om] = m;
  const num = (v: string | undefined, dflt: number) => (v === undefined ? dflt : Number(v));
  const month = num(mo, 1);
  const day = num(d, 1);
  if (month < 1 || month > 12 || day < 1 || day > 31) return raw;
  const hasTime = h !== undefined;
  let ms = Date.UTC(Number(y), month - 1, day, num(h, 0), num(mi, 0), num(s, 0));
  const zoned = sign !== undefined;
  if (sign === "+" || sign === "-") {
    const offset = (num(oh, 0) * 60 + num(om, 0)) * 60_000;
    ms += sign === "+" ? -offset : offset;
  }
  const opts: Intl.DateTimeFormatOptions = { year: "numeric" };
  if (mo !== undefined) opts.month = "long";
  if (d !== undefined) opts.day = "numeric";
  if (hasTime) {
    opts.hour = "2-digit";
    opts.minute = "2-digit";
  }
  // without a zone the wall-clock time is shown as written
  if (!zoned || !hasTime) opts.timeZone = "UTC";
  try {
    return new Intl.DateTimeFormat(locale, opts).format(new Date(ms));
  } catch {
    return raw;
  }
}

export interface MetaForm {
  title: string;
  author: string;
  subject: string;
  keywords: string;
}

export function metaFormOf(meta: DocMeta): MetaForm {
  return {
    title: meta.title ?? "",
    author: meta.author ?? "",
    subject: meta.subject ?? "",
    keywords: meta.keywords ?? "",
  };
}

/**
 * Trimmed; an empty field stays `""`, which `set_metadata` reads as "remove this key".
 * (`undefined` would be dropped from the JSON payload and mean "leave unchanged".)
 */
export function metaFromForm(form: MetaForm): DocMeta {
  const clean = (v: string) => v.trim();
  return {
    title: clean(form.title),
    author: clean(form.author),
    subject: clean(form.subject),
    keywords: clean(form.keywords),
  };
}

export function metaChanged(meta: DocMeta, form: MetaForm): boolean {
  const next = metaFromForm(form);
  return (["title", "author", "subject", "keywords"] as const).some((k) => (meta[k]?.trim() ?? "") !== next[k]);
}
