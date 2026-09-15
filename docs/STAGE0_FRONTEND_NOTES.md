# Stage 0 — frontend foundation, notes for the Stage 1 agents

What landed, what is a stub, and the conventions you inherit. Companions: `IPC_CONTRACT.md` (the seam,
frozen), `UI_SPEC.md` (layout, tokens, keymap, i18n), `ARCHITECTURE.md` §10 (frontend architecture),
`WORKPLAN.md` (who owns which file).

```
src/
  ipc/      types.ts api.ts events.ts protocol.ts mock.ts binary.ts png.ts env.ts bus.ts
  store/    appStore docStore viewStore annotStore pagesStore jobStore
  i18n/     index.ts useT.ts ko.json en.json          (414 keys, ko is the design reference)
  styles/   tokens.css base.css shell.css
  keys/     keymap.ts useKeymap.ts
  app/      TitleBar ModeSwitcher ToolStrip SidebarFrame CanvasStub Inspector StatusBar WelcomeStub
            IconButton Tooltip tools.ts useCommands.ts canvasLayout.ts
  tools/    ToolController.ts                         (stub interface — (d) implements)
  test/     setup.ts ipc-samples/*.json
```

Run it: `VITE_SEEPDF_MOCK=1 npm run dev` → http://localhost:1420, no Rust involved.
Check it: `npm run check` (typecheck + vitest + i18n), `npm run check:size` after a build.

---

## 1. The IPC layer

* **`types.ts` is frozen** (WORKPLAN §0.2). It is the TypeScript of `IPC_CONTRACT.md`, verbatim. A change
  is a PR to the integrator together with `src-tauri/src/ipc/types.rs`.
* **`api.ts` is the only file that may import `invoke`.** One exported function per command, camelCase of
  the Rust name (`open_document` → `openDocument`). Never call `invoke` from a component.
* Errors arrive as `SeePdfError extends Error` with `.code: ErrorCode`, `.page`, `.detail`.
  Render them with `t(errorKey(e))` and put `e.message` behind 자세히 (`error.details`).
* **Progress channels are plain callbacks** in the wrapper API: `saveDocument(a, (e: JobEvent) => …)`.
  `api.ts` wraps the callback in a real `tauri::ipc::Channel` outside mock mode; feed the events straight
  into `useJobStore.getState().apply(kind, labelKey, e)` and the status bar slot animates for free.
* **Binary payloads** (`binary.ts`): `getTextLayer()` returns a decoded `TextLayerView` over the §10.1
  buffer (`char(i)`, `word(i)`, `line(i)`, `text`, typed arrays inside — no DOM node per character);
  `getTextLayerBuffer()` gives you the raw `ArrayBuffer` if you want to own the views.
  `renderPageRaw()` returns `{ width, height, stride, pixels }` from the §10.2 "SPRX" buffer, ready for
  `new ImageData(...)` → `createImageBitmap`.
* **`protocol.ts`** builds every `seepdf://` URL: `tileUrl`, `pageUrl`, `thumbUrl`, `ocrImageUrl`,
  `recentThumbUrl`, `rawDocumentUrl`, plus `scaleKey(zoomPercent, dpr)` and `tileGrid(w, h)`.
  It never sniffs the OS when Tauri is present (`convertFileSrc('', 'seepdf')`); the sniffing fallback
  (`seepdf://localhost` vs `http://seepdf.localhost`) only exists for `vite dev` and vitest.
* **`events.ts`** wraps the §8 broadcasts. Every helper returns a *synchronous* unsubscribe, so
  `useEffect(() => onDocChanged(fn), [fn])` is correct. `onMenuCommand(handler, MENU_IDS)` bridges the
  native menu; `onFileDrop` uses `getCurrentWebview().onDragDropEvent` in Tauri and DOM drag events in
  the browser.

### Using the mock

`VITE_SEEPDF_MOCK=1` (and *automatically* whenever `window.__TAURI_INTERNALS__` is missing) routes every
`api.*` call to `src/ipc/mock.ts`. It is loaded with a dynamic `import()`, so neither it nor its fixtures
reach the production bundle — do not import `mock.ts` from app code.

What is real in the mock: the document/page model, `docGeneration` + `dirty` + undo/redo, a **binary text
layer with genuine word boxes** (selection, hit-testing and search can be exercised), streaming
`search_start` with cancel, annotations CRUD, form fields, `page_ops`, job progress with cancel, recents,
settings, and `doc-changed`/`doc-saved`/`recents-changed` events. Pixels are generated PNG `data:` URLs
(`png.ts` is a ~100-line encoder), so `<img src={pageUrl(...)}>` paints without a backend.

* Fixtures live in `src/test/ipc-samples/` — `document.json` (a 3-page A4 `DocInfo`), `text.json` (the
  lines each page is built from: `{ text, x, y, size }` in PDF points), `outline.json`, `annots.json`,
  `form-fields.json`, `recents.json`, `settings.json`. Add to these rather than hard-coding data in a test.
* Test helpers: `resetMock()` (called for you in `src/test/setup.ts` before every test) and
  `pushPendingOpen(path)` for the `take_pending_opens` path.
* Opening a path that matches `/encrypted/i` rejects with `passwordRequired` — use it for the password
  dialog flow. A path listed in `recents.json` opens with that entry's page count.

---

## 2. Store shapes

All zustand, one file per owner, `create<T>()` with actions inside the state object. Outside React use
`useXStore.getState()`; inside, always select (`useDocStore((s) => s.info)`).

| Store | Stage 1 owner | Holds |
|---|---|---|
| `appStore` | Stage 0 → (e) for settings/recents | `os`, `locale`, `theme`, `settings`, `recents`, `ready`, `sidebarOpen/Tab/Width`, `inspectorOpen`, `mode`, `tool`, `momentaryFrom`; `bootstrap()`, `setMode`, `setTool(tool, momentary)`, `releaseMomentary()` |
| `docStore` | (c) | `docId`, `info: DocInfo`, `outline`, `status`, `error`, `changeNonce`, `changedPages`; `open/close/refresh/undo/redo`, `applyDocChanged(e)` |
| `viewStore` | (c) | `zoomPercent`, `zoomMode`, `layout`, `rotation`, `night`, `currentPage`, `scrollRequest`; `setZoom`, `zoomIn/Out`, `setLayout`, `rotate(±90)`, `goToPage` |
| `annotStore` | (d) | `byPage`, `selected`, `ghosts` (optimistic), `style` (colour/opacity/width/fontSize/fill); `PALETTE` = the 8 swatches with their highlight alphas |
| `pagesStore` | (e) | `selected`, `lastAnchor`, `thumbSize`, `dropAt`; `toggle(page, additive)`, `selectRange`, `selectAll` |
| `jobStore` | (f), shared | `jobs`, `active`; `apply(kind, labelKey, jobEvent)` folds `JobEvent`s into the status-bar slot, `cancel(id)` calls `cancel_job` |

Rules inherited from ARCHITECTURE §10: **scroll position, in-gesture zoom scale, tile inventory, ink
points and pointer state never enter a store** — they are refs/classes, so React does not re-render per
frame. `changeNonce` exists so per-page caches can invalidate without deep comparison.

`applyDocChanged` is the single entry point for a new generation; `App.tsx` already wires
`onDocChanged → docStore.applyDocChanged`, and a `structure: true` event triggers a `get_document`.

---

## 3. What (c) must provide — PageShell and `pageToDevice`

Stage 0 ships `src/app/CanvasStub.tsx`: real page boxes, real fit-width/fit-page maths
(`src/app/canvasLayout.ts`), one `<img src={pageUrl(...)}>` per page, a scroll listener that maintains
`viewStore.currentPage`, and `scrollRequest` handling. **Delete it** when `src/viewer/**` lands; the shell
only needs a component in the same slot inside `App.tsx`.

Expectations the rest of the frontend is written against:

* `.page-shell` is the positioned outer box of one page: `position: relative`, size
  `displaySize(page, zoomPercent, rotation)` in CSS px, `data-page={index}`, `--page-shadow`, 16 px gap.
  Inside it, bottom to top: placeholder `<img class="ph">` → tile `<img>`s → selection/search rects →
  SVG annotation overlay → form inputs → pointer-capture surface (ARCHITECTURE §10).
* Export **`pageToDevice(geom, viewRotation, scale): Mat6`** from `src/viewer/geometry.ts`, exactly the
  function in `IPC_CONTRACT.md` §3 (crop box + `/Rotate` + view rotation + scale, row-vector convention
  `dx = a·x + c·y + e`). (d) and (e) build every hit-test and every handle on it, and it is cross-tested
  against the Rust table — **freeze it by day 2 of Stage 1** and tell the other agents.
* Give the SVG overlay a `viewBox` in PDF user space with the y-flip `matrix(1 0 0 -1 0 h)` so shapes do
  not recompute on zoom, and keep the tile layer the only thing night mode filters.
* The tile URL is `tileUrl({ doc, gen, page, sk: scaleKey(zoomPercent), rot, tx, ty })`; `gen` must be
  `info.docGeneration` or the engine answers `410 Gone`.

---

## 4. i18n

```ts
import { useT } from "../i18n/useT";
const t = useT();                       // re-renders on locale change
t("sidebar.search.results", { count })   // → "결과 3개" / "3 results"
```

* 414 flat keys in `ko.json` / `en.json`, generated from the tables of `UI_SPEC.md` §15. Korean is the
  design reference: size components against the Korean string first.
* `_other` is used when `count !== 1`; Korean repeats the singular on purpose. Never concatenate —
  interpolate `{{placeholders}}`, identical in both locales.
* Numbers and dates go through `formatNumber` / `formatBytes` / `formatRelativeDay` (Intl with the active
  locale). Outside React use `t(key, params)` from `src/i18n` directly.
* `node scripts/check-i18n.mjs` fails on missing/extra keys, placeholder drift, empty strings, a plural
  without its singular, **and a literal `t("…")` in `src/` that no catalogue defines**. Add your keys to
  both files in the same commit.

---

## 5. Keymap, menu ids and commands

* `src/keys/keymap.ts` is the table of `UI_SPEC.md` §13: `{ id, labelKey, mac[], win[], when, group }`.
  `id` is simultaneously the keymap id, the native menu id (`menu:<id>` events) and the command id.
* `when` is the context guard: `always` · `doc` (a document is open) · `canvas` (canvas focused, no input
  editing) · `pages` (페이지 mode). `App.tsx` computes the active contexts; single-key bindings never fire
  while an input has focus.
* One documented chord collision: `Space` = 다음 페이지 on a tap and 손 도구 while held
  (`sharesChordWith`). The 200 ms tap/hold discrimination belongs to (c)'s pointer layer.
* Tooltips and menus render the chord with `shortcutFor(id, os)` → `⌘⇧S` / `Ctrl+Shift+S`.
* **`src/app/useCommands.ts` is the single dispatcher.** Every toolbar button, keymap hit and native menu
  item calls `run(id)`. Ids that belong to a Stage 1 module currently log
  `[command] <id> — not implemented in Stage 0`; that log line is your integration checklist. Known gaps:
  `file.print`, `file.export`, `file.docInfo`, `file.openRecent`, `app.settings`, `app.overflow`,
  `edit.cut/copy/paste/delete/duplicate`, `edit.findNext/findPrevious`, `go.goToPage`, `go.back/forward`,
  `view.readingMode`, `view.fullScreen`, `pages.extract`, `pages.insert`.

---

## 6. Styling

`styles/tokens.css` holds every token of `UI_SPEC.md` §14 — **no colour literal anywhere else**.
Light is on bare `:root`; dark is defined twice, under `@media (prefers-color-scheme: dark)
:root:not([data-theme="light"])` and under `:root[data-theme="dark"]`, so the settings override wins in
both directions. `data-os` on `<html>` drives the 78 px macOS traffic-light gutter.

`styles/shell.css` styles the chrome. Reusable primitives already there: `.icon-btn` (28×28, 20 px lucide
glyph, stroke 1.75, `data-active` = accent-subtle + 1 px accent border), `.segmented/.segment`, `.field`,
`.btn(.primary|.quiet)`, `.chip`, `.swatch`, `.empty`, `.banner`, `.tooltip`. Use `<IconButton>` and
`<Tooltip>` from `src/app/` rather than re-rolling them — the 500 ms tooltip with the shortcut appended is
a spec rule, not a nicety.

The "not templated" rules of §14.7 are enforced by review: accent only for state, no shadow on anything
that is not floating, alpha borders, radius steps with elevation (6 flat / 10 floating / 14 sheets).

Keyboard focus must stay visible: `:focus-visible` gets `--focus-ring`; never set `outline: none` without
replacing it.

---

## 7. Tests and scripts

* `vitest + jsdom`, config in the `test` block of `vite.config.ts`, setup in `src/test/setup.ts`
  (jest-dom matchers, a `ResizeObserver` shim, `resetMock()` + `setLocale("ko")` before each test).
  `VITE_SEEPDF_MOCK=1` is forced for tests.
* Existing suites: i18n parity/plurals/Intl, keymap (ids, per-context chord uniqueness, mac/win matching,
  display), protocol (per-platform origin, query building, tile maths), binary (§10.1/§10.2 round trips),
  mock adapter (open/errors/text layer/search/annotations/undo/page ops/save/recents/forms), stores, and
  an `App` render smoke test (welcome + recents, chrome, mode switch, ⌘2 and `H`, status-bar zoom).
* `scripts/check-i18n.mjs` — the i18n gate (above).
* `scripts/check-bundle-size.mjs` — run after `npm run build`; budgets are 120 kB gz for the critical path
  (entry JS + CSS) and 2048 kB for `dist/`. Today: **103.4 kB gz / 370 kB**. Stage 1 must code-split
  dialogs, the organizer and the OCR worker to stay under it.
* `scripts/perf-baseline.mjs [--write]` — writes `docs/perf/baseline.md`. It fills the two frontend rows
  automatically from the bundle report; every engine row is `—` until Stage 2 drives the release binary.
* `.github/workflows/ci.yml` — a fast `frontend` job (i18n, typecheck, vitest, mock build, size gate) plus
  the 3-target matrix (macos-latest/arm64, macos-13/x64, windows-latest): `npm ci` with `PDFIUM_TARGET`,
  typecheck, vitest, `cargo fmt/clippy/test --release`, `tauri build --debug`, artifact upload.

---

## 8. Open issues / known gaps

1. **Pretendard subset is not committed.** `--font-ui` falls back to Apple SD Gothic Neo / Malgun Gothic,
   so Korean metrics differ per platform. Dropping the woff2 into `public/fonts/` plus an `@font-face`
   with `font-display: swap` is a Stage 1 chore (ARCHITECTURE §5 assets table).
2. **`CanvasStub` is not virtualised** — it mounts every page. Fine for a 3-page mock, wrong for 500 pages;
   (c) replaces it wholesale.
3. **페이지 mode still shows the canvas.** UI_SPEC §9 wants the grid to replace it and the sidebar to
   auto-collapse — (e)'s `src/organize/**`.
4. **Sidebar panel bodies are chrome-only**: the thumbnail rail renders real (mock) thumbnails and the
   outline tree navigates, but the annotation list and the search panel are empty states. Search input is
   not wired to `search_start` yet — (c) owns `SearchPanel`, (d) owns `AnnotationList`.
5. **`ToolController` is a stub** (`src/tools/ToolController.ts`): it remembers the armed tool and its
   cursor and commits nothing. The interface (`ToolModule.onDown/onMove/onUp → { state, preview, commit }`)
   is the contract the tool strip already calls; (d) replaces the implementation, keeps the export name.
6. **Native menu ids are declared but nothing emits them yet** (the Rust side is mid-rewrite). `App.tsx`
   already listens for `menu:<id>` for every keymap id, so the backend only has to emit them.
7. **`window.__TAURI_INTERNALS__` detection decides mock vs real**: in a Tauri window without
   `VITE_SEEPDF_MOCK` the app talks to the engine, and every command the backend still stubs will reject
   with `unsupported` — expect that until Stage 1 (a)/(b) land.
8. Inspector contents are a style panel for 주석 only (swatches / 불투명도 / 굵기 bound to `annotStore`);
   the per-context panels of UI_SPEC §7 are (d)'s `src/app/Inspector/**`.
9. No context menus, no dialogs, no toasts yet — (e) owns `ContextMenu.tsx`, `Toasts.tsx`, `src/dialogs/**`.
