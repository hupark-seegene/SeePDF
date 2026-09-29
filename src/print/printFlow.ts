/**
 * 인쇄 (F-26) — the v0.3 option plumbing between the 인쇄 dialog, `flows.runPrint` and the two
 * print paths (pkg8: X7 주석, X8 fit / chunks / temp files, X2 모아찍기 · 소책자 · 크기 · 흑백).
 *
 * * DOM path (`startDomPrint`): fills the print store; `PrintRoot` renders the pages through the
 *   `print=` variant of `seepdf://page`. An n-up / booklet job first asks the engine for the
 *   n-up document (`make_nup`, a print temp file), opens it beside the window's document (never
 *   through `docStore`) and prints its sheets; `PrintRoot` closes it when the job ends.
 * * Handler path (`prepareHandlerFile`): `print_prepare` (flattened with `FLAT_PRINT` and the
 *   주석 option) or, for n-up, the `make_nup` temp file. Both live in the print temp directory,
 *   deleted after 10 minutes and by the startup / exit sweeps.
 */
import * as api from "../ipc/api";
import type { DocInfo, PageIndex, PageGeom, Rotation } from "../ipc/types";
import { PRINT_DPI, scaleKeyForDpi, usePrintStore, type PrintOptions } from "./printStore";

/** True when the options need the engine's n-up document rather than the pages themselves. */
export function wantsNup(o: PrintOptions): boolean {
  return !!o.booklet || (o.perSheet ?? 1) > 1;
}

/** Display size of a page under the view rotation, in points. */
export function displaySize(g: PageGeom | undefined, rotation: Rotation): [number, number] {
  if (!g) return [612, 792];
  return rotation === 90 || rotation === 270 ? [g.heightPt, g.widthPt] : [g.widthPt, g.heightPt];
}

async function nupFile(info: DocInfo, pages: PageIndex[] | undefined, o: PrintOptions) {
  return api.makeNup({
    docId: info.docId,
    pages,
    options: {
      perSheet: o.perSheet ?? 1,
      order: o.order ?? "across",
      booklet: o.booklet ?? false,
      paper: "auto",
      annots: o.annots ?? "all",
    },
  });
}

/** The DOM path: one sheet per page (or per n-up sheet) off the `print=` page route. */
export async function startDomPrint(
  info: DocInfo,
  pages: PageIndex[] | undefined,
  rotation: Rotation,
  o: PrintOptions = {},
): Promise<void> {
  const base = {
    scaleKey: scaleKeyForDpi(PRINT_DPI),
    annots: o.annots ?? "all",
    fit: o.fit ?? "fit",
    grayscale: o.grayscale ?? false,
  } as const;
  if (wantsNup(o)) {
    const made = await nupFile(info, pages, o);
    const sheets = await api.openDocument({ path: made.path });
    const all = sheets.pages.map((p) => p.index);
    usePrintStore.getState().start({
      ...base,
      docId: sheets.docId,
      generation: sheets.docGeneration,
      pages: all,
      rotation: 0,
      sizes: all.map((p) => displaySize(sheets.pages[p], 0)),
      tempDocId: sheets.docId,
    });
    return;
  }
  const list = pages?.length ? pages : Array.from({ length: info.pageCount }, (_, i) => i);
  usePrintStore.getState().start({
    ...base,
    docId: info.docId,
    generation: info.docGeneration,
    pages: list,
    rotation,
    sizes: list.map((p) => displaySize(info.pages[p], rotation)),
  });
}

/** The handler path's file: the flattened copy, or the n-up document. */
export async function prepareHandlerFile(
  info: DocInfo,
  pages: PageIndex[] | undefined,
  o: PrintOptions = {},
): Promise<string> {
  if (wantsNup(o)) return (await nupFile(info, pages, o)).path;
  const { tempPath } = await api.printPrepare({ docId: info.docId, pages, annots: o.annots ?? "all" });
  return tempPath;
}
