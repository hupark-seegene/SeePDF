# Stage 1 (c) — viewer: what landed, what is frozen, what is missing

Module (c) of `WORKPLAN.md` §3: the continuous-scroll viewer, the tile pipeline, the text layer,
search, and the 축소판 / 목차 / 검색 sidebar panels. Companions: `ARCHITECTURE.md` §3 and §10,
`IPC_CONTRACT.md` §3/§6/§9/§10.1, `UI_SPEC.md` §5/§8/§13, `FEATURES.md` F-01…F-07.

```
src/viewer/
  Viewer.tsx          the component App.tsx mounts (CanvasStub re-exports it)
  Scroller.tsx        scroll container, virtualisation, gestures, pointer, tile collection
  PageShell.tsx       one page + the frozen layer slots            ← (d)/(e) build on this
  TileManager.ts      admission: centre-out, in-flight cap, cancellation, LRU
  geometry.ts         page px, tile grid, scale ladder, pageToDevice / deviceToPage ← frozen
  layout.ts           rows (단일/연속/두 쪽), visibleRange, fit maths
  zoom.ts             ladder, wheel/pinch curve, cursor anchor
  viewerCommands.ts   ⌘A ⌘C ⌘G/⇧⌘G Space-tap-vs-hold Esc + the clipboard mirror
  viewer.css          canvas, page, layer and mark styling (tokens only)
  index.ts            the public surface for (d) and (e)
  text/  TextLayer.ts  textLayers.ts  selection.ts  PageMarks.tsx
  search/ SearchController.ts
src/sidebar/
  Thumbnails.tsx  Outline.tsx  SearchPanel.tsx  sidebar.css
```

---

## 1. Frozen API — `pageToDevice` and the `PageShell` slots

### 1.1 The affine (`src/viewer/geometry.ts`)

```ts
export function pageToDevice(g: PageGeom, viewRotation: Rotation, s: number): Mat6;
export function deviceToPage(m: Mat6): Mat6;
export function applyMat(m: Mat6, x: number, y: number): Point;      // dx = a·x + c·y + e
export function rectToBox(rect: Rect, m: Mat6): { x, y, w, h };      // user-space rect → CSS box
```

`IPC_CONTRACT.md` §3 verbatim, cross-tested against `engine::render::geometry::page_to_device`
(`vitest geometry.rotations` reproduces the Rust `render_rotation_sizes` table, the corner mapping
of `matrix_maps_the_crop_box_corners`, and both device rectangles measured in `spikes/text.md` §2).
`s` is **CSS px per point** (`zoomPercent / 100`) for anything in the DOM; the *device* scale that
the engine renders at is `scaleKey / 100` and only the bitmap layer uses it.

Also frozen and exported from `src/viewer` (see `index.ts`): `pagePixels`, `pageBoxCss`,
`shouldTile`, `tileGridFor`, `tileRect`, `tilePriority`, `scaleKeyFor`, `placeholderScaleKey`,
`computeLayout`, `visibleRange`, `pageRows`, `fitZoomPercent`, `anchorAt`, `scrollForAnchor`.

### 1.2 `PageShell` layers

Bottom to top inside `.page-shell` (absolutely positioned, `data-page={index}`):

| # | element | owner | notes |
|---|---|---|---|
| 1 | `.page-bitmaps > img.ph` | (c) | low-res whole-page placeholder, `s = min(1, 640/w_pt)`; always mounted |
| 2 | `.page-bitmaps > img.page-bitmap` **or** `img.page-tile[]` | (c) | whole page while `s ≤ 2` and `≤ 2.5 Mpx`, else the 512 px mosaic |
| 3 | `.page-marks` | (c) | selection + search rectangles (`mix-blend-mode: multiply`) |
| 4 | `.page-annots` | **(d)** | `layers.annotations` |
| 5 | `.page-forms` | **(d)** | `layers.forms` |
| 6 | `.page-surface` | **(d)** | `layers.surface` — pointer capture |

Only `.page-bitmaps` is filtered by night mode and CSS-scaled during a zoom gesture; every layer
above it is laid out at the live zoom, so handles and inputs never blur or lag.

```tsx
// (d)/(e): one renderer, called once per mounted page
<Viewer layers={(ctx: PageLayerContext) => ({ annotations, forms, surface, under })} />

export interface PageLayerContext {
  docId: DocId; docGeneration: DocGeneration; index: PageIndex; page: PageGeom;
  rotation: Rotation;            // view rotation only — the intrinsic /Rotate is inside `page`
  zoomPercent: number; scale: number;          // scale = zoomPercent / 100 (CSS px per point)
  width: number; height: number;               // CSS size of the page box
  matrix: Mat6; inverse: Mat6;                 // pageToDevice / deviceToPage at `scale`
  toDevice(x, y): Point; toPage(x, y): Point;  // PDF user space ⇄ CSS px inside the box
  rectToBox(rect: Rect): { x, y, w, h };
  svg: { viewBox: string; transform: string }; // user-space viewBox + the y-flip, for own <svg>
}
export type PageLayers = { under?: ReactNode; annotations?: ReactNode; forms?: ReactNode; surface?: ReactNode };
```

`annotations` is rendered inside `<svg viewBox="0 0 w h"><g transform="matrix(a b c d e f)">`, so
a slot draws **in PDF user space** and nothing recomputes on zoom — only the `<g>` transform
changes. `forms` and `surface` are absolutely positioned boxes covering the page; place HTML with
`ctx.rectToBox(...)`.

`PageShell` itself is exported for anyone who needs to mount a page outside the scroller (the
organizer's big preview, a print DOM): `makePageLayerContext({ docId, docGeneration, page,
rotation, zoomPercent, width, height })` builds the context.

### 1.3 Selection and search, for (d)

```ts
useSelectionStore()      // { selection: { docId, anchor: {page, offset}, focus }, setSelection, clear }
orderSelection(sel)      // → { start, end } in document order
pageRange(sel, page, n)  // → [start, end) char offsets this page contributes
selectionText(sel, gen)  // → the string, with real line breaks
getTextLayer(docId, gen, page)   // PageTextLayer | null (cached, LRU 24 pages)
  .hitTest(x, y) .wordRange(i) .lineRange(i) .rangeRects(a, b) .rangeText(a, b)
useSearchStore()         // { hits, byPage, current, run, step, select, cancel, … }
```

A markup tool in (d) reads `useSelectionStore.getState().selection` on pointer-up and turns
`layer.rangeRects(...)` into the `rects` of an `AnnotSpec` — the rectangles are already one per
line, which is exactly what the quad builder wants (IPC_CONTRACT §7.1).

---

## 2. Store shapes (unchanged contracts, new owners)

`docStore` and `viewStore` kept their Stage 0 shape — the status bar, the keymap, the native menu
and the sidebar all drive the viewer through them and nothing else:

| store | field | who writes it |
|---|---|---|
| `docStore` | `info`, `outline`, `changeNonce`, `changedPages` | `open`/`applyDocChanged`; the viewer only reads |
| `viewStore` | `zoomPercent`, `zoomMode` | status bar, ⌘±/⌘0/⌘8/⌘9, and the viewer when a fit mode resolves (`setZoomMode(mode, pct)`) |
| | `layout`, `rotation`, `night` | status bar / keymap |
| | `currentPage` | **the scroller** on every scroll frame (`currentPageAt`) |
| | `scrollRequest` | anyone calling `goToPage`; the scroller consumes it |

New stores, both inside `src/viewer/**` (not `src/store/`, because they are the viewer's own):
`useSelectionStore` (text selection) and `useSearchStore` (query, streamed hits, current hit).

What deliberately stayed out of every store (ARCHITECTURE §10): scroll offset, in-gesture zoom,
the tile inventory, pointer state, and the layout itself. A fling over 500 pages does **zero**
React work per frame; the scroll offset is committed to state only every 96 px or on settle.

---

## 3. How the pipeline works (the parts worth knowing)

* **Virtualisation** — rows (`pageRows`) instead of pages, so 두 쪽 is the same code path. Only
  rows inside `[scrollTop − 1.5·vh, scrollTop + 2.5·vh]` are mounted.
* **Scale ladder** — `scaleKey = round(zoomPercent × dpr)` (≤ 800), device scale `s = sk/100`,
  page pixels `round(fround(pt) × s)`, tiles 512 device px above `s > 2` or 2.5 Mpx. The CSS page
  box is `devicePixels / dpr`, so the mosaic covers the box exactly — no seams, no 1 px edge.
* **Zoom** — the layout is recomputed at the live zoom immediately (cheap: it is arithmetic over
  `DocInfo.pages`), and the scroll offset is re-anchored in a layout effect before paint. The
  *bitmap* layer keeps the previous scale key and wears a CSS `scale()` until 120 ms after the
  last zoom event, so a pinch never storms the engine and sharp tiles land one settle later.
* **Tile admission** (`TileManager`) — centre-out Chebyshev priority plus a page-distance term,
  ≤ 24 requests in flight (8 during a fling > 1.5 px/ms, halved again on `engine-pressure: high`),
  tiles of a superseded render key dropped before they mount, ≤ 120 mounted tiles with the
  farthest evicted first. Tiles already fetched once are re-admitted for free (the webview cache
  serves them `immutable`).
* **Invalidation** — every URL carries `gen`, so a `doc-changed` bumps the render key and the old
  `<img>`s are replaced in the same commit; `changedPages` also invalidates the text-layer cache.
* **Text layer** — one `get_text_layer` per (doc, generation, page), wrapped in `PageTextLayer`:
  typed arrays, no DOM node per character. Hit-testing snaps to the nearest line, then the nearest
  glyph; pdfium's generated zero-size whitespace is never a hit but is always copied.
* **Clipboard** — macOS puts the native Edit menu in front of the webview (`app/menu.rs` uses the
  predefined `copy:` / `selectAll:` items), so ⌘C and ⌘A never reach a JS handler. The viewer
  keeps an off-screen **clipboard mirror** holding exactly the selected string and points the DOM
  selection at it; native Copy, the Edit menu and ⌘C then all copy the right text, and a
  `selectionchange` that covers the canvas is turned into a page-wide selection (that is ⌘A).
  `document.execCommand("copy")` runs synchronously inside the key gesture — awaiting
  `navigator.clipboard.writeText` first loses the gesture and silently copies nothing.

---

## 4. Measured, on this Mac (M-series, dpr 2, `npm run tauri dev`, debug build)

| Scenario | Measured | Budget (ARCHITECTURE §13) |
|---|---|---|
| `tracemonkey.pdf` (14 p): document open → first page painted | **29 ms** | ≤ 250 ms |
| `gen/500p.pdf` (500 p): document open → first page painted | **21 ms** | ≤ 400 ms |
| webview start → `open_document` resolved (dev server, not a release bundle) | 187 ms / 225 ms | — |
| fling 1 → 14 and 1 → 500 at ~1,200 px/frame | no white gaps, no dropped page boxes, thumbnail rail keeps up | 60 fps |
| 400 % ctrl+wheel zoom then pan | tiles crisp, anchor holds under the cursor | sharp ≤ 250 ms |
| search `monkey` over `tracemonkey.pdf` | **62 hits**, streamed, first results ≈ instant | 62 (F-07) |
| select + copy the title line | clipboard = `Trace-based Just-in-Time Type Specialization for ` | exact (F-06) |
| ⌘A on page 1 | 5,087 chars selected (= the text spike's char count for p1) | — |

The two first-paint numbers are *document open → first `<img>` onload* (`window.__seepdfOpenAt` →
`window.__seepdfFirstPaint`, both left in the build as a perf probe). They exclude the vite dev
server's module loading, which is what the release harness of `scripts/perf-baseline.mjs` will
replace in Stage 2.

Jank seen: none in the canvas. The one visible artefact during a hard fling is a page box that is
still white for one or two frames when it enters the window at > 1,000 px/frame — its placeholder
is in flight. Pages entering at normal scroll speed always have their placeholder already.

---

## 5. Verified live (screenshots taken with `screencapture` during a `tauri dev` session)

Open by argv · continuous scroll 1 → 14 and 1 → 500 · fit width on mount with the page top at the
top · ctrl+wheel zoom to 400 % and back to 25 % (cursor-anchored) · pan · drag text selection with
correct rectangles · ⌘C · ⌘A · search with a live count, highlight-all, orange current hit and the
results list · thumbnails (virtualised, current-page ring, click to navigate) · outline
(`gen/outline-labels.pdf`: 7 nodes, 3 levels, disclosure triangles, active-section highlight,
click navigates to page 6) · 두 쪽 spreads · view rotation (⌘R and the status-bar buttons, page
boxes and thumbnails both rotate) · dark theme throughout.

---

## 6. Files outside `src/viewer/**` and `src/sidebar/**` that (c) touched

Three edits the integrator should be aware of — each is the minimum that makes (c)'s work
reachable, and none changes anyone else's logic:

1. `src/app/CanvasStub.tsx` — now a one-line re-export of `src/viewer/Viewer` (planned by
   STAGE0_FRONTEND_NOTES §3; `App.tsx` is untouched).
2. `src/app/canvasLayout.ts` — **deleted**; it had one import site (`CanvasStub`) and its own
   docstring says Stage 1 (c) moves it to `src/viewer/layout.ts`.
3. `src/app/SidebarFrame.tsx` — the three inline Stage 0 panel bodies (thumbnail rail, outline
   tree, empty search box) replaced with `<Thumbnails />`, `<Outline />`, `<SearchPanel />`. The
   주석 tab still renders the empty state, untouched, for (d)'s `AnnotationList`.
4. `src/app/shell.test.tsx` — one assertion: `.page-shell` count `3` → "at least one, and page 0
   is mounted". The Stage 0 canvas mounted every page; a virtualised viewer must not.

---

## 7. Gaps, and who should close them

1. **`OutlineNode` has no destination rectangle** (IPC_CONTRACT §4 gives `page` only), so 목차
   navigation is a page jump, not "scroll to the heading". Closing it needs a contract change
   (`dest?: { x, y, zoom }`) — integrator + backend.
2. **⌘G / ⇧⌘G / 선택 해제 arrive as `menu:<id>` on macOS** and are handled inside the viewer
   (`viewerCommands.ts`) because `useCommands` logs them as unimplemented. When (e) or the
   integrator fills those branches in `src/app/useCommands.ts`, delete the duplicate here.
   `edit.copy` / `edit.selectAll` cannot be handled there at all: they are *predefined* native
   items with no event — the clipboard mirror is the only way.
3. **`src/ipc/events.ts` rejects on unlisten under React 19 StrictMode** ("undefined is not an
   object (evaluating 'listeners[eventId].handlerId')") — every `useEffect(() => onX(...))` in
   `App.tsx` and the viewer's `onEnginePressure` produce one unhandled rejection per mount in dev.
   Harmless, noisy; the fix belongs in `events.ts` (guard the double unlisten).
4. **Two-page mode pairs from an even index** (1|2, 3|4 …). If the spec later wants a lone cover
   page, change `pageRows` — `layout.twoPage` is the test that pins it.
5. **No reading-position restore yet**: recents carry `lastPage`/`zoomPercent`/`layout`
   (IPC_CONTRACT §11) but nobody writes them back. (e) owns recents; one `goToPage` + `setZoom`
   on open closes it.
6. **Snapshot tool, hand-tool cursor art, night-mode sepia tuning** are stubs/approximations:
   `render_page_raw` is wired in `api.ts` but the 스냅샷 marquee is (d)'s tool, and night mode is
   P1 (the CSS filter and the `night` URL parameter are in place and work).
7. **Critical-path bundle is 116.6 kB gz of a 120 kB budget** (`scripts/check-bundle-size.mjs`);
   the viewer added ~13 kB gz. The next module to land must code-split, or the budget moves.
   (`dist/` total fails the 2048 kB gate only because `public/ocr/**` (8.4 MB) is in the tree —
   that is (f)'s asset pipeline, not a viewer regression.)
8. **Colour literal**: `.page-shell` paper white is `var(--page-paper, #fff)` — the token does not
   exist yet, and the same literal is already in `styles/shell.css`. Adding `--page-paper` to
   `styles/tokens.css` is a one-line chore for whoever owns tokens next.
