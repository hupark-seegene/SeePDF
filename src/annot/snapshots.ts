/**
 * Appearance snapshots for hide-while-dragging (Stage 8): the overlay cannot draw an image
 * stamp, a foreign (third-party) stamp or an image signature itself, so before `dragHide` hides
 * one it asks the engine for the pixels of the annotation's rectangle (`render_page_raw`, the
 * page as it is now, annotation included) and the ghost paints that bitmap at the dragged rect.
 *
 * The bitmap is the page region as the user sees it — `/Rotate` applied — so the ghost counter-
 * rotates it by the page rotation (the overlay's matrix rotates it back). A transparent stamp
 * carries the page content under it along: it is a snapshot, not a re-render. When the snapshot
 * fails the ghost falls back to an outline.
 */
import * as api from "../ipc/api";
import type { Annot, AnnotId, DocId, PageIndex, Rotation } from "../ipc/types";
import { isBuiltinStampKind } from "../tools/stampCatalog";

export interface AppearanceSnapshot {
  url: string;
  /** the page's `/Rotate` the bitmap was rendered with */
  rotation: Rotation;
}

/** Device scale of a snapshot (2 = sharp at 200 %), capped by area below. */
const SNAPSHOT_SCALE = 2;
const SNAPSHOT_MAX_PX = 400_000;

const shots = new Map<AnnotId, AppearanceSnapshot>();
const listeners = new Set<() => void>();
let version = 0;

function emit(): void {
  version += 1;
  for (const l of listeners) l();
}

/** `useSyncExternalStore` plumbing for the overlay. */
export function subscribeSnapshots(l: () => void): () => void {
  listeners.add(l);
  return () => listeners.delete(l);
}

export function snapshotVersion(): number {
  return version;
}

export function getSnapshot(id: AnnotId): AppearanceSnapshot | undefined {
  return shots.get(id);
}

/** Does the overlay need the engine's pixels to paint `a` while its bitmap copy is hidden? */
export function needsSnapshot(a: Annot): boolean {
  if (a.kind === "stamp") return !isBuiltinStampKind(a.stampKind);
  if (a.kind === "signature") return !(a.inkPaths?.length);
  return false;
}

function revoke(shot: AppearanceSnapshot): void {
  if (shot.url.startsWith("blob:") && typeof URL.revokeObjectURL === "function") URL.revokeObjectURL(shot.url);
}

/** Forget every snapshot except `keep` (the ghosts still on screen). */
export function pruneSnapshots(keep: ReadonlySet<AnnotId> = new Set()): void {
  let changed = false;
  for (const [id, shot] of shots) {
    if (keep.has(id)) continue;
    revoke(shot);
    shots.delete(id);
    changed = true;
  }
  if (changed) emit();
}

/** RGBA rows without padding, as the PNG encoder wants them. */
function tightRgba(raw: api.RawPage): Uint8Array {
  const row = raw.width * 4;
  const px = new Uint8Array(raw.pixels.buffer, raw.pixels.byteOffset, raw.pixels.byteLength);
  if (raw.stride === row) return px.subarray(0, row * raw.height);
  const out = new Uint8Array(row * raw.height);
  for (let y = 0; y < raw.height; y++) out.set(px.subarray(y * raw.stride, y * raw.stride + row), y * row);
  return out;
}

async function toUrl(raw: api.RawPage): Promise<string> {
  const { encodePng, toDataUrl } = await import("../ipc/png");
  const png = encodePng(raw.width, raw.height, tightRgba(raw));
  if (typeof URL.createObjectURL === "function" && typeof Blob !== "undefined") {
    return URL.createObjectURL(new Blob([png as BlobPart], { type: "image/png" }));
  }
  return toDataUrl(png);
}

/**
 * Snapshot `annots` (all on `page`) as they are rendered now. Never rejects: an annotation whose
 * snapshot fails is simply painted as an outline.
 */
export async function takeSnapshots(docId: DocId, page: PageIndex, rotation: Rotation, annots: Annot[]): Promise<void> {
  await Promise.all(
    annots.map(async (a) => {
      const w = Math.abs(a.rect.r - a.rect.l);
      const h = Math.abs(a.rect.t - a.rect.b);
      if (w < 1 || h < 1) return;
      try {
        const scale = Math.max(0.25, Math.min(SNAPSHOT_SCALE, Math.sqrt(SNAPSHOT_MAX_PX / (w * h))));
        const raw = await api.renderPageRaw({ docId, page, scale, rect: a.rect });
        if (raw.width < 1 || raw.height < 1) return;
        const previous = shots.get(a.id);
        if (previous) revoke(previous);
        shots.set(a.id, { url: await toUrl(raw), rotation });
        emit();
      } catch {
        // outline fallback
      }
    }),
  );
}
