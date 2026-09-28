/**
 * 여러 파일 OCR (P1-7) — the queue model and the output naming, as pure functions.
 *
 * Everything here is synchronous and side-effect free except `pickOutputPath`, whose only effect
 * is the `exists` probe it is handed, so the rules the dialog promises ("never overwrite the
 * source", "append (2) when the name is taken", "one file at a time") are testable without a
 * document, a worker or a file system.
 */

/** 대기 / 여는 중 / 진행 중 n/m 페이지 / 저장 중 / 완료 / 건너뜀 / 실패 / 취소됨 */
export type BatchStatus =
  | "queued" | "opening" | "running" | "saving"
  | "done" | "skipped" | "failed" | "cancelled";

export interface BatchItem {
  id: number;
  path: string;
  name: string;
  status: BatchStatus;
  /** pages recognised so far */
  done: number;
  /** pages to recognise (after "skip pages that already have text"); 0 until known */
  total: number;
  /** i18n key: why the file was skipped or failed */
  reasonKey?: string;
  /** the engine's own message, shown as a tooltip */
  detail?: string;
  /** where the searchable copy was written */
  output?: string;
}

export interface BatchSummary {
  total: number;
  queued: number;
  active: number;
  done: number;
  skipped: number;
  failed: number;
  cancelled: number;
}

export function baseName(path: string): string {
  const parts = path.split(/[\\/]/);
  return parts[parts.length - 1] || path;
}

/** `true` while the file is being worked on (opening, recognising or saving). */
export function isActive(status: BatchStatus): boolean {
  return status === "opening" || status === "running" || status === "saving";
}

/**
 * Which files 시작 processes: every file without a searchable copy yet. So 시작 after 취소 resumes
 * (the cancelled file and everything still waiting), and 시작 after a run retries what failed or was
 * skipped (a dismissed password prompt, say) without writing a second copy of what is done.
 */
export function isRunnable(item: Pick<BatchItem, "status">): boolean {
  return item.status !== "done" && !isActive(item.status);
}

/** Append `paths`, skipping any already in the list (same path, compared case-insensitively). */
export function addPaths(items: BatchItem[], paths: string[], nextId: () => number): BatchItem[] {
  const seen = new Set(items.map((i) => i.path.toLowerCase()));
  const added: BatchItem[] = [];
  for (const path of paths) {
    const key = path.toLowerCase();
    if (!path || seen.has(key)) continue;
    seen.add(key);
    added.push({ id: nextId(), path, name: baseName(path), status: "queued", done: 0, total: 0 });
  }
  return added.length ? [...items, ...added] : items;
}

export function removeItem(items: BatchItem[], id: number): BatchItem[] {
  return items.filter((i) => i.id !== id || isActive(i.status));
}

export function patchItem(items: BatchItem[], id: number, patch: Partial<Omit<BatchItem, "id">>): BatchItem[] {
  return items.map((i) => (i.id === id ? { ...i, ...patch } : i));
}

/** Reset the files a new run will process back to 대기, dropping the previous run's outcome. */
export function requeue(items: BatchItem[], ids: ReadonlySet<number>): BatchItem[] {
  return items.map((i) =>
    ids.has(i.id)
      ? { ...i, status: "queued", done: 0, total: 0, reasonKey: undefined, detail: undefined, output: undefined }
      : i,
  );
}

export function summarize(items: BatchItem[]): BatchSummary {
  const s: BatchSummary = { total: items.length, queued: 0, active: 0, done: 0, skipped: 0, failed: 0, cancelled: 0 };
  for (const i of items) {
    if (isActive(i.status)) s.active += 1;
    else s[i.status as "queued" | "done" | "skipped" | "failed" | "cancelled"] += 1;
  }
  return s;
}

// ---------------------------------------------------------------------------
// Output names
// ---------------------------------------------------------------------------

export const OCR_SUFFIX = "-ocr";

interface PathParts {
  dir: string;
  sep: string;
  stem: string;
  ext: string;
}

function splitPath(path: string): PathParts {
  const cut = Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\"));
  const dir = cut >= 0 ? path.slice(0, cut) : "";
  const sep = cut >= 0 ? path[cut] : "/";
  const name = cut >= 0 ? path.slice(cut + 1) : path;
  const m = /\.pdf$/i.exec(name);
  return m ? { dir, sep, stem: name.slice(0, m.index), ext: m[0] } : { dir, sep, stem: name, ext: ".pdf" };
}

/** A folder path without its trailing separator(s) — `C:\` and `/` keep theirs. */
function trimFolder(folder: string): { dir: string; sep: string } {
  const sep = folder.includes("\\") && !folder.includes("/") ? "\\" : "/";
  let dir = folder;
  while (dir.length > 1 && /[\\/]$/.test(dir) && !/^[A-Za-z]:[\\/]$/.test(dir)) dir = dir.slice(0, -1);
  return { dir, sep };
}

/**
 * The `n`-th candidate for `source`'s searchable copy: `스캔.pdf` → `스캔-ocr.pdf`, then
 * `스캔-ocr (2).pdf`, `스캔-ocr (3).pdf`, … — beside the source, or in `folder` when one is chosen.
 */
export function outputCandidate(source: string, folder: string | null, n = 1): string {
  const src = splitPath(source);
  const target = folder ? trimFolder(folder) : { dir: src.dir, sep: src.sep };
  const name = `${src.stem}${OCR_SUFFIX}${n > 1 ? ` (${n})` : ""}${src.ext}`;
  if (!target.dir) return name;
  return /[\\/]$/.test(target.dir) ? `${target.dir}${name}` : `${target.dir}${target.sep}${name}`;
}

/** Paths compare case-insensitively: APFS and NTFS are, by default. */
export function pathKey(path: string): string {
  return path.replace(/\\/g, "/").toLowerCase();
}

/** Give up after this many taken names rather than probing the disk forever. */
export const MAX_CANDIDATES = 999;

/**
 * The first free output path for `source`: never the source itself, never a path that exists on
 * disk (`exists`), never one this batch has already written (`reserved`, as `pathKey`s — two
 * `보고서.pdf` from different folders going into one chosen folder).
 */
export async function pickOutputPath(
  source: string,
  folder: string | null,
  exists: (path: string) => Promise<boolean>,
  reserved: ReadonlySet<string> = new Set(),
): Promise<string> {
  const sourceKey = pathKey(source);
  for (let n = 1; n <= MAX_CANDIDATES; n++) {
    const candidate = outputCandidate(source, folder, n);
    const key = pathKey(candidate);
    if (key === sourceKey || reserved.has(key)) continue;
    if (await exists(candidate)) continue;
    return candidate;
  }
  throw new Error(`no free output name for ${source}`);
}
