# Stage 1 (e) — page organizer, dialogs, welcome: what landed, what integration must wire

Module (e) of `WORKPLAN.md` §3. Companions: `UI_SPEC.md` §9–§12, `IPC_CONTRACT.md` §7.3/§7.6/§7.7/§11,
`FEATURES.md` F-15…F-18, F-20, F-23…F-26, `spikes/pages.md` (§1 move semantics, §4 export costs).

```
src/organize/
  Organizer.tsx        the virtualised grid + the right action rail (page 페이지 mode)
  selection.ts         click / ⇧click / ⌘click / marquee / roving focus — pure
  moveOp.ts            drag caret → exactly one `PageOp` (FPDF_MovePages semantics) — pure
  organize.css         grid, cell, caret, marquee, rail
  index.ts             `default` = Organizer (the lazy target) + the pure helpers
src/dialogs/
  dialogState.ts       the dialog stack + `askUnsaved/askPassword/askMultipleFiles` — ENTRY-SAFE
  flows.ts             open (password retry, position restore), save/save as, unsaved gate,
                       multi-file drop, merge, `runPageOps`, extract, insert-from-file, print
  Dialog.tsx           the shared sheet (Esc, focus restore, 36 px accent primary)
  RangePicker.tsx      모든/현재/선택한/직접 입력 page range, shared by 내보내기·인쇄·추출
  pageRange.ts         `parsePageRange("1-3,5,8-")` ⇄ `formatPageRange` (1-based on the wire)
  ExportDialog · MergeDialog · SplitDialog · PrintDialog · SettingsDialog · DocInfoDialog
  Prompts.tsx          암호 · 저장하지 않은 변경 사항 · 여러 파일 · 페이지 추출 · 파일에서 삽입
  DialogHost.tsx       renders the top of the stack + (f)'s OCR sheet; lazy, `default` export
  dialogs.css
src/welcome/
  Welcome.tsx  welcome.css  index.ts      (replaces `src/app/WelcomeStub.tsx`, deleted)
src/app/
  ContextMenu.tsx  contextMenuStore.ts    UI_SPEC §12, keyboard-navigable, themed
  Toasts.tsx       toastStore.ts          bottom-trailing stack, actions + 자세히
  pageMenus.ts                            the canvas / 축소판 menus, built off (c)'s DOM contract
src/store/pagesStore.ts                   selection, thumb size, drop caret, optimistic order
```

Run it: `VITE_SEEPDF_MOCK=1 npm run dev -- --port 1421`.
Check it: `npx vitest run src/organize src/dialogs src/welcome` · `npm run check` · `npm run check:size`.

---

## 1. The seams other modules may use

Everything below is entry-safe (tiny, no CSS, no component): import it from anywhere, including
`useCommands.ts`.

```ts
import { openDialog, askUnsaved, askPassword, askMultipleFiles } from "../dialogs/dialogState";
openDialog("export" | "merge" | "split" | "print" | "settings" | "docInfo" | "extract" | "insertFrom");
await askUnsaved(name);            // "save" | "dontSave" | "cancel"
await askPassword(fileName, wrong) // string | null, retried in place

import { toast } from "../app/toastStore";
toast("export.done", { name }, { tone: "success", actions: [revealAction(dir)], detail });

import { openContextMenu } from "../app/contextMenuStore";
openContextMenu({ x, y, items: [{ id, labelKey, onSelect }, { id, separator: true }] });
// `label` (raw text) exists for file names only; everything else goes through i18n.
```

Everything heavy is behind a dynamic import and must stay that way:
`import("../dialogs/flows")`, `import("./dialogs/DialogHost")`, `import("./organize")`,
`import("./welcome")`, `import("./app/Toasts")`, `import("./app/ContextMenu")`,
`import("./app/pageMenus")`. The critical path is **103.3 kB gz of the 120 kB budget** with all of
this landed (it was 117.3 kB before this module — deleting `WelcomeStub` and code-splitting the
organizer/dialogs bought back 14 kB; CSS grew 4.9 → 9.7 kB gz and *every* CSS file counts against
that budget, lazy or not, so keep new CSS lean).

`pagesStore` is the organizer's state: `selected` (ascending, unique), `lastAnchor`, `focus`,
`thumbSize`, `dropAt` (the insertion caret) and `pendingOrder` (the optimistic drag order, dropped on
the next `changeNonce`). `reset()` on every document change.

---

## 2. Files outside my directories that I touched

1. **`src/App.tsx`** — mount points only: lazy `Welcome`/`Organizer`/`DialogHost`/`Toasts`/
   `ContextMenu`, 페이지 mode routes to the organizer and auto-collapses the sidebar (UI_SPEC §9),
   `take_pending_opens` + `open-file` + `onFileDrop` now go through `flows.openPaths`, the window
   close guard (`onCloseRequested` → 저장/저장 안 함/취소), one document-level `contextmenu`
   listener, and the keymap context is now `doc + (pages | canvas)` so the grid owns the arrow keys,
   ⌫ and the tool letters in 페이지 mode.
2. **`src/app/useCommands.ts`** — filled in `file.open/openRecent/close/save/saveAs/export/print/
   docInfo`, `app.settings/overflow`, `tools.ocr/merge/split`, `edit.delete/duplicate` (페이지 mode),
   `go.goToPage`, `pages.*` (rotate/delete/duplicate/reverse/extract/insert/insertFrom/selectAll) and
   the 페이지-mode branch of `view.rotateLeft/Right`. Annotation, object, undo/redo and search ids are
   untouched — they stay with (c)/(d).
3. **`src/app/WelcomeStub.tsx`** — deleted (replaced by `src/welcome/**`).
4. **`src/i18n/{ko,en}.json`** — 24 new keys in both (`pages.range.*`, `pages.goTo`,
   `pages.insertAfter`, `pages.exportImage`, `pages.{extract,split,merge}.done`, `common.{open,remove,
   moveUp,moveDown,browse,yes,no}`, `settings.{defaultZoom,recentsCount,backups}`,
   `dialog.docInfo.pages`). `node scripts/check-i18n.mjs` is green at 442/442.

`useDocStore.setState(...)` is used once, in `flows.mergePaths`, because `merge_documents` returns a
brand-new in-memory `DocInfo` and `docStore` has no "adopt this document" action. **Request to (c) /
the integrator:** add `docStore.adopt(info, outline)` and delete that `setState`.

---

## 3. Behaviour that the backend has to match

| What the UI does | What (b) must provide |
|---|---|
| One drag = **one** `page_ops` with a single `move` op. `pages` is the block in document order, `to` is an index into the array *with the moved pages removed* — `move([2,3] → 4)` on 14 pages produces `A … D C B …` exactly as `spikes/pages.md` §1 measured. `organize.dnd` pins the table. | `FPDF_MovePages` ordered-list semantics, validated (no duplicates, `dest + len ≤ count`) before the call |
| Every page op goes through `page_ops` and the UI then waits for `doc-changed { structure: true }` → `get_document`. The optimistic order is dropped on the next `changeNonce`. | `structure: true` on every page op, and a fresh `DocInfo` in the reply |
| Rotation is shown from `PageGeom.rotation` and the cell aspect from `widthPt/heightPt`. | `widthPt/heightPt` must be the **display** (rotated) size after a rotate op, per IPC_CONTRACT §4. The mock does not swap them, so the mock's cells do not re-aspect — the real engine must. |
| `insertBlank` is sent with `size: "sameAs"`. | "same as the page currently at `at`" (falling back to A4 for an empty document) |
| 내보내기 calls `estimate_export` on every range/DPI change (debounced 120 ms) and refuses to run with an empty range. | the ±25 % accuracy of F-24; `sampledPages` is ignored by the UI |
| 이미지 내보내기 asks for a **directory** (`plugin:dialog|open` with `directory: true`) and sends `outDir` + `baseName`; text and flattened ask for a file path. | `export_images` writes `<baseName>-<n>.<ext>`; the `done` event's `outputs` is what the toast's Finder 버튼 reveals |
| 분할 shows the destination folder pre-filled with the document's own directory and reports `outputs.length` in the toast. | `split_document`'s `done` event must carry `outputs` |
| 저장 catches `readOnly` and falls through to Save As with `dialog.saveAs.readOnly`. | `save_document` on a read-only target must reject with `readOnly`, and a merged (path-less) document must also reject so Save As runs |
| 인쇄: `print_prepare` → `getCurrentWebview().print()` if it exists, else the opener plugin on the temp file. | `print_prepare` returns a flattened temp path; **integrator**: the opener permission (or `core:webview:allow-print`) has to be in `capabilities/default.json` — WORKPLAN §6 lists this as unverified and nothing verifies it yet |
| Recents are written on open, on save and on window close: `lastPage`/`zoomPercent`/`layout` come from `viewStore`, `thumbId` from `write_recent_thumbnail`. | `write_recent_thumbnail` must work immediately after `open_document` (it is called once per new entry) |
| Password: `open_document` is retried in place with the typed password. | `passwordRequired` first, `passwordWrong` on a bad retry — the dialog keys off exactly those two codes |

Nothing here needs a contract change except the two items in §5.

---

## 4. Tests

```sh
npx vitest run src/organize src/dialogs src/welcome     # 6 files, 39 tests, green
npx vitest run                                          # 266 tests, green (whole repo)
npx tsc --noEmit                                        # clean
node scripts/check-i18n.mjs                             # 442/442
VITE_SEEPDF_MOCK=1 npm run build && node scripts/check-bundle-size.mjs   # 103.3 kB gz / 120
```

* `organize.selection` (10) — click replaces, ⇧click ranges from a *sticky* anchor, ⇧⌘click unions,
  ⌘click toggles, a press inside a multi-selection keeps it (so the block can be dragged), marquee
  replace vs union, box intersection, roving focus clamping, and the `pagesStore` wrapper.
* `organize.dnd` (6) — the `spikes/pages.md` row (`[3,2] → 1` ⇒ `A D C B`), caret → `dest`
  conversion, **one** `page_ops` call with **one** op for a drag, no-op drops return `null`, the
  optimistic order is cleared by the response, and moving every page is refused.
* `pageRange.parse` (5) — `1-3,5,8-`, open-ended both ways, reversed ranges, duplicates, en-dash,
  every rejection (0, past the end, letters, `1--3`, empty), and the `formatPageRange` round trip.
* `dialogs.export.estimate` (3) — the estimate renders and changes with the DPI and with the range;
  텍스트 has no estimate.
* `dialogs.unsaved.flow` (7) — no prompt when clean, 저장 안 함 / 취소 / Esc / 저장 (which really
  saves), the close flow cancels, and a document with a path never opens the save panel.
* `welcome.recents` (8) — the cards and their metadata, pinned first, the filter, opening a recent
  **restores page/zoom/layout**, 하나로 합치기 vs 각각 열기 on a multi-file drop, a single file opens
  with no prompt, the card context menu, and that opening records a recent.

---

## 5. Requests to other owners

1. **(c) — `src/sidebar/Thumbnails.tsx`**: the rail's `.thumb` button has no `data-page`. The
   context menu currently reads the page number out of the `.thumb-num` chip. One attribute fixes it.
2. **(d) — `src/annot/ToolSurface.tsx`**: it calls `e.stopPropagation()` on `contextmenu`, which
   swallows the canvas context menu. My listener is on the **capture** phase so it still works, but a
   layer that wants to own the menu should mark itself `data-context-menu` (both `App.tsx` and
   `pageMenus.ts` skip those subtrees) rather than swallowing the event. The canvas menu deliberately
   omits 붙여넣기 · 메모 추가 · 스냅샷 (UI_SPEC §12) — they are (d)'s and can be pushed into the same
   `items` array.
3. **Integrator — `Settings` has no recents-count field** (IPC_CONTRACT §11). 설정 › 일반 stores it in
   `Settings.toolDefaults.recentsCount` (`SettingsDialog.recentsCountOf`). If the contract gains
   `recentsCount: number`, both call sites are one line each.
4. **Integrator — `docStore.adopt(info)`** (see §2), so `merge_documents` does not need `setState`.
5. **Mock adapter (`src/ipc/mock.ts`, not mine)**: `openFileDialog` always returns the same single
   path, so 파일 합치기 cannot reach two inputs in mock mode and the directory picker always answers
   the same folder. Returning 2–3 distinct fixture paths for `multiple: true` (and a different one for
   `directory: true`) would make the merge dialog and the export destination exercisable without a
   backend. `page_ops { rotate }` should also swap `widthPt/heightPt` so the organizer's cells rotate.

---

## 6. Live verification (mock, Chrome, `--port 1421`, dark + light)

Welcome with recents (thumbnails, folder, relative date, page count, size, pinned first, filter) ·
open a recent → **reading position restored** (tracemonkey: page 3 / 150 % / 단일, straight from
`recents.json`) · 페이지 mode: the grid replaces the canvas and the sidebar collapses · click and
⇧click select a range (`3쪽 선택됨`) · hover reveals rotate-left/right/delete and the `+` insert
affordance · 오른쪽으로 회전 on a 3-page selection writes a 90° badge on each, dirties the title and
enables undo · **drag page 1 between 4 and 5 → the DOM order becomes `[1,2,3,0,4…]` with the rotated
pages following their content**, one `page_ops` call · marquee over 8 cells selects exactly those ·
cell context menu (9 items, 삭제 in danger) · 축소판 rail context menu · overflow ⋯ menu ·
내보내기 (PNG 2.4 MB → JPEG 1.2 MB estimate, DPI/quality sliders, `99` on a 14-page document shows
페이지 범위를 확인해 주세요 and disables 내보내기) · 설정 (**language and theme switch live**, whole
shell, no reload) · 문서 정보 · 문서 분할 (ran: job → 3 outputs → toast with Finder에서 보기) ·
페이지 추출 · 암호 입력 (wrong-password retry path, then opens) · 저장하지 않은 변경 사항 →
저장 안 함 closes the document · 여러 파일 드롭 → 하나로 합치기 → `merged.pdf` (9 pages, dirty, no
path) · 인쇄 (range picker → 인쇄 준비 중… toast) · ⌘S → status-bar job slot + 저장됨 toast ·
success/danger toasts with an action and 자세히 (the engine `message` stays behind it).
**Zero console errors or warnings for the whole session.**

---

## 7. Gaps / not done

1. **OS file drop into the organizer at a caret.** UI_SPEC §9 wants a dropped PDF inserted at the
   insertion caret; `onFileDrop` (Tauri) carries paths but the position is not plumbed through, so a
   drop anywhere opens/merges the files instead. The grid's `+` affordance inserts a **blank** page;
   파일에서 삽입… is on the rail and in the cell menu.
2. **Selection is only remapped after a move.** After 삭제/복제 the selection is cleared or left as-is
   rather than following the new indices.
3. **No page-size or `/PageLabels` chip in the cell** (the label is `PageGeom.label`, read-only in v1)
   and no 위치 이동… dialog (`pages.moveTo` is in the catalogue, unused).
4. **인쇄 has no print-only DOM** — it is `print_prepare` + the OS handler, with
   `getCurrentWebview().print()` tried first. F-26's primary path is unverified until the capability
   question in §3 is settled.
5. **문서 정보 is read-only** (P1-1 needs `lopdf`), and there is no 보안 dialog: 암호 설정/제거 are
   P1-2/P1-3 and their keys sit unused in the catalogue.
6. **내보내기** does not offer 하나의 이미지로 이어 붙이기 or PDF (압축) — neither has a backend
   command in v1 (`export.singleImage` / `export.format.pdfCompressed` stay unused).
7. **The thumbnail-size slider lives in the organizer rail**, not in the status bar as UI_SPEC §8
   says: `StatusBar.tsx` is not owned by this module. Moving it is a five-line change for whoever
   owns the status bar next.
8. **The marquee does not auto-scroll** past the viewport edge, and dragging is disabled while a
   `page_ops` call is in flight (the grid shows `data-busy`).
9. **`pages.changed` (변경됨 · 페이지 {{from}} → {{to}})** is not rendered anywhere: the original page
   count is not kept across a session. The status bar's 저장되지 않은 변경 사항 covers the same ground.
