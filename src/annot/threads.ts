/**
 * Annotation threads (P2) as pure functions over one page's list.
 *
 * The engine reports each reply's parent (`Annot.inReplyTo`, from `/IRT`); everything a thread
 * needs beyond that is computed here, so the 주석 tab, the note popover and the delete confirm all
 * agree on who belongs to which thread:
 *
 * * a **thread** is a top-level annotation plus every reply below it, at any depth, flattened and
 *   oldest first (`Annot.replies`) — Acrobat's comment list shows nested replies the same way;
 * * a reply whose parent is not on the page (a foreign file, a parent deleted by another viewer)
 *   is shown as a top-level annotation rather than lost;
 * * replies are never drawn on the page (their appearance is empty), so the canvas hit-tests only
 *   top-level annotations (`withoutReplies`).
 */
import type { Annot, AnnotId } from "../ipc/types";

/** `D:YYYYMMDDHHmmSS±HH'mm'` or ISO-8601 → epoch ms, `null` when neither parses. */
export function annotTime(raw: string | null | undefined): number | null {
  if (!raw) return null;
  const m = /^D:(\d{4})(\d{2})?(\d{2})?(\d{2})?(\d{2})?(\d{2})?(?:([Zz+-])(\d{2})?'?(\d{2})?'?)?/.exec(raw.trim());
  if (m) {
    const n = (v: string | undefined, d: number) => (v === undefined ? d : Number(v));
    let ms = Date.UTC(Number(m[1]), n(m[2], 1) - 1, n(m[3], 1), n(m[4], 0), n(m[5], 0), n(m[6], 0));
    if (m[7] === "+" || m[7] === "-") {
      const offset = (n(m[8], 0) * 60 + n(m[9], 0)) * 60_000;
      ms += m[7] === "+" ? -offset : offset;
    }
    return Number.isNaN(ms) ? null : ms;
  }
  const iso = Date.parse(raw);
  return Number.isNaN(iso) ? null : iso;
}

/** An ISO string for `formatRelativeDay`, whatever the annotation's date format (`""` = none). */
export function isoOf(raw: string | null | undefined): string {
  const ms = annotTime(raw);
  return ms === null ? "" : new Date(ms).toISOString();
}

/**
 * Oldest first. The dated annotations are sorted among the slots they occupy; one without a date
 * keeps its list position (a comparator mixing the two would not be transitive).
 */
export function byCreated(list: Annot[]): Annot[] {
  const times = list.map((a) => annotTime(a.created));
  const dated = list
    .map((a, i) => ({ a, i, t: times[i] }))
    .filter((e): e is { a: Annot; i: number; t: number } => e.t !== null)
    .sort((x, y) => x.t - y.t || x.i - y.i);
  let next = 0;
  return list.map((a, i) => (times[i] === null ? a : dated[next++].a));
}

/** The top-level annotation `id` belongs to (itself when it is not a reply, or its parent is gone). */
export function threadRootId(list: Annot[], id: AnnotId): AnnotId {
  const byId = new Map(list.map((a) => [a.id, a]));
  let current = byId.get(id);
  const seen = new Set<AnnotId>();
  while (current?.inReplyTo && byId.has(current.inReplyTo) && !seen.has(current.id)) {
    seen.add(current.id);
    current = byId.get(current.inReplyTo);
  }
  return current?.id ?? id;
}

/** Every reply below `id`, at any depth (not `id` itself). */
export function descendantIds(list: Annot[], id: AnnotId): AnnotId[] {
  const out: AnnotId[] = [];
  const doomed = new Set<AnnotId>([id]);
  for (let grew = true; grew; ) {
    grew = false;
    for (const a of list) {
      if (a.inReplyTo && doomed.has(a.inReplyTo) && !doomed.has(a.id)) {
        doomed.add(a.id);
        out.push(a.id);
        grew = true;
      }
    }
  }
  return out;
}

/** Is `a` a reply to something that is on the page? */
export function isAttachedReply(a: Annot, ids: Set<AnnotId>): boolean {
  return !!a.inReplyTo && ids.has(a.inReplyTo) && a.inReplyTo !== a.id;
}

/**
 * One page's list as threads: the top-level annotations in list order, each carrying
 * `replies` (every reply below it, oldest first) when it has any.
 */
export function threaded(list: Annot[]): Annot[] {
  const ids = new Set(list.map((a) => a.id));
  const replies = new Map<AnnotId, Annot[]>();
  const roots: Annot[] = [];
  for (const a of list) {
    if (!isAttachedReply(a, ids)) {
      roots.push(a);
      continue;
    }
    const root = threadRootId(list, a.id);
    replies.set(root, [...(replies.get(root) ?? []), a]);
  }
  return roots.map((root) => {
    const own = replies.get(root.id);
    return own ? { ...root, replies: byCreated(own) } : root;
  });
}

/** The replies of the thread `id` belongs to — what the note popover lists under the note. */
export function threadReplies(list: Annot[], id: AnnotId): Annot[] {
  const root = threadRootId(list, id);
  return threaded(list).find((a) => a.id === root)?.replies ?? [];
}

/** The page list without attached replies: what the canvas hit-tests and outlines. */
export function withoutReplies(list: Annot[]): Annot[] {
  if (!list.some((a) => a.inReplyTo)) return list;
  const ids = new Set(list.map((a) => a.id));
  return list.filter((a) => !isAttachedReply(a, ids));
}
